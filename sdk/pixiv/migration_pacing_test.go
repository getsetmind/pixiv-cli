package pixiv

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

var migrationUpdatePacing = flag.Bool("migration-update-pacing", false, "capture shared transport pacing contracts")

type migrationPacingRow struct {
	IntervalMS int64    `json:"interval_ms"`
	Calls      []string `json:"calls"`
	Spaced     bool     `json:"spaced"`
}

func TestMigrationDefaultSDKClientLeavesTotalTimeoutToContext(t *testing.T) {
	client, err := New("fixture-access")
	if err != nil {
		t.Fatal(err)
	}
	defer client.CloseIdleConnections()
	if client.httpClient.Timeout != 0 {
		t.Fatalf("default SDK client has total timeout %v", client.httpClient.Timeout)
	}
}

func TestMigrationPacingCanceledWaitDoesNotReachTransportOrAdvanceItsLastStart(t *testing.T) {
	calls := 0
	base := &http.Client{Transport: migrationOAuthTransport(func(request *http.Request) (*http.Response, error) {
		calls++
		return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader("{}")), Request: request}, nil
	})}
	client, err := NewWith("fixture-access", Options{HTTPClient: base, Pacing: Pacing{MinInterval: time.Second}})
	if err != nil {
		t.Fatal(err)
	}
	response, err := client.httpClient.Get("https://app-api.pixiv.net/first")
	if err != nil {
		t.Fatal(err)
	}
	if err := response.Body.Close(); err != nil {
		t.Fatal(err)
	}
	pacer := client.httpClient.Transport.(*diagnosticRoundTripper).inner.(*pacingRoundTripper)
	pacer.mu.Lock()
	last := pacer.last
	pacer.mu.Unlock()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	request, err := http.NewRequestWithContext(ctx, "GET", "https://app-api.pixiv.net/canceled", nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := client.httpClient.Do(request); !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled wait: %v", err)
	}
	pacer.mu.Lock()
	unchanged := pacer.last == last
	pacer.mu.Unlock()
	if calls != 1 || !unchanged {
		t.Fatalf("canceled pacing wait reached transport or changed last start: calls=%d unchanged=%v", calls, unchanged)
	}
}

func TestMigrationPacingSharesOAuthContentResourceRedirectAndMutationStarts(t *testing.T) {
	var rows []migrationPacingRow
	for _, interval := range []time.Duration{0, 125 * time.Millisecond} {
		row := migrationPacingRow{IntervalMS: interval.Milliseconds(), Spaced: true}
		var last time.Time
		transport := migrationOAuthTransport(func(request *http.Request) (*http.Response, error) {
			now := time.Now()
			if !last.IsZero() && now.Sub(last)+5*time.Millisecond < interval {
				row.Spaced = false
			}
			last = now
			row.Calls = append(row.Calls, request.Method+" "+request.URL.Path)
			body := `{}`
			status := 200
			headers := http.Header{}
			if request.URL.Path == "/auth/token" {
				body = `{"access_token":"fixture-access","refresh_token":"fixture-refresh","user":{"id":42}}`
			}
			if request.URL.Path == "/media-start" {
				status = 302
				headers.Set("Location", "/media-final")
			}
			return &http.Response{StatusCode: status, Header: headers, Body: io.NopCloser(strings.NewReader(body)), Request: request}, nil
		})
		client, _, err := OpenWith(context.Background(), "fixture-refresh", Options{HTTPClient: &http.Client{Transport: transport}, Pacing: Pacing{MinInterval: interval}})
		if err != nil {
			t.Fatal(err)
		}
		for _, request := range []struct{ method, url string }{
			{"GET", "https://app-api.pixiv.net/v1/illust/detail"},
			{"GET", "http://fixture.invalid/media-start"},
			{"POST", "https://app-api.pixiv.net/v2/illust/bookmark/add"},
		} {
			req, err := http.NewRequest(request.method, request.url, nil)
			if err != nil {
				t.Fatal(err)
			}
			response, err := client.httpClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := io.Copy(io.Discard, response.Body); err != nil {
				t.Fatal(err)
			}
			if err := response.Body.Close(); err != nil {
				t.Fatal(err)
			}
		}
		client.CloseIdleConnections()
		if !row.Spaced {
			t.Fatal("transport did not preserve spacing across operation kinds")
		}
		rows = append(rows, row)
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "pacing.json")
	if *migrationUpdatePacing {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("transport pacing differs from fixed Go reference")
	}
}

package resource_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/resource"
)

var migrationUpdateRedirectHeaders = flag.Bool("migration-update-redirect-headers", false, "capture redirect header contracts from the fixed Go reference")

func TestMigrationResourceRedirectHeadersMatchGoTrustBoundary(t *testing.T) {
	type row struct {
		CrossHost bool        `json:"cross_host"`
		Headers   http.Header `json:"headers"`
	}
	rows := []row{}
	for _, crossHost := range []bool{false, true} {
		seen := make(chan http.Header, 1)
		target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			headers := http.Header{}
			for _, name := range []string{"Authorization", "Www-Authenticate", "Cookie", "Cookie2", "Range", "Referer", "User-Agent"} {
				if values := r.Header.Values(name); len(values) > 0 {
					headers[name] = values
				}
			}
			seen <- headers
			w.WriteHeader(http.StatusNoContent)
		}))
		destination := target.URL
		if crossHost {
			destination = strings.Replace(destination, "127.0.0.1", "localhost", 1)
		}
		initial := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Location", destination)
			w.WriteHeader(http.StatusFound)
		}))
		response, err := resource.NewApp(initial.Client()).Open(context.Background(), resource.OpenRequest{
			URL: initial.URL, DisableCookies: true,
			Header: http.Header{"Authorization": {"fixture-auth"}, "Www-Authenticate": {"fixture-challenge"}, "Cookie": {"fixture=cookie"}, "Cookie2": {"fixture=cookie2"}, "Range": {"bytes=0-2"}},
		})
		if err != nil {
			t.Fatal(err)
		}
		if _, err := io.Copy(io.Discard, response.Body); err != nil {
			t.Fatal(err)
		}
		if err := response.Body.Close(); err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{crossHost, <-seen})
		initial.Close()
		target.Close()
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "docs", "migration", "contracts", "resource-redirect-headers.json")
	if *migrationUpdateRedirectHeaders {
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
		t.Fatal("resource redirect headers differ from the fixed Go reference")
	}
}

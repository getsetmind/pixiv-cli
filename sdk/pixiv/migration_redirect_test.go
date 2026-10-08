package pixiv

import (
	"bytes"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

var migrationUpdateRedirect = flag.Bool("migration-update-redirect", false, "capture SDK HTTP redirect contracts")

type migrationRedirectCall struct {
	Method  string            `json:"method"`
	URL     string            `json:"url"`
	Body    string            `json:"body"`
	Headers map[string]string `json:"headers"`
}

type migrationRedirectRow struct {
	Name       string                  `json:"name"`
	Method     string                  `json:"method"`
	Statuses   []int                   `json:"statuses"`
	Locations  []string                `json:"locations"`
	Referer    string                  `json:"referer"`
	IntervalMS int64                   `json:"interval_ms"`
	Spaced     bool                    `json:"spaced"`
	Calls      []migrationRedirectCall `json:"calls"`
	Status     int                     `json:"status"`
	Failed     bool                    `json:"failed"`
}

func TestMigrationSDKHTTPRedirectPreservesMethodsHeadersAndOriginalBody(t *testing.T) {
	var rows []migrationRedirectRow
	for _, method := range []string{"GET", "POST"} {
		for _, status := range []int{301, 302, 303, 307, 308} {
			rows = append(rows, migrationRedirectRow{Name: method + ":" + http.StatusText(status), Method: method, Statuses: []int{status}, Locations: []string{"/final"}})
		}
	}
	rows = append(rows,
		migrationRedirectRow{Name: "original-body-after-method-rewrite", Method: "POST", Statuses: []int{302, 307}, Locations: []string{"/middle", "/final"}},
		migrationRedirectRow{Name: "relative-query", Method: "GET", Statuses: []int{302}, Locations: []string{"?redirect=value"}},
		migrationRedirectRow{Name: "explicit-referer", Method: "POST", Statuses: []int{303}, Locations: []string{"/final"}, Referer: "http://explicit.invalid/source"},
		migrationRedirectRow{Name: "trusted-subdomain", Method: "GET", Statuses: []int{302}, Locations: []string{"http://child.source.invalid/final"}},
		migrationRedirectRow{Name: "trusted-port", Method: "GET", Statuses: []int{302}, Locations: []string{"http://source.invalid:81/final"}},
		migrationRedirectRow{Name: "untrusted-host", Method: "POST", Statuses: []int{307}, Locations: []string{"http://other.invalid/final"}},
		migrationRedirectRow{Name: "suffix-without-dot", Method: "GET", Statuses: []int{302}, Locations: []string{"http://othersource.invalid/final"}},
		migrationRedirectRow{Name: "untrusted-return-to-original", Method: "POST", Statuses: []int{307, 307}, Locations: []string{"http://other.invalid/middle", "http://source.invalid/final"}},
		migrationRedirectRow{Name: "missing-location", Method: "GET", Statuses: []int{302}, Locations: []string{""}},
		migrationRedirectRow{Name: "malformed-location", Method: "GET", Statuses: []int{302}, Locations: []string{"http://other.invalid/%"}},
		migrationRedirectRow{Name: "unsupported-scheme", Method: "GET", Statuses: []int{302}, Locations: []string{"ftp://other.invalid/final"}},
		migrationRedirectRow{Name: "redirect-limit", Method: "GET", Statuses: []int{302, 302, 302, 302, 302, 302, 302, 302, 302, 302}, Locations: []string{"/loop", "/loop", "/loop", "/loop", "/loop", "/loop", "/loop", "/loop", "/loop", "/loop"}},
		migrationRedirectRow{Name: "paced-redirect", Method: "GET", Statuses: []int{307, 307}, Locations: []string{"/middle", "/final"}, IntervalMS: 5},
	)
	for i := range rows {
		row := &rows[i]
		row.Spaced = true
		var last time.Time
		transport := migrationOAuthTransport(func(request *http.Request) (*http.Response, error) {
			if request.URL.Scheme != "http" && request.URL.Scheme != "https" {
				return http.DefaultTransport.RoundTrip(request)
			}
			now := time.Now()
			if !last.IsZero() && now.Sub(last) < time.Duration(row.IntervalMS)*time.Millisecond {
				row.Spaced = false
			}
			last = now
			body := ""
			if request.Body != nil {
				data, err := io.ReadAll(request.Body)
				if err != nil {
					t.Fatal(err)
				}
				body = string(data)
			}
			headers := map[string]string{}
			for _, name := range []string{"Authorization", "Www-Authenticate", "Cookie", "Cookie2", "Proxy-Authorization", "Proxy-Authenticate", "Content-Type", "Content-Encoding", "Content-Language", "Content-Location", "Referer", "X-Fixture"} {
				if value := request.Header.Get(name); value != "" {
					headers[strings.ToLower(name)] = value
				}
			}
			index := len(row.Calls)
			row.Calls = append(row.Calls, migrationRedirectCall{Method: request.Method, URL: request.URL.String(), Body: body, Headers: headers})
			status := 200
			responseHeaders := http.Header{}
			if index < len(row.Statuses) {
				status = row.Statuses[index]
				if row.Locations[index] != "" {
					responseHeaders.Set("Location", row.Locations[index])
				}
			}
			return &http.Response{StatusCode: status, Header: responseHeaders, Body: io.NopCloser(strings.NewReader("{}")), Request: request}, nil
		})
		client, err := NewWith("fixture-access", Options{HTTPClient: &http.Client{Transport: transport}, Pacing: Pacing{MinInterval: time.Duration(row.IntervalMS) * time.Millisecond}})
		if err != nil {
			t.Fatal(err)
		}
		address := "http://source.invalid/start"
		var body io.Reader
		if row.Method == "GET" {
			address += "?value=fixture-value"
		} else {
			body = strings.NewReader("value=fixture-value")
		}
		request, err := http.NewRequest(row.Method, address, body)
		if err != nil {
			t.Fatal(err)
		}
		for _, name := range []string{"Authorization", "Www-Authenticate", "Cookie", "Cookie2", "Proxy-Authorization", "Proxy-Authenticate", "Content-Encoding", "Content-Language", "Content-Location", "X-Fixture"} {
			request.Header.Set(name, "fixture-value")
		}
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		if row.Referer != "" {
			request.Header.Set("Referer", row.Referer)
		}
		response, err := client.httpClient.Do(request)
		row.Failed = err != nil
		if err == nil {
			row.Status = response.StatusCode
			if err := response.Body.Close(); err != nil {
				t.Fatal(err)
			}
		}
		client.CloseIdleConnections()
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "http-redirect.json")
	if *migrationUpdateRedirect {
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
		t.Fatal("SDK HTTP redirect differs from fixed Go reference")
	}
}

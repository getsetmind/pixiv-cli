package resource_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/protocol"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/resource"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateStream = flag.Bool("migration-update-stream", false, "capture resource HTTP stream contracts from the fixed Go reference")

func TestMigrationResourceStreamMatchesFrozenHTTPBehavior(t *testing.T) {
	type seen struct {
		Method        string `json:"method"`
		Path          string `json:"path"`
		Referer       string `json:"referer"`
		UserAgent     string `json:"user_agent"`
		Range         string `json:"range"`
		IfNoneMatch   string `json:"if_none_match"`
		Cookie        string `json:"cookie"`
		Authorization string `json:"authorization"`
	}
	type row struct {
		Name         string      `json:"name"`
		Method       string      `json:"method"`
		Status       int         `json:"status"`
		Redirect     bool        `json:"redirect"`
		Blocked      bool        `json:"blocked"`
		Loop         bool        `json:"loop"`
		Partial      bool        `json:"partial"`
		ResultStatus int         `json:"result_status"`
		Body         string      `json:"body"`
		ReadFailed   bool        `json:"read_failed"`
		Failure      string      `json:"failure"`
		Headers      http.Header `json:"headers"`
		Requests     []seen      `json:"requests"`
		Validated    []string    `json:"validated"`
	}
	cases := []row{{Name: "get", Method: "GET", Status: 200}, {Name: "head", Method: "HEAD", Status: 200}, {Name: "partial-content", Method: "GET", Status: 206}, {Name: "no-content", Method: "GET", Status: 204}, {Name: "not-modified", Method: "GET", Status: 304}, {Name: "not-found", Method: "GET", Status: 404}, {Name: "truncated", Method: "GET", Status: 200, Partial: true}, {Name: "redirect-301", Method: "GET", Status: 301, Redirect: true}, {Name: "redirect-302", Method: "GET", Status: 302, Redirect: true}, {Name: "redirect-303-head", Method: "HEAD", Status: 303, Redirect: true}, {Name: "redirect-307", Method: "GET", Status: 307, Redirect: true}, {Name: "redirect-308", Method: "GET", Status: 308, Redirect: true}, {Name: "blocked-redirect", Method: "GET", Status: 302, Redirect: true, Blocked: true}, {Name: "redirect-limit", Method: "GET", Status: 302, Redirect: true, Loop: true}, {Name: "missing-location", Method: "GET", Status: 302}}
	rows := make([]row, 0, len(cases))
	for _, input := range cases {
		input.Requests = []seen{}
		input.Validated = []string{}
		var lock sync.Mutex
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			lock.Lock()
			input.Requests = append(input.Requests, seen{r.Method, r.URL.Path, r.Header.Get("Referer"), r.Header.Get("User-Agent"), r.Header.Get("Range"), r.Header.Get("If-None-Match"), r.Header.Get("Cookie"), r.Header.Get("Authorization")})
			lock.Unlock()
			if input.Redirect && (r.URL.Path == "/start" || input.Loop) {
				target := "/target"
				if input.Blocked {
					target = "/blocked"
				}
				if input.Loop {
					target = "/start"
				}
				w.Header().Set("Location", target)
				w.WriteHeader(input.Status)
				return
			}
			status := input.Status
			if input.Redirect {
				status = 200
			}
			w.Header().Set("Content-Type", "application/octet-stream")
			w.Header().Set("ETag", `"fixture"`)
			w.Header().Set("Set-Cookie", "fixture-cookie-secret")
			w.Header().Set("X-Internal", "fixture-internal-secret")
			if input.Partial {
				w.Header().Set("Content-Length", "99")
			} else if status != 204 && status != 304 {
				w.Header().Set("Content-Length", "3")
			}
			w.WriteHeader(status)
			if status != 204 && status != 304 && r.Method != "HEAD" {
				fmt.Fprint(w, "abc")
			}
		}))
		validate := func(raw string) error {
			parsed, err := url.Parse(raw)
			if err != nil {
				return err
			}
			input.Validated = append(input.Validated, parsed.Path)
			if parsed.Path == "/blocked" {
				return errors.New("fixture-policy-secret")
			}
			return nil
		}
		response, err := resource.NewApp(server.Client()).Open(context.Background(), resource.OpenRequest{URL: server.URL + "/start", Method: input.Method, Header: http.Header{"Range": {"bytes=0-2"}, "If-None-Match": {`"fixture-match"`}, "Referer": {"fixture-override"}, "User-Agent": {"fixture-override"}}, Validate: validate, DisableCookies: true})
		if err != nil {
			var failure protocol.Failure
			if !errors.As(err, &failure) {
				t.Fatal(err)
			}
			input.Failure = string(failure.Kind)
		} else {
			input.ResultStatus = response.StatusCode
			body, readErr := io.ReadAll(response.Body)
			input.Body = string(body)
			input.ReadFailed = readErr != nil
			input.Headers = sdk.NewResourceResponse(response.StatusCode, response.Header, response.Body).Header()
			if err := response.Body.Close(); err != nil {
				t.Fatal(err)
			}
		}
		server.Close()
		rows = append(rows, input)
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "docs", "migration", "contracts", "resource-stream.json")
	if *migrationUpdateStream {
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
		t.Fatal("resource HTTP streams differ from the fixed Go reference")
	}
}

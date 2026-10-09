package pixiv

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

type migrationLoginProxyCase struct {
	Name       string            `json:"name"`
	URL        string            `json:"url"`
	Env        map[string]string `json:"env"`
	AfterBuild map[string]string `json:"after_build"`
	AfterFirst map[string]string `json:"after_first"`
	Route      string            `json:"route"`
	Error      string            `json:"error"`
	Login      bool              `json:"login"`
	Disabled   bool              `json:"disabled"`
	Requests   int               `json:"requests"`
}

func TestMigrationLoginDefaultEnvironmentProxy(t *testing.T) {
	raw, err := os.ReadFile("../../crates/pixiv-sdk/tests/fixtures/login_environment_proxy.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []migrationLoginProxyCase
	if err = json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	for _, row := range cases {
		t.Run(row.Name, func(t *testing.T) {
			raw, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			cmd := exec.Command(exe, "-test.run=^TestMigrationLoginEnvironmentProxyChild$", "-test.v")
			for _, entry := range os.Environ() {
				key := strings.SplitN(entry, "=", 2)[0]
				switch key {
				case "HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy", "ALL_PROXY", "all_proxy", "REQUEST_METHOD", "PIXIV_MIGRATION_LOGIN_PROXY_CASE":
				default:
					cmd.Env = append(cmd.Env, entry)
				}
			}
			cmd.Env = append(cmd.Env, "PIXIV_MIGRATION_LOGIN_PROXY_CASE="+string(raw))
			output, err := cmd.CombinedOutput()
			if err != nil {
				t.Fatalf("%v\n%s", err, output)
			}
		})
	}
}

func TestMigrationLoginEnvironmentProxyChild(t *testing.T) {
	raw := os.Getenv("PIXIV_MIGRATION_LOGIN_PROXY_CASE")
	if raw == "" {
		t.Skip("isolated default-login environment child")
	}
	var row migrationLoginProxyCase
	if err := json.Unmarshal([]byte(raw), &row); err != nil {
		t.Fatal(err)
	}
	routes := make(chan string, 4)
	serve := func(route string) *httptest.Server {
		return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			routes <- route
			if route != "direct" {
				if row.Login {
					if r.Method != "CONNECT" || r.Host != "oauth.secure.pixiv.net:443" {
						t.Errorf("unexpected login proxy target: %s %s", r.Method, r.Host)
					}
					w.WriteHeader(http.StatusBadGateway)
					return
				}
				if r.Method != "GET" || r.URL.String() != "http://public.example/resource" {
					t.Errorf("unexpected proxy target: %s %s", r.Method, r.URL.String())
				}
			} else if r.Method != "GET" || r.URL.Path != "/resource" {
				t.Errorf("unexpected direct target: %s %s", r.Method, r.URL.Path)
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = io.WriteString(w, `{"route":"`+route+`"}`)
		}))
	}
	upper, lower, direct := serve("upper"), serve("lower"), serve("direct")
	defer upper.Close()
	defer lower.Close()
	defer direct.Close()
	replace := strings.NewReplacer("<UPPER>", upper.URL, "<UPPER_FTP>", strings.Replace(upper.URL, "http:", "ftp:", 1), "<UPPER_SOCKS4>", strings.Replace(upper.URL, "http:", "socks4:", 1), "<LOWER>", lower.URL, "<DIRECT>", direct.URL)
	set := func(values map[string]string) {
		for key, value := range values {
			t.Setenv(key, replace.Replace(value))
		}
	}
	set(row.Env)
	client, _ := loginHTTPClient(nil)
	if row.Disabled {
		client.Transport.(*http.Transport).Proxy = nil
	}
	defer client.CloseIdleConnections()
	client.Timeout = 3 * time.Second
	set(row.AfterBuild)
	count := row.Requests
	if count == 0 {
		count = 1
	}
	for i := 0; i < count; i++ {
		var err error
		if row.Login {
			session, e := BeginLogin(LoginOptions{})
			if e != nil {
				t.Fatal(e)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			_, err = session.Complete(ctx, "synthetic-code")
			cancel()
			session.CloseIdleConnections()
		} else {
			var response *http.Response
			response, err = client.Get(replace.Replace(row.URL))
			if response != nil {
				_, _ = io.Copy(io.Discard, response.Body)
				response.Body.Close()
			}
		}
		switch row.Error {
		case "":
			if err != nil {
				t.Fatal(err)
			}
		case "missing_proxy_port":
			foundPortZero := false
			for cause := err; cause != nil; cause = errors.Unwrap(cause) {
				if native, ok := cause.(*net.OpError); ok && native.Addr != nil && native.Addr.String() == "127.0.0.1:0" {
					foundPortZero = true
				}
			}
			if !foundPortZero {
				t.Fatalf("unknown proxy scheme without port error=%v", err)
			}
		case "cgi":
			if err == nil || !strings.Contains(err.Error(), "refusing to use HTTP_PROXY value in CGI environment") {
				t.Fatalf("CGI error=%v", err)
			}
		case "upstream_unavailable":
			var classified *sdk.Error
			if !errors.As(err, &classified) || classified.Reason != sdk.UpstreamUnavailable || classified.Transport != sdk.TransportHTTP || classified.Detail != "transport: unknown" {
				t.Fatalf("login transport error=%#v", err)
			}
		default:
			t.Fatalf("unknown error fixture: %s", row.Error)
		}
		if row.Route != "" {
			select {
			case got := <-routes:
				if got != row.Route {
					t.Fatalf("route=%s want %s", got, row.Route)
				}
			case <-time.After(time.Second):
				t.Fatal("missing synthetic route")
			}
		}
		if i == 0 {
			set(row.AfterFirst)
		}
	}
	select {
	case route := <-routes:
		t.Fatalf("unexpected additional route: %s", route)
	default:
	}
}

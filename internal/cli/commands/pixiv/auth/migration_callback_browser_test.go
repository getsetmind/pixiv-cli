package auth_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
)

type migrationCallbackTransport func(*http.Request) (*http.Response, error)

func (f migrationCallbackTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type migrationCallbackBody struct {
	io.Reader
	calls *[]string
}

func (b migrationCallbackBody) Read(p []byte) (int, error) {
	*b.calls = append(*b.calls, "read")
	return b.Reader.Read(p)
}
func (b migrationCallbackBody) Close() error { *b.calls = append(*b.calls, "close"); return nil }

func TestMigrationCallbackBrowserActions(t *testing.T) {
	body, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/callback_dispatch.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Browser []struct {
			Name, Kind, Body, Error string
			OpenError               bool `json:"open_error"`
			Calls                   []string
		}
	}
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	for _, c := range fixture.Browser {
		t.Run(c.Name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			calls := []string{}
			const resultURL = "https://relay.example/result/c3ludGhldGlj"
			const loginURL = "https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge_method=S256&code_challenge=synthetic&state=synthetic"
			t.Cleanup(loginhelper.SetCallbackRelayURLForHandler(func(string) (string, error) {
				if c.Kind == "local" {
					return "http://127.0.0.1:9/callback#pixiv://account/login?code=synthetic", nil
				}
				return "", loginhelper.ErrNoActiveLocalCallback
			}))
			t.Cleanup(loginhelper.SetDelegateToPreviousForHandler(func(context.Context, string) error { calls = append(calls, "delegate"); return nil }))
			t.Cleanup(loginhelper.SetHandoffHTTPClient(&http.Client{Transport: migrationCallbackTransport(func(r *http.Request) (*http.Response, error) {
				response := &http.Response{StatusCode: 200, Header: make(http.Header)}
				if c.Kind == "start" {
					calls = append(calls, "start")
					if r.Method != "POST" || r.URL.Path != "/start/synthetic" {
						t.Fatalf("start request=%s %s", r.Method, r.URL)
					}
					response.Body = io.NopCloser(strings.NewReader(`{"authorization_url":"` + loginURL + `"}`))
				} else {
					calls = append(calls, "forward")
					if r.Method != "POST" || r.URL.Path != "/callback/synthetic" {
						t.Fatalf("callback request=%s %s", r.Method, r.URL)
					}
					response.Header.Set(loginhelper.RelayResultURLHeader, resultURL)
					response.Body = migrationCallbackBody{Reader: strings.NewReader(c.Body), calls: &calls}
				}
				return response, nil
			})}))
			t.Cleanup(auth.SetClearRemoteLoginHandoffForHandler(func(start loginhelper.RemoteLoginStart) error {
				calls = append(calls, "clear")
				if start.Origin != "https://relay.example" || start.SessionID != "synthetic" || start.Proof != "synthetic" {
					t.Fatalf("clear start=%+v", start)
				}
				return nil
			}))
			t.Cleanup(auth.SetOpenBrowser(func(raw string) error {
				calls = append(calls, "open")
				want := resultURL
				if c.Kind == "local" {
					want = "http://127.0.0.1:9/callback#pixiv://account/login?code=synthetic"
				}
				if c.Kind == "start" {
					want = loginURL
				}
				if raw != want {
					t.Fatalf("open=%q want=%q", raw, want)
				}
				if c.Kind == "remote" {
					if _, err := loginhelper.LoadActiveRemoteLogin(); !errors.Is(err, loginhelper.ErrNoActiveRemoteLogin) {
						t.Fatalf("state not consumed before open: %v", err)
					}
				}
				if c.OpenError {
					return errors.New("synthetic browser failure with private URL")
				}
				return nil
			}))
			raw := "pixiv://account/login?code=synthetic"
			if c.Kind == "delegate" {
				raw = "pixiv://account/works/1"
			}
			if c.Kind == "start" {
				raw = "pixiv://account/remote-login?origin=https%3A%2F%2Frelay.example&session=synthetic&access=synthetic"
			}
			if c.Kind == "remote" {
				if err := loginhelper.SaveActiveRemoteLogin(loginhelper.ActiveRemoteLogin{Version: 1, Origin: "https://relay.example", SessionID: "synthetic", Proof: "synthetic"}); err != nil {
					t.Fatal(err)
				}
			}
			cmd := auth.NewAccountURLCallbackCommand()
			var out bytes.Buffer
			cmd.SetOut(&out)
			cmd.SetErr(&out)
			cmd.SilenceErrors = true
			cmd.SilenceUsage = true
			cmd.SetArgs([]string{raw})
			err := cmd.ExecuteContext(context.Background())
			message := ""
			if err != nil {
				message = err.Error()
			}
			if message != c.Error || !reflect.DeepEqual(calls, c.Calls) || out.Len() != 0 {
				t.Fatalf("calls=%v error=%q output=%q; want %v %q", calls, message, out.String(), c.Calls, c.Error)
			}
		})
	}
}

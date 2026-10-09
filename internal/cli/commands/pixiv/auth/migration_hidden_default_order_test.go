package auth_test

import (
	"context"
	"io"
	"net/http"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
)

func TestMigrationHiddenDefaultStatePathResolutionOrder(t *testing.T) {
	missingHome := "$HOME is not defined"
	if runtime.GOOS == "windows" {
		missingHome = "%USERPROFILE% is not defined"
	}
	for _, c := range []struct {
		name, url, err string
		calls          []string
	}{
		{"invalid-before-home", "https://invalid.example/?code=private", "invalid Pixiv login link", []string{}},
		{"local-before-network", "pixiv://account/login?code=synthetic", missingHome, []string{}},
		{"remote-request-before-home", "pixiv://account/remote-login?origin=https%3A%2F%2Frelay.example&session=synthetic&access=synthetic", missingHome, []string{"request", "read", "read", "read", "read", "close"}},
	} {
		t.Run(c.name, func(t *testing.T) {
			t.Setenv("HOME", "")
			t.Setenv("USERPROFILE", "")
			calls := []string{}
			restore := loginhelper.SetHandoffHTTPClient(&http.Client{Transport: migrationCallbackTransport(func(r *http.Request) (*http.Response, error) {
				calls = append(calls, "request")
				if r.Method != http.MethodPost || r.URL.String() != "https://relay.example/start/synthetic" {
					t.Fatalf("request %s %s", r.Method, r.URL)
				}
				return &http.Response{StatusCode: 200, Header: http.Header{}, Body: migrationCallbackBody{Reader: strings.NewReader(`{"authorization_url":"https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge_method=S256&code_challenge=synthetic&state=synthetic"}`), calls: &calls}}, nil
			})})
			defer restore()
			restoreBrowser := auth.SetOpenBrowser(func(string) error { t.Fatal("state failure must precede browser"); return nil })
			defer restoreBrowser()
			cmd := auth.NewAccountURLCallbackCommand()
			cmd.SetContext(context.Background())
			cmd.SetOut(io.Discard)
			cmd.SetErr(io.Discard)
			cmd.SetArgs([]string{c.url})
			err := cmd.Execute()
			if err == nil || err.Error() != c.err {
				t.Fatalf("error %v, expected %q", err, c.err)
			}
			if !reflect.DeepEqual(calls, c.calls) {
				t.Fatalf("calls %v, expected %v", calls, c.calls)
			}
		})
	}
}

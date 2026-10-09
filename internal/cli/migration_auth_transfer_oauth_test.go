package cli

import (
	"bytes"
	"context"
	"fmt"
	authcommands "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"io"
	"net/http"
	"strings"
	"testing"
)

type migrationTransferTransport func(*http.Request) (*http.Response, error)

func (f migrationTransferTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type migrationTransferPort struct {
	authcommands.AccountPort
	calls     []string
	existing  bool
	transport migrationTransferTransport
}

func (p *migrationTransferPort) ListAccounts(context.Context) ([]account.AccountSummary, error) {
	p.calls = append(p.calls, "list")
	if p.existing {
		return []account.AccountSummary{{UserID: 42}}, nil
	}
	return nil, nil
}
func (p *migrationTransferPort) ImportAccountWith(ctx context.Context, token string, setDefault bool, opts pixiv.Options) (account.AccountSummary, error) {
	p.calls = append(p.calls, "import")
	if setDefault {
		return account.AccountSummary{}, fmt.Errorf("unexpected set default")
	}
	opts.HTTPClient = &http.Client{Transport: p.transport}
	client, c, err := pixiv.OpenWith(ctx, token, opts)
	if err != nil {
		return account.AccountSummary{}, err
	}
	defer client.CloseIdleConnections()
	return account.AccountSummary{UserID: c.UserID, Username: c.Username}, nil
}
func TestMigrationAuthTransferOAuth(t *testing.T) {
	for _, existing := range []bool{false, true} {
		for _, stdin := range []bool{false, true} {
			for _, jsonOut := range []bool{false, true} {
				p := &migrationTransferPort{existing: existing}
				requests := 0
				p.transport = func(req *http.Request) (*http.Response, error) {
					requests++
					if req.Method != "POST" || req.URL.String() != "https://oauth.secure.pixiv.net/auth/token" {
						t.Fatal("unexpected OAuth route")
					}
					if err := req.ParseForm(); err != nil {
						t.Fatal(err)
					}
					if req.PostForm.Get("refresh_token") != "synthetic-token" || req.PostForm.Get("grant_type") != "refresh_token" || len(req.PostForm) != 5 {
						t.Fatal("unexpected OAuth form")
					}
					return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","expires_in":3600,"user":{"id":42,"name":"fixture-name"}}`)), Request: req}, nil
				}
				var out bytes.Buffer
				cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader("  synthetic-token  \r\n"), Output: &out, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
					return authcommands.AccountService{Pixiv: p, LoadRuntime: func() (config.RuntimeConfig, error) {
						p.calls = append(p.calls, "runtime")
						return config.RuntimeConfig{}, nil
					}}, nil
				}})
				args := []string{"import"}
				if !stdin {
					args = append(args, "  synthetic-token  ")
				}
				if jsonOut {
					args = append(args, "--json")
				}
				cmd.SetArgs(args)
				cmd.SilenceErrors = true
				cmd.SilenceUsage = true
				if err := cmd.Execute(); err != nil {
					t.Fatal(err)
				}
				status := "added"
				if existing {
					status = "updated"
				}
				want := status + " uid:42\nusername:fixture-name\n"
				if jsonOut {
					want = fmt.Sprintf("{\n  \"user_id\": 42,\n  \"username\": \"fixture-name\",\n  \"status\": \"%s\"\n}\n", status)
				}
				if out.String() != want || strings.Join(p.calls, ",") != "list,runtime,import" || requests != 1 {
					t.Fatalf("output=%q calls=%q requests=%d", out.String(), p.calls, requests)
				}
			}
		}
	}
}

func TestMigrationAuthTransferOpaqueNonUTF8Token(t *testing.T) {
	token := string([]byte{'s', 0xff, 'x'})
	p := &migrationTransferPort{}
	p.transport = func(req *http.Request) (*http.Response, error) {
		if err := req.ParseForm(); err != nil {
			t.Fatal(err)
		}
		if req.PostForm.Get("refresh_token") != token {
			t.Fatal("auth import changed opaque stdin bytes")
		}
		return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","user":{"id":42}}`)), Request: req}, nil
	}
	var out bytes.Buffer
	cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(token + "\n"), Output: &out, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) { return authcommands.AccountService{Pixiv: p}, nil }})
	cmd.SetArgs([]string{"import", "--no-proxy"})
	cmd.SilenceErrors = true
	cmd.SilenceUsage = true
	if err := cmd.Execute(); err != nil {
		t.Fatal(err)
	}
	if out.String() != "added uid:42\n" {
		t.Fatalf("output %q", out.String())
	}
}

func TestMigrationAuthTransferJSONClassifierPreservesGoStringValidityAndProxyPrecedence(t *testing.T) {
	for _, body := range [][]byte{[]byte("{\"name\":\"" + string([]byte{0xff, 0xff}) + "\"}"), []byte(`{"name":"\ud800"}`)} {
		calls := 0
		cmd := authcommands.New(authcommands.Deps{Input: bytes.NewReader(body), Output: io.Discard, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
			calls++
			return authcommands.AccountService{}, fmt.Errorf("unexpected account factory")
		}})
		cmd.SetArgs([]string{"import", "--proxy=x"})
		cmd.SilenceErrors = true
		cmd.SilenceUsage = true
		err := cmd.Execute()
		if err == nil || err.Error() != "bundle import cannot be combined with --proxy or --no-proxy" || calls != 0 {
			t.Fatalf("error=%v dependency calls=%d", err, calls)
		}
	}
}

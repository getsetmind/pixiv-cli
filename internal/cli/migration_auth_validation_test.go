package cli

import (
	"bytes"
	"context"
	"fmt"
	authcommands "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	database "github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"io"
	"net/http"
	"strings"
	"testing"
)

type migrationValidationDefaults struct{ id int64 }

func (d *migrationValidationDefaults) ReadPixivDefaultUserID() (int64, bool, error) {
	return d.id, d.id != 0, nil
}
func (d *migrationValidationDefaults) SetPixivDefaultUserID(id int64) error { d.id = id; return nil }
func (d *migrationValidationDefaults) ClearPixivDefaultUserID() error       { d.id = 0; return nil }

type migrationValidationPort struct {
	*account.Service
	transport migrationTransferTransport
	calls     []string
}

func (p *migrationValidationPort) CurrentUser(ctx context.Context) (*account.AccountSummary, error) {
	p.calls = append(p.calls, "current")
	return p.Service.CurrentUser(ctx)
}
func (p *migrationValidationPort) ListAccounts(ctx context.Context) ([]account.AccountSummary, error) {
	p.calls = append(p.calls, "list")
	return p.Service.ListAccounts(ctx)
}
func (p *migrationValidationPort) CheckAccountWith(ctx context.Context, id int64, o pixiv.Options) (account.AccountSummary, error) {
	p.calls = append(p.calls, "check")
	o.HTTPClient = &http.Client{Transport: p.transport}
	return p.Service.CheckAccountWith(ctx, id, o)
}
func (p *migrationValidationPort) RefreshAccountWith(ctx context.Context, id int64, o pixiv.Options) (account.AccountSummary, error) {
	p.calls = append(p.calls, "refresh")
	o.HTTPClient = &http.Client{Transport: p.transport}
	return p.Service.RefreshAccountWith(ctx, id, o)
}
func TestMigrationAuthValidationOAuthStorageAndOutput(t *testing.T) {
	for _, op := range []string{"check", "refresh"} {
		for _, selection := range []int{0, 1, 2} {
			current := selection == 0
			for _, jsonOut := range []bool{false, true} {
				db, err := database.Open(t.TempDir())
				if err != nil {
					t.Fatal(err)
				}
				defer db.Close()
				a := account.New(42, "local-name", []byte("synthetic-token"))
				a.PoolLastSelected = true
				if err = db.SavePixivCredential(context.Background(), a); err != nil {
					t.Fatal(err)
				}
				p := &migrationValidationPort{Service: account.NewService(db, &migrationValidationDefaults{42})}
				requests := 0
				p.transport = func(req *http.Request) (*http.Response, error) {
					requests++
					body := ""
					if requests == 1 {
						if req.Method != "POST" || req.URL.String() != "https://oauth.secure.pixiv.net/auth/token" {
							t.Fatal("OAuth route")
						}
						if err := req.ParseForm(); err != nil {
							t.Fatal(err)
						}
						if req.PostForm.Get("refresh_token") != "synthetic-token" {
							t.Fatal("OAuth token")
						}
						body = `{"access_token":"synthetic-access","refresh_token":"synthetic-rotated","user":{"id":42,"name":"oauth-name"}}`
					} else {
						stored, e := db.GetPixiv(context.Background(), 42)
						if e != nil || string(stored.RefreshTokenCopy()) != "synthetic-rotated" || stored.CredentialRevision != 2 {
							t.Fatal("rotation precedes profile")
						}
						if req.Method != "GET" || req.URL.Path != "/v1/user/detail" || req.URL.Query().Get("user_id") != "42" {
							t.Fatal("profile route")
						}
						body = `{"user":{"id":42,"name":"profile-name"},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}}`
					}
					return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
				}
				var out bytes.Buffer
				input := ""
				if selection == 2 {
					input = "42\r\n"
				}
				cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(input), Output: &out, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
					return authcommands.AccountService{Pixiv: p, LoadRuntime: func() (config.RuntimeConfig, error) {
						p.calls = append(p.calls, "runtime")
						return config.RuntimeConfig{}, nil
					}}, nil
				}})
				args := []string{op}
				if selection == 1 {
					args = append(args, "42")
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
				want := "account uid:42 ok\nusername:oauth-name\n"
				if current {
					want = "token ok, uid:42\nusername:oauth-name\n"
				}
				if jsonOut {
					want = "{\n  \"user_id\": 42,\n  \"username\": \"oauth-name\",\n  \"default\": false,\n  \"has_token\": true\n}\n"
				}
				if op == "refresh" {
					want = "✓ refreshed uid:42 premium:yes\n"
					if jsonOut {
						want = "{\n  \"accounts\": [\n    {\n      \"user_id\": 42,\n      \"username\": \"local-name\",\n      \"default\": true,\n      \"has_token\": true,\n      \"premium_status\": true,\n      \"schedulable\": true,\n      \"eligible\": true\n    }\n  ]\n}\n"
					}
				}
				calls := "runtime," + op
				if current {
					calls = "current," + calls
				}
				if out.String() != want || strings.Join(p.calls, ",") != calls {
					t.Fatalf("%s current=%t json=%t output=%q calls=%v", op, current, jsonOut, out.String(), p.calls)
				}
				stored, err := db.GetPixiv(context.Background(), 42)
				if err != nil || string(stored.RefreshTokenCopy()) != "synthetic-rotated" || stored.CredentialRevision != 2 || stored.Username != "local-name" {
					t.Fatal("rotation preserves account metadata")
				}
				if requests != map[string]int{"check": 1, "refresh": 2}[op] {
					t.Fatal("request count")
				}
			}
		}
	}
}
func TestMigrationAuthValidationAllRejectsUIDBeforeParsing(t *testing.T) {
	cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(""), Output: io.Discard, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }})
	cmd.SetArgs([]string{"refresh", "bad", "--all"})
	cmd.SilenceErrors = true
	cmd.SilenceUsage = true
	if err := cmd.Execute(); err == nil || err.Error() != "--all cannot be combined with a UID" {
		t.Fatal(fmt.Sprint(err))
	}
}

func TestMigrationAuthValidationAllCommitsSequentiallyAndBuffersOutput(t *testing.T) {
	for _, fail := range []bool{false, true} {
		db, err := database.Open(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		defer db.Close()
		for _, id := range []int64{2, 1} {
			if err := db.SavePixivCredential(context.Background(), account.New(id, "local", []byte(fmt.Sprintf("synthetic-%d", id)))); err != nil {
				t.Fatal(err)
			}
		}
		p := &migrationValidationPort{Service: account.NewService(db, &migrationValidationDefaults{1})}
		var routes []string
		var selected int64
		p.transport = func(req *http.Request) (*http.Response, error) {
			body := ""
			if req.Method == "POST" {
				if err := req.ParseForm(); err != nil {
					t.Fatal(err)
				}
				fmt.Sscanf(req.PostForm.Get("refresh_token"), "synthetic-%d", &selected)
				routes = append(routes, fmt.Sprintf("oauth:%d", selected))
				body = fmt.Sprintf(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated-%d","user":{"id":%d}}`, selected, selected)
			} else {
				routes = append(routes, fmt.Sprintf("profile:%d", selected))
				if fail && selected == 1 {
					return nil, fmt.Errorf("synthetic profile failure")
				}
				body = fmt.Sprintf(`{"user":{"id":%d},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}}`, selected)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
		}
		var out bytes.Buffer
		cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader("bad ignored stdin"), Output: &out, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
			return authcommands.AccountService{Pixiv: p, LoadRuntime: func() (config.RuntimeConfig, error) { return config.RuntimeConfig{}, nil }}, nil
		}})
		cmd.SetArgs([]string{"refresh", "--all"})
		cmd.SilenceErrors = true
		cmd.SilenceUsage = true
		err = cmd.Execute()
		if (err != nil) != fail {
			t.Fatalf("fail=%t err=%v", fail, err)
		}
		want := "✓ refreshed uid:2 premium:yes\n✓ refreshed uid:1 premium:yes\n"
		if fail {
			want = ""
		}
		if out.String() != want || strings.Join(routes, ",") != "oauth:2,profile:2,oauth:1,profile:1" {
			t.Fatalf("output=%q routes=%v", out.String(), routes)
		}
		for _, id := range []int64{1, 2} {
			a, err := db.GetPixiv(context.Background(), id)
			if err != nil || a.CredentialRevision != 2 || string(a.RefreshTokenCopy()) != fmt.Sprintf("synthetic-rotated-%d", id) {
				t.Fatal("partial commits retained")
			}
			if (a.PremiumStatus != nil) != (!fail || id == 2) {
				t.Fatal("premium commit boundary")
			}
		}
	}
}

type migrationValidationProxyPort struct {
	authcommands.AccountPort
	proxy string
}

func (p *migrationValidationProxyPort) ListAccounts(context.Context) ([]account.AccountSummary, error) {
	return nil, nil
}
func (p *migrationValidationProxyPort) record(o pixiv.Options) (account.AccountSummary, error) {
	transport := o.HTTPClient.Transport.(*http.Transport)
	if transport.Proxy != nil {
		req, _ := http.NewRequest("GET", "https://app-api.pixiv.net", nil)
		proxy, err := transport.Proxy(req)
		if err != nil {
			return account.AccountSummary{}, err
		}
		p.proxy = proxy.String()
	}
	return account.AccountSummary{UserID: 42}, nil
}
func (p *migrationValidationProxyPort) ImportAccountWith(_ context.Context, _ string, _ bool, o pixiv.Options) (account.AccountSummary, error) {
	return p.record(o)
}
func (p *migrationValidationProxyPort) CheckAccountWith(_ context.Context, _ int64, o pixiv.Options) (account.AccountSummary, error) {
	return p.record(o)
}
func (p *migrationValidationProxyPort) RefreshAccountWith(_ context.Context, _ int64, o pixiv.Options) (account.AccountSummary, error) {
	return p.record(o)
}
func TestMigrationAuthValidationProxyFalsePreservesConfiguredProxy(t *testing.T) {
	for _, op := range []string{"import", "check", "refresh"} {
		for _, flag := range []string{"--no-proxy=false", "--no-proxy", "--proxy=http://command-proxy:8080"} {
			p := &migrationValidationProxyPort{}
			cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(""), Output: io.Discard, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
				return authcommands.AccountService{Pixiv: p, LoadRuntime: func() (config.RuntimeConfig, error) {
					return config.RuntimeConfig{HTTPSProxy: "http://synthetic-proxy:8080"}, nil
				}}, nil
			}})
			value := "42"
			if op == "import" {
				value = "synthetic-token"
			}
			cmd.SetArgs([]string{op, value, flag})
			cmd.SilenceErrors = true
			cmd.SilenceUsage = true
			if err := cmd.Execute(); err != nil {
				t.Fatal(err)
			}
			want := "http://synthetic-proxy:8080"
			if flag == "--no-proxy" {
				want = ""
			} else if strings.HasPrefix(flag, "--proxy=") {
				want = "http://command-proxy:8080"
			}
			if p.proxy != want {
				t.Fatalf("%s %s proxy=%q", op, flag, p.proxy)
			}
		}
	}
}

func TestMigrationAuthValidationJSONTimeFailureAfterAllCommits(t *testing.T) {
	db, err := database.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	for _, id := range []int64{2, 1} {
		a := account.New(id, "local", []byte(fmt.Sprintf("synthetic-%d", id)))
		if id == 2 {
			f := int64(253402300800)
			a.PoolFrozenUntil = &f
		}
		if err := db.SavePixivCredential(context.Background(), a); err != nil {
			t.Fatal(err)
		}
	}
	p := &migrationValidationPort{Service: account.NewService(db, &migrationValidationDefaults{2})}
	var selected int64
	requests := 0
	p.transport = func(req *http.Request) (*http.Response, error) {
		requests++
		body := ""
		if req.Method == "POST" {
			if err := req.ParseForm(); err != nil {
				t.Fatal(err)
			}
			fmt.Sscanf(req.PostForm.Get("refresh_token"), "synthetic-%d", &selected)
			body = fmt.Sprintf(`{"access_token":"synthetic-access","refresh_token":"synthetic-rotated-%d","user":{"id":%d}}`, selected, selected)
		} else {
			body = fmt.Sprintf(`{"user":{"id":%d},"profile":{"is_premium":true},"profile_publicity":{},"workspace":{}}`, selected)
		}
		return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
	}
	var out bytes.Buffer
	cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(""), Output: &out, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
		return authcommands.AccountService{Pixiv: p, LoadRuntime: func() (config.RuntimeConfig, error) { return config.RuntimeConfig{}, nil }}, nil
	}})
	cmd.SetArgs([]string{"refresh", "--all", "--json"})
	cmd.SilenceErrors = true
	cmd.SilenceUsage = true
	err = cmd.Execute()
	if err == nil || err.Error() != "json: error calling MarshalJSON for type *time.Time: year outside of range [0,9999]" {
		t.Fatalf("JSON error %v", err)
	}
	if out.Len() != 0 || requests != 4 {
		t.Fatal("JSON serialization occurs after all refresh calls")
	}
	for _, id := range []int64{1, 2} {
		a, err := db.GetPixiv(context.Background(), id)
		if err != nil || a.CredentialRevision != 2 || a.PremiumStatus == nil {
			t.Fatal("all commits precede serialization")
		}
	}
}

type migrationValidationRuntimePort struct{ migrationValidationProxyPort }

func (p *migrationValidationRuntimePort) ListAccounts(context.Context) ([]account.AccountSummary, error) {
	return []account.AccountSummary{{UserID: 2}, {UserID: 1}}, nil
}
func TestMigrationAuthValidationExplicitProxySkipsPerAccountRuntimeLoader(t *testing.T) {
	for _, explicit := range []bool{false, true} {
		p := &migrationValidationRuntimePort{}
		loads := 0
		var out bytes.Buffer
		cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(""), Output: &out, ErrorOutput: io.Discard, CanPrompt: func() bool { return false }, Account: func() (authcommands.AccountService, error) {
			return authcommands.AccountService{Pixiv: p, LoadRuntime: func() (config.RuntimeConfig, error) {
				loads++
				if loads == 2 {
					return config.RuntimeConfig{}, fmt.Errorf("synthetic runtime failure")
				}
				return config.RuntimeConfig{}, nil
			}}, nil
		}})
		args := []string{"refresh", "--all"}
		if explicit {
			args = append(args, "--no-proxy")
		}
		cmd.SetArgs(args)
		cmd.SilenceErrors = true
		cmd.SilenceUsage = true
		err := cmd.Execute()
		if (err == nil) != explicit || loads != map[bool]int{false: 2, true: 0}[explicit] {
			t.Fatalf("explicit=%t err=%v loads=%d", explicit, err, loads)
		}
		if !explicit && out.Len() != 0 {
			t.Fatal("runtime failure suppresses buffered output")
		}
	}
}

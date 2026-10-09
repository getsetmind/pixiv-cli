package auth

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivaccount "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/spf13/cobra"
)

type migrationCLIConfig struct {
	Open       bool   `json:"login_open_browser"`
	Use        bool   `json:"login_use_after_login"`
	Public     string `json:"login_relay_public_url"`
	Listen     string `json:"login_relay_listen_addr"`
	Cert       string `json:"login_relay_tls_cert_file"`
	Key        string `json:"login_relay_tls_key_file"`
	Proxy      string `json:"https_proxy"`
	PixivProxy string `json:"pixiv_proxy"`
}

func (c migrationCLIConfig) runtime(replace *strings.Replacer) config.RuntimeConfig {
	return config.RuntimeConfig{LoginOpenBrowser: c.Open, LoginUseAfterLogin: c.Use, LoginRelayPublicURL: replace.Replace(c.Public), LoginRelayListenAddr: replace.Replace(c.Listen), LoginRelayTLSCertFile: replace.Replace(c.Cert), LoginRelayTLSKeyFile: replace.Replace(c.Key), HTTPSProxy: c.Proxy, PixivNetwork: config.PixivNetworkConfig{ProxyURL: config.OptionalString{Present: c.PixivProxy != "", Value: c.PixivProxy}}}
}

type migrationCLIExpected struct {
	Error            string   `json:"error"`
	Stdout           string   `json:"stdout"`
	Stderr           string   `json:"stderr"`
	Events           []string `json:"events"`
	DependencyEvents []string `json:"dependency_events"`
	Default          int64    `json:"default_user_id"`
	Stored           bool     `json:"stored"`
	FinalStatus      int      `json:"final_status"`
	Deadline         bool     `json:"deadline"`
}
type migrationCLIFlow struct {
	Name            string               `json:"name"`
	Args            []string             `json:"args"`
	Runtime         migrationCLIConfig   `json:"runtime"`
	Mode            string               `json:"mode"`
	Submission      string               `json:"submission"`
	HookErrors      bool                 `json:"hook_errors"`
	OAuthStatus     int                  `json:"oauth_status"`
	CommandCanceled bool                 `json:"command_canceled"`
	Expected        migrationCLIExpected `json:"expected"`
}
type migrationCLIFixture struct {
	Help             string `json:"help"`
	AuthorizationURL string `json:"authorization_url"`
	Code             string `json:"code"`
	OAuthResponse    string `json:"oauth_response"`
	OAuthFailure     string `json:"oauth_failure"`
	Validation       []struct {
		Name         string               `json:"name"`
		Args         []string             `json:"args"`
		Runtime      migrationCLIConfig   `json:"runtime"`
		ServiceError string               `json:"service_error"`
		Expected     migrationCLIExpected `json:"expected"`
	} `json:"validation"`
	Flows         []migrationCLIFlow `json:"flows"`
	TerminalInput []struct {
		Name         string `json:"name"`
		Input        string `json:"input"`
		DefaultValue string `json:"default_value"`
		Value        string `json:"value"`
		Error        string `json:"error"`
	} `json:"terminal_input"`
}

func migrationCLILoad(t *testing.T) migrationCLIFixture {
	t.Helper()
	body, err := os.ReadFile("../../../../../crates/pixiv-cli/tests/fixtures/cli_login_flow.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationCLIFixture
	if err = json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	return fixture
}

type migrationCLIEvents struct {
	mu     sync.Mutex
	values []string
}

func (e *migrationCLIEvents) add(value string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.values = append(e.values, value)
}
func (e *migrationCLIEvents) snapshot() []string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return append([]string{}, e.values...)
}

type migrationCLIBuffer struct {
	mu sync.Mutex
	bytes.Buffer
	ready    chan string
	marker   string
	once     sync.Once
	writeErr error
	writes   int
}

func (b *migrationCLIBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.writes++
	if b.writeErr != nil {
		return 0, b.writeErr
	}
	n, _ := b.Buffer.Write(p)
	if b.ready != nil && strings.HasPrefix(string(p), b.marker) {
		b.once.Do(func() { b.ready <- strings.TrimSpace(strings.TrimPrefix(string(p), b.marker)) })
	}
	return n, nil
}
func (b *migrationCLIBuffer) WriteString(value string) (int, error) { return b.Write([]byte(value)) }
func (b *migrationCLIBuffer) text() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.Buffer.String()
}
func migrationCLICommand(deps Deps, args []string) *cobra.Command {
	root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
	root.SetOut(deps.Output)
	root.SetErr(deps.ErrorOutput)
	root.AddCommand(New(deps))
	root.SetArgs(append([]string{"auth", "login"}, args...))
	return root
}
func migrationCLIError(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}
func migrationCLIAssert(t *testing.T, what string, got, want any) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%s:\ngot  %#v\nwant %#v", what, got, want)
	}
}
func migrationCLIHookFixture(t *testing.T, open func(string) error, install urlSchemeRelayInstaller, ensure urlSchemeRelayEnsurer) {
	t.Helper()
	loginHooksMu.Lock()
	oldOpen, oldInstall, oldEnsure := openBrowser, installURLSchemeRelay, ensureURLSchemeRelay
	openBrowser, installURLSchemeRelay, ensureURLSchemeRelay = open, install, ensure
	loginHooksMu.Unlock()
	t.Cleanup(func() {
		loginHooksMu.Lock()
		defer loginHooksMu.Unlock()
		openBrowser, installURLSchemeRelay, ensureURLSchemeRelay = oldOpen, oldInstall, oldEnsure
	})
}
func TestMigrationCLILoginValidationFixture(t *testing.T) {
	fixture := migrationCLILoad(t)
	for _, row := range fixture.Validation {
		t.Run(row.Name, func(t *testing.T) {
			events := &migrationCLIEvents{}
			var out, log bytes.Buffer
			deps := Deps{Input: strings.NewReader(""), Output: &out, ErrorOutput: &log, CanPrompt: func() bool { t.Fatal("validation reached listener"); return false }, Account: func() (AccountService, error) {
				events.add("account")
				if row.ServiceError == "account" {
					return AccountService{}, errors.New("synthetic account factory failure")
				}
				return AccountService{}, nil
			}, Login: func() (pixivaccount.LoginService, error) {
				events.add("login")
				if row.ServiceError == "login" {
					return pixivaccount.LoginService{}, errors.New("synthetic login factory failure")
				}
				return pixivaccount.LoginService{}, nil
			}, LoadRuntime: func() (config.RuntimeConfig, error) {
				events.add("runtime")
				if row.ServiceError == "runtime" {
					return config.RuntimeConfig{}, errors.New("synthetic runtime failure")
				}
				return row.Runtime.runtime(strings.NewReplacer()), nil
			}}
			if row.ServiceError == "missing_runtime" {
				deps.LoadRuntime = nil
			}
			migrationCLIHookFixture(t, func(string) error { t.Fatal("validation opened browser"); return nil }, func(context.Context, string) (func(), error) {
				t.Fatal("validation installed handler")
				return nil, nil
			}, func(context.Context) error { t.Fatal("validation ensured handler"); return nil })
			err := migrationCLICommand(deps, row.Args).Execute()
			migrationCLIAssert(t, "error", migrationCLIError(err), row.Expected.Error)
			migrationCLIAssert(t, "dependency order", events.snapshot(), row.Expected.DependencyEvents)
			migrationCLIAssert(t, "stdout", out.String(), "")
			migrationCLIAssert(t, "stderr", log.String(), "")
			if strings.Contains(migrationCLIError(err), "synthetic-password") {
				t.Fatal("validation leaked proxy/relay credentials")
			}
		})
	}
}

type migrationCLIFileStore struct{ file string }

func (f migrationCLIFileStore) Path() (string, error)             { return f.file, nil }
func (f migrationCLIFileStore) ReadFile(p string) ([]byte, error) { return os.ReadFile(p) }
func (f migrationCLIFileStore) WritePrivateFile(p string, b []byte) error {
	return os.WriteFile(p, b, 0600)
}
func (f migrationCLIFileStore) EnsurePrivateFile(p string, b []byte) error {
	if _, err := os.Stat(p); err == nil {
		return nil
	}
	return f.WritePrivateFile(p, b)
}

type migrationCLIRepository struct {
	*database.DB
	deadline   bool
	contextErr error
}

func (r *migrationCLIRepository) SavePixivCredential(ctx context.Context, account pixivaccount.Account) error {
	_, r.deadline = ctx.Deadline()
	r.contextErr = ctx.Err()
	err := r.DB.SavePixivCredential(ctx, account)
	return err
}
func migrationCLIAuthorization(t *testing.T, log string, fixture migrationCLIFixture) string {
	t.Helper()
	parts := strings.Split(log, "Open this Pixiv login URL:\n")
	if len(parts) != 2 {
		t.Fatalf("authorization diagnostic=%q", log)
	}
	raw := strings.SplitN(parts[1], "\n", 2)[0]
	migrationCLIValidateAuthorization(t, raw, fixture)
	return raw
}
func migrationCLIValidateAuthorization(t *testing.T, raw string, fixture migrationCLIFixture) {
	t.Helper()
	u, err := url.Parse(raw)
	if err != nil {
		t.Fatal(err)
	}
	q := u.Query()
	for _, key := range []string{"state", "code_challenge"} {
		decoded, err := base64.RawURLEncoding.DecodeString(q.Get(key))
		if err != nil || len(decoded) != 32 {
			t.Fatalf("invalid %s shape", key)
		}
	}
	normalized := strings.NewReplacer(q.Get("state"), "<STATE>", q.Get("code_challenge"), "<CHALLENGE>").Replace(raw)
	migrationCLIAssert(t, "authorization URL", normalized, fixture.AuthorizationURL)
}

type migrationCLIHTTPReply struct {
	response *http.Response
	err      error
}

func migrationCLIGet(t *testing.T, ch <-chan migrationCLIHTTPReply) *http.Response {
	t.Helper()
	select {
	case r := <-ch:
		if r.err != nil {
			t.Fatal(r.err)
		}
		return r.response
	case <-time.After(5 * time.Second):
		t.Fatal("login HTTP response did not return")
		return nil
	}
}
func migrationCLIReadBody(t *testing.T, response *http.Response) string {
	t.Helper()
	defer response.Body.Close()
	body, err := io.ReadAll(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	return string(body)
}
func migrationCLIPage(t *testing.T, response *http.Response, status int) {
	t.Helper()
	body := migrationCLIReadBody(t, response)
	migrationCLIAssert(t, "final status", response.StatusCode, status)
	kind := "success"
	if status != 200 {
		kind = "failure"
	}
	fixture, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/login_page.json")
	if err != nil {
		t.Fatal(err)
	}
	var pages []struct{ Name, SHA256 string }
	if err = json.Unmarshal(fixture, &pages); err != nil {
		t.Fatal(err)
	}
	for _, page := range pages {
		if page.Name == kind {
			migrationCLIAssert(t, "final page", fmt.Sprintf("%x", sha256.Sum256([]byte(body))), page.SHA256)
			return
		}
	}
	t.Fatal("missing final-page fixture")
}
func TestMigrationCLILoginFlowFixture(t *testing.T) {
	fixture := migrationCLILoad(t)
	for _, row := range fixture.Flows {
		t.Run(row.Name, func(t *testing.T) { migrationCLIRunFlow(t, fixture, row) })
	}
}
func migrationCLIRunFlow(t *testing.T, fixture migrationCLIFixture, row migrationCLIFlow, outputErrors ...error) {
	t.Helper()
	events, dependencies := &migrationCLIEvents{}, &migrationCLIEvents{}
	out, log := &migrationCLIBuffer{}, &migrationCLIBuffer{ready: make(chan string, 1), marker: "Manual fallback page: "}
	if len(outputErrors) > 0 {
		out.writeErr = outputErrors[0]
	}
	if row.Mode == "relay" {
		log.marker = "Open remote Pixiv login session:\n"
	}
	db, err := database.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })
	if err = db.SavePixivCredential(context.Background(), pixivaccount.New(7, "previous-account", []byte("previous-synthetic-refresh"))); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(t.TempDir(), "config.toml")
	if err = os.WriteFile(file, []byte("# preserve this comment\n[pixiv.auth]\ndefault_user_id = 7\n"), 0600); err != nil {
		t.Fatal(err)
	}
	defaults := config.Store{Files: migrationCLIFileStore{file}}
	repo := &migrationCLIRepository{DB: db}
	service := pixivaccount.NewService(repo, defaults)
	address := "127.0.0.1:0"
	if row.Mode == "relay" {
		ln, err := net.Listen("tcp", address)
		if err != nil {
			t.Fatal(err)
		}
		address = ln.Addr().String()
		if err = ln.Close(); err != nil {
			t.Fatal(err)
		}
	}
	public := "http://" + address
	cert, key := filepath.Join(t.TempDir(), "missing-private-cert.pem"), filepath.Join(t.TempDir(), "missing-private-key.pem")
	replace := strings.NewReplacer("<ADDR>", address, "<RELAY>", public, "<CERT>", cert, "<KEY>", key)
	args := make([]string, len(row.Args))
	for i, arg := range row.Args {
		args[i] = replace.Replace(arg)
	}
	cfg := row.Runtime.runtime(replace)
	commandDone := make(chan error, 1)
	oauthEntered := make(chan url.Values, 1)
	oauthRelease := make(chan struct{})
	var releaseOnce sync.Once
	release := func() { releaseOnce.Do(func() { close(oauthRelease) }) }
	defer release()
	tlsServer := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" || r.URL.Path != "/auth/token" || r.Host != "oauth.secure.pixiv.net" {
			t.Error("unexpected OAuth request destination")
			http.Error(w, "unexpected request", 500)
			return
		}
		if err := r.ParseForm(); err != nil {
			t.Error(err)
			return
		}
		events.add("oauth")
		oauthEntered <- r.PostForm
		select {
		case <-oauthRelease:
		case <-r.Context().Done():
			return
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(row.OAuthStatus)
		body := fixture.OAuthResponse
		if row.OAuthStatus != 200 {
			body = fixture.OAuthFailure
		}
		_, _ = io.WriteString(w, body)
	}))
	t.Cleanup(tlsServer.Close)
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.Proxy = nil
	transport.TLSClientConfig = tlsServer.Client().Transport.(*http.Transport).TLSClientConfig.Clone()
	transport.TLSClientConfig.ServerName = "example.com"
	if transport.TLSClientConfig.InsecureSkipVerify {
		t.Fatal("synthetic OAuth TLS must validate its fixture certificate")
	}
	transport.DialContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		if address != "oauth.secure.pixiv.net:443" {
			return nil, fmt.Errorf("unexpected synthetic OAuth dial destination")
		}
		return (&net.Dialer{}).DialContext(ctx, network, tlsServer.Listener.Addr().String())
	}
	oldTransport := http.DefaultTransport
	http.DefaultTransport = transport
	t.Cleanup(func() { http.DefaultTransport = oldTransport; transport.CloseIdleConnections() })
	migrationCLIHookFixture(t, func(raw string) error {
		if strings.HasPrefix(raw, "https://accounts.pixiv.net/post-redirect?") {
			events.add("open_relay")
		} else {
			events.add("open")
			migrationCLIValidateAuthorization(t, raw, fixture)
		}
		if row.HookErrors {
			return errors.New("synthetic open failure")
		}
		return nil
	}, func(ctx context.Context, callback string) (func(), error) {
		events.add("install")
		if !strings.HasPrefix(callback, "http://127.0.0.1:") || !strings.HasSuffix(callback, "/callback") {
			t.Error("installer callback address")
		}
		_, deadline := ctx.Deadline()
		migrationCLIAssert(t, "installer deadline", deadline, row.Expected.Deadline)
		if ctx.Err() != nil {
			t.Error("command context reached installer")
		}
		if row.HookErrors {
			return nil, errors.New("synthetic install failure")
		}
		return func() { events.add("temporary_cleanup") }, nil
	}, func(ctx context.Context) error {
		events.add("ensure")
		if ctx.Err() != nil {
			t.Error("command context reached ensurer")
		}
		if row.HookErrors {
			return errors.New("synthetic ensure failure")
		}
		return nil
	})
	promptCount := 0
	input := strings.NewReader("synthetic stdin must stay unread")
	deps := Deps{Input: input, Output: out, ErrorOutput: log, Account: func() (AccountService, error) {
		dependencies.add("account")
		return AccountService{Pixiv: service}, nil
	}, Login: func() (pixivaccount.LoginService, error) {
		dependencies.add("login")
		return pixivaccount.LoginService{Pixiv: service}, nil
	}, LoadRuntime: func() (config.RuntimeConfig, error) { dependencies.add("runtime"); return cfg, nil }, CanPrompt: func() bool { events.add("can_prompt"); return strings.HasPrefix(row.Submission, "terminal") }, PromptInput: func(message, defaultValue string) (string, error) {
		events.add("prompt")
		migrationCLIAssert(t, "prompt label", message, "Paste the returned Pixiv sign-in address, relay address, or value")
		migrationCLIAssert(t, "prompt default", defaultValue, "")
		promptCount++
		if row.Submission == "terminal_relay" && promptCount == 1 {
			u, _ := url.Parse(migrationCLIAuthorization(t, log.text(), fixture))
			start := "https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge=" + u.Query().Get("code_challenge")
			return "https://accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape(start), nil
		}
		return "  " + fixture.Code + "  ", nil
	}}
	cmd := migrationCLICommand(deps, args)
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	if row.CommandCanceled {
		cancel()
	}
	go func() { commandDone <- cmd.ExecuteContext(ctx) }()
	var published string
	select {
	case published = <-log.ready:
	case err := <-commandDone:
		t.Fatalf("login ended before listener publication: %v", err)
	case <-time.After(5 * time.Second):
		t.Fatal("listener diagnostic was not published")
	}
	client := &http.Client{Timeout: 4 * time.Second, Transport: &http.Transport{}, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	t.Cleanup(client.CloseIdleConnections)
	var loginURL, sessionID, proof, resultID string
	var responseCh chan migrationCLIHTTPReply
	var callbackBody chan string
	base := strings.TrimSuffix(published, "/")
	if row.Mode == "relay" {
		base = public
		sessionURL, _ := url.Parse(published)
		sessionID = path.Base(sessionURL.Path)
	} else {
		loginURL = migrationCLIAuthorization(t, log.text(), fixture)
		address = strings.TrimPrefix(base, "http://")
	}
	if row.Submission == "relay" {
		response, err := client.Get(published)
		if err != nil {
			t.Fatal(err)
		}
		migrationCLIAssert(t, "session handoff status", response.StatusCode, 303)
		start, err := loginhelper.ParseRemoteLoginLink(response.Header.Get("Location"))
		if err != nil {
			t.Fatal(err)
		}
		response.Body.Close()
		proof = start.Proof
		body, _ := json.Marshal(map[string]string{"proof": proof})
		response, err = client.Post(base+"/start/"+sessionID, "application/json", bytes.NewReader(body))
		if err != nil {
			t.Fatal(err)
		}
		var startResponse struct {
			AuthorizationURL string `json:"authorization_url"`
		}
		if err = json.Unmarshal([]byte(migrationCLIReadBody(t, response)), &startResponse); err != nil {
			t.Fatal(err)
		}
		loginURL = startResponse.AuthorizationURL
		migrationCLIValidateAuthorization(t, loginURL, fixture)
		body, _ = json.Marshal(map[string]string{"proof": proof, "callback_url": "pixiv://account/login?code=" + fixture.Code})
		response, err = client.Post(base+"/callback/"+sessionID, "application/json", bytes.NewReader(body))
		if err != nil {
			t.Fatal(err)
		}
		migrationCLIAssert(t, "callback status", response.StatusCode, 200)
		resultURL := response.Header.Get(loginhelper.RelayResultURLHeader)
		resultURLParsed, err := url.Parse(resultURL)
		if err != nil {
			t.Fatal(err)
		}
		resultID = path.Base(resultURLParsed.Path)
		callbackBody = make(chan string, 1)
		go func() { body, _ := io.ReadAll(response.Body); _ = response.Body.Close(); callbackBody <- string(body) }()
		responseCh = make(chan migrationCLIHTTPReply, 1)
		go func() { response, err := client.Get(resultURL); responseCh <- migrationCLIHTTPReply{response, err} }()
	} else if row.Submission == "manual" || row.Submission == "callback" {
		responseCh = make(chan migrationCLIHTTPReply, 1)
		go func() {
			var response *http.Response
			var err error
			if row.Submission == "callback" {
				u, _ := url.Parse(loginURL)
				response, err = client.Get(base + "/callback?" + url.Values{"code": {fixture.Code}, "state": {u.Query().Get("state")}}.Encode())
			} else {
				response, err = client.PostForm(base+"/manual", url.Values{"login_result": {"  " + fixture.Code + "  "}})
			}
			responseCh <- migrationCLIHTTPReply{response, err}
		}()
	}
	if row.Submission != "none" {
		select {
		case form := <-oauthEntered:
			migrationCLIAssert(t, "OAuth grant", form.Get("grant_type"), "authorization_code")
			migrationCLIAssert(t, "OAuth code", form.Get("code"), fixture.Code)
			verifier, err := base64.RawURLEncoding.DecodeString(form.Get("code_verifier"))
			if err != nil || len(verifier) != 64 {
				t.Fatal("OAuth verifier shape")
			}
			u, _ := url.Parse(loginURL)
			hash := sha256.Sum256([]byte(form.Get("code_verifier")))
			migrationCLIAssert(t, "PKCE binding", base64.RawURLEncoding.EncodeToString(hash[:]), u.Query().Get("code_challenge"))
			migrationCLIAssert(t, "OAuth redirect", form.Get("redirect_uri"), "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback")
		case <-time.After(5 * time.Second):
			t.Fatal("synthetic OAuth exchange did not start")
		}
		select {
		case err := <-commandDone:
			t.Fatalf("login completed before OAuth: %v", err)
		default:
		}
		if responseCh != nil {
			select {
			case <-responseCh:
				t.Fatal("browser received final page before OAuth completed")
			default:
			}
		}
		if !strings.HasPrefix(row.Submission, "terminal") && out.text() != "" {
			t.Fatal("account summary preceded OAuth completion")
		}
		release()
	}
	if responseCh != nil {
		migrationCLIPage(t, migrationCLIGet(t, responseCh), row.Expected.FinalStatus)
	}
	select {
	case err = <-commandDone:
	case <-time.After(5 * time.Second):
		t.Fatal("CLI login did not finish")
	}
	commandError := migrationCLIError(err)
	migrationCLIAssert(t, "command error", commandError, row.Expected.Error)
	if callbackBody != nil {
		want := "{\"success\":true}\n"
		if row.Expected.FinalStatus != 200 {
			want = "{\"success\":false}\n"
		}
		select {
		case body := <-callbackBody:
			migrationCLIAssert(t, "relay final JSON", body, want)
		case <-time.After(5 * time.Second):
			t.Fatal("relay callback did not reach EOF")
		}
	}
	if row.Expected.Error == "context deadline exceeded" && !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("timeout lost context cause")
	}
	migrationCLIAssert(t, "stdout", out.text(), row.Expected.Stdout)
	migrationCLIAssert(t, "stdin remains unread", input.Len(), len("synthetic stdin must stay unread"))
	if out.writeErr != nil {
		want := 3
		for _, arg := range row.Args {
			if arg == "--json" {
				want = 1
			}
		}
		out.mu.Lock()
		writes := out.writes
		out.mu.Unlock()
		migrationCLIAssert(t, "summary writer attempts", writes, want)
	}
	migrationCLIAssert(t, "hooks and OAuth", events.snapshot(), row.Expected.Events)
	migrationCLIAssert(t, "dependency order", dependencies.snapshot(), row.Expected.DependencyEvents)
	_, port, _ := net.SplitHostPort(address)
	// Only per-attempt values are normalized; wording, order and line boundaries remain exact.
	normalize := strings.NewReplacer(public, "<RELAY>", address, "<ADDR>", sessionID, "<SESSION>")
	if row.Mode != "relay" {
		normalize = strings.NewReplacer(loginURL, "<LOGIN_URL>", address, "<ADDR>", "ssh -N -L "+port+":127.0.0.1:"+port, "ssh -N -L <PORT>:127.0.0.1:<PORT>")
	}
	migrationCLIAssert(t, "stderr", normalize.Replace(log.text()), row.Expected.Stderr)
	id, ok, err := defaults.ReadPixivDefaultUserID()
	if err != nil {
		t.Fatal(err)
	}
	if !ok {
		t.Fatal("default account disappeared")
	}
	migrationCLIAssert(t, "default account", id, row.Expected.Default)
	account, err := db.GetPixiv(context.Background(), 42)
	if row.Expected.Stored {
		if err != nil {
			t.Fatal(err)
		}
		migrationCLIAssert(t, "saved username", account.Username, "synthetic-user")
		migrationCLIAssert(t, "saved refresh token", string(account.RefreshTokenCopy()), "synthetic-refresh-secret")
		migrationCLIAssert(t, "credential revision", account.CredentialRevision, int64(1))
		migrationCLIAssert(t, "completion deadline", repo.deadline, row.Expected.Deadline)
		if repo.contextErr != nil {
			t.Fatal("command context reached completion")
		}
	} else if !errors.Is(err, pixivaccount.ErrNotFound) {
		t.Fatalf("failed login stored account: %v", err)
	}
	configBytes, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(configBytes), "# preserve this comment") {
		t.Fatal("default update damaged unrelated configuration")
	}
	for _, secret := range []string{"synthetic-refresh-secret", "synthetic-access-secret", fixture.Code, proof, resultID, cert, key} {
		if secret != "" && strings.Contains(out.text()+log.text()+commandError, secret) {
			t.Fatalf("diagnostic leaked %q", secret)
		}
	}
	// Missing-PEM ServeTLS exits before Go tracks the listener, so closure is not proven for that reference path.
	if row.Name != "relay_tls_server_error_is_sanitized" {
		connection, err := net.DialTimeout("tcp", address, 250*time.Millisecond)
		if err == nil {
			connection.Close()
			t.Fatal("CLI login cleanup left its listener alive")
		}
	}
}

func TestMigrationCLILoginWriterFailureDoesNotUndoFinalSuccessOrSavedCredentials(t *testing.T) {
	fixture := migrationCLILoad(t)
	for _, jsonOutput := range []bool{false, true} {
		name := "text"
		if jsonOutput {
			name = "json"
		}
		t.Run(name, func(t *testing.T) {
			row := fixture.Flows[0]
			row.Name = "summary_writer_failure_" + name
			row.Expected.Stdout = ""
			row.Args = nil
			if jsonOutput {
				row.Args = []string{"--json"}
				row.Expected.Error = "synthetic output failure"
			}
			migrationCLIRunFlow(t, fixture, row, errors.New("synthetic output failure"))
		})
	}
}

func TestMigrationCLILoginRejectsUnknownNoInputFlag(t *testing.T) {
	var output, diagnostic bytes.Buffer
	command := migrationCLICommand(Deps{Output: &output, ErrorOutput: &diagnostic}, []string{"--no-input"})
	migrationCLIAssert(t, "unknown standalone flag", migrationCLIError(command.Execute()), "unknown flag: --no-input")
	migrationCLIAssert(t, "unknown flag stdout", output.String(), "")
	migrationCLIAssert(t, "unknown flag stderr", diagnostic.String(), "")
}

func TestMigrationCLILoginHelpFixture(t *testing.T) {
	fixture := migrationCLILoad(t)
	var output, diagnostic bytes.Buffer
	unexpected := func() { t.Fatal("help reached login dependencies") }
	deps := Deps{Output: &output, ErrorOutput: &diagnostic, Account: func() (AccountService, error) { unexpected(); return AccountService{}, nil }, Login: func() (pixivaccount.LoginService, error) { unexpected(); return pixivaccount.LoginService{}, nil }, LoadRuntime: func() (config.RuntimeConfig, error) { unexpected(); return config.RuntimeConfig{}, nil }, CanPrompt: func() bool { unexpected(); return false }}
	migrationCLIHookFixture(t, func(string) error { unexpected(); return nil }, func(context.Context, string) (func(), error) { unexpected(); return nil, nil }, func(context.Context) error { unexpected(); return nil })
	command := migrationCLICommand(deps, []string{"--help"})
	migrationCLIAssert(t, "help error", migrationCLIError(command.Execute()), "")
	migrationCLIAssert(t, "help stdout", output.String(), fixture.Help)
	migrationCLIAssert(t, "help stderr", diagnostic.String(), "")
}

func TestMigrationTerminalLoginInputContract(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("synthetic ANSI terminal contract is POSIX; native Windows console remains unverified")
	}
	t.Setenv("CLICOLOR_FORCE", "1")
	fixture := migrationCLILoad(t)
	for _, row := range fixture.TerminalInput {
		t.Run(row.Name, func(t *testing.T) {
			stream := &surveyContractTerminal{input: strings.NewReader(row.Input)}
			if strings.Contains(row.Name, "retry") {
				stream.firstReadLimit = strings.Index(row.Input, "\r") + 1
			}
			value, err := terminalPromptInput(stream, stream, io.Discard, "Paste the returned Pixiv sign-in address, relay address, or value", row.DefaultValue)
			migrationCLIAssert(t, "input value", value, row.Value)
			migrationCLIAssert(t, "input error", migrationCLIError(err), row.Error)
			if !strings.Contains(stream.output.String(), "\x1b[?25h") {
				t.Fatal("input prompt did not restore cursor")
			}
		})
	}
}

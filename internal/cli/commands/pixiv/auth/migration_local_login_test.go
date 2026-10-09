package auth

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"testing"
	"time"
)

const migrationLocalLoginURL = "https://app-api.pixiv.net/web/v1/login?state=one&code_challenge=two"

type migrationLocalLog struct {
	mu sync.Mutex
	bytes.Buffer
	ready chan string
	once  sync.Once
}

func (l *migrationLocalLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	n, _ := l.Buffer.Write(p)
	if strings.HasPrefix(string(p), "Manual fallback page: ") {
		l.once.Do(func() { l.ready <- strings.TrimSpace(strings.TrimPrefix(string(p), "Manual fallback page: ")) })
	}
	return n, nil
}
func (l *migrationLocalLog) text() string { l.mu.Lock(); defer l.mu.Unlock(); return l.Buffer.String() }

type migrationLocalResult struct {
	code    string
	notify  func(bool)
	cleanup func()
	err     error
}
type migrationLocalSession struct {
	base   string
	cancel context.CancelFunc
	result chan migrationLocalResult
	log    *migrationLocalLog
	output *migrationLocalLog
	client *http.Client
}

func migrationLocalStart(t *testing.T, deps Deps, accepts CallbackURLAccepter, noOpen bool, open func(string) error, install urlSchemeRelayInstaller, ensure urlSchemeRelayEnsurer) *migrationLocalSession {
	t.Helper()
	if deps.CanPrompt == nil {
		deps.CanPrompt = func() bool { return false }
	}
	ctx, cancel := context.WithCancel(context.Background())
	log := &migrationLocalLog{ready: make(chan string, 1)}
	output := &migrationLocalLog{}
	a := controller{deps: deps, out: output, errOut: log}
	s := &migrationLocalSession{cancel: cancel, result: make(chan migrationLocalResult, 1), log: log, output: output, client: &http.Client{Timeout: 3 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}}
	go func() {
		code, notify, cleanup, err := a.waitForLoginCode(ctx, "127.0.0.1:0", accepts, migrationLocalLoginURL, noOpen, open, install, ensure)
		s.result <- migrationLocalResult{code, notify, cleanup, err}
	}()
	select {
	case base := <-log.ready:
		s.base = strings.TrimSuffix(base, "/")
	case <-time.After(3 * time.Second):
		cancel()
		t.Fatal("listener did not publish fallback URL")
	}
	t.Cleanup(func() { cancel(); s.client.CloseIdleConnections() })
	return s
}
func migrationLocalTake(t *testing.T, s *migrationLocalSession) migrationLocalResult {
	t.Helper()
	select {
	case r := <-s.result:
		return r
	case <-time.After(3 * time.Second):
		t.Fatal("login wait did not return")
		return migrationLocalResult{}
	}
}
func migrationLocalRequest(t *testing.T, s *migrationLocalSession, method, path, body string) *http.Response {
	t.Helper()
	r, err := http.NewRequest(method, s.base+path, strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := s.client.Do(r)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}
func migrationLocalPage(t *testing.T, resp *http.Response, kind string, status int) {
	t.Helper()
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != status {
		t.Fatalf("status=%d want=%d body=%q", resp.StatusCode, status, body)
	}
	expected := map[string]string{"method": "method not allowed\n", "notfound": "404 page not found\n", "empty": ""}
	if v, ok := expected[kind]; ok {
		if string(body) != v {
			t.Fatalf("body=%q want=%q", body, v)
		}
		return
	}
	if kind == "redirect" {
		if string(body) != "<a href=\"/manual?x=y\">Temporary Redirect</a>.\n\n" {
			t.Fatalf("redirect body=%q", body)
		}
		return
	}
	raw, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/login_page.json")
	if err != nil {
		t.Fatal(err)
	}
	var pages []struct{ Name, URL, SHA256 string }
	if err = json.Unmarshal(raw, &pages); err != nil {
		t.Fatal(err)
	}
	for _, page := range pages {
		if page.Name == kind && (kind != "manual" || page.URL == migrationLocalLoginURL) {
			if fmt.Sprintf("%x", sha256.Sum256(body)) != page.SHA256 {
				t.Fatalf("%s page hash changed", kind)
			}
			if resp.Header.Get("Content-Type") != "text/html; charset=utf-8" {
				t.Fatal("HTML content type changed")
			}
			return
		}
	}
	t.Fatalf("no page fixture for %s", kind)
}
func TestMigrationLocalLoginHTTPFixture(t *testing.T) {
	raw, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/local_login_http.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Routes []struct {
			Name, Method, Path, Body, Page, Location string
			Status                                   int
		}
		Forms []struct{ Name, Path, Body, Code, Diagnostic string }
	}
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	s := migrationLocalStart(t, Deps{}, func(string) bool { return false }, true, nil, nil, nil)
	for _, row := range fixture.Routes {
		t.Run(row.Name, func(t *testing.T) {
			resp := migrationLocalRequest(t, s, row.Method, row.Path, row.Body)
			if resp.Header.Get("Location") != row.Location {
				t.Fatalf("location=%q", resp.Header.Get("Location"))
			}
			migrationLocalPage(t, resp, row.Page, row.Status)
		})
	}
	s.cancel()
	r := migrationLocalTake(t, s)
	if !errors.Is(r.err, context.Canceled) {
		t.Fatalf("cancel=%v", r.err)
	}
	r.cleanup()
	r.cleanup()
	for _, row := range fixture.Forms {
		t.Run(row.Name, func(t *testing.T) {
			s := migrationLocalStart(t, Deps{}, func(string) bool { return true }, true, nil, nil, nil)
			if row.Code == "" {
				migrationLocalPage(t, migrationLocalRequest(t, s, "POST", row.Path, row.Body), "failure", 400)
				if !strings.Contains(s.log.text(), "invalid login submission: "+row.Diagnostic+"\n") {
					t.Fatalf("diagnostic=%q", s.log.text())
				}
				select {
				case r := <-s.result:
					t.Fatalf("invalid input completed wait: %+v", r)
				default:
				}
				s.cancel()
				migrationLocalTake(t, s)
				return
			}
			replies := make(chan *http.Response, 1)
			go func() { replies <- migrationLocalRequest(t, s, "POST", row.Path, row.Body) }()
			r := migrationLocalTake(t, s)
			if r.err != nil || r.code != row.Code {
				t.Fatalf("result=%+v", r)
			}
			migrationLocalPage(t, migrationLocalRequest(t, s, "GET", "/", ""), "manual", 200)
			select {
			case <-replies:
				t.Fatal("final response arrived before notify")
			default:
			}
			r.notify(true)
			r.notify(false)
			migrationLocalPage(t, <-replies, "success", 200)
			r.cleanup()
			r.cleanup()
			if resp, err := s.client.Get(s.base + "/"); err == nil {
				resp.Body.Close()
				t.Fatal("cleanup left listener alive")
			}
		})
	}
}

func TestMigrationLocalLoginCallbackAndFinalFailure(t *testing.T) {
	accepted := make(chan string, 1)
	s := migrationLocalStart(t, Deps{}, func(raw string) bool { accepted <- raw; return true }, true, nil, nil, nil)
	replies := make(chan *http.Response, 1)
	go func() { replies <- migrationLocalRequest(t, s, "GET", "/callback?code=%zz&state=one#ignored", "") }()
	r := migrationLocalTake(t, s)
	want := s.base + "/callback?code=%zz&state=one"
	if r.err != nil || r.code != want || <-accepted != want {
		t.Fatalf("callback raw query/fragment=%+v want=%q", r, want)
	}
	r.notify(false)
	migrationLocalPage(t, <-replies, "failure", 400)
	r.cleanup()
}

func TestMigrationLocalLoginRelayStaysInRequestBrowser(t *testing.T) {
	s := migrationLocalStart(t, Deps{}, func(string) bool { t.Error("relay reached accepter"); return false }, true, func(string) error { t.Error("HTTP relay invoked opener"); return nil }, nil, nil)
	start := "https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge=two"
	bridge := "https://accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape(start)
	resp := migrationLocalRequest(t, s, "POST", "/manual", "login_result="+url.QueryEscape(bridge))
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != 303 || resp.Header.Get("Location") != bridge || len(body) != 0 {
		t.Fatalf("relay status=%d location=%q body=%q", resp.StatusCode, resp.Header.Get("Location"), body)
	}
	if !strings.Contains(s.log.text(), "Detected Pixiv authorization relay page; continuing it in the current browser.\n") {
		t.Fatal("missing relay diagnostic")
	}
	select {
	case <-s.result:
		t.Fatal("relay completed login")
	default:
	}
	s.cancel()
	migrationLocalTake(t, s)
}

func TestMigrationLocalLoginDuplicateFinalIsSingleConsumer(t *testing.T) {
	s := migrationLocalStart(t, Deps{}, nil, true, nil, nil, nil)
	requests := make([]*http.Request, 2)
	cancels := make([]context.CancelFunc, 2)
	responses := make(chan *http.Response, 2)
	failures := make(chan error, 2)
	for i := range 2 {
		ctx, cancel := context.WithCancel(context.Background())
		cancels[i] = cancel
		requests[i], _ = http.NewRequestWithContext(ctx, "POST", s.base+"/manual", strings.NewReader(fmt.Sprintf("code=fake-%d", i)))
		requests[i].Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	go func() {
		resp, err := s.client.Do(requests[0])
		if err != nil {
			failures <- err
		} else {
			responses <- resp
		}
	}()
	r := migrationLocalTake(t, s)
	if r.code != "fake-0" {
		t.Fatalf("first code=%q", r.code)
	}
	go func() {
		resp, err := s.client.Do(requests[1])
		if err != nil {
			failures <- err
		} else {
			responses <- resp
		}
	}()
	time.Sleep(30 * time.Millisecond)
	done := make(chan struct{})
	go func() { r.notify(true); close(done) }()
	select {
	case resp := <-responses:
		migrationLocalPage(t, resp, "success", 200)
	case <-time.After(time.Second):
		t.Fatal("no final consumer")
	}
	select {
	case <-done:
		t.Fatal("notify broadcast to duplicate")
	case <-time.After(30 * time.Millisecond):
	}
	cancels[0]()
	cancels[1]()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("disconnect did not release notify")
	}
	select {
	case err := <-failures:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("disconnect=%v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("duplicate not canceled")
	}
	r.cleanup()
	r.cleanup()
}

func TestMigrationLocalLoginContextCancelsPendingFinal(t *testing.T) {
	s := migrationLocalStart(t, Deps{}, nil, true, nil, nil, nil)
	replies := make(chan *http.Response, 1)
	go func() { replies <- migrationLocalRequest(t, s, "POST", "/manual", "code=fake") }()
	r := migrationLocalTake(t, s)
	s.cancel()
	migrationLocalPage(t, <-replies, "failure", 400)
	r.notify(true)
	r.cleanup()
}

func TestMigrationLocalLoginBindErrorRetainsTypedCause(t *testing.T) {
	a := controller{out: io.Discard, errOut: io.Discard}
	code, notify, cleanup, err := a.waitForLoginCode(context.Background(), "127.0.0.1:65536", nil, migrationLocalLoginURL, true, nil, nil, nil)
	var source *net.OpError
	if code != "" || !errors.As(err, &source) || source.Op != "listen" {
		t.Fatalf("bind error=%v", err)
	}
	var addr *net.AddrError
	if !errors.As(err, &addr) {
		t.Fatalf("bind cause=%v", err)
	}
	notify(true)
	notify(false)
	cleanup()
	cleanup()
}

func TestMigrationLocalLoginBrowserHooksAndTemporaryCleanup(t *testing.T) {
	for _, failed := range []bool{false, true} {
		t.Run(fmt.Sprintf("failure=%v", failed), func(t *testing.T) {
			var mu sync.Mutex
			var events []string
			event := func(s string) { mu.Lock(); defer mu.Unlock(); events = append(events, s) }
			ensure := func(context.Context) error {
				event("ensure")
				if failed {
					return errors.New("ensure fake failure")
				}
				return nil
			}
			install := func(_ context.Context, callback string) (func(), error) {
				event("install")
				if !strings.HasPrefix(callback, "http://127.0.0.1:") || !strings.HasSuffix(callback, "/callback") {
					t.Error("installer callback URL")
				}
				if failed {
					return nil, errors.New("install fake failure")
				}
				return func() { event("temporary cleanup") }, nil
			}
			open := func(raw string) error {
				event("open")
				if raw != migrationLocalLoginURL {
					t.Error("browser URL")
				}
				if failed {
					return errors.New("open fake failure")
				}
				return nil
			}
			s := migrationLocalStart(t, Deps{CanPrompt: func() bool { event("can prompt"); return false }}, nil, false, open, install, ensure)
			replies := make(chan *http.Response, 1)
			go func() { replies <- migrationLocalRequest(t, s, "POST", "/manual", "code=fake") }()
			r := migrationLocalTake(t, s)
			mu.Lock()
			got := strings.Join(events, ",")
			mu.Unlock()
			want := "ensure,install,can prompt,open,temporary cleanup"
			if failed {
				want = "ensure,install,can prompt,open"
			}
			if got != want {
				t.Fatalf("hook order=%q", got)
			}
			log := s.log.text()
			if failed {
				for _, line := range []string{"warning: persistent pixiv:// callback handler is unavailable: ensure fake failure\n", "warning: pixiv:// callback handler is unavailable: install fake failure\n", "warning: could not open browser: open fake failure\n"} {
					if !strings.Contains(log, line) {
						t.Fatalf("missing diagnostic %q", line)
					}
				}
			} else if !strings.Contains(log, "Registered pixiv:// callback handler for this login attempt.\nAfter confirming the Pixiv account, keep this terminal open while the browser shows the final result.\n") {
				t.Fatal("registration diagnostic")
			}
			r.notify(true)
			migrationLocalPage(t, <-replies, "success", 200)
			r.cleanup()
		})
	}
}

func TestMigrationLocalLoginNoOpenAndTerminalRetry(t *testing.T) {
	inputs := []string{"", "https://accounts.pixiv.net/post-redirect?return_to=" + url.QueryEscape("https://app-api.pixiv.net/web/v1/users/auth/pixiv/start?code_challenge=two"), "fake-terminal"}
	var opened []string
	var mu sync.Mutex
	promptDone := make(chan struct{})
	deps := Deps{CanPrompt: func() bool { return true }, PromptInput: func(message, defaultValue string) (string, error) {
		if message != "Paste the returned Pixiv sign-in address, relay address, or value" || defaultValue != "" {
			t.Error("prompt contract")
		}
		if len(inputs) == 0 {
			return "", io.EOF
		}
		input := inputs[0]
		inputs = inputs[1:]
		if len(inputs) == 0 {
			defer close(promptDone)
		}
		return input, nil
	}}
	s := migrationLocalStart(t, deps, nil, true, func(raw string) error { mu.Lock(); defer mu.Unlock(); opened = append(opened, raw); return nil }, func(context.Context, string) (func(), error) { t.Error("noOpen installer"); return nil, nil }, func(context.Context) error { t.Error("noOpen ensure"); return nil })
	r := migrationLocalTake(t, s)
	<-promptDone
	if s.output.text() != "\n\n\n" {
		t.Fatalf("terminal accepted-line separators=%q", s.output.text())
	}
	if r.err != nil || r.code != "fake-terminal" {
		t.Fatalf("terminal=%+v", r)
	}
	mu.Lock()
	count := len(opened)
	mu.Unlock()
	if count != 1 {
		t.Fatalf("terminal relay opener=%d", count)
	}
	log := s.log.text()
	for _, line := range []string{"Browser opening is disabled; use the manual fallback page or terminal prompt.\n", "An SSH tunnel alone cannot receive Pixiv's final app link; remote browser login requires a desktop handoff.\n", "invalid login submission: sign-in result cannot be empty\n", "Detected Pixiv authorization relay page; opening Pixiv relay URL once.\n"} {
		if !strings.Contains(log, line) {
			t.Fatalf("missing diagnostic %q", line)
		}
	}
	addr := strings.TrimPrefix(s.base, "http://")
	hint, _ := LoginSSHTunnelCommand(addr)
	if !strings.Contains(log, "  "+hint+"\n") {
		t.Fatal("bound port SSH hint")
	}
	r.notify(true)
	r.cleanup()
}

func TestMigrationLocalLoginFormParseBoundaries(t *testing.T) {
	cases := []struct{ name, contentType, body, diagnostic string }{
		{"malformed content type", "application/x-www-form-urlencoded;bad", "code=fake", "mime: invalid media parameter"},
		{"oversized form", "application/x-www-form-urlencoded", "code=" + strings.Repeat("x", 10<<20), "http: POST too large"},
		{"nonform ignores body", "text/plain", "code=fake", "sign-in result cannot be empty"},
	}
	for _, row := range cases {
		t.Run(row.name, func(t *testing.T) {
			s := migrationLocalStart(t, Deps{}, nil, true, nil, nil, nil)
			req, _ := http.NewRequest("POST", s.base+"/manual", strings.NewReader(row.body))
			req.Header.Set("Content-Type", row.contentType)
			resp, err := s.client.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			migrationLocalPage(t, resp, "failure", 400)
			if !strings.Contains(s.log.text(), "invalid login submission: "+row.diagnostic+"\n") {
				t.Fatalf("diagnostic=%q", s.log.text())
			}
			s.cancel()
			r := migrationLocalTake(t, s)
			if !errors.Is(r.err, context.Canceled) {
				t.Fatalf("cancel=%v", r.err)
			}
		})
	}
}

func TestMigrationLocalLoginNoOpenExactDiagnosticsAndPromptEOF(t *testing.T) {
	prompt := make(chan struct{})
	deps := Deps{CanPrompt: func() bool { return true }, PromptInput: func(string, string) (string, error) { close(prompt); return "", io.EOF }}
	s := migrationLocalStart(t, deps, nil, true, func(string) error { t.Error("noOpen auto browser"); return nil }, func(context.Context, string) (func(), error) { t.Error("noOpen installer"); return nil, nil }, func(context.Context) error { t.Error("noOpen ensure"); return nil })
	<-prompt
	addr := strings.TrimPrefix(s.base, "http://")
	hint, _ := LoginSSHTunnelCommand(addr)
	want := "Open this Pixiv login URL:\n" + migrationLocalLoginURL + "\nBrowser opening is disabled; use the manual fallback page or terminal prompt.\nWhen this CLI runs on an SSH host, forward its loopback listener from the browser machine:\n  " + hint + "\nAn SSH tunnel alone cannot receive Pixiv's final app link; remote browser login requires a desktop handoff.\nManual fallback page: " + s.base + "/\n"
	if s.log.text() != want {
		t.Fatalf("diagnostics=%q want=%q", s.log.text(), want)
	}
	select {
	case <-s.result:
		t.Fatal("prompt EOF terminated HTTP fallback")
	default:
	}
	s.cancel()
	r := migrationLocalTake(t, s)
	if !errors.Is(r.err, context.Canceled) {
		t.Fatalf("cancel=%v", r.err)
	}
	r.cleanup()
	r.notify(false)
}

func TestMigrationLocalLoginResponseFraming(t *testing.T) {
	raw, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/local_login_http.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Framing []struct {
			Method, Path     string
			ContentType      string `json:"content_type"`
			Nosniff          string
			ContentLength    int64    `json:"content_length"`
			TransferEncoding []string `json:"transfer_encoding"`
		}
	}
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	s := migrationLocalStart(t, Deps{}, nil, true, nil, nil, nil)
	for _, row := range fixture.Framing {
		before := time.Now()
		resp := migrationLocalRequest(t, s, row.Method, row.Path, "")
		after := time.Now()
		if resp.Header.Get("Content-Type") != row.ContentType || resp.Header.Get("X-Content-Type-Options") != row.Nosniff || resp.ContentLength != row.ContentLength || strings.Join(resp.TransferEncoding, ",") != strings.Join(row.TransferEncoding, ",") {
			t.Fatalf("%s %s headers=%v length=%d transfer=%v", row.Method, row.Path, resp.Header, resp.ContentLength, resp.TransferEncoding)
		}
		date, err := http.ParseTime(resp.Header.Get("Date"))
		if err != nil || date.Before(before.Truncate(time.Second)) || date.After(after) {
			t.Fatalf("Date=%q err=%v", resp.Header.Get("Date"), err)
		}
		io.Copy(io.Discard, resp.Body)
		resp.Body.Close()
	}
	s.cancel()
	migrationLocalTake(t, s)
}

func TestMigrationLocalLoginTerminalRetainsFinalForLaterHTTP(t *testing.T) {
	deps := Deps{CanPrompt: func() bool { return true }, PromptInput: func(string, string) (string, error) { return "fake-terminal", nil }}
	s := migrationLocalStart(t, deps, nil, true, nil, nil, nil)
	r := migrationLocalTake(t, s)
	if r.err != nil || r.code != "fake-terminal" || s.output.text() != "\n" {
		t.Fatalf("terminal=%+v stdout=%q", r, s.output.text())
	}
	done := make(chan struct{})
	go func() { r.notify(true); close(done) }()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("notify with no HTTP waiters blocked")
	}
	r.notify(false)
	migrationLocalPage(t, migrationLocalRequest(t, s, "POST", "/manual", "code=fake-later"), "success", 200)
	r.cleanup()
	r.cleanup()
}

func TestMigrationLocalLoginMIMEFixture(t *testing.T) {
	raw, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/local_login_http.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		MIME []struct {
			Name             string
			ContentType      *string `json:"content_type"`
			Code, Diagnostic string
		}
	}
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	for _, row := range fixture.MIME {
		t.Run(row.Name, func(t *testing.T) {
			s := migrationLocalStart(t, Deps{}, nil, true, nil, nil, nil)
			req, _ := http.NewRequest("POST", s.base+"/manual", strings.NewReader("code=fake"))
			if row.ContentType != nil {
				req.Header.Set("Content-Type", *row.ContentType)
			}
			if row.Code == "" {
				resp, err := s.client.Do(req)
				if err != nil {
					t.Fatal(err)
				}
				migrationLocalPage(t, resp, "failure", 400)
				if !strings.Contains(s.log.text(), "invalid login submission: "+row.Diagnostic+"\n") {
					t.Fatalf("diagnostic=%q", s.log.text())
				}
				s.cancel()
				r := migrationLocalTake(t, s)
				if !errors.Is(r.err, context.Canceled) {
					t.Fatalf("cancel=%v", r.err)
				}
				return
			}
			replies := make(chan *http.Response, 1)
			failed := make(chan error, 1)
			go func() {
				resp, err := s.client.Do(req)
				if err != nil {
					failed <- err
				} else {
					replies <- resp
				}
			}()
			r := migrationLocalTake(t, s)
			if r.err != nil || r.code != row.Code {
				t.Fatalf("result=%+v", r)
			}
			r.notify(true)
			select {
			case resp := <-replies:
				migrationLocalPage(t, resp, "success", 200)
			case err := <-failed:
				t.Fatal(err)
			case <-time.After(time.Second):
				t.Fatal("MIME final response missing")
			}
			r.cleanup()
		})
	}
}

func TestMigrationLocalLoginConsumedFinalLeavesLateRequestPending(t *testing.T) {
	accepted := make(chan string, 1)
	deps := Deps{CanPrompt: func() bool { return true }, PromptInput: func(string, string) (string, error) { return "fake-terminal", nil }}
	s := migrationLocalStart(t, deps, func(raw string) bool { accepted <- raw; return true }, true, nil, nil, nil)
	r := migrationLocalTake(t, s)
	r.notify(true)
	migrationLocalPage(t, migrationLocalRequest(t, s, "POST", "/manual", "code=fake-first"), "success", 200)
	replies := make(chan *http.Response, 1)
	go func() { replies <- migrationLocalRequest(t, s, "GET", "/callback?code=fake-late", "") }()
	select {
	case raw := <-accepted:
		if raw != s.base+"/callback?code=fake-late" {
			t.Fatalf("accepted=%q", raw)
		}
	case <-time.After(time.Second):
		t.Fatal("late callback not classified")
	}
	select {
	case resp := <-replies:
		resp.Body.Close()
		t.Fatal("consumed final produced immediate late response")
	case <-time.After(30 * time.Millisecond):
	}
	s.cancel()
	select {
	case resp := <-replies:
		migrationLocalPage(t, resp, "failure", 400)
	case <-time.After(time.Second):
		t.Fatal("cancel did not release late callback")
	}
	r.cleanup()
	r.notify(false)
}

func TestMigrationLocalLoginTemporaryCleanupObservesServerLifecycle(t *testing.T) {
	for _, success := range []bool{true, false} {
		t.Run(fmt.Sprintf("input_success=%v", success), func(t *testing.T) {
			observed := make(chan bool, 1)
			probe := &http.Client{Timeout: time.Second}
			defer probe.CloseIdleConnections()
			install := func(_ context.Context, callback string) (func(), error) {
				base := strings.TrimSuffix(callback, "/callback")
				return func() {
					resp, err := probe.Get(base + "/")
					alive := err == nil
					if resp != nil {
						io.Copy(io.Discard, resp.Body)
						resp.Body.Close()
					}
					observed <- alive
				}, nil
			}
			s := migrationLocalStart(t, Deps{}, nil, false, func(string) error { return nil }, install, func(context.Context) error { return nil })
			replies := make(chan *http.Response, 1)
			if success {
				go func() { replies <- migrationLocalRequest(t, s, "POST", "/manual", "code=fake") }()
			} else {
				s.cancel()
			}
			r := migrationLocalTake(t, s)
			select {
			case alive := <-observed:
				if alive != success {
					t.Fatalf("installer cleanup observed listener alive=%v want=%v", alive, success)
				}
			case <-time.After(time.Second):
				t.Fatal("temporary cleanup did not run before input wait returned")
			}
			if success {
				if r.err != nil || r.code != "fake" {
					t.Fatalf("success=%+v", r)
				}
				r.notify(true)
				migrationLocalPage(t, <-replies, "success", 200)
				r.cleanup()
			} else if !errors.Is(r.err, context.Canceled) {
				t.Fatalf("cancel=%v", r.err)
			}
			r.cleanup()
			r.cleanup()
		})
	}
}

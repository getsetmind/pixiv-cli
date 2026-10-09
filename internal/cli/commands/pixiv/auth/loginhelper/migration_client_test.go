package loginhelper_test

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
	"github.com/stretchr/testify/require"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"
)

type migrationClientCase struct {
	Name      string `json:"name"`
	Body      string `json:"body"`
	Error     string `json:"error"`
	ReadError bool   `json:"read_error"`
	DataError bool   `json:"data_error"`
}
type migrationProxyCase struct {
	Name  string            `json:"name"`
	URL   string            `json:"url"`
	Env   map[string]string `json:"env"`
	Proxy string            `json:"proxy"`
	Error string            `json:"error"`
}
type migrationClientFixture struct {
	Proxy            []migrationProxyCase  `json:"proxy"`
	AuthorizationURL string                `json:"authorization_url"`
	Start            []migrationClientCase `json:"start"`
	Completion       []migrationClientCase `json:"completion"`
}
type migrationClientBody struct {
	reader        io.Reader
	reads, closes int
	fail          bool
	dataError     bool
}

func (b *migrationClientBody) Read(p []byte) (int, error) {
	b.reads++
	n, e := b.reader.Read(p)
	if n > 0 && b.dataError {
		return n, errors.New("synthetic simultaneous read failure")
	}
	if n == 0 && e == io.EOF && b.fail {
		return 0, errors.New("synthetic read failure")
	}
	return n, e
}
func (b *migrationClientBody) Close() error {
	b.closes++
	return errors.New("synthetic ignored close failure")
}
func migrationClientSetup(t *testing.T) loginhelper.ActiveRemoteLogin {
	t.Helper()
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	return loginhelper.ActiveRemoteLogin{Version: 1, Origin: "https://relay.example/prefix", SessionID: "session", Proof: "synthetic"}
}
func migrationClientFixtureLoad(t *testing.T) migrationClientFixture {
	t.Helper()
	raw, e := os.ReadFile("../../../../../../crates/pixiv-app/tests/fixtures/handoff_client.json")
	require.NoError(t, e)
	var f migrationClientFixture
	require.NoError(t, json.Unmarshal(raw, &f))
	return f
}
func migrationClientResponse(t *testing.T, b *migrationClientBody, status int, result string, inspect func(*http.Request)) {
	t.Helper()
	t.Cleanup(loginhelper.SetHandoffHTTPClient(&http.Client{Transport: handoffRoundTripper(func(r *http.Request) (*http.Response, error) {
		if inspect != nil {
			inspect(r)
		}
		h := make(http.Header)
		h.Set(loginhelper.RelayResultURLHeader, result)
		return &http.Response{StatusCode: status, Header: h, Body: b}, nil
	})}))
}
func migrationClientExpect(t *testing.T, e error, want string) {
	t.Helper()
	if want == "" {
		require.NoError(t, e)
	} else {
		require.EqualError(t, e, want)
	}
}
func TestMigrationHandoffClientStartJSON(t *testing.T) {
	f := migrationClientFixtureLoad(t)
	for _, c := range f.Start {
		t.Run(c.Name, func(t *testing.T) {
			a := migrationClientSetup(t)
			b := &migrationClientBody{reader: strings.NewReader(c.Body), fail: c.ReadError, dataError: c.DataError}
			migrationClientResponse(t, b, 200, "", nil)
			got, e := loginhelper.StartRemoteLogin(context.Background(), loginhelper.RemoteLoginStart{Origin: a.Origin, SessionID: a.SessionID, Proof: a.Proof})
			migrationClientExpect(t, e, c.Error)
			require.Equal(t, 1, b.closes)
			state, se := loginhelper.LoadActiveRemoteLogin()
			if c.Error == "" {
				require.Equal(t, f.AuthorizationURL, got)
				require.NoError(t, se)
				require.Equal(t, a, state)
			} else {
				require.Empty(t, got)
				require.ErrorIs(t, se, loginhelper.ErrNoActiveRemoteLogin)
			}
		})
	}
}
func TestMigrationHandoffClientCompletionJSON(t *testing.T) {
	for _, c := range migrationClientFixtureLoad(t).Completion {
		t.Run(c.Name, func(t *testing.T) {
			a := migrationClientSetup(t)
			require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
			b := &migrationClientBody{reader: strings.NewReader(c.Body), fail: c.ReadError, dataError: c.DataError}
			migrationClientResponse(t, b, 200, a.Origin+"/result/YWJj", nil)
			s, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
			require.NoError(t, e)
			require.Zero(t, b.reads)
			require.Zero(t, b.closes)
			_, e = loginhelper.LoadActiveRemoteLogin()
			require.ErrorIs(t, e, loginhelper.ErrNoActiveRemoteLogin)
			migrationClientExpect(t, s.Complete(), c.Error)
			require.Equal(t, 1, b.closes)
			s.Abort()
			require.Equal(t, 1, b.closes)
			require.EqualError(t, s.Complete(), "remote Pixiv login relay did not return a final result")
			require.Equal(t, 1, b.closes)
		})
	}
}
func TestMigrationHandoffClientExactRequests(t *testing.T) {
	a := migrationClientSetup(t)
	a.SessionID = "nested/../session space"
	a.Proof = "<&>\"\\\n\u2028\u2029"
	f := migrationClientFixtureLoad(t)
	raw, _ := json.Marshal(loginhelper.RemoteLoginStartResponse{AuthorizationURL: f.AuthorizationURL})
	b := &migrationClientBody{reader: strings.NewReader(string(raw))}
	migrationClientResponse(t, b, 200, "", func(r *http.Request) {
		require.Equal(t, "POST", r.Method)
		require.Equal(t, "https://relay.example/prefix/start/session%20space", r.URL.String())
		require.Equal(t, "application/json", r.Header.Get("Content-Type"))
		raw, e := io.ReadAll(r.Body)
		require.NoError(t, e)
		require.Equal(t, `{"proof":"\u003c\u0026\u003e\"\\\n\u2028\u2029"}`, string(raw))
	})
	_, e := loginhelper.StartRemoteLogin(context.Background(), loginhelper.RemoteLoginStart{Origin: a.Origin, SessionID: a.SessionID, Proof: a.Proof})
	require.NoError(t, e)
	callback := " pixiv://account/login?code=synthetic&extra=<tag> "
	b2 := &migrationClientBody{reader: strings.NewReader(`{"success":true}`)}
	migrationClientResponse(t, b2, 200, " \t"+a.Origin+"/result/YWJj\r\n", func(r *http.Request) {
		require.Equal(t, "POST", r.Method)
		require.Equal(t, "/prefix/callback/session space", r.URL.Path)
		require.Empty(t, r.URL.RawQuery)
		require.Equal(t, "application/json", r.Header.Get("Content-Type"))
		raw, e := io.ReadAll(r.Body)
		require.NoError(t, e)
		require.Equal(t, `{"callback_url":" pixiv://account/login?code=synthetic\u0026extra=\u003ctag\u003e ","proof":"\u003c\u0026\u003e\"\\\n\u2028\u2029"}`, string(raw))
	})
	s, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), callback)
	require.NoError(t, e)
	require.Equal(t, a.Origin+"/result/YWJj", s.ResultURL)
	require.NoError(t, s.Complete())
}

func TestMigrationHandoffClientHeaderAndStateOrdering(t *testing.T) {
	for _, c := range []struct {
		name     string
		status   int
		result   string
		mutation string
		want     string
	}{
		{"status_before_header", 201, "bad", "", "remote Pixiv login relay rejected the login result"},
		{"invalid_header_keeps_state", 200, "https://other.example/result/YWJj", "", "invalid remote login relay result URL"},
		{"clear_failure", 200, "https://relay.example/prefix/result/YWJj", "invalid", "could not clear active remote login handoff"},
		{"newer_state", 200, "https://relay.example/prefix/result/YWJj", "newer", ""},
		{"missing_state_after_delivery", 200, "https://relay.example/prefix/result/YWJj", "missing", ""},
	} {
		t.Run(c.name, func(t *testing.T) {
			a := migrationClientSetup(t)
			require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
			b := &migrationClientBody{reader: strings.NewReader(`{"success":true}`)}
			statePath, e := loginhelper.ActiveRemoteLoginPath()
			require.NoError(t, e)
			migrationClientResponse(t, b, c.status, c.result, func(*http.Request) {
				switch c.mutation {
				case "invalid":
					require.NoError(t, os.WriteFile(statePath, []byte("{"), 0600))
				case "newer":
					newer := a
					newer.SessionID = "newer"
					require.NoError(t, loginhelper.SaveActiveRemoteLogin(newer))
				case "missing":
					require.NoError(t, os.Remove(statePath))
				}
			})
			s, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
			migrationClientExpect(t, e, c.want)
			require.Zero(t, b.reads)
			if c.want != "" {
				require.Nil(t, s)
				require.Equal(t, 1, b.closes)
			} else {
				require.Zero(t, b.closes)
				s.Abort()
				s.Abort()
				require.Equal(t, 1, b.closes)
				require.NoError(t, s.Complete())
				require.Equal(t, 1, b.closes)
			}
			saved, se := loginhelper.LoadActiveRemoteLogin()
			switch c.mutation {
			case "newer":
				require.NoError(t, se)
				require.Equal(t, "newer", saved.SessionID)
			case "missing":
				require.ErrorIs(t, se, loginhelper.ErrNoActiveRemoteLogin)
			case "invalid":
				require.EqualError(t, se, "active remote login handoff is invalid")
			default:
				require.NoError(t, se)
				require.Equal(t, a, saved)
			}
		})
	}
	var absent *loginhelper.RemoteCallbackSession
	absent.Abort()
	require.EqualError(t, absent.Complete(), "remote Pixiv login relay session is unavailable")
	empty := new(loginhelper.RemoteCallbackSession)
	empty.Abort()
	require.EqualError(t, empty.Complete(), "remote Pixiv login relay session is unavailable")
}

func TestMigrationHandoffClientStartStatusAndCancellation(t *testing.T) {
	a := migrationClientSetup(t)
	require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
	b := &migrationClientBody{reader: strings.NewReader("bad")}
	migrationClientResponse(t, b, 201, "", nil)
	_, e := loginhelper.StartRemoteLogin(context.Background(), loginhelper.RemoteLoginStart{Origin: a.Origin, SessionID: a.SessionID, Proof: a.Proof})
	require.EqualError(t, e, "remote Pixiv login relay rejected login handoff")
	require.Zero(t, b.reads)
	require.Equal(t, 1, b.closes)
	saved, e := loginhelper.LoadActiveRemoteLogin()
	require.NoError(t, e)
	require.Equal(t, a, saved)
	t.Cleanup(loginhelper.SetHandoffHTTPClient(&http.Client{Transport: handoffRoundTripper(func(r *http.Request) (*http.Response, error) {
		require.ErrorIs(t, r.Context().Err(), context.Canceled)
		return nil, r.Context().Err()
	})}))
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, e = loginhelper.StartRemoteLogin(ctx, loginhelper.RemoteLoginStart{Origin: a.Origin, SessionID: a.SessionID, Proof: a.Proof})
	require.EqualError(t, e, "could not contact remote Pixiv login relay")
	s, e := loginhelper.ForwardActiveRemoteLoginCallback(ctx, "pixiv://account/login?code=synthetic")
	require.Nil(t, s)
	require.EqualError(t, e, "could not contact remote Pixiv login relay")
	saved, e = loginhelper.LoadActiveRemoteLogin()
	require.NoError(t, e)
	require.Equal(t, a, saved)
}

func TestMigrationHandoffClientReceivesHeadersBeforeFinalEOF(t *testing.T) {
	a := migrationClientSetup(t)
	release := make(chan struct{})
	delivered := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.UserAgent() != "Go-http-client/1.1" {
			t.Errorf("unexpected native User-Agent: %q", r.UserAgent())
		}
		_, _ = io.Copy(io.Discard, r.Body)
		w.Header().Set(loginhelper.RelayResultURLHeader, "http://"+r.Host+"/result/YWJj")
		w.WriteHeader(200)
		w.(http.Flusher).Flush()
		close(delivered)
		select {
		case <-release:
			_, _ = io.WriteString(w, `{"success":true}`)
		case <-r.Context().Done():
		}
	}))
	defer server.Close()
	a.Origin = server.URL
	require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	s, e := loginhelper.ForwardActiveRemoteLoginCallback(ctx, "pixiv://account/login?code=synthetic")
	require.NoError(t, e)
	<-delivered
	require.Equal(t, server.URL+"/result/YWJj", s.ResultURL)
	_, e = loginhelper.LoadActiveRemoteLogin()
	require.ErrorIs(t, e, loginhelper.ErrNoActiveRemoteLogin)
	done := make(chan error, 1)
	go func() { done <- s.Complete() }()
	select {
	case e := <-done:
		t.Fatalf("completed before final body: %v", e)
	case <-time.After(20 * time.Millisecond):
	}
	close(release)
	require.NoError(t, <-done)
}

func TestMigrationHandoffProxyEnvironment(t *testing.T) {
	for _, c := range migrationClientFixtureLoad(t).Proxy {
		t.Run(c.Name, func(t *testing.T) {
			raw, e := json.Marshal(c)
			require.NoError(t, e)
			exe, e := os.Executable()
			require.NoError(t, e)
			cmd := exec.Command(exe, "-test.run=^TestMigrationHandoffProxyChild$")
			for _, entry := range os.Environ() {
				key := strings.SplitN(entry, "=", 2)[0]
				switch key {
				case "HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy", "ALL_PROXY", "all_proxy", "REQUEST_METHOD", "PIXIV_MIGRATION_PROXY_CASE":
				default:
					cmd.Env = append(cmd.Env, entry)
				}
			}
			cmd.Env = append(cmd.Env, "PIXIV_MIGRATION_PROXY_CASE="+string(raw))
			for key, value := range c.Env {
				cmd.Env = append(cmd.Env, key+"="+value)
			}
			output, e := cmd.CombinedOutput()
			require.NoError(t, e, string(output))
		})
	}
}
func TestMigrationHandoffProxyChild(t *testing.T) {
	raw := os.Getenv("PIXIV_MIGRATION_PROXY_CASE")
	if raw == "" {
		t.Skip("isolated proxy selector child")
	}
	var c migrationProxyCase
	require.NoError(t, json.Unmarshal([]byte(raw), &c))
	request, e := http.NewRequest("POST", c.URL, nil)
	require.NoError(t, e)
	proxy, e := http.ProxyFromEnvironment(request)
	migrationClientExpect(t, e, c.Error)
	if c.Proxy == "" {
		require.Nil(t, proxy)
	} else {
		require.NotNil(t, proxy)
		require.Equal(t, c.Proxy, proxy.String())
	}
}

func TestMigrationHandoffClientAbortAndCancellationUnblockCompletion(t *testing.T) {
	for _, action := range []string{"abort", "cancel"} {
		t.Run(action, func(t *testing.T) {
			a := migrationClientSetup(t)
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				_, _ = io.Copy(io.Discard, r.Body)
				w.Header().Set(loginhelper.RelayResultURLHeader, "http://"+r.Host+"/result/YWJj")
				w.WriteHeader(200)
				w.(http.Flusher).Flush()
				<-r.Context().Done()
			}))
			defer server.Close()
			a.Origin = server.URL
			require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			s, e := loginhelper.ForwardActiveRemoteLoginCallback(ctx, "pixiv://account/login?code=synthetic")
			require.NoError(t, e)
			done := make(chan error, 1)
			go func() { done <- s.Complete() }()
			if action == "abort" {
				s.Abort()
			} else {
				cancel()
			}
			select {
			case e := <-done:
				require.EqualError(t, e, "remote Pixiv login relay did not return a final result")
			case <-time.After(3 * time.Second):
				t.Fatal("completion did not unblock")
			}
			s.Abort()
			_, e = loginhelper.LoadActiveRemoteLogin()
			require.ErrorIs(t, e, loginhelper.ErrNoActiveRemoteLogin)
		})
	}
}

func TestMigrationHandoffClientValidationBeforeTransport(t *testing.T) {
	a := migrationClientSetup(t)
	var calls int
	t.Cleanup(loginhelper.SetHandoffHTTPClient(&http.Client{Transport: handoffRoundTripper(func(*http.Request) (*http.Response, error) { calls++; return nil, errors.New("unexpected transport") })}))
	for _, start := range []loginhelper.RemoteLoginStart{{Origin: "https://user@relay.example", SessionID: a.SessionID, Proof: a.Proof}, {Origin: a.Origin, SessionID: " ", Proof: a.Proof}, {Origin: a.Origin, SessionID: a.SessionID, Proof: "\u0085"}} {
		_, e := loginhelper.StartRemoteLogin(context.Background(), start)
		require.EqualError(t, e, "invalid remote login start request")
	}
	s, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://other/login?code=synthetic")
	require.Nil(t, s)
	require.EqualError(t, e, "this Pixiv login link cannot be used for remote sign-in")
	s, e = loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
	require.Nil(t, s)
	require.ErrorIs(t, e, loginhelper.ErrNoActiveRemoteLogin)
	require.Zero(t, calls)
}

func TestMigrationHandoffClientAllRedirectsNeverReplay(t *testing.T) {
	a := migrationClientSetup(t)
	for _, status := range []int{301, 302, 303, 307, 308} {
		t.Run(http.StatusText(status), func(t *testing.T) {
			var received int
			target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { received++; _, _ = io.Copy(io.Discard, r.Body) }))
			defer target.Close()
			relay := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				_, _ = io.Copy(io.Discard, r.Body)
				http.Redirect(w, r, target.URL+"/capture", status)
			}))
			defer relay.Close()
			a.Origin = relay.URL
			_, e := loginhelper.StartRemoteLogin(context.Background(), loginhelper.RemoteLoginStart{Origin: a.Origin, SessionID: a.SessionID, Proof: a.Proof})
			require.EqualError(t, e, "remote Pixiv login relay rejected login handoff")
			require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
			s, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
			require.Nil(t, s)
			require.EqualError(t, e, "remote Pixiv login relay rejected the login result")
			require.Zero(t, received)
			saved, e := loginhelper.LoadActiveRemoteLogin()
			require.NoError(t, e)
			require.Equal(t, a, saved)
		})
	}
}

func TestMigrationHandoffClientDeepTrailingRetainsFinalErrorPhase(t *testing.T) {
	a := migrationClientSetup(t)
	require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
	body := &migrationClientBody{reader: strings.NewReader(`{"success":true} ` + strings.Repeat("[", 10001) + strings.Repeat("]", 10001))}
	migrationClientResponse(t, body, 200, a.Origin+"/result/YWJj", nil)
	session, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
	require.NoError(t, e)
	require.EqualError(t, session.Complete(), "remote Pixiv login relay returned an invalid final result")
	require.Equal(t, 1, body.closes)
}

func TestMigrationHandoffClientNativeReusesConnection(t *testing.T) {
	a := migrationClientSetup(t)
	addresses := make(chan string, 2)
	f := migrationClientFixtureLoad(t)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		addresses <- r.RemoteAddr
		_, _ = io.Copy(io.Discard, r.Body)
		if r.UserAgent() != "Go-http-client/1.1" {
			t.Errorf("unexpected native User-Agent: %q", r.UserAgent())
		}
		if values, present := r.Header["Accept"]; present {
			t.Errorf("Go native handoff must omit Accept, got %q", values)
		}
		w.Header().Set("Content-Type", "application/json")
		if strings.Contains(r.URL.Path, "/start/") {
			_ = json.NewEncoder(w).Encode(loginhelper.RemoteLoginStartResponse{AuthorizationURL: f.AuthorizationURL})
		} else {
			w.Header().Set(loginhelper.RelayResultURLHeader, "http://"+r.Host+"/result/YWJj")
			_, _ = io.WriteString(w, `{"success":true}`)
		}
	}))
	defer server.Close()
	a.Origin = server.URL
	_, e := loginhelper.StartRemoteLogin(context.Background(), loginhelper.RemoteLoginStart{Origin: a.Origin, SessionID: a.SessionID, Proof: a.Proof})
	require.NoError(t, e)
	session, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
	require.NoError(t, e)
	require.NoError(t, session.Complete())
	require.Equal(t, <-addresses, <-addresses)
}

func TestMigrationHandoffClientNativeHeaderFirstValueAndUnicodeTrim(t *testing.T) {
	for _, kind := range []string{"first_invalid_utf8", "unicode_trim"} {
		t.Run(kind, func(t *testing.T) {
			a := migrationClientSetup(t)
			listener, e := net.Listen("tcp", "127.0.0.1:0")
			require.NoError(t, e)
			defer listener.Close()
			a.Origin = "http://" + listener.Addr().String()
			require.NoError(t, loginhelper.SaveActiveRemoteLogin(a))
			done := make(chan error, 1)
			go func() {
				socket, e := listener.Accept()
				if e != nil {
					done <- e
					return
				}
				defer socket.Close()
				request, e := http.ReadRequest(bufio.NewReader(socket))
				if e != nil {
					done <- e
					return
				}
				_, e = io.Copy(io.Discard, request.Body)
				if e != nil {
					done <- e
					return
				}
				header := "X-Pixiv-Relay-Result-URL: \u00a0" + a.Origin + "/result/YWJj\u00a0\r\n"
				if kind == "first_invalid_utf8" {
					header = "X-Pixiv-Relay-Result-URL: \xff\r\nX-Pixiv-Relay-Result-URL: " + a.Origin + "/result/YWJj\r\n"
				}
				_, e = fmt.Fprintf(socket, "HTTP/1.1 200 OK\r\n%sContent-Length: 16\r\nConnection: close\r\n\r\n{\"success\":true}", header)
				done <- e
			}()
			session, e := loginhelper.ForwardActiveRemoteLoginCallback(context.Background(), "pixiv://account/login?code=synthetic")
			if kind == "first_invalid_utf8" {
				require.Nil(t, session)
				require.EqualError(t, e, "invalid remote login relay result URL")
				saved, se := loginhelper.LoadActiveRemoteLogin()
				require.NoError(t, se)
				require.Equal(t, a, saved)
			} else {
				require.NoError(t, e)
				require.Equal(t, a.Origin+"/result/YWJj", session.ResultURL)
				require.NoError(t, session.Complete())
				_, se := loginhelper.LoadActiveRemoteLogin()
				require.ErrorIs(t, se, loginhelper.ErrNoActiveRemoteLogin)
			}
			require.NoError(t, <-done)
		})
	}
}

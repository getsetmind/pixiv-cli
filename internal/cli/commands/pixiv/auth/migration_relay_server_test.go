package auth_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	auth "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/stretchr/testify/require"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const migrationRelayCallback = "pixiv://account/login?code=synthetic-relay-code"
const migrationRelayAuthorization = "https://app-api.pixiv.net/web/v1/login?client=pixiv-android&code_challenge_method=S256&code_challenge=synthetic-challenge&state=synthetic-state"

type migrationRelayOutcome struct {
	code    string
	notify  func(bool)
	cleanup func()
	err     error
}
type migrationRelayHarness struct {
	address  string
	start    loginhelper.RemoteLoginStart
	output   *synchronizedOutput
	outcomes chan migrationRelayOutcome
	outcome  *migrationRelayOutcome
	watcher  chan struct{}
}

func migrationStartRelay(t *testing.T) *migrationRelayHarness {
	return migrationStartRelayWithPublicURL(t, "")
}
func migrationStartRelayWithPublicURL(t *testing.T, publicURL string) *migrationRelayHarness {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, err)
	ctx, cancel := context.WithCancel(context.Background())
	h := &migrationRelayHarness{address: "http://" + listener.Addr().String(), output: newSynchronizedOutput(), outcomes: make(chan migrationRelayOutcome, 1), watcher: make(chan struct{})}
	if publicURL == "" {
		publicURL = h.address
	}
	go func() {
		code, notify, cleanup, err := auth.WaitForHandoffRelayLoginCode(ctx, h.output, auth.RelayServerOptions{PublicURL: publicURL, ListenAddr: listener.Addr().String(), Listen: func(string, string) (net.Listener, error) { return listener, nil }, ContextWaiterExited: func() { close(h.watcher) }}, func(raw string) bool { return raw == migrationRelayCallback }, migrationRelayAuthorization)
		h.outcomes <- migrationRelayOutcome{code, notify, cleanup, err}
	}()
	t.Cleanup(func() {
		cancel()
		if h.outcome == nil {
			select {
			case result := <-h.outcomes:
				h.outcome = &result
			case <-time.After(5 * time.Second):
				t.Fatal("relay cancellation did not return")
			}
		}
		h.outcome.cleanup()
	})
	sessionURL, err := url.Parse(h.output.waitForSessionURL(t))
	require.NoError(t, err)
	h.start, err = loginhelper.ParseRemoteLoginLink(remoteLoginDeepLinkFromSessionURL(t, h.address+"/session/"+path.Base(sessionURL.Path)))
	require.NoError(t, err)
	return h
}
func (h *migrationRelayHarness) post(t *testing.T, segment, body string) *http.Response {
	t.Helper()
	response, err := http.Post(h.address+"/"+segment+"/"+h.start.SessionID, "application/json", strings.NewReader(body))
	require.NoError(t, err)
	return response
}
func migrationRelayBody(t *testing.T, response *http.Response) string {
	t.Helper()
	body, err := io.ReadAll(response.Body)
	require.NoError(t, err)
	require.NoError(t, response.Body.Close())
	return string(body)
}
func (h *migrationRelayHarness) accept(t *testing.T) {
	t.Helper()
	select {
	case result := <-h.outcomes:
		h.outcome = &result
		require.NoError(t, result.err)
		require.Equal(t, migrationRelayCallback, result.code)
	case <-time.After(5 * time.Second):
		t.Fatal("callback not delivered")
	}
	select {
	case <-h.watcher:
	case <-time.After(5 * time.Second):
		t.Fatal("context watcher not stopped after callback")
	}
}
func TestMigrationRelayServerJSONAndCapabilities(t *testing.T) {
	h := migrationStartRelay(t)
	for _, id := range []string{h.start.SessionID, h.start.Proof} {
		decoded, err := base64.RawURLEncoding.DecodeString(id)
		require.NoError(t, err)
		require.Len(t, decoded, 32)
	}
	require.NotEqual(t, h.start.SessionID, h.start.Proof)
	h.output.mu.Lock()
	output := h.output.buffer.String()
	h.output.mu.Unlock()
	require.NotContains(t, output, h.start.Proof)
	require.NotContains(t, output, migrationRelayAuthorization)
	fixtureBytes, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/relay_server.json")
	require.NoError(t, err)
	var fixture struct {
		RejectedStart []struct {
			Name string `json:"name"`
			Body string `json:"body"`
		} `json:"rejected_start"`
		AcceptedStart string `json:"accepted_start"`
	}
	require.NoError(t, json.Unmarshal(fixtureBytes, &fixture))
	cases := fixture.RejectedStart
	for _, tc := range cases {
		t.Run(tc.Name, func(t *testing.T) {
			response := h.post(t, "start", strings.ReplaceAll(tc.Body, "$PROOF", h.start.Proof))
			require.Equal(t, http.StatusUnauthorized, response.StatusCode)
			require.Equal(t, "invalid remote login session\n", migrationRelayBody(t, response))
		})
	}
	body := `{"proof":"` + h.start.Proof + `","callback_url":"` + migrationRelayCallback + `"}`
	response := h.post(t, "callback", body)
	require.Equal(t, http.StatusConflict, response.StatusCode)
	require.Equal(t, "remote login session is not ready\n", migrationRelayBody(t, response))
	for i := 0; i < 2; i++ {
		response = h.post(t, "start", strings.ReplaceAll(fixture.AcceptedStart, "$PROOF", h.start.Proof))
		require.Equal(t, http.StatusOK, response.StatusCode)
		require.Equal(t, "{\"authorization_url\":\""+strings.ReplaceAll(migrationRelayAuthorization, "&", `\u0026`)+"\"}\n", migrationRelayBody(t, response))
	}
	for _, tc := range []struct{ callback, message string }{{"https://example.invalid/?code=hidden", "invalid Pixiv login result"}, {"pixiv://account/login?code=other", "login result does not match this session"}} {
		response = h.post(t, "callback", `{"proof":"`+h.start.Proof+`","callback_url":"`+tc.callback+`"}`)
		require.Equal(t, http.StatusBadRequest, response.StatusCode)
		require.Equal(t, tc.message+"\n", migrationRelayBody(t, response))
	}
}
func TestMigrationRelayServerFinalPagePrecedesCallbackEOF(t *testing.T) {
	for _, success := range []bool{true, false} {
		t.Run(map[bool]string{true: "success", false: "failure"}[success], func(t *testing.T) {
			h := migrationStartRelay(t)
			response := h.post(t, "start", `{"proof":"`+h.start.Proof+`"}`)
			require.Equal(t, http.StatusOK, response.StatusCode)
			migrationRelayBody(t, response)
			response = h.post(t, "callback", `{"CALLBACK_URL":"`+migrationRelayCallback+`","callback_url":null,"PROOF":"`+h.start.Proof+`","proof":null}`)
			require.Equal(t, http.StatusOK, response.StatusCode)
			resultURL := response.Header.Get(loginhelper.RelayResultURLHeader)
			require.NotEmpty(t, resultURL)
			parsed, err := url.Parse(resultURL)
			require.NoError(t, err)
			resultID := path.Base(parsed.Path)
			decoded, err := base64.RawURLEncoding.DecodeString(resultID)
			require.NoError(t, err)
			require.Len(t, decoded, 32)
			require.NotEqual(t, resultID, h.start.SessionID)
			require.NotEqual(t, resultID, h.start.Proof)
			h.accept(t)
			bodyReady := make(chan string, 1)
			go func() { body, _ := io.ReadAll(response.Body); _ = response.Body.Close(); bodyReady <- string(body) }()
			notified := make(chan struct{})
			go func() { h.outcome.notify(success); close(notified) }()
			select {
			case <-notified:
				t.Fatal("notify completed before result page")
			case <-time.After(30 * time.Millisecond):
			}
			select {
			case <-bodyReady:
				t.Fatal("callback reached EOF before final page")
			default:
			}
			secondNotified := make(chan struct{})
			go func() { h.outcome.notify(!success); close(secondNotified) }()

			replay := h.post(t, "callback", `{"proof":"`+h.start.Proof+`","callback_url":"`+migrationRelayCallback+`"}`)
			require.Equal(t, http.StatusConflict, replay.StatusCode)
			require.Equal(t, "login result has already been received\n", migrationRelayBody(t, replay))
			replay = h.post(t, "start", `{"proof":"`+h.start.Proof+`"}`)
			require.Equal(t, http.StatusConflict, replay.StatusCode)
			migrationRelayBody(t, replay)
			page, err := http.Get(resultURL)
			require.NoError(t, err)
			expectedStatus := http.StatusBadRequest
			if success {
				expectedStatus = http.StatusOK
			}
			require.Equal(t, expectedStatus, page.StatusCode)
			html := migrationRelayBody(t, page)
			pageFixtureBytes, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/login_page.json")
			require.NoError(t, err)
			var pages []struct{ Name, SHA256 string }
			require.NoError(t, json.Unmarshal(pageFixtureBytes, &pages))
			pageName := "failure"
			if success {
				pageName = "success"
			}
			matched := false
			for _, page := range pages {
				if page.Name == pageName {
					matched = true
					require.Equal(t, page.SHA256, fmt.Sprintf("%x", sha256.Sum256([]byte(html))))
				}
			}
			require.True(t, matched)

			require.NotContains(t, html, migrationRelayCallback)
			require.NotContains(t, html, h.start.Proof)
			require.NotContains(t, html, migrationRelayAuthorization)
			select {
			case <-notified:
			case <-time.After(5 * time.Second):
				t.Fatal("notify did not complete")
			}
			expected := "{\"success\":false}\n"
			if success {
				expected = "{\"success\":true}\n"
			}
			select {
			case body := <-bodyReady:
				require.Equal(t, expected, body)
			case <-time.After(5 * time.Second):
				t.Fatal("callback body did not complete")
			}
			page, err = http.Get(resultURL)
			require.NoError(t, err)
			require.Equal(t, http.StatusConflict, page.StatusCode)
			require.Equal(t, "login result has already been opened\n", migrationRelayBody(t, page))
			select {
			case <-secondNotified:
			case <-time.After(5 * time.Second):
				t.Fatal("second notify did not finish")
			}
			h.outcome.notify(!success)
		})
	}
}
func TestMigrationRelayServerTLSFailureRedactsPrivatePaths(t *testing.T) {
	directory := t.TempDir()
	cert := filepath.Join(directory, "private-cert.pem")
	key := filepath.Join(directory, "private-key.pem")
	require.NoError(t, os.WriteFile(cert, []byte("synthetic invalid PEM"), 0600))
	require.NoError(t, os.WriteFile(key, []byte("synthetic invalid PEM"), 0600))
	watcher := make(chan struct{})
	var output bytes.Buffer
	_, notify, cleanup, err := auth.WaitForHandoffRelayLoginCode(context.Background(), &output, auth.RelayServerOptions{PublicURL: "https://relay.example", ListenAddr: "127.0.0.1:0", TLSCertFile: cert, TLSKeyFile: key, ContextWaiterExited: func() { close(watcher) }}, func(string) bool { return false }, migrationRelayAuthorization)
	require.EqualError(t, err, "remote login relay server failed; verify its listener and TLS configuration")
	require.NotContains(t, output.String(), directory)
	require.NotContains(t, output.String(), migrationRelayAuthorization)
	notify(true)
	cleanup()
	cleanup()
	select {
	case <-watcher:
	case <-time.After(5 * time.Second):
		t.Fatal("watcher not stopped")
	}
}

func TestMigrationRelayServerCallbackAbandonmentReleasesFinal(t *testing.T) {
	h := migrationStartRelay(t)
	response := h.post(t, "start", `{"proof":"`+h.start.Proof+`"}`)
	migrationRelayBody(t, response)
	response = h.post(t, "callback", `{"proof":"`+h.start.Proof+`","callback_url":"`+migrationRelayCallback+`"}`)
	require.Equal(t, http.StatusOK, response.StatusCode)
	h.accept(t)
	require.NoError(t, response.Body.Close())
	notified := make(chan struct{})
	go func() { h.outcome.notify(true); close(notified) }()
	select {
	case <-notified:
	case <-time.After(5 * time.Second):
		t.Fatal("abandoned callback did not release final latch")
	}
	h.outcome.cleanup()
	h.outcome.cleanup()
}

func TestMigrationRelayServerCallerDeadlineStopsSession(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	watcher := make(chan struct{})
	_, notify, cleanup, err := auth.WaitForHandoffRelayLoginCode(ctx, io.Discard, auth.RelayServerOptions{PublicURL: "http://relay.example", ListenAddr: "127.0.0.1:0", ContextWaiterExited: func() { close(watcher) }}, func(string) bool { return false }, migrationRelayAuthorization)
	require.ErrorIs(t, err, context.DeadlineExceeded)
	notify(false)
	cleanup()
	select {
	case <-watcher:
	case <-time.After(5 * time.Second):
		t.Fatal("deadline watcher not stopped")
	}
}

func TestMigrationRelayServerMuxPaths(t *testing.T) {
	h := migrationStartRelay(t)
	client := &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	for _, tc := range []struct {
		method, path, location string
		status                 int
	}{
		{"GET", "/session", "/session/", 307},
		{"POST", "/start", "/start/", 307},
		{"GET", "//session/" + h.start.SessionID, "/session/" + h.start.SessionID, 307},
		{"GET", "/session/./" + h.start.SessionID, "/session/" + h.start.SessionID, 307},
		{"CONNECT", "/session", "/session/", 307},
		{"CONNECT", "/session/./" + h.start.SessionID, "", 404},
		{"GET", "/session/wrong", "", 404},
		{"POST", "/session/" + h.start.SessionID, "", 404},
		{"GET", "/start/" + h.start.SessionID, "", 404},
		{"GET", "/session/" + h.start.SessionID + "/", "", 404},
	} {
		request, err := http.NewRequest(tc.method, h.address+tc.path, nil)
		require.NoError(t, err)
		response, err := client.Do(request)
		require.NoError(t, err)
		require.Equal(t, tc.status, response.StatusCode, tc.path)
		require.Equal(t, tc.location, response.Header.Get("Location"), tc.path)
		migrationRelayBody(t, response)
	}
}

func TestMigrationRelayServerConfiguredOptions(t *testing.T) {
	fixtureBytes, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/relay_server.json")
	require.NoError(t, err)
	var fixture struct {
		Options []struct {
			Name, Public, Listen, Cert, Key, Error string
			Clear                                  []string
			Enabled                                bool
		}
	}
	require.NoError(t, json.Unmarshal(fixtureBytes, &fixture))
	for _, tc := range fixture.Options {
		t.Run(tc.Name, func(t *testing.T) {
			flags := changedFlags{}
			for _, flag := range tc.Clear {
				flags[flag] = true
			}
			opts, enabled, err := auth.ConfiguredRelayServerOptions(flags, auth.AccountLoginOptions{}, config.RuntimeConfig{LoginRelayPublicURL: tc.Public, LoginRelayListenAddr: tc.Listen, LoginRelayTLSCertFile: tc.Cert, LoginRelayTLSKeyFile: tc.Key})
			if tc.Error != "" {
				require.EqualError(t, err, tc.Error)
			} else {
				require.NoError(t, err)
			}
			require.Equal(t, tc.Enabled, enabled)
			if enabled {
				require.Equal(t, tc.Public, opts.PublicURL)
				require.Equal(t, tc.Listen, opts.ListenAddr)
				require.Equal(t, tc.Cert, opts.TLSCertFile)
				require.Equal(t, tc.Key, opts.TLSKeyFile)
			} else {
				require.Empty(t, opts)
			}
		})
	}
}

func TestMigrationRelayServerClaimedResultDisconnectReleasesFinal(t *testing.T) {
	h := migrationStartRelay(t)
	response := h.post(t, "start", `{"proof":"`+h.start.Proof+`"}`)
	migrationRelayBody(t, response)
	response = h.post(t, "callback", `{"proof":"`+h.start.Proof+`","callback_url":"`+migrationRelayCallback+`"}`)
	resultURL := response.Header.Get(loginhelper.RelayResultURLHeader)
	h.accept(t)
	ctx, cancel := context.WithCancel(context.Background())
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, resultURL, nil)
	require.NoError(t, err)
	pageDone := make(chan error, 1)
	go func() {
		page, err := http.DefaultClient.Do(request)
		if page != nil {
			_ = page.Body.Close()
		}
		pageDone <- err
	}()
	time.Sleep(30 * time.Millisecond)
	cancel()
	select {
	case err := <-pageDone:
		require.Error(t, err)
	case <-time.After(5 * time.Second):
		t.Fatal("result request did not cancel")
	}
	notified := make(chan struct{})
	go func() { h.outcome.notify(true); close(notified) }()
	select {
	case <-notified:
	case <-time.After(5 * time.Second):
		t.Fatal("claimed result disconnect did not release notify")
	}
	require.Equal(t, "{\"success\":true}\n", migrationRelayBody(t, response))
}
func TestMigrationRelayServerSyntheticTLSStart(t *testing.T) {
	root := "../../../../../crates/pixiv-app/tests/fixtures/"
	certBytes, err := os.ReadFile(root + "relay_server_cert.pem")
	require.NoError(t, err)
	roots := x509.NewCertPool()
	require.True(t, roots.AppendCertsFromPEM(certBytes))
	client := &http.Client{Transport: &http.Transport{ForceAttemptHTTP2: true, TLSClientConfig: &tls.Config{RootCAs: roots, MinVersion: tls.VersionTLS12}}, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	defer client.CloseIdleConnections()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, err)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	output := newSynchronizedOutput()
	done := make(chan error, 1)
	go func() {
		_, _, cleanup, err := auth.WaitForHandoffRelayLoginCode(ctx, output, auth.RelayServerOptions{PublicURL: "https://" + listener.Addr().String(), ListenAddr: listener.Addr().String(), TLSCertFile: root + "relay_server_cert.pem", TLSKeyFile: root + "relay_server_key.pem", Listen: func(string, string) (net.Listener, error) { return listener, nil }}, func(string) bool { return false }, migrationRelayAuthorization)
		cleanup()
		done <- err
	}()
	sessionURL := output.waitForSessionURL(t)
	response, err := client.Get(sessionURL)
	require.NoError(t, err)
	require.Equal(t, 303, response.StatusCode)
	require.Equal(t, 2, response.ProtoMajor)
	deep := response.Header.Get("Location")
	migrationRelayBody(t, response)
	start, err := loginhelper.ParseRemoteLoginLink(deep)
	require.NoError(t, err)
	response, err = client.Post(start.Origin+"/start/"+start.SessionID, "application/json", strings.NewReader(`{"proof":"`+start.Proof+`"}`))
	require.NoError(t, err)
	require.Equal(t, 200, response.StatusCode)
	require.Contains(t, migrationRelayBody(t, response), "authorization_url")
	cancel()
	select {
	case err := <-done:
		require.ErrorIs(t, err, context.Canceled)
	case <-time.After(5 * time.Second):
		t.Fatal("TLS relay did not stop")
	}
}

func TestMigrationRelayServerCanonicalSpecialPublicOrigin(t *testing.T) {
	h := migrationStartRelayWithPublicURL(t, " HTTPS://RELAY.EXAMPLE//~/*/日本 語/// ")
	require.Equal(t, "https://relay.example/~/%2A/%E6%97%A5%E6%9C%AC%20%E8%AA%9E", h.start.Origin)
}

func TestMigrationRelayServerEmptyPortBindsEphemeral(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	output := newSynchronizedOutput()
	done := make(chan error, 1)
	go func() {
		_, _, cleanup, err := auth.WaitForHandoffRelayLoginCode(ctx, output, auth.RelayServerOptions{PublicURL: "http://relay.example", ListenAddr: "127.0.0.1:"}, func(string) bool { return false }, migrationRelayAuthorization)
		cleanup()
		done <- err
	}()
	sessionURL, err := url.Parse(output.waitForSessionURL(t))
	require.NoError(t, err)
	output.mu.Lock()
	log := output.buffer.String()
	output.mu.Unlock()
	first, _, _ := strings.Cut(log, "\n")
	address := strings.TrimSuffix(strings.TrimPrefix(first, "Remote Pixiv login relay is listening on "), ".")
	host, port, err := net.SplitHostPort(address)
	require.NoError(t, err)
	require.Equal(t, "127.0.0.1", host)
	require.NotEmpty(t, port)
	require.NotEqual(t, "0", port)
	remoteLoginDeepLinkFromSessionURL(t, "http://"+address+"/session/"+path.Base(sessionURL.Path))
	cancel()
	select {
	case err := <-done:
		require.ErrorIs(t, err, context.Canceled)
	case <-time.After(5 * time.Second):
		t.Fatal("empty-port relay did not stop")
	}
}

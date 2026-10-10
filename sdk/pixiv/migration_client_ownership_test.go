package pixiv

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationCaptureClientOwnership = flag.Bool("migration-capture-client-ownership", false, "capture frozen public SDK client ownership")

const migrationOwnershipFixture = "../../crates/pixiv-sdk/tests/fixtures/client-ownership.json"
const migrationOwnershipEvidence = "../../crates/pixiv-sdk/tests/support/client-ownership-evidence"
const migrationOwnershipArtwork = `{"illust":{"id":73,"create_date":"2026-01-02T03:04:05Z"}}`
const migrationOwnershipOAuth = `{"access_token":"fixture-access","refresh_token":"fixture-rotated","user":{"id":42,"name":"fixture-name"}}`

type migrationOwnershipError struct {
	Reason    string `json:"reason"`
	Operation string `json:"operation"`
	Detail    string `json:"detail"`
	Message   string `json:"message"`
}

type migrationOwnershipConstructor struct {
	Name               string                   `json:"name"`
	Input              string                   `json:"input"`
	Mode               string                   `json:"mode"`
	PacingMS           int64                    `json:"pacing_ms"`
	ClientNil          bool                     `json:"client_nil"`
	Owned              bool                     `json:"owned"`
	SameClient         bool                     `json:"same_injected_client"`
	Wrappers           []string                 `json:"wrappers"`
	TimeoutMS          int64                    `json:"timeout_ms"`
	JarPreserved       bool                     `json:"jar_preserved"`
	RedirectPreserved  bool                     `json:"redirect_preserved"`
	CallerUnchanged    bool                     `json:"caller_unchanged"`
	Identity           int64                    `json:"identity"`
	CursorEntropyBytes int                      `json:"cursor_entropy_bytes"`
	TransportCalls     int                      `json:"transport_calls"`
	CloseCalls         int                      `json:"injected_close_calls"`
	Error              *migrationOwnershipError `json:"error"`
}

type migrationOwnershipRequest struct {
	Method         string `json:"method"`
	Host           string `json:"host"`
	Path           string `json:"path"`
	Conn           int    `json:"connection"`
	Authorization  string `json:"authorization"`
	UserID         string `json:"user_id"`
	AcceptLanguage string `json:"accept_language"`
	Refresh        string `json:"refresh_token"`
}

type migrationOwnershipNative struct {
	Name                           string                      `json:"name"`
	PacingMS                       int64                       `json:"pacing_ms"`
	Owned                          bool                        `json:"owned"`
	Sequence                       []string                    `json:"sequence"`
	Requests                       []migrationOwnershipRequest `json:"requests"`
	ConnectionsClosedBeforeCleanup int                         `json:"connections_closed_before_test_cleanup"`
}

type migrationOwnershipOpen struct {
	PacingMS                         int64                       `json:"pacing_ms"`
	ConnectionsClosedAfterSDKCleanup int                         `json:"connections_closed_after_sdk_cleanup"`
	Name                             string                      `json:"name"`
	Mode                             string                      `json:"mode"`
	Status                           int                         `json:"oauth_status"`
	Body                             string                      `json:"oauth_body"`
	ClientNil                        bool                        `json:"client_nil"`
	CredentialsEmpty                 bool                        `json:"credentials_empty"`
	Owned                            bool                        `json:"owned"`
	Identity                         int64                       `json:"identity"`
	Username                         string                      `json:"username"`
	Access                           string                      `json:"access_token"`
	Refresh                          string                      `json:"refresh_token"`
	CursorEmpty                      bool                        `json:"cursor_instance_empty"`
	ConnectionsClosedAfterOpen       int                         `json:"connections_closed_after_open"`
	Requests                         []migrationOwnershipRequest `json:"requests"`
	Error                            *migrationOwnershipError    `json:"error"`
}

type migrationOwnershipOrder struct {
	Input          string                   `json:"input"`
	ExitCode       int                      `json:"exit_code"`
	Name           string                   `json:"name"`
	Boundary       string                   `json:"boundary"`
	Outcome        string                   `json:"outcome"`
	TransportCalls int                      `json:"transport_calls"`
	EntropyBytes   int                      `json:"entropy_bytes"`
	Error          *migrationOwnershipError `json:"error"`
}

type migrationOwnershipContract struct {
	SchemaVersion          int                             `json:"schema_version"`
	SourceCommit           string                          `json:"source_commit"`
	SourceSHA256           map[string]string               `json:"source_sha256"`
	DependencySHA256       map[string]string               `json:"dependency_source_sha256"`
	GoVersion              string                          `json:"go_version"`
	Constructors           []migrationOwnershipConstructor `json:"constructors"`
	NativeLifecycle        []migrationOwnershipNative      `json:"native_lifecycle"`
	OAuth                  []migrationOwnershipOpen        `json:"oauth"`
	ConstructionOrder      []migrationOwnershipOrder       `json:"construction_order"`
	PrivateSourceWitnesses []string                        `json:"private_source_witnesses"`
}

func migrationOwnershipClassify(t *testing.T, err error) *migrationOwnershipError {
	t.Helper()
	if err == nil {
		return nil
	}
	var classified *sdk.Error
	if !errors.As(err, &classified) {
		t.Fatalf("unclassified ownership error: %v", err)
	}
	return &migrationOwnershipError{string(classified.Reason), classified.Operation, classified.Detail, err.Error()}
}

type migrationOwnershipTransport struct {
	calls  int
	closes int
}

func (r *migrationOwnershipTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	r.calls++
	body := migrationOwnershipArtwork
	if req.URL.Path == "/auth/token" {
		body = migrationOwnershipOAuth
	}
	return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
}
func (r *migrationOwnershipTransport) CloseIdleConnections() { r.closes++ }

type migrationOwnershipEntropy struct {
	bytes int
	fail  bool
}

func (r *migrationOwnershipEntropy) Read(p []byte) (int, error) {
	r.bytes += len(p)
	if r.fail {
		fmt.Fprintf(os.Stderr, "ownership-entropy-read=%d\n", len(p))
		return 0, errors.New("fixture entropy unavailable")
	}
	for i := range p {
		p[i] = byte(i + 1)
	}
	return len(p), nil
}

func migrationOwnershipWrappers(rt http.RoundTripper) []string {
	result := []string{}
	for {
		switch transport := rt.(type) {
		case *diagnosticRoundTripper:
			result = append(result, "diagnostic")
			rt = transport.inner
		case *pacingRoundTripper:
			result = append(result, "pacing")
			rt = transport.inner
		case *http.Transport:
			return append(result, "native")
		case *migrationOwnershipTransport:
			return append(result, "injected")
		case nil:
			return append(result, "nil")
		default:
			panic(fmt.Sprintf("unexpected transport %T", rt))
		}
	}
}

func migrationOwnershipConstructors(t *testing.T) []migrationOwnershipConstructor {
	t.Helper()
	var rows []migrationOwnershipConstructor
	for _, item := range []struct {
		name, input, mode string
		interval          time.Duration
	}{
		{"new-default", "  fixture-access  ", "default", 0},
		{"new-default-paced", "fixture-access", "default", 10 * time.Millisecond},
		{"new-default-negative-pacing", "fixture-access", "default", -time.Millisecond},
		{"new-injected", "fixture-access", "custom", 0},
		{"new-opaque-access", " Cookie: fixture=value ", "custom", 0},
		{"new-injected-paced", "fixture-access", "custom", 10 * time.Millisecond},
		{"new-injected-negative-pacing", "fixture-access", "custom", -time.Millisecond},
		{"new-injected-nil-transport", "fixture-access", "nil", 0},
		{"new-injected-nil-transport-paced", "fixture-access", "nil", 10 * time.Millisecond},
		{"new-empty-default", "", "default", 0},
		{"new-blank-injected-paced", " \t\n", "custom", 10 * time.Millisecond},
	} {
		row := migrationOwnershipConstructor{Name: item.name, Input: item.input, Mode: item.mode, PacingMS: item.interval.Milliseconds(), Wrappers: []string{}}
		rt := &migrationOwnershipTransport{}
		jar, err := cookiejar.New(nil)
		if err != nil {
			t.Fatal(err)
		}
		redirect := func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
		base := &http.Client{Transport: rt, Timeout: 7 * time.Second, Jar: jar, CheckRedirect: redirect}
		if item.mode == "nil" {
			base.Transport = nil
		}
		options := Options{Pacing: Pacing{MinInterval: item.interval}}
		if item.mode != "default" {
			options.HTTPClient = base
		}
		entropy := &migrationOwnershipEntropy{}
		oldRandom := rand.Reader
		rand.Reader = entropy
		client, err := NewWith(item.input, options)
		rand.Reader = oldRandom
		row.ClientNil, row.Error, row.CursorEntropyBytes = client == nil, migrationOwnershipClassify(t, err), entropy.bytes
		expectedBaseTransport := http.RoundTripper(rt)
		if item.mode == "nil" {
			expectedBaseTransport = nil
		}
		row.CallerUnchanged = base.Transport == expectedBaseTransport && base.Timeout == 7*time.Second && base.Jar == jar && reflect.ValueOf(base.CheckRedirect).Pointer() == reflect.ValueOf(redirect).Pointer()
		if client != nil {
			row.Owned, row.SameClient, row.Wrappers = client.selfHTTP, client.httpClient == base, migrationOwnershipWrappers(client.httpClient.Transport)
			row.TimeoutMS, row.Identity = client.httpClient.Timeout.Milliseconds(), client.UserID()
			row.JarPreserved = client.httpClient.Jar == jar
			row.RedirectPreserved = client.httpClient.CheckRedirect != nil && reflect.ValueOf(client.httpClient.CheckRedirect).Pointer() == reflect.ValueOf(redirect).Pointer()
			if client.cursorInstance != "0102030405060708090a0b0c0d0e0f10" {
				t.Fatalf("New cursor entropy binding: %q", client.cursorInstance)
			}
			client.CloseIdleConnections()
			client.CloseIdleConnections()
		}
		row.TransportCalls, row.CloseCalls = rt.calls, rt.closes
		expectUnchanged := item.mode != "nil" || item.interval > 0 || client == nil
		if rt.calls != 0 || rt.closes != 0 || row.CallerUnchanged != expectUnchanged {
			t.Fatalf("constructor touched caller ownership: %+v", row)
		}
		rows = append(rows, row)
	}
	return rows
}

type migrationOwnershipConn struct {
	net.Conn
	closed atomic.Bool
}

func (c *migrationOwnershipConn) Close() error { c.closed.Store(true); return c.Conn.Close() }

type migrationOwnershipPeer struct {
	server      *httptest.Server
	transport   *http.Transport
	mu          sync.Mutex
	conns       []*migrationOwnershipConn
	requests    []migrationOwnershipRequest
	nextConn    int
	oauthStatus int
	oauthBody   string
}

type migrationOwnershipConnKey struct{}

func migrationOwnershipNewPeer(t *testing.T, oauthStatus int, oauthBody string) *migrationOwnershipPeer {
	t.Helper()
	peer := &migrationOwnershipPeer{oauthStatus: oauthStatus, oauthBody: oauthBody, requests: []migrationOwnershipRequest{}}
	peer.server = httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		row := migrationOwnershipRequest{Method: req.Method, Host: req.Host, Path: req.URL.Path, Conn: req.Context().Value(migrationOwnershipConnKey{}).(int), Authorization: req.Header.Get("Authorization"), UserID: req.Header.Get("X-User-Id"), AcceptLanguage: req.Header.Get("Accept-Language")}
		body, status := migrationOwnershipArtwork, 200
		if req.URL.Path == "/auth/token" {
			if err := req.ParseForm(); err != nil {
				t.Errorf("synthetic OAuth form: %v", err)
			}
			row.Refresh = req.PostForm.Get("refresh_token")
			body, status = peer.oauthBody, peer.oauthStatus
		}
		peer.mu.Lock()
		peer.requests = append(peer.requests, row)
		peer.mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		_, _ = io.WriteString(w, body)
	}))
	peer.server.Config.ConnContext = func(ctx context.Context, _ net.Conn) context.Context {
		peer.mu.Lock()
		peer.nextConn++
		number := peer.nextConn
		peer.mu.Unlock()
		return context.WithValue(ctx, migrationOwnershipConnKey{}, number)
	}
	peer.server.StartTLS()
	certs := x509.NewCertPool()
	certs.AddCert(peer.server.Certificate())
	peer.transport = http.DefaultTransport.(*http.Transport).Clone()
	peer.transport.Proxy = nil
	peer.transport.ForceAttemptHTTP2 = false
	peer.transport.TLSNextProto = map[string]func(string, *tls.Conn) http.RoundTripper{}
	peer.transport.TLSClientConfig = &tls.Config{RootCAs: certs, ServerName: "example.com", MinVersion: tls.VersionTLS12}
	peer.transport.IdleConnTimeout = 0
	peer.transport.DialContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		if network != "tcp" || (address != "app-api.pixiv.net:443" && address != "oauth.secure.pixiv.net:443") {
			return nil, fmt.Errorf("unowned route %s %s", network, address)
		}
		connection, err := (&net.Dialer{}).DialContext(ctx, "tcp", peer.server.Listener.Addr().String())
		if err != nil {
			return nil, err
		}
		tracked := &migrationOwnershipConn{Conn: connection}
		peer.mu.Lock()
		peer.conns = append(peer.conns, tracked)
		peer.mu.Unlock()
		return tracked, nil
	}
	t.Cleanup(func() {
		peer.transport.CloseIdleConnections()
		peer.mu.Lock()
		for _, c := range peer.conns {
			_ = c.Close()
		}
		peer.mu.Unlock()
		peer.server.Close()
	})
	return peer
}
func (p *migrationOwnershipPeer) installDefault(t *testing.T) {
	t.Helper()
	before := http.DefaultTransport
	http.DefaultTransport = p.transport
	t.Cleanup(func() { http.DefaultTransport = before })
}
func (p *migrationOwnershipPeer) observations() ([]migrationOwnershipRequest, int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	rows := append([]migrationOwnershipRequest{}, p.requests...)
	closed := 0
	for _, c := range p.conns {
		if c.closed.Load() {
			closed++
		}
	}
	return rows, closed
}
func migrationOwnershipArtworkCall(t *testing.T, c *Client) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	art, err := c.Artwork(ctx, ArtworkRequest{ArtworkID: 73})
	if err != nil || art.ID != 73 {
		t.Fatalf("public Artwork: id=%d err=%v", art.ID, err)
	}
}

func migrationOwnershipNativeLifecycle(t *testing.T) []migrationOwnershipNative {
	t.Helper()
	var rows []migrationOwnershipNative
	for _, mode := range []string{"default", "default-paced", "injected-shared", "injected-shared-paced"} {
		t.Run(mode, func(t *testing.T) {
			peer := migrationOwnershipNewPeer(t, 200, migrationOwnershipOAuth)
			peer.installDefault(t)
			options := Options{}
			if strings.Contains(mode, "paced") {
				options.Pacing.MinInterval = 10 * time.Millisecond
			}
			if strings.HasPrefix(mode, "injected") {
				options.HTTPClient = &http.Client{Transport: peer.transport}
			}
			a, err := NewWith("fixture-access", options)
			if err != nil {
				t.Fatal(err)
			}
			b, err := NewWith("fixture-access", options)
			if err != nil {
				t.Fatal(err)
			}
			defer a.CloseIdleConnections()
			defer b.CloseIdleConnections()
			row := migrationOwnershipNative{Name: mode, PacingMS: options.Pacing.MinInterval.Milliseconds(), Owned: a.selfHTTP, Sequence: []string{"a-request", "a-request", "b-request", "b-request", "a-close-idle", "a-close-idle", "a-request", "b-request"}}
			migrationOwnershipArtworkCall(t, a)
			migrationOwnershipArtworkCall(t, a)
			migrationOwnershipArtworkCall(t, b)
			migrationOwnershipArtworkCall(t, b)
			a.CloseIdleConnections()
			a.CloseIdleConnections()
			migrationOwnershipArtworkCall(t, a)
			migrationOwnershipArtworkCall(t, b)
			row.Requests, row.ConnectionsClosedBeforeCleanup = peer.observations()
			expected := []int{1, 1, 2, 2, 3, 2}
			expectedClosed := 1
			if !a.selfHTTP {
				expected = []int{1, 1, 1, 1, 1, 1}
				expectedClosed = 0
			}
			if len(row.Requests) != len(expected) {
				t.Fatalf("request count: %+v", row.Requests)
			}
			for i, req := range row.Requests {
				if req.Conn != expected[i] {
					t.Fatalf("pool sequence %s: %+v", mode, row.Requests)
				}
			}
			if row.ConnectionsClosedBeforeCleanup != expectedClosed {
				t.Fatalf("idle retirement: %+v", row)
			}
			rows = append(rows, row)
		})
	}
	var nilClient *Client
	nilClient.CloseIdleConnections()
	(&Client{}).CloseIdleConnections()
	(&Client{selfHTTP: true}).CloseIdleConnections()
	return rows
}

func migrationOwnershipOpenCases(t *testing.T) []migrationOwnershipOpen {
	t.Helper()
	var rows []migrationOwnershipOpen
	for _, item := range []struct {
		name, mode string
		status     int
		body       string
	}{
		{"open-default-success", "default", 200, migrationOwnershipOAuth},
		{"open-injected-success", "injected", 200, migrationOwnershipOAuth},
		{"open-default-paced-success", "default-paced", 200, migrationOwnershipOAuth},
		{"open-injected-paced-success", "injected-paced", 200, migrationOwnershipOAuth},
		{"open-default-unauthorized", "default", 401, `{"error":"fixture"}`},
		{"open-default-no-access", "default", 200, `{"refresh_token":"fixture-rotated","user":{"id":42}}`},
		{"open-default-no-identity", "default", 200, `{"access_token":"fixture-access","user":{"name":"fixture-name"}}`},
		{"open-injected-no-identity", "injected", 200, `{"access_token":"fixture-access","user":{"name":"fixture-name"}}`},
	} {
		t.Run(item.name, func(t *testing.T) {
			peer := migrationOwnershipNewPeer(t, item.status, item.body)
			peer.installDefault(t)
			options := Options{AcceptLanguage: "fixture-language"}
			if strings.HasPrefix(item.mode, "injected") {
				options.HTTPClient = &http.Client{Transport: peer.transport}
			}
			if strings.HasSuffix(item.mode, "paced") {
				options.Pacing.MinInterval = 10 * time.Millisecond
			}
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			var client *Client
			var credentials Credentials
			var err error
			if item.mode == "default" {
				client, credentials, err = Open(ctx, "  fixture-refresh  ")
			} else {
				client, credentials, err = OpenWith(ctx, "  fixture-refresh  ", options)
			}
			row := migrationOwnershipOpen{PacingMS: options.Pacing.MinInterval.Milliseconds(), Name: item.name, Mode: item.mode, Status: item.status, Body: item.body, ClientNil: client == nil, CredentialsEmpty: credentials == Credentials{}, Error: migrationOwnershipClassify(t, err)}
			_, row.ConnectionsClosedAfterOpen = peer.observations()
			if row.ConnectionsClosedAfterOpen != 0 {
				t.Fatalf("Open unexpectedly closed native connection: %+v", row)
			}
			if client != nil {
				defer client.CloseIdleConnections()
				row.Owned, row.Identity, row.Username, row.CursorEmpty = client.selfHTTP, client.UserID(), client.Username(), client.cursorInstance == ""
				row.Access, row.Refresh = credentials.AccessToken(), credentials.RefreshToken()
				migrationOwnershipArtworkCall(t, client)
				client.CloseIdleConnections()
				client.CloseIdleConnections()
				_, row.ConnectionsClosedAfterSDKCleanup = peer.observations()
				expectedClosed := 0
				if client.selfHTTP {
					expectedClosed = 2
				}
				if row.ConnectionsClosedAfterSDKCleanup != expectedClosed {
					t.Fatalf("Open pool ownership cleanup: %+v", row)
				}
				migrationOwnershipArtworkCall(t, client)
			}
			row.Requests, _ = peer.observations()
			rows = append(rows, row)
		})
	}
	return rows
}

func migrationOwnershipConstructionOrder(t *testing.T) []migrationOwnershipOrder {
	t.Helper()
	var rows []migrationOwnershipOrder
	for _, item := range []struct{ name, op, input string }{
		{"new-empty-before-build", "New", " \t\n"},
		{"open-empty-before-build", "Open", " \t\n"},
		{"new-build-before-entropy", "New", "fixture-access"},
		{"open-build-before-oauth", "Open", "fixture-refresh"},
	} {
		row := migrationOwnershipOrder{Name: item.name, Input: item.input, Boundary: "public constructor", Outcome: "returned"}
		entropy := &migrationOwnershipEntropy{}
		originalTransport, originalEntropy := http.DefaultTransport, rand.Reader
		http.DefaultTransport = &migrationOwnershipTransport{}
		rand.Reader = entropy
		func() {
			defer func() {
				if recovered := recover(); recovered != nil {
					row.Outcome = "default-transport-type-assertion-panic"
				}
			}()
			var err error
			if item.op == "New" {
				_, err = New(item.input)
			} else {
				_, _, err = Open(context.Background(), item.input)
			}
			row.Error = migrationOwnershipClassify(t, err)
		}()
		http.DefaultTransport, rand.Reader = originalTransport, originalEntropy
		row.EntropyBytes = entropy.bytes
		if row.EntropyBytes != 0 {
			t.Fatalf("constructor order advanced entropy: %+v", row)
		}
		rows = append(rows, row)
	}
	for _, item := range []struct{ name, input string }{
		{"open-invalid-refresh-before-wire", "PHPSESSID=fixture"},
		{"open-verified-identity-skips-entropy", "fixture-refresh"},
	} {
		row := migrationOwnershipOrder{Name: item.name, Input: item.input, Boundary: "public OpenWith with injected fixture transport", Outcome: "returned"}
		transport := &migrationOwnershipTransport{}
		entropy := &migrationOwnershipEntropy{fail: true}
		originalEntropy := rand.Reader
		rand.Reader = entropy
		client, _, err := OpenWith(context.Background(), item.input, Options{HTTPClient: &http.Client{Transport: transport}})
		rand.Reader = originalEntropy
		row.Error = migrationOwnershipClassify(t, err)
		row.EntropyBytes, row.TransportCalls = entropy.bytes, transport.calls
		if client != nil {
			client.CloseIdleConnections()
		}
		if row.EntropyBytes != 0 {
			t.Fatalf("verified OAuth did not skip cursor entropy: %+v", row)
		}
		rows = append(rows, row)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationClientOwnershipEntropyChild$", "-test.count=1")
	command.Env = append(os.Environ(), "PIXIV_MIGRATION_OWNERSHIP_ENTROPY_CHILD=1")
	output, err := command.CombinedOutput()
	var exitError *exec.ExitError
	if ctx.Err() != nil || !errors.As(err, &exitError) || exitError.ExitCode() != 2 || !bytes.Contains(output, []byte("ownership-entropy-read=16")) || !bytes.Contains(output, []byte("crypto/rand: failed to read random data")) {
		t.Fatalf("real crypto/rand fatal child: err=%v output=%s", err, output)
	}
	if *migrationCaptureClientOwnership {
		if err := os.MkdirAll(migrationOwnershipEvidence, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(migrationOwnershipEvidence, "entropy-fatal.raw.txt"), output, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	rows = append(rows, migrationOwnershipOrder{Name: "new-entropy-error", Boundary: "actual Go 1.27.1 crypto/rand in owned subprocess", Input: "fixture-access", Outcome: "fatal-process-exit", EntropyBytes: 16, ExitCode: 2})
	return rows
}

func TestMigrationClientOwnershipEntropyChild(t *testing.T) {
	if os.Getenv("PIXIV_MIGRATION_OWNERSHIP_ENTROPY_CHILD") != "1" {
		t.Skip("owned entropy-failure subprocess only")
	}
	rand.Reader = &migrationOwnershipEntropy{fail: true}
	_, _ = NewWith("fixture-access", Options{HTTPClient: &http.Client{Transport: &migrationOwnershipTransport{}}})
	t.Fatal("entropy error unexpectedly returned")
}

func TestMigrationClientOwnershipFrozenGo(t *testing.T) {
	contract := migrationOwnershipContract{SchemaVersion: 1, SourceCommit: "4b4426487ef18bed276706daec385e0d0a6979f9", GoVersion: runtime.Version(), SourceSHA256: map[string]string{
		"sdk/pixiv/pixiv.go":                           "daefb42f9f90359f5ce3d326d18df7afe8c10615750d88fef049cce78224d317",
		"sdk/pixiv/cursor.go":                          "7fc144718fb4d526680973e47d2b26b7b3628adb2e8e7daa26bb052fe4c5fe4e",
		"sdk/pixiv/errors.go":                          "9a1830393129ca195ef4d57c0bda17f219f0f750c7293abf5520aa888bfe81a8",
		"sdk/error.go":                                 "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
		"internal/services/pixiv/oauth/oauth.go":       "cdc91d906255256f452789a3ce4b5efd8844e5b84a44f29756c1172b051ce376",
		"internal/services/pixiv/appapi/appapi.go":     "b5d9502ad9c534c88bda076740553c8bb6f389c3d2f4e3bd80c32c7f281ba9ff",
		"internal/services/pixiv/resource/resource.go": "c516f925b15a20531eba51f83e6a7797280b6569ad0fd519591e9330199b581a",
		"internal/shared/diagnostics/diagnostics.go":   "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76",
		"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
	}}
	for path, expected := range contract.SourceSHA256 {
		source, err := os.ReadFile(filepath.Join("..", "..", path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(source)); got != expected {
			t.Fatalf("frozen ownership source %s changed: %s", path, got)
		}
	}
	contract.DependencySHA256 = map[string]string{
		"go-resty/resty/v2@v2.17.2/resty.go":  "29be984b35ddc47ef4fd8445692c45564c9e39479be8467d9ef39ef590579e99",
		"go-resty/resty/v2@v2.17.2/client.go": "bc0107de3d9a596b6843dfd864f71208b81aa9c17402fd068a69d4acecb8783f",
		"Go1.27.1/src/crypto/rand/rand.go":    "7c289648d453d0bdca0f05091d00bb86c94b69637080c748ff3d3476677521ac",
	}
	dependencyPaths := map[string]string{
		"go-resty/resty/v2@v2.17.2/resty.go":  filepath.Join(os.Getenv("GOMODCACHE"), "github.com", "go-resty", "resty", "v2@v2.17.2", "resty.go"),
		"go-resty/resty/v2@v2.17.2/client.go": filepath.Join(os.Getenv("GOMODCACHE"), "github.com", "go-resty", "resty", "v2@v2.17.2", "client.go"),
		"Go1.27.1/src/crypto/rand/rand.go":    filepath.Join(runtime.GOROOT(), "src", "crypto", "rand", "rand.go"),
	}
	for name, path := range dependencyPaths {
		source, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(source)); got != contract.DependencySHA256[name] {
			t.Fatalf("ownership dependency %s changed: %s", name, got)
		}
	}
	contract.Constructors = migrationOwnershipConstructors(t)
	contract.NativeLifecycle = migrationOwnershipNativeLifecycle(t)
	contract.OAuth = migrationOwnershipOpenCases(t)
	contract.ConstructionOrder = migrationOwnershipConstructionOrder(t)
	contract.PrivateSourceWitnesses = []string{
		"sdk/pixiv/pixiv.go:newClient calls newCursorInstanceID only for userID <= 0 and returns its error before appapi/resource construction",
		"sdk/pixiv/pixiv.go:NewWith wraps a returned newClient entropy error as New/LocalStateError/cannot initialize cursor instance; actual Go 1.27.1 rand.Read aborts instead of returning entropy errors",
		"sdk/pixiv/pixiv.go:OpenWith rejects missing identity before newClient and supplies verified positive userID, so its cursor initialization error branch is not reached through successful public OAuth",
		"sdk/pixiv/pixiv.go:OpenWith returns on refresh, missing token, missing identity, or cursor construction errors without calling CloseIdleConnections; native failure observations retain the idle OAuth connection until test-owned cleanup",
		"go-resty/resty/v2@v2.17.2:NewWithClient/createClient mutates a caller HTTPClient with nil Transport to a newly allocated native transport when pacing is not positive; positive pacing derives a separate HTTPClient before Resty and leaves the caller unchanged",
		"sdk/pixiv/pixiv.go:Client.CloseIdleConnections handles nil client, false selfHTTP, and nil httpClient as no-ops; owned diagnostic and pacing wrappers forward to an inner CloseIdleConnections method",
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *migrationCaptureClientOwnership {
		if err := os.WriteFile(migrationOwnershipFixture, data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile(migrationOwnershipFixture)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, expected) {
		t.Fatalf("frozen Go client ownership differs:\n%s", data)
	}
	t.Logf("frozen Go %s: %d constructors, %d native lifecycle, %d Open, %d construction order", contract.GoVersion, len(contract.Constructors), len(contract.NativeLifecycle), len(contract.OAuth), len(contract.ConstructionOrder))
}

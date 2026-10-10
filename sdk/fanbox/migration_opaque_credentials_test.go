package fanbox_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureOpaqueCredentials = flag.Bool("migration-capture-fanbox-opaque-credentials", false, "capture frozen Go raw FANBOX credential constructors")

type migrationOpaqueCredentialCase struct {
	TrimmedSessionHex   string              `json:"trimmed_session_hex"`
	ErrorAfterTrim      map[string]string   `json:"error_after_trim"`
	RequestsAfterTrim   []map[string]string `json:"requests_after_trim"`
	IdleClosesAfterTrim int                 `json:"idle_closes_after_trim"`

	Name                    string              `json:"name"`
	SessionHex              string              `json:"session_hex"`
	UserAgent               string              `json:"user_agent"`
	ProxyURL                string              `json:"proxy_url"`
	SolverURL               string              `json:"solver_url"`
	SolverProxyURL          string              `json:"solver_proxy_url"`
	ClientMode              string              `json:"client_mode"`
	CloseCalls              int                 `json:"close_calls"`
	Error                   map[string]string   `json:"error"`
	Requests                []map[string]string `json:"requests"`
	IdleCloses              int                 `json:"idle_closes"`
	InjectedClientUnchanged bool                `json:"go_only_injected_client_unchanged"`
	DefaultClientRequests   int                 `json:"go_only_default_client_requests"`
	DefaultClientIdleCloses int                 `json:"go_only_default_client_idle_closes"`
}
type migrationOpaqueCredentialTransport struct {
	requests   []map[string]string
	idleCloses int
	t          *testing.T
}

func (p *migrationOpaqueCredentialTransport) RoundTrip(r *http.Request) (*http.Response, error) {
	p.requests = append(p.requests, map[string]string{"method": r.Method, "url": r.URL.String()})
	p.t.Fatal("OpenWith must never send a transport request")
	return nil, errors.New("owned fixture unexpected request")
}
func (p *migrationOpaqueCredentialTransport) CloseIdleConnections() { p.idleCloses++ }

func migrationOpaqueCredentialRows() []migrationOpaqueCredentialCase {
	rows := []migrationOpaqueCredentialCase{}
	add := func(name string, b []byte) *migrationOpaqueCredentialCase {
		rows = append(rows, migrationOpaqueCredentialCase{Name: name, SessionHex: hex.EncodeToString(b), ClientMode: "injected", CloseCalls: 1, Requests: []map[string]string{}})
		return &rows[len(rows)-1]
	}
	for _, r := range []struct {
		name string
		b    []byte
	}{
		{"ascii", []byte("synthetic-opaque-session")}, {"ascii-surrounding-space", []byte(" \t synthetic-opaque-session \t ")}, {"empty", []byte{}}, {"whitespace", []byte(" \t ")},
		{"invalid-ff", []byte{'a', 0xff, 'b'}}, {"invalid-ff-prefix", []byte{0xff, 'a'}}, {"invalid-ff-tail", []byte{'a', 0xff}}, {"invalid-utf8-incomplete", []byte{'a', 0xe2, 0x82}},
		{"invalid-ff-plus-lf", []byte{'a', 0xff, '\n'}}, {"invalid-ff-plus-cr", []byte{'a', 0xff, '\r'}}, {"invalid-ff-plus-crlf", []byte{'a', 0xff, '\r', '\n'}}, {"invalid-ff-interior-lf", []byte{'a', '\n', 0xff, 'b'}},
		{"invalid-ff-whitespace-tail", []byte{'a', 0xff, ' ', '\t'}}, {"valid-utf8-nonascii", []byte("合成")}, {"embedded-nul", []byte{'a', 0, 'b'}}, {"double-quote", []byte("a\"b")}, {"comma", []byte("a,b")}, {"backslash", []byte("a\\b")},
		{"cookie-pairs", []byte("a; Other=b")}, {"duplicate-precedes-later-invalid-byte", append([]byte("a; FANBOXSESSID=b; Other="), 0xff)},
		{"earlier-malformed-precedes-duplicate", []byte("a; malformed; FANBOXSESSID=b")}, {"later-line-break-precedes-duplicate", []byte("a; FANBOXSESSID=b; Other=x\n")},
		{"earlier-invalid-byte-precedes-duplicate", append(append([]byte("a; Other="), 0xff), []byte("; FANBOXSESSID=b")...)},
		{"duplicate-and-later-invalid-byte-plus-lf", append(append([]byte("a; FANBOXSESSID=b; Other="), 0xff), '\n')},
	} {
		add("credential/"+r.name, r.b)
	}
	add("credential/invalid-ff-outer-cr", []byte{'\r', 'a', 0xff, '\r'})
	add("credential/invalid-ff-outer-crlf", []byte{'\r', '\n', 'a', 0xff, '\r', '\n'})
	add("credential/invalid-ff-outer-unicode-space", append(append([]byte("\u3000"), 0xff), []byte("\u00a0\u2003")...))
	for _, mode := range []string{"agent", "proxy", "solver-service", "solver-proxy", "agent-before-proxy-before-solver", "proxy-before-solver", "solver-service-before-solver-proxy"} {
		r := add("options/invalid-utf8/"+mode, []byte{'a', 0xff, 'b'})
		switch mode {
		case "agent":
			r.UserAgent = "bad\nagent"
		case "proxy":
			r.ProxyURL = "://invalid"
		case "solver-service":
			r.SolverURL = "http://solver.invalid/private"
		case "solver-proxy":
			r.SolverURL = "http://solver.invalid/"
			r.SolverProxyURL = "https://proxy.invalid"
		case "agent-before-proxy-before-solver":
			r.UserAgent = "bad\ragent"
			r.ProxyURL = "://invalid"
			r.SolverURL = "http://solver.invalid/private"
		case "proxy-before-solver":
			r.ProxyURL = "://invalid"
			r.SolverURL = "http://solver.invalid/private"
		case "solver-service-before-solver-proxy":
			r.SolverURL = "http://solver.invalid/private"
			r.SolverProxyURL = "https://proxy.invalid"
		}
	}
	r := add("options/invalid-utf8-lf-invalid-agent", []byte{'a', 0xff, '\n'})
	r.UserAgent = "bad\x00agent"
	r = add("options/valid-network-options-invalid-utf8", []byte{'a', 0xff})
	r.UserAgent = "synthetic-opaque-agent"
	r.ProxyURL = "https://proxy.invalid:8443/path"
	r.SolverURL = "http://solver.invalid/"
	r.SolverProxyURL = "socks5://proxy.invalid:1080"
	r = add("options/valid-network-options-ascii", []byte("synthetic-opaque-session"))
	r.UserAgent = "synthetic-opaque-agent"
	r.ProxyURL = "https://proxy.invalid:8443/path"
	r.SolverURL = "http://solver.invalid/"
	r.SolverProxyURL = "socks5://proxy.invalid:1080"
	r = add("ownership/caller-client-close-twice", []byte("synthetic-opaque-session"))
	r.CloseCalls = 2
	r = add("ownership/caller-client-no-close", []byte("synthetic-opaque-session"))
	r.CloseCalls = 0
	r = add("ownership/default-client-explicitly-injected", []byte("synthetic-opaque-session"))
	r.ClientMode = "default-injected"
	return rows
}
func migrationOpaqueCredentialObserve(t *testing.T, r *migrationOpaqueCredentialCase) {
	t.Helper()
	migrationOpaqueCredentialObserveBase(t, r)
	raw, e := hex.DecodeString(r.SessionHex)
	if e != nil {
		t.Fatal(e)
	}
	trimmed := strings.TrimSpace(string(raw))
	r.TrimmedSessionHex = hex.EncodeToString([]byte(trimmed))
	after := *r
	after.SessionHex = r.TrimmedSessionHex
	migrationOpaqueCredentialObserveBase(t, &after)
	r.ErrorAfterTrim = after.Error
	r.RequestsAfterTrim = after.Requests
	r.IdleClosesAfterTrim = after.IdleCloses
}
func migrationOpaqueCredentialObserveBase(t *testing.T, r *migrationOpaqueCredentialCase) {
	t.Helper()
	raw, e := hex.DecodeString(r.SessionHex)
	if e != nil || hex.EncodeToString(raw) != r.SessionHex {
		t.Fatal("raw credential input must preserve exact lowercase hex")
	}
	native := &migrationOpaqueCredentialTransport{requests: []map[string]string{}, t: t}
	fallback := &migrationOpaqueCredentialTransport{requests: []map[string]string{}, t: t}
	previousDefault, previousTransport := http.DefaultClient, http.DefaultTransport
	http.DefaultClient = &http.Client{Transport: fallback}
	http.DefaultTransport = fallback
	defer func() { http.DefaultClient = previousDefault; http.DefaultTransport = previousTransport }()
	client := &http.Client{Transport: native}
	if r.ClientMode == "default-injected" {
		client = http.DefaultClient
		client.Transport = native
	}
	options := fanbox.Options{HTTPClient: client, UserAgent: r.UserAgent, ProxyURL: r.ProxyURL}
	if r.SolverURL != "" || r.SolverProxyURL != "" {
		options.FlareSolverr = &fanbox.FlareSolverrOptions{URL: r.SolverURL, ProxyURL: r.SolverProxyURL}
	}
	opened, e := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: string(raw)}, options)
	r.Error = nil
	if e != nil {
		var classified *sdk.Error
		if !errors.As(e, &classified) {
			t.Fatalf("constructor did not return classified SDK error: %v", e)
		}
		cause := ""
		if c := errors.Unwrap(e); c != nil {
			cause = c.Error()
		}
		r.Error = map[string]string{"reason": string(classified.Reason), "message": e.Error(), "cause": cause}
		if opened != nil {
			t.Fatal("failed OpenWith retained client")
		}
	}
	if e == nil {
		if opened == nil {
			t.Fatal("successful OpenWith returned nil")
		}
		for i := 0; i < r.CloseCalls; i++ {
			opened.CloseIdleConnections()
		}
	}
	var nilClient *fanbox.Client
	nilClient.CloseIdleConnections()
	r.Requests = native.requests
	r.IdleCloses = native.idleCloses
	r.InjectedClientUnchanged = client.Transport == native && client.Jar == nil && client.Timeout == 0 && client.CheckRedirect == nil
	r.DefaultClientRequests = len(fallback.requests)
	r.DefaultClientIdleCloses = fallback.idleCloses
	if !r.InjectedClientUnchanged || r.DefaultClientRequests != 0 || r.DefaultClientIdleCloses != 0 {
		t.Fatal("constructor changed or touched noninjected caller/default ownership")
	}
}
func TestMigrationFanboxOpaqueCredentials(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	guard := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	preserved := map[string]string{"internal/cli/migration_fanbox_auth_test.go": "10f64780a8a1bb49d0ea9ba7098c86f2c96d743775d8d9505ab7a1281ef56a08", "crates/pixiv-cli/tests/fixtures/fanbox-auth.json": "1d3bab3ca00810abace1ebb34125f61df2d692db548baa53e31ecd0b814c2351"}
	for p, want := range preserved {
		b, e := os.ReadFile(filepath.Join(root, p))
		if e != nil || fmt.Sprintf("%x", sha256.Sum256(b)) != want {
			t.Fatalf("sealed prior FANBOX auth changed: %s: %v", p, e)
		}
	}
	rows := migrationOpaqueCredentialRows()
	for i := range rows {
		r := &rows[i]
		t.Run(r.Name, func(t *testing.T) { migrationOpaqueCredentialObserve(t, r) })
	}
	if t.Failed() {
		return
	}
	fixture := map[string]any{"source_commit": reference.SourceCommit, "go_version": runtime.Version(), "source_sha256": reference.SourceSHA256, "go_stdlib_sha256": reference.GoStdlibSHA256, "dependencies": reference.Dependencies, "frozen_go_production_guard": guard, "preserved_auth_sha256": preserved, "cases": rows, "public_operation": "fanbox.OpenWith(SessionCredentials{FANBOXSESSID:string(rawbytes)}, Options{HTTPClient:explicitSyntheticClient})", "evidence": "Actual unchanged public OpenWith with raw byte inputs decoded only from lowercase hex to Go opaque strings, ordinary injected http.Client/RoundTripper and constructor/idle-close observations. No secret substitution, lossy normalization, CurrentUser DTO or private normalizer oracle supplies expectations. trimmed_session_hex comes from actual standard strings.TrimSpace on the raw Go string, followed by a separate actual OpenWith with a fresh injected transport; it records the service input preflight correspondence without claiming to execute AccountService.", "limitations": []string{"Every constructor receives a nonnil http.Client with a synthetic explicit transport. No CurrentUser or other operation sends a request. Unexpected calls fail before returning any response.", "Nil SDK Client.CloseIdleConnections is called and observed as a no-op. http.DefaultClient/http.DefaultTransport are synthetic canaries restored after each row. One row explicitly injects the synthetic default client, never a native/default transport.", "Go pointer identity and HTTP-client shape remain Go-only ownership projections; constructor classification, exact message/cause, request count and explicit idle-close counts are public observations.", "Successful nil Options.HTTPClient native transport construction and live TLS/platform/browser/authentication are outside this raw-byte supplement.", "The denied supplemental native multiplex/unfinished-HEAD/upload probe is neither reconstructed nor retried."}}
	data, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates/pixiv-sdk/tests/fixtures/fanbox-opaque-credentials.json")
	if *migrationCaptureOpaqueCredentials {
		if e = os.WriteFile(path, data, 0644); e != nil {
			t.Fatal(e)
		}
	}
	want, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("raw FANBOX credential behavior differs from frozen Go capture")
	}
	t.Logf("frozen Go raw-credential constructor cases=%d sha256=%x", len(rows), sha256.Sum256(data))
}

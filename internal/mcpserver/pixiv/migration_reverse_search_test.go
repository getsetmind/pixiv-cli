package pixiv_test

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/reversesearch"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPReverseSearch = flag.Bool("migration-update-mcp-reverse-search", false, "capture frozen Go reverse_search MCP wire contracts")

type migrationReverseWire struct {
	mu       sync.Mutex
	sent     []json.RawMessage
	received []json.RawMessage
	changed  chan struct{}
}

type migrationReverseTransport struct {
	mcp.Transport
	wire *migrationReverseWire
}

type migrationReverseConnection struct {
	mcp.Connection
	wire *migrationReverseWire
}

func (t migrationReverseTransport) Connect(ctx context.Context) (mcp.Connection, error) {
	c, err := t.Transport.Connect(ctx)
	if err != nil {
		return nil, err
	}
	return migrationReverseConnection{c, t.wire}, nil
}

func (c migrationReverseConnection) Read(ctx context.Context) (jsonrpc.Message, error) {
	m, err := c.Connection.Read(ctx)
	if err == nil {
		err = c.wire.record(m, false)
	}
	return m, err
}

func (c migrationReverseConnection) Write(ctx context.Context, m jsonrpc.Message) error {
	if err := c.wire.record(m, true); err != nil {
		return err
	}
	return c.Connection.Write(ctx, m)
}

func (w *migrationReverseWire) record(m jsonrpc.Message, sent bool) error {
	data, err := jsonrpc.EncodeMessage(m)
	if err != nil {
		return err
	}
	w.mu.Lock()
	if sent {
		w.sent = append(w.sent, data)
	} else {
		w.received = append(w.received, data)
	}
	w.mu.Unlock()
	select {
	case w.changed <- struct{}{}:
	default:
	}
	return nil
}

func (w *migrationReverseWire) messages() ([]json.RawMessage, []json.RawMessage) {
	w.mu.Lock()
	defer w.mu.Unlock()
	return append([]json.RawMessage{}, w.sent...), append([]json.RawMessage{}, w.received...)
}

func (w *migrationReverseWire) response(ctx context.Context, id int) (json.RawMessage, error) {
	for {
		_, received := w.messages()
		for _, raw := range received {
			var message struct {
				ID int `json:"id"`
			}
			if err := json.Unmarshal(raw, &message); err != nil {
				return nil, err
			}
			if message.ID == id {
				return raw, nil
			}
		}
		select {
		case <-w.changed:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
}

type migrationReverseSearcher struct {
	mu       sync.Mutex
	requests []reversesearch.Request
	closed   int
	search   func(context.Context, reversesearch.Request) (reversesearch.Response, error)
}

func (s *migrationReverseSearcher) Search(ctx context.Context, request reversesearch.Request) (reversesearch.Response, error) {
	s.mu.Lock()
	s.requests = append(s.requests, request)
	s.mu.Unlock()
	return s.search(ctx, request)
}

func (s *migrationReverseSearcher) Close() error {
	s.mu.Lock()
	s.closed++
	s.mu.Unlock()
	return nil
}

func (s *migrationReverseSearcher) snapshot() ([]reversesearch.Request, int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]reversesearch.Request{}, s.requests...), s.closed
}

func migrationReverseSession(t *testing.T, ctx context.Context, searcher reversesearch.Searcher, provider reversesearch.Provider, pixivOnly bool, executeCalls *atomic.Int32) (*mcp.ClientSession, *migrationReverseWire, func()) {
	t.Helper()
	proxy := "http://startup-proxy.invalid:1080"
	server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{
		ReverseSearch: pixivmcp.ReverseSearchPorts{Searcher: searcher, Provider: provider, PixivOnly: pixivOnly},
		Execute: func(context.Context, pixivmcp.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
			executeCalls.Add(1)
			return errors.New("account-execute-secret must not be called")
		},
	}, pixivmcp.Account{HTTPSProxyOverride: &proxy})
	clientTransport, serverTransport := mcp.NewInMemoryTransports()
	runCtx, cancel := context.WithCancel(ctx)
	done := make(chan error, 1)
	go func() { done <- server.Run(runCtx, serverTransport) }()
	wire := &migrationReverseWire{changed: make(chan struct{}, 1)}
	session, err := mcp.NewClient(&mcp.Implementation{Name: "migration-reverse-search", Version: "0"}, nil).Connect(ctx, migrationReverseTransport{clientTransport, wire}, nil)
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	var once sync.Once
	cleanup := func() {
		once.Do(func() {
			if err := session.Close(); err != nil {
				t.Errorf("session Close: %v", err)
			}
			if err := session.Close(); err != nil {
				t.Errorf("repeated session Close: %v", err)
			}
			cancel()
			select {
			case <-done:
			case <-ctx.Done():
				t.Errorf("server Run did not finish: %v", ctx.Err())
			}
		})
	}
	t.Cleanup(cleanup)
	return session, wire, cleanup
}

type migrationReverseCase struct {
	Name         string                  `json:"name"`
	Arguments    json.RawMessage         `json:"arguments,omitempty"`
	Provider     reversesearch.Provider  `json:"startup_provider"`
	PixivOnly    bool                    `json:"startup_pixiv_only"`
	Unconfigured bool                    `json:"unconfigured"`
	Response     reversesearch.Response  `json:"searcher_response"`
	ErrorCode    reversesearch.ErrorCode `json:"searcher_error_code"`
	ErrorKind    string                  `json:"searcher_error_kind"`
	WireRequest  json.RawMessage         `json:"wire_request"`
	WireResponse json.RawMessage         `json:"wire_response"`
	RPCError     string                  `json:"rpc_error"`
	Requests     []reversesearch.Request `json:"searcher_requests"`
	CloseCalls   int                     `json:"searcher_close_calls_after_session_close"`
	ExecuteCalls int32                   `json:"sdk_execute_calls"`
}

type migrationReverseLifecycle struct {
	Sent         []json.RawMessage       `json:"sent"`
	Received     []json.RawMessage       `json:"received"`
	Requests     []reversesearch.Request `json:"searcher_requests"`
	CloseCalls   int                     `json:"searcher_close_calls_after_session_close"`
	ExecuteCalls int32                   `json:"sdk_execute_calls"`
	LocalError   string                  `json:"local_error"`
}

func migrationReverseResponse() reversesearch.Response {
	return reversesearch.Response{
		Input: reversesearch.Input{Kind: reversesearch.SourceKindFile, SHA256: "synthetic-sha256"},
		Providers: []reversesearch.ProviderSummary{{Name: reversesearch.ProviderASCII2DColor, Status: reversesearch.ProviderStatusSuccess, ResultCount: 3,
			Quota: &reversesearch.Quota{ShortRemaining: 3, LongRemaining: 9, ShortLimit: 5, LongLimit: 10}}},
		Results: []reversesearch.Result{
			{Pixiv: &reversesearch.PixivRef{Type: reversesearch.PixivRefArtwork, ID: 42}, Title: "Synthetic artwork", Author: "Synthetic author", Evidence: []reversesearch.Evidence{{Provider: reversesearch.ProviderASCII2DColor, Rank: 1, Similarity: 91.25, IndexID: 5, IndexName: "Synthetic index", Title: "Evidence title", Author: "Evidence author", ExternalURLs: []string{"https://www.pixiv.net/artworks/42"}}}},
			{Pixiv: &reversesearch.PixivRef{Type: reversesearch.PixivRefUser, ID: 7}},
			{Title: "External match", Evidence: []reversesearch.Evidence{{Provider: reversesearch.ProviderASCII2DBOVW, Rank: 2, ExternalURLs: []string{"https://external.invalid/synthetic"}}}},
		},
	}
}

func TestMigrationMCPReverseSearchMatchesFrozenConnectedWire(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()
	var executeCalls atomic.Int32
	session, wire, closeSession := migrationReverseSession(t, ctx, nil, "", false, &executeCalls)
	if _, err := session.ListTools(ctx, nil); err != nil {
		t.Fatal(err)
	}
	listing, err := wire.response(ctx, 2)
	if err != nil {
		t.Fatal(err)
	}
	var listed struct {
		Result struct {
			Tools []json.RawMessage `json:"tools"`
		} `json:"result"`
	}
	if err := json.Unmarshal(listing, &listed); err != nil {
		t.Fatal(err)
	}
	var tool json.RawMessage
	for _, raw := range listed.Result.Tools {
		var item struct {
			Name string `json:"name"`
		}
		if err := json.Unmarshal(raw, &item); err != nil {
			t.Fatal(err)
		}
		if item.Name == "reverse_search" {
			tool = raw
		}
	}
	if len(tool) == 0 {
		t.Fatal("reverse_search not listed on the actual SDK wire")
	}
	initialization, err := wire.response(ctx, 1)
	if err != nil {
		t.Fatal(err)
	}
	closeSession()
	var cases []migrationReverseCase
	for _, input := range []struct{ name, args string }{
		{"missing-arguments", ""}, {"null-arguments", "null"}, {"empty-object", "{}"}, {"array-arguments", "[]"}, {"string-arguments", `"private-source-secret"`},
		{"number-source", `{"source":42}`}, {"boolean-source", `{"source":true}`}, {"null-source", `{"source":null}`},
		{"empty-source", `{"source":""}`}, {"blank-source", `{"source":" \t\n "}`},
		{"unknown-property", `{"source":"/synthetic/private-source-secret.png","extra":true}`},
		{"forbidden-pixiv-only", `{"source":"/synthetic/private-source-secret.png","pixiv_only":false}`},
		{"forbidden-proxy", `{"source":"/synthetic/private-source-secret.png","https_proxy":"http://input-proxy-secret.invalid"}`},
		{"forbidden-key", `{"source":"/synthetic/private-source-secret.png","api_key":"input-key-secret"}`},
		{"invalid-provider", `{"source":"/synthetic/private-source-secret.png","provider":"missing"}`},
		{"empty-provider", `{"source":"/synthetic/private-source-secret.png","provider":""}`},
		{"null-provider", `{"source":"/synthetic/private-source-secret.png","provider":null}`},
		{"number-provider", `{"source":"/synthetic/private-source-secret.png","provider":42}`},
		{"default-provider", `{"source":"/synthetic/private-source-secret.png"}`},
	} {
		cases = append(cases, migrationReverseCase{Name: input.name, Arguments: json.RawMessage(input.args)})
	}
	for _, provider := range []reversesearch.Provider{reversesearch.ProviderSauceNAO, reversesearch.ProviderASCII2DColor, reversesearch.ProviderASCII2DBOVW, reversesearch.ProviderAll} {
		cases = append(cases, migrationReverseCase{Name: "override-" + string(provider), Arguments: json.RawMessage(fmt.Sprintf(`{"source":"https://synthetic-source-secret.invalid/image.png?token=source-key-secret","provider":%q}`, provider)), Provider: reversesearch.ProviderAll, PixivOnly: true, Response: migrationReverseResponse()})
	}
	cases = append(cases,
		migrationReverseCase{Name: "startup-provider-pixiv-only-false", Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png"}`), Provider: reversesearch.ProviderASCII2DBOVW, Response: migrationReverseResponse()},
		migrationReverseCase{Name: "startup-provider-pixiv-only-true", Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png"}`), Provider: reversesearch.ProviderAll, PixivOnly: true, Response: migrationReverseResponse()},
		migrationReverseCase{Name: "unconfigured", Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png"}`), Unconfigured: true},
	)
	partial := migrationReverseResponse()
	partial.Partial = true
	partial.Providers = append([]reversesearch.ProviderSummary{{Name: reversesearch.ProviderSauceNAO, Status: reversesearch.ProviderStatusError}}, partial.Providers...)
	partial.ProviderErrors = []reversesearch.ProviderError{{Provider: reversesearch.ProviderSauceNAO, Code: reversesearch.CodeMissingCredential, Message: "SauceNAO API key is required"}}
	cases = append(cases, migrationReverseCase{Name: "partial-results-not-mcp-error", Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png","provider":"all"}`), Response: partial})
	for _, code := range []reversesearch.ErrorCode{reversesearch.CodeInvalidRequest, reversesearch.CodeInvalidSource, reversesearch.CodeSourceNotRegularFile, reversesearch.CodeSourceReadFailed, reversesearch.CodeSourceHTTPStatus, reversesearch.CodeSnapshotFailed, reversesearch.CodeSourceLoaderNotConfigured, reversesearch.CodeProviderNotConfigured, reversesearch.CodeMissingCredential, reversesearch.CodeMalformedUpstreamResponse, reversesearch.CodeUpstreamHTTPStatus, reversesearch.CodeProviderFailed, reversesearch.CodeAllProvidersFailed, reversesearch.CodeChallengeRequired, reversesearch.CodeSolverUnavailable, reversesearch.CodeSolverFailed, reversesearch.CodeMalformedSolverResponse, reversesearch.CodeUnknown} {
		response := migrationReverseResponse()
		response.ProviderErrors = []reversesearch.ProviderError{{Provider: reversesearch.ProviderASCII2DColor, Code: code, Message: "safe provider failure"}}
		cases = append(cases, migrationReverseCase{Name: "error-" + string(code), Arguments: json.RawMessage(`{"source":"https://synthetic-source-secret.invalid/image.png?token=source-key-secret"}`), Response: response, ErrorCode: code})
	}
	for _, kind := range []string{"unclassified", "canceled", "deadline"} {
		cases = append(cases, migrationReverseCase{Name: "error-" + kind, Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png"}`), Response: migrationReverseResponse(), ErrorKind: kind})
	}
	for _, invalid := range []struct {
		name string
		ref  reversesearch.PixivRef
	}{{"invalid-identity-type", reversesearch.PixivRef{Type: "novel", ID: 42}}, {"zero-identity", reversesearch.PixivRef{Type: reversesearch.PixivRefArtwork}}, {"negative-identity", reversesearch.PixivRef{Type: reversesearch.PixivRefUser, ID: -1}}} {
		response := migrationReverseResponse()
		response.Results[1].Pixiv = &invalid.ref
		cases = append(cases, migrationReverseCase{Name: invalid.name, Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png"}`), Response: response})
	}
	duplicate := migrationReverseResponse()
	duplicate.Results = append(duplicate.Results, duplicate.Results[0])
	cases = append(cases, migrationReverseCase{Name: "duplicate-records-preserved", Arguments: json.RawMessage(`{"source":"/synthetic/private-source-secret.png"}`), Response: duplicate})
	for index := range cases {
		row := &cases[index]
		searcher := &migrationReverseSearcher{search: func(context.Context, reversesearch.Request) (reversesearch.Response, error) {
			var err error
			if row.ErrorCode != "" {
				err = reversesearch.NewError(row.ErrorCode, "domain-message-secret", errors.New("cause-key-secret csrf-secret upstream-body-secret location-secret"))
			}
			switch row.ErrorKind {
			case "unclassified":
				err = errors.New("cause-key-secret csrf-secret upstream-body-secret location-secret")
			case "canceled":
				err = context.Canceled
			case "deadline":
				err = context.DeadlineExceeded
			}
			return row.Response, err
		}}
		var port reversesearch.Searcher = searcher
		if row.Unconfigured {
			port = nil
		}
		var calls atomic.Int32
		session, wire, closeSession := migrationReverseSession(t, ctx, port, row.Provider, row.PixivOnly, &calls)
		var args any
		if len(row.Arguments) > 0 {
			args = row.Arguments
		}
		_, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "reverse_search", Arguments: args})
		if err != nil {
			row.RPCError = err.Error()
		}
		row.WireResponse, err = wire.response(ctx, 2)
		if err != nil {
			t.Fatalf("%s: %v", row.Name, err)
		}
		sent, _ := wire.messages()
		row.WireRequest = sent[len(sent)-1]
		migrationReverseNoSecrets(t, row.WireResponse)
		closeSession()
		row.Requests, row.CloseCalls = searcher.snapshot()
		row.ExecuteCalls = calls.Load()
		if row.CloseCalls != 0 || row.ExecuteCalls != 0 {
			t.Fatalf("%s: reverse_search used unrelated SDK execution or closed caller-owned Searcher", row.Name)
		}
	}
	cancellation := migrationReverseCancellation(t, ctx)
	concurrent := migrationReverseConcurrent(t, ctx)
	stdio := migrationReverseStdio(t, ctx)
	data, err := json.MarshalIndent(struct {
		Reference      string                    `json:"reference"`
		Scope          string                    `json:"scope"`
		StartupProxy   string                    `json:"account_https_proxy_override"`
		Initialization json.RawMessage           `json:"initialization"`
		Tool           json.RawMessage           `json:"tool"`
		Cases          []migrationReverseCase    `json:"cases"`
		Cancellation   migrationReverseLifecycle `json:"cancellation_and_reuse"`
		Concurrent     migrationReverseLifecycle `json:"concurrent_and_reuse"`
		Stdio          []json.RawMessage         `json:"owned_test_child_stdio_responses"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", "Actual SDK sessions and registered MCP handler; in-memory transport and owned Go test-binary stdio child; Searcher dependency fixtures. No CLI composition, real provider/fingerprint transport, proxy traffic, media fetch or upload.", "http://startup-proxy.invalid:1080", initialization, tool, cases, cancellation, concurrent, stdio}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "crates", "pixiv-cli", "tests", "fixtures", "reverse-search-mcp.json")
	if *migrationUpdateMCPReverseSearch {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		_ = os.WriteFile(filepath.Join(os.TempDir(), "reverse-mcp-actual.json"), data, 0o644)
		t.Fatal("connected reverse_search MCP wire differs from frozen Go fixture")
	}
}

func migrationReverseNoSecrets(t *testing.T, data []byte) {
	t.Helper()
	for _, secret := range []string{"private-source-secret", "synthetic-source-secret", "source-key-secret", "domain-message-secret", "cause-key-secret", "csrf-secret", "upstream-body-secret", "location-secret", "account-execute-secret"} {
		if strings.Contains(string(data), secret) {
			t.Fatalf("MCP output leaked %q: %s", secret, data)
		}
	}
}

func migrationReverseLifecycleSnapshot(t *testing.T, wire *migrationReverseWire, searcher *migrationReverseSearcher, calls *atomic.Int32, localError string) migrationReverseLifecycle {
	t.Helper()
	sent, received := wire.messages()
	for _, raw := range received {
		migrationReverseNoSecrets(t, raw)
	}
	requests, closed := searcher.snapshot()
	if closed != 0 || calls.Load() != 0 {
		t.Fatal("MCP session owns neither the Searcher Close nor account Execute boundary")
	}
	return migrationReverseLifecycle{sent, received, requests, closed, calls.Load(), localError}
}

func migrationReverseCancellation(t *testing.T, ctx context.Context) migrationReverseLifecycle {
	t.Helper()
	started, stopped := make(chan struct{}), make(chan struct{})
	searcher := &migrationReverseSearcher{search: func(ctx context.Context, request reversesearch.Request) (reversesearch.Response, error) {
		if request.Source == "/synthetic/private-source-secret-cancel.png" {
			close(started)
			<-ctx.Done()
			defer close(stopped)
			return migrationReverseResponse(), ctx.Err()
		}
		return migrationReverseResponse(), nil
	}}
	var calls atomic.Int32
	session, wire, closeSession := migrationReverseSession(t, ctx, searcher, reversesearch.ProviderAll, true, &calls)
	callCtx, cancel := context.WithCancel(ctx)
	done := make(chan error, 1)
	go func() {
		_, err := session.CallTool(callCtx, &mcp.CallToolParams{Name: "reverse_search", Arguments: map[string]any{"source": "/synthetic/private-source-secret-cancel.png"}})
		done <- err
	}()
	select {
	case <-started:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	cancel()
	err := <-done
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled SDK call = %v", err)
	}
	select {
	case <-stopped:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	if _, err := wire.response(ctx, 2); err != nil {
		t.Fatal(err)
	}
	if result, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "reverse_search", Arguments: map[string]any{"source": "/synthetic/private-source-secret-reuse.png"}}); err != nil || result.IsError {
		t.Fatalf("reuse after cancellation: result=%v error=%v", result, err)
	}
	closeSession()
	return migrationReverseLifecycleSnapshot(t, wire, searcher, &calls, err.Error())
}

func migrationReverseConcurrent(t *testing.T, ctx context.Context) migrationReverseLifecycle {
	t.Helper()
	startedA, startedB, releaseA, releaseB := make(chan struct{}), make(chan struct{}), make(chan struct{}), make(chan struct{})
	searcher := &migrationReverseSearcher{search: func(ctx context.Context, request reversesearch.Request) (reversesearch.Response, error) {
		var started, release chan struct{}
		switch request.Source {
		case "/synthetic/private-source-secret-a.png":
			started, release = startedA, releaseA
		case "/synthetic/private-source-secret-b.png":
			started, release = startedB, releaseB
		}
		if started != nil {
			close(started)
			select {
			case <-release:
			case <-ctx.Done():
				return reversesearch.Response{}, ctx.Err()
			}
		}
		response := migrationReverseResponse()
		response.Input.SHA256 = "synthetic-" + string(request.Provider)
		return response, nil
	}}
	var calls atomic.Int32
	session, wire, closeSession := migrationReverseSession(t, ctx, searcher, reversesearch.ProviderAll, false, &calls)
	call := func(source, provider string) <-chan error {
		done := make(chan error, 1)
		go func() {
			result, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "reverse_search", Arguments: map[string]any{"source": source, "provider": provider}})
			if err == nil && result.IsError {
				err = errors.New("unexpected MCP tool error")
			}
			done <- err
		}()
		return done
	}
	a := call("/synthetic/private-source-secret-a.png", "ascii2d-color")
	select {
	case <-startedA:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	b := call("/synthetic/private-source-secret-b.png", "ascii2d-bovw")
	select {
	case <-startedB:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	close(releaseB)
	if err := <-b; err != nil {
		t.Fatal(err)
	}
	close(releaseA)
	if err := <-a; err != nil {
		t.Fatal(err)
	}
	if err := <-call("/synthetic/private-source-secret-reuse.png", "all"); err != nil {
		t.Fatal(err)
	}
	closeSession()
	_, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "reverse_search", Arguments: map[string]any{"source": "/synthetic/private-source-secret-after-close.png"}})
	if err == nil {
		t.Fatal("closed SDK session accepted a new call")
	}
	return migrationReverseLifecycleSnapshot(t, wire, searcher, &calls, err.Error())
}

func migrationReverseStdio(t *testing.T, ctx context.Context) []json.RawMessage {
	t.Helper()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationMCPReverseSearchStdioHelper$")
	command.Env = append(os.Environ(), "PIXIV_MIGRATION_REVERSE_STDIO=1")
	stdin, err := command.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	stdout, err := command.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	var stderr bytes.Buffer
	command.Stderr = &stderr
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() {
		_ = stdin.Close()
		if command.ProcessState == nil {
			_ = command.Process.Kill()
			_ = command.Wait()
		}
	}()
	scanner := bufio.NewScanner(stdout)
	scanner.Buffer(make([]byte, 4096), 1024*1024)
	var responses []json.RawMessage
	for _, request := range []string{
		`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"migration-reverse-search","version":"0"}}}`,
		`{"jsonrpc":"2.0","method":"notifications/initialized"}`,
		`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"/synthetic/private-source-secret.png"}}}`,
		`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"/synthetic/private-source-secret.png","provider":"ascii2d-color"}}}`,
		`{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"reverse_search","arguments":{"source":"/synthetic/private-source-secret.png","pixiv_only":false}}}`,
	} {
		if _, err := io.WriteString(stdin, request+"\n"); err != nil {
			t.Fatal(err)
		}
		if strings.Contains(request, `"notifications/initialized"`) {
			continue
		}
		if !scanner.Scan() {
			t.Fatalf("owned stdio child terminated: %v", scanner.Err())
		}
		raw := append(json.RawMessage{}, scanner.Bytes()...)
		if !json.Valid(raw) {
			t.Fatalf("stdio stdout is not protocol-only: %s", raw)
		}
		migrationReverseNoSecrets(t, raw)
		responses = append(responses, raw)
	}
	if err := stdin.Close(); err != nil {
		t.Fatal(err)
	}
	for scanner.Scan() {
		t.Fatalf("owned stdio child emitted unexpected stdout: %s", scanner.Text())
	}
	if err := command.Wait(); err != nil {
		t.Fatalf("owned stdio child: %v; stderr=%s", err, stderr.String())
	}
	if scanner.Err() != nil || stderr.Len() != 0 {
		t.Fatalf("stdio stderr or scanner error: %q; %v", stderr.String(), scanner.Err())
	}
	return responses
}

func TestMigrationMCPReverseSearchStdioHelper(t *testing.T) {
	if os.Getenv("PIXIV_MIGRATION_REVERSE_STDIO") != "1" {
		return
	}
	searcher := reverseSearcherFunc(func(_ context.Context, request reversesearch.Request) (reversesearch.Response, error) {
		response := migrationReverseResponse()
		response.Input.SHA256 = "synthetic-" + string(request.Provider)
		return response, nil
	})
	server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{ReverseSearch: pixivmcp.ReverseSearchPorts{Searcher: searcher, Provider: reversesearch.ProviderASCII2DBOVW, PixivOnly: true}}, pixivmcp.Account{})
	if err := server.Run(context.Background(), &mcp.StdioTransport{}); err != nil {
		os.Exit(1)
	}
	os.Exit(0)
}

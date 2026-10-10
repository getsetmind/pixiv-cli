package fanbox

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxSavedLeases = flag.Bool("migration-capture-fanbox-saved-leases", false, "capture frozen FANBOX saved-account owned lease contracts to /tmp")

const migrationFanboxLeaseSourceCommit = "4b4426487ef18bed276706daec385e0d0a6979f9"

var migrationFanboxLeaseSourceHashes = map[string]string{
	"go.mod":                                        "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
	"go.sum":                                        "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
	"internal/services/fanbox/fanbox.go":            "694071130c0a6ad5c1ae174df2dead5ca6d929f9a8826e09c1ab86b5fecfe9e7",
	"internal/shared/lifecycle/lifecycle.go":        "2c5524fc40cc2202ba06934e01369824909ad2bd7f0f43478ceab9e344298384",
	"internal/shared/lifecycle/lease.go":            "9715a8f3592aa0269b6f85b1a52751f2e0f2947e34fb133836fb1ef07172b381",
	"internal/shared/lifecycle/attempt.go":          "5f329d3433b77b30a33570f8425d9742e4ac1fb09012aee2a628f6f14578d614",
	"sdk/fanbox/fanbox.go":                          "2208576144b94b89efd57b6f054ee018268812d52d556fd0758e2ee322577542",
	"sdk/fanbox/errors.go":                          "523577a066e3d53ce9eced8c7afbe29f422a75ce6aca58bc3b5b6fbf8da59909",
	"sdk/error.go":                                  "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
	"internal/services/fanbox/protocol/protocol.go": "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3",
	"internal/services/fanbox/protocol/cookie.go":   "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9",
	"internal/services/fanbox/protocol/solver.go":   "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea",
}

type migrationFanboxLeaseSection struct {
	SourceCommit string                     `json:"source_commit"`
	SourceSHA256 map[string]string          `json:"source_sha256"`
	GoVersion    string                     `json:"go_version"`
	Cases        []migrationFanboxLeaseCase `json:"cases"`
	GoOnly       []string                   `json:"go_only"`
	Limitations  []string                   `json:"limitations"`
}

type migrationFanboxLeaseCase struct {
	ID          string         `json:"id"`
	Operation   string         `json:"operation"`
	Observation map[string]any `json:"observation"`
}

type migrationFanboxLeaseError struct {
	Message          string   `json:"message"`
	ConcreteType     string   `json:"concrete_type"`
	Children         []string `json:"children"`
	NilContext       bool     `json:"nil_context"`
	NilUse           bool     `json:"nil_use"`
	MissingAccounts  bool     `json:"missing_accounts"`
	NilClient        bool     `json:"nil_client"`
	OpenFailure      bool     `json:"open_failure"`
	UseFailure       bool     `json:"use_failure"`
	CloseFailure     bool     `json:"close_failure"`
	SameOpenFailure  bool     `json:"same_open_failure"`
	SameUseFailure   bool     `json:"same_use_failure"`
	SameCloseFailure bool     `json:"same_close_failure"`
	Canceled         bool     `json:"canceled"`
	DeadlineExceeded bool     `json:"deadline_exceeded"`
	SDKReason        string   `json:"sdk_reason"`
	RetrySafe        bool     `json:"retry_safe"`
	RetryHasAfter    bool     `json:"retry_has_after"`
	RetryAfterUnix   int64    `json:"retry_after_unix"`
}

type migrationFanboxLeaseErrors struct {
	open  error
	use   error
	close error
}

func (refs migrationFanboxLeaseErrors) observe(err error) migrationFanboxLeaseError {
	row := migrationFanboxLeaseError{
		Children: []string{}, NilContext: errors.Is(err, lifecycle.ErrNilContext),
		NilUse: errors.Is(err, lifecycle.ErrNilUse), MissingAccounts: errors.Is(err, ErrAccountServiceNotConfigured),
		NilClient: errors.Is(err, ErrNilClient), OpenFailure: errors.Is(err, refs.open),
		UseFailure: errors.Is(err, refs.use), CloseFailure: errors.Is(err, refs.close),
		SameOpenFailure: err != nil && err == refs.open, SameUseFailure: err != nil && err == refs.use,
		SameCloseFailure: err != nil && err == refs.close,
		Canceled:         errors.Is(err, context.Canceled), DeadlineExceeded: errors.Is(err, context.DeadlineExceeded),
	}
	if err == nil {
		return row
	}
	row.Message, row.ConcreteType = err.Error(), fmt.Sprintf("%T", err)
	if joined, ok := err.(interface{ Unwrap() []error }); ok {
		for _, child := range joined.Unwrap() {
			row.Children = append(row.Children, child.Error())
		}
	} else if child := errors.Unwrap(err); child != nil {
		row.Children = append(row.Children, child.Error())
	}
	var classified *sdk.Error
	if errors.As(err, &classified) {
		row.SDKReason = string(classified.Reason)
		row.RetrySafe, row.RetryHasAfter = classified.Retry.Safe, classified.Retry.HasAfter
		if classified.Retry.HasAfter {
			row.RetryAfterUnix = classified.Retry.After.Unix()
		}
	}
	return row
}

type migrationFanboxLeaseOpener struct {
	open func(context.Context, *string) (*fanboxsdk.Client, error)
}

func (o migrationFanboxLeaseOpener) OpenClientWithProxy(ctx context.Context, proxy *string) (*fanboxsdk.Client, error) {
	return o.open(ctx, proxy)
}

type migrationFanboxLeaseTransport struct {
	requests atomic.Int32
	idle     atomic.Int32
}

func (transport *migrationFanboxLeaseTransport) RoundTrip(*http.Request) (*http.Response, error) {
	transport.requests.Add(1)
	return nil, errors.New("synthetic lease transport rejects all network requests")
}

func (transport *migrationFanboxLeaseTransport) CloseIdleConnections() {
	transport.idle.Add(1)
}

func migrationFanboxLeaseClient(t *testing.T) (*fanboxsdk.Client, *migrationFanboxLeaseTransport) {
	t.Helper()
	transport := &migrationFanboxLeaseTransport{}
	client, err := fanboxsdk.OpenWith(fanboxsdk.SessionCredentials{FANBOXSESSID: "synthetic-owned-lease-session"}, fanboxsdk.Options{
		HTTPClient: &http.Client{Transport: transport},
	})
	if err != nil {
		t.Fatalf("construct synthetic injected SDK client: %v", err)
	}
	return client, transport
}

type migrationFanboxLeaseContextKey struct{}

func migrationFanboxLeaseContextError(ctx context.Context) string {
	if ctx == nil || ctx.Err() == nil {
		return ""
	}
	return ctx.Err().Error()
}

func migrationFanboxLeaseRateError(operation string) error {
	return sdk.NewError("fanbox", operation, sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{
		Safe: true, HasAfter: true, After: time.Unix(1120, 0),
	}))
}

func migrationFanboxLeaseCall(call func() error) (err error, panicValue string, returned bool) {
	defer func() {
		if value := recover(); value != nil {
			panicValue = fmt.Sprint(value)
		}
	}()
	err = call()
	returned = true
	return
}

func migrationFanboxLeaseRun(t *testing.T, operation, mode string) migrationFanboxLeaseCase {
	t.Helper()
	client, transport := migrationFanboxLeaseClient(t)
	refs := migrationFanboxLeaseErrors{
		open: errors.New("synthetic open failure"), use: errors.New("synthetic use failure"), close: errors.New("synthetic close failure"),
	}
	var openError, useError, closeError error
	partial, noClient, openPanic, usePanic, closePanic := false, false, false, false, false
	commit, cancelInOpen, cancelInUse := false, false, false
	var proxy *string
	proxyValue := "http://127.0.0.1:7890"
	defaultCloser := false
	missingFacade, missingAccounts, nilUse := false, false, false

	switch mode {
	case "missing_facade", "nil_context_missing_facade", "nil_callback_missing_facade":
		missingFacade = true
	case "missing_accounts":
		missingAccounts = true
	case "nil_client_success":
		noClient = true
	case "open_error":
		noClient, openError = true, refs.open
	case "partial_open_error", "default_partial_open_error", "default_nil_closer_partial_open_error", "literal_nil_closer_partial_open_error":
		partial, openError = true, refs.open
	case "partial_open_close_error":
		partial, openError, closeError = true, refs.open, refs.close
	case "partial_open_close_canceled":
		partial, openError, closeError = true, refs.open, context.Canceled
	case "partial_open_close_deadline":
		partial, openError, closeError = true, refs.open, context.DeadlineExceeded
	case "partial_open_close_rate_error":
		partial, openError, closeError = true, refs.open, migrationFanboxLeaseRateError("Close")
	case "partial_open_close_panic":
		partial, openError, closePanic = true, refs.open, true
	case "open_panic":
		openPanic = true
	case "use_error", "cancel_parent_use_error", "committed_use_error":
		useError = refs.use
	case "close_error", "repeated_close_error", "concurrent_close_error", "use_panic_close_error":
		closeError = refs.close
	case "use_close_error":
		useError, closeError = refs.use, refs.close
	case "use_canceled", "precanceled_use_canceled":
		useError = context.Canceled
	case "use_deadline":
		useError = context.DeadlineExceeded
	case "close_canceled":
		closeError = context.Canceled
	case "close_deadline":
		closeError = context.DeadlineExceeded
	case "use_error_close_canceled":
		useError, closeError = refs.use, context.Canceled
	case "use_rate_error", "committed_use_rate_error":
		useError = migrationFanboxLeaseRateError("Use")
	case "close_rate_error", "committed_close_rate_error":
		closeError = migrationFanboxLeaseRateError("Close")
	case "open_rate_error":
		noClient, openError = true, migrationFanboxLeaseRateError("Open")
	case "partial_open_rate_error":
		partial, openError = true, migrationFanboxLeaseRateError("Open")
	case "use_panic", "use_panic_close_panic":
		usePanic = true
	case "close_panic", "repeated_close_panic", "concurrent_close_panic":
		closePanic = true
	case "proxy_empty":
		proxyValue = ""
		proxy = &proxyValue
	case "proxy_value", "proxy_mutation":
		proxy = &proxyValue
	}
	switch mode {
	case "default_close", "default_partial_open_error", "default_nil_closer", "default_nil_closer_partial_open_error", "literal_nil_closer", "literal_nil_closer_partial_open_error":
		defaultCloser = true
	case "nil_callback", "nil_context_nil_callback", "nil_callback_missing_facade":
		nilUse = true
	case "committed_success", "committed_use_error", "committed_use_rate_error", "committed_close_rate_error", "committed_use_panic":
		commit = true
	case "cancel_parent_in_open", "cancel_parent_in_open_error":
		cancelInOpen = true
	case "cancel_parent_success", "cancel_parent_use_error", "cancel_parent_canceled":
		cancelInUse = true
	}
	if mode == "cancel_parent_in_open_error" {
		noClient, openError = true, refs.open
	}
	if mode == "use_panic_close_error" || mode == "committed_use_panic" {
		usePanic = true
	}
	if mode == "use_panic_close_panic" {
		closePanic = true
	}

	deadline := time.Unix(4102444800, 0)
	if mode == "expired_deadline_success" || mode == "expired_deadline_error" {
		deadline = time.Unix(946684800, 0)
	}
	parent, cancelParent := context.WithCancel(context.WithValue(context.Background(), migrationFanboxLeaseContextKey{}, "synthetic-context-value"))
	defer cancelParent()
	parent, cancelDeadline := context.WithDeadline(parent, deadline)
	defer cancelDeadline()
	if mode == "precanceled_success" || mode == "precanceled_use_canceled" {
		cancelParent()
	}
	if mode == "expired_deadline_error" {
		useError = context.DeadlineExceeded
	}
	if mode == "nil_context" || mode == "nil_context_missing_facade" || mode == "nil_context_nil_callback" {
		parent = nil
	}
	parentBefore := migrationFanboxLeaseContextError(parent)
	var openCtx, useCtx context.Context
	var openBefore, useBefore, closeBefore string
	var seenProxy *string
	var openCalls, useCalls int
	var closeCalls atomic.Int32
	var closeSame atomic.Bool
	var attempt *lifecycle.Attempt
	attemptBefore, attemptAfter := false, false
	events := []string{}

	opener := migrationFanboxLeaseOpener{open: func(ctx context.Context, receivedProxy *string) (*fanboxsdk.Client, error) {
		openCalls++
		events = append(events, "open")
		openCtx, seenProxy = ctx, receivedProxy
		openBefore = migrationFanboxLeaseContextError(ctx)
		if cancelInOpen {
			cancelParent()
		}
		if mode == "proxy_mutation" && receivedProxy != nil {
			*receivedProxy = "http://127.0.0.1:7900"
		}
		if openPanic {
			panic("synthetic open panic")
		}
		if noClient {
			return nil, openError
		}
		return client, openError
	}}
	closer := func(received *fanboxsdk.Client) error {
		closeCalls.Add(1)
		closeSame.Store(received == client)
		events = append(events, "close")
		closeBefore = migrationFanboxLeaseContextError(openCtx)
		if closePanic {
			panic("synthetic close panic")
		}
		return closeError
	}
	facade := NewFacadeWithCloseClient(opener, closer)
	if defaultCloser {
		switch mode {
		case "default_close", "default_partial_open_error":
			facade = NewFacade(opener)
		case "literal_nil_closer", "literal_nil_closer_partial_open_error":
			facade = &Facade{accounts: opener}
		default:
			facade = NewFacadeWithCloseClient(opener, nil)
		}
	}
	if missingAccounts {
		facade = NewFacadeWithCloseClient(nil, closer)
	}
	if missingFacade {
		facade = nil
	}
	callback := func(ctx context.Context, received *fanboxsdk.Client, receivedAttempt *lifecycle.Attempt) error {
		useCalls++
		events = append(events, "use")
		if received != client {
			t.Fatal("Use changed the SDK client identity")
		}
		useCtx, attempt = ctx, receivedAttempt
		useBefore, attemptBefore = migrationFanboxLeaseContextError(ctx), attempt.Committed()
		if cancelInUse {
			cancelParent()
		}
		if commit {
			attempt.Commit()
			attempt.Commit()
		}
		attemptAfter = attempt.Committed()
		if usePanic {
			panic("synthetic use panic")
		}
		if mode == "cancel_parent_canceled" {
			return ctx.Err()
		}
		return useError
	}
	if nilUse {
		callback = nil
	}

	var lease *lifecycle.Lease[*fanboxsdk.Client]
	var result error
	var panicValue string
	var returned bool
	closeResults := []migrationFanboxLeaseError{}
	closePanics := []string{}
	closeResultIdentical := true
	if operation == "use" {
		result, panicValue, returned = migrationFanboxLeaseCall(func() error {
			return facade.Use(parent, OpenRequest{ProxyOverride: proxy}, callback)
		})
	} else {
		result, panicValue, returned = migrationFanboxLeaseCall(func() error {
			var err error
			lease, err = facade.Open(parent, OpenRequest{ProxyOverride: proxy})
			return err
		})
		if lease != nil {
			count := 1
			if operation == "lease" {
				count = 3
			}
			concurrent := mode == "concurrent_close_success" || mode == "concurrent_close_error" || mode == "concurrent_close_panic"
			if concurrent {
				count = 8
			}
			results := make([]error, count)
			panics := make([]string, count)
			closeOne := func(index int) {
				results[index], panics[index], _ = migrationFanboxLeaseCall(lease.Close)
			}
			if concurrent {
				var group sync.WaitGroup
				start := make(chan struct{})
				for index := range count {
					group.Add(1)
					go func() {
						defer group.Done()
						<-start
						closeOne(index)
					}()
				}
				close(start)
				group.Wait()
			} else {
				for index := range count {
					closeOne(index)
				}
			}
			for index, err := range results {
				closeResults = append(closeResults, refs.observe(err))
				if err != results[0] {
					closeResultIdentical = false
				}
				if panics[index] != "" {
					closePanics = append(closePanics, panics[index])
				}
			}
		}
	}
	proxyReceived := ""
	if seenProxy != nil {
		proxyReceived = *seenProxy
	}
	contextValue, deadlinePreserved := false, false
	if openCtx != nil {
		contextValue = openCtx.Value(migrationFanboxLeaseContextKey{}) == "synthetic-context-value"
		receivedDeadline, hasDeadline := openCtx.Deadline()
		deadlinePreserved = hasDeadline && receivedDeadline.Equal(deadline)
	}
	observation := map[string]any{
		"returned": returned, "error": refs.observe(result), "panic": panicValue,
		"open_calls": openCalls, "use_calls": useCalls, "close_calls": closeCalls.Load(),
		"idle_close_calls": transport.idle.Load(), "request_calls": transport.requests.Load(),
		"close_client_same": closeSame.Load(), "events": events,
		"proxy": map[string]any{"requested_nil": proxy == nil, "received_nil": seenProxy == nil, "same_pointer": seenProxy == proxy, "received_value": proxyReceived},
		"context": map[string]any{
			"open_seen": openCtx != nil, "use_seen": useCtx != nil, "open_same_parent": openCtx != nil && openCtx == parent,
			"use_same_open": useCtx != nil && useCtx == openCtx, "value_preserved": contextValue, "deadline_preserved": deadlinePreserved,
			"parent_before": parentBefore, "open_before": openBefore, "use_before": useBefore, "close_before": closeBefore,
			"parent_after": migrationFanboxLeaseContextError(parent), "open_after": migrationFanboxLeaseContextError(openCtx), "use_after": migrationFanboxLeaseContextError(useCtx),
		},
		"attempt": map[string]any{"present": attempt != nil, "before": attemptBefore, "after": attemptAfter, "retained_committed": attempt.Committed()},
	}
	if operation != "use" {
		observation["lease_present"] = lease != nil
		observation["lease_value_same"] = lease != nil && lease.Value() == client
		observation["close_results"] = closeResults
		observation["close_panics"] = closePanics
		observation["close_results_identical"] = closeResultIdentical
	}
	if transport.requests.Load() != 0 {
		t.Fatal("owned-lease test attempted a FANBOX request")
	}
	if closeCalls.Load() > 1 || transport.idle.Load() > 1 {
		t.Fatalf("release happened more than once: custom=%d SDK-idle=%d", closeCalls.Load(), transport.idle.Load())
	}
	if operation == "use" && openCtx != nil && !errors.Is(openCtx.Err(), context.Canceled) && !errors.Is(openCtx.Err(), context.DeadlineExceeded) {
		t.Fatal("Use did not cancel its child context after returning or panicking")
	}
	if openCalls > 1 || useCalls > 1 {
		t.Fatalf("Facade replayed a single attempt: open=%d use=%d", openCalls, useCalls)
	}
	if defaultCloser && !noClient && !missingFacade && !missingAccounts && parent != nil && !nilUse && !openPanic && (partial || result == nil) && transport.idle.Load() != 1 {
		t.Fatal("default Facade closer did not reach the injected SDK transport")
	}
	return migrationFanboxLeaseCase{ID: operation + "/" + mode, Operation: operation, Observation: observation}
}

func migrationFanboxLeaseCanonicalJSON(t *testing.T, data []byte) []byte {
	t.Helper()
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var value any
	if err := decoder.Decode(&value); err != nil {
		t.Fatalf("decode contract JSON: %v", err)
	}
	var trailing any
	if err := decoder.Decode(&trailing); !errors.Is(err, io.EOF) {
		t.Fatalf("contract JSON contains trailing data: %v", err)
	}
	canonical, err := json.Marshal(value)
	if err != nil {
		t.Fatalf("encode canonical contract JSON: %v", err)
	}
	return canonical
}

func TestMigrationFanboxSavedAccountOwnedLeases(t *testing.T) {
	root := filepath.Join("..", "..", "..")
	for path, want := range migrationFanboxLeaseSourceHashes {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(path)))
		if err != nil {
			t.Fatalf("read frozen source %s: %v", path, err)
		}
		sum := sha256.Sum256(data)
		if got := hex.EncodeToString(sum[:]); got != want {
			t.Fatalf("source %s differs from frozen Go %s: got %s want %s", path, migrationFanboxLeaseSourceCommit, got, want)
		}
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("frozen FANBOX lease contracts require go1.27.1; got %s", runtime.Version())
	}
	section := migrationFanboxLeaseSection{
		SourceCommit: migrationFanboxLeaseSourceCommit, SourceSHA256: migrationFanboxLeaseSourceHashes, GoVersion: runtime.Version(),
		Cases: []migrationFanboxLeaseCase{},
		GoOnly: []string{
			"Nil context, facade, account opener, callback, and raw nil SDK client are Go API boundary cases; Rust must record its explicit type-level correspondence.",
			"Pointer identity, error object identity, Go concrete error types, and panic values are retained as Go-only observations instead of being silently normalized away.",
			"A closer panic consumes sync.Once; later closes return nil because the closer did not finish assigning its error. Concurrent panic records one panic without assigning it to a scheduling-dependent goroutine.",
			"Attempt.Commit is idempotent; nil Attempt.Commit and nil Attempt.Committed are safe. FANBOX Facade does not replay any uncommitted or committed rate-limited attempt.",
		},
		Limitations: []string{
			"The real public SDK client uses an explicitly injected synthetic HTTP transport; construction and release perform no external I/O.",
			"Actual injected transport CloseIdleConnections is observed even though SDK Options documentation says the caller-owned HTTP client is never closed. Frozen Go behavior is retained without treating the documentation as the result.",
			"Idle-close callback invocation does not establish physical native TCP/TLS/HTTP2 cleanup, active-request shutdown, or a native platform transport contract.",
			"Saved account selection, credential verification, database state, and connection option loading belong to separate fixture sections; this section fixes the Facade AccountOpener ownership boundary.",
			"No live FANBOX account, browser, user home, stored credential, TLS peer, or supplemental HTTP2 multiplex/upload probe is used.",
		},
	}
	groups := []struct {
		operation string
		modes     []string
	}{
		{"open", []string{
			"nil_context", "nil_context_missing_facade", "missing_facade", "missing_accounts", "nil_client_success", "open_error",
			"partial_open_error", "partial_open_close_error", "partial_open_close_canceled", "partial_open_close_deadline", "partial_open_close_rate_error", "partial_open_close_panic", "open_panic",
			"success", "proxy_empty", "proxy_value", "proxy_mutation", "precanceled_success", "expired_deadline_success", "cancel_parent_in_open", "cancel_parent_in_open_error",
			"default_close", "default_nil_closer", "literal_nil_closer", "default_partial_open_error", "default_nil_closer_partial_open_error", "literal_nil_closer_partial_open_error",
		}},
		{"lease", []string{"repeated_close_success", "repeated_close_error", "repeated_close_panic", "concurrent_close_success", "concurrent_close_error", "concurrent_close_panic", "default_close"}},
		{"use", []string{
			"nil_context", "nil_context_missing_facade", "nil_context_nil_callback", "nil_callback", "nil_callback_missing_facade", "missing_facade", "missing_accounts", "nil_client_success",
			"open_error", "partial_open_error", "partial_open_close_error", "partial_open_close_panic", "open_panic",
			"success", "use_error", "close_error", "use_close_error", "use_canceled", "use_deadline", "close_canceled", "close_deadline", "use_error_close_canceled",
			"precanceled_success", "precanceled_use_canceled", "expired_deadline_success", "expired_deadline_error", "cancel_parent_success", "cancel_parent_use_error", "cancel_parent_canceled", "cancel_parent_in_open", "cancel_parent_in_open_error",
			"committed_success", "committed_use_error", "use_rate_error", "committed_use_rate_error", "close_rate_error", "committed_close_rate_error", "open_rate_error", "partial_open_rate_error",
			"use_panic", "committed_use_panic", "use_panic_close_error", "close_panic", "use_panic_close_panic", "proxy_empty", "proxy_value", "proxy_mutation", "default_close", "default_partial_open_error",
		}},
	}
	for _, group := range groups {
		for _, mode := range group.modes {
			t.Run(group.operation+"/"+mode, func(t *testing.T) {
				section.Cases = append(section.Cases, migrationFanboxLeaseRun(t, group.operation, mode))
			})
		}
	}
	var nilAttempt *lifecycle.Attempt
	nilAttempt.Commit()
	section.Cases = append(section.Cases, migrationFanboxLeaseCase{
		ID: "attempt/nil", Operation: "attempt", Observation: map[string]any{"nil_commit_returned": true, "nil_committed": nilAttempt.Committed()},
	})
	data, err := json.MarshalIndent(section, "", "  ")
	if err != nil {
		t.Fatalf("encode owned lease section: %v", err)
	}
	data = append(data, '\n')
	var expected []byte
	if *migrationCaptureFanboxSavedLeases {
		path := "/tmp/fanbox-saved-accounts-lease.json"
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatalf("capture owned lease section: %v", err)
		}
		expected, err = os.ReadFile(path)
		if err == nil && !bytes.Equal(expected, data) {
			t.Fatal("captured owned lease bytes differ immediately after writing")
		}
	} else {
		path := filepath.Join(root, "crates", "pixiv-app", "tests", "fixtures", "fanbox-saved-accounts.json")
		var envelope map[string]json.RawMessage
		var fixture []byte
		fixture, err = os.ReadFile(path)
		if err == nil {
			err = json.Unmarshal(fixture, &envelope)
		}
		if err == nil {
			var ok bool
			expected, ok = envelope["leases"]
			if !ok {
				t.Fatal("saved-account fixture has no leases section")
			}
		}
	}
	if err != nil {
		t.Fatalf("read owned lease section: %v", err)
	}
	if !bytes.Equal(migrationFanboxLeaseCanonicalJSON(t, expected), migrationFanboxLeaseCanonicalJSON(t, data)) {
		t.Fatal("FANBOX owned lease contracts differ from the frozen saved-account fixture")
	}
}

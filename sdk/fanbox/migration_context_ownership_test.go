package fanbox_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxContext = flag.Bool("migration-capture-fanbox-context-ownership", false, "capture fixed Go standard context and FANBOX diagnostic ownership")

type migrationContextSource struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type migrationContextRow struct {
	Name        string         `json:"name"`
	Input       map[string]any `json:"input"`
	Observation map[string]any `json:"observation"`
}

type migrationContextFixture struct {
	Reference       string                   `json:"reference"`
	GoVersion       string                   `json:"go_version"`
	Sources         []migrationContextSource `json:"sources"`
	StandardSources []migrationContextSource `json:"standard_sources"`
	Evidence        string                   `json:"evidence"`
	Limitations     []string                 `json:"limitations"`
	Cases           []migrationContextRow    `json:"cases"`
}

type migrationOwnershipKey string
type migrationOwnershipOtherKey string
type migrationOwnershipStructKey struct {
	Number int
	Label  string
}
type migrationOwnershipPointerKey struct{ Number int }

const migrationOwnershipValueKey migrationOwnershipKey = "owned-value"

func migrationContextError(err error) map[string]any {
	result := map[string]any{"message": "", "reason": sdk.ReasonOf(err), "canceled": errors.Is(err, context.Canceled), "deadline_exceeded": errors.Is(err, context.DeadlineExceeded), "go_only_error_tree": []map[string]string{}}
	var tree []map[string]string
	var walk func(error)
	walk = func(current error) {
		if current == nil {
			return
		}
		tree = append(tree, map[string]string{"type": fmt.Sprintf("%T", current), "message": current.Error()})
		if joined, ok := current.(interface{ Unwrap() []error }); ok {
			for _, child := range joined.Unwrap() {
				walk(child)
			}
		} else {
			walk(errors.Unwrap(current))
		}
	}
	if err != nil {
		result["message"] = err.Error()
		walk(err)
		result["go_only_error_tree"] = tree
	}
	var classified *sdk.Error
	if errors.As(err, &classified) {
		result["sdk"] = map[string]any{"product": classified.Product, "operation": classified.Operation, "reason": classified.Reason, "detail": classified.Detail, "http_status": classified.HTTPStatus, "transport": classified.Transport, "retry_safe": classified.Retry.Safe, "retry_has_after": classified.Retry.HasAfter, "go_only_reason_sentinel_matches": errors.Is(err, sdk.NewError("ignored", "ignored", classified.Reason))}
	}
	return result
}

func migrationContextState(ctx context.Context) map[string]any {
	if ctx == nil {
		return map[string]any{"nil_context": true}
	}
	deadline, hasDeadline := ctx.Deadline()
	done := ctx.Done()
	closed := false
	if done != nil {
		select {
		case <-done:
			closed = true
		default:
		}
	}
	_, scope := diagnostics.ScopeFromContext(ctx)
	result := map[string]any{"nil_context": false, "done_nil": done == nil, "done_closed": closed, "error": migrationContextError(ctx.Err()), "cause": migrationContextError(context.Cause(ctx)), "has_deadline": hasDeadline, "scope_present": scope, "value": ctx.Value(migrationOwnershipValueKey), "go_only_context_type": fmt.Sprintf("%T", ctx)}
	if hasDeadline {
		result["go_only_deadline_unix_nano"] = deadline.UnixNano()
		result["go_only_deadline_rfc3339_nano"] = deadline.Format(time.RFC3339Nano)
	}
	return result
}

func migrationContextAwait(t *testing.T, done <-chan struct{}, name string) {
	t.Helper()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatalf("%s did not finish within owned bound", name)
	}
}

func migrationContextPanic(call func()) (result string) {
	defer func() {
		if value := recover(); value != nil {
			result = fmt.Sprint(value)
		}
	}()
	call()
	return ""
}

func migrationContextTypedEvent() diagnostics.Event {
	return diagnostics.Event{Kind: diagnostics.EventAccount, Operation: "owned operation", Resource: "user 42", Route: "owned-route", Target: "owned.file", Proxy: "synthetic proxy", UserAgent: "owned-agent", Reason: diagnostics.ReasonAccountFrozen, Status: 429, Count: 3, RequestID: 999, Duration: 123456789}
}

func migrationContextStandardRows(t *testing.T) []migrationContextRow {
	t.Helper()
	var rows []migrationContextRow
	add := func(name string, input, result map[string]any) {
		rows = append(rows, migrationContextRow{name, input, result})
	}
	for _, root := range []struct {
		name string
		ctx  context.Context
	}{{"background", context.Background()}, {"todo", context.TODO()}} {
		add("standard/"+root.name, map[string]any{"operation": root.name}, map[string]any{"state": migrationContextState(root.ctx), "go_only_string": fmt.Sprint(root.ctx), "go_only_repeated_root_interface_equal": root.ctx == map[string]context.Context{"background": context.Background(), "todo": context.TODO()}[root.name]})
	}
	base := context.WithValue(context.Background(), migrationOwnershipValueKey, "owned-parent")
	shadow := context.WithValue(base, migrationOwnershipValueKey, "owned-child")
	other := context.WithValue(shadow, migrationOwnershipOtherKey("owned-value"), "owned-distinct-key-type")
	structured := context.WithValue(other, migrationOwnershipStructKey{7, "owned"}, "owned-struct-value")
	pointer := &migrationOwnershipPointerKey{7}
	pointerOther := &migrationOwnershipPointerKey{7}
	payload := &migrationOwnershipPointerKey{13}
	valued := context.WithValue(structured, pointer, payload)
	nilValue := context.WithValue(valued, migrationOwnershipValueKey, nil)
	add("standard/typed_values_and_shadowing", map[string]any{"operation": "WithValue", "go_only_comparable_keys": []string{"defined string type", "distinct defined string type", "struct with equal comparable fields", "distinct nonzero-sized pointers"}, "values": []any{"owned-parent", "owned-child", "owned-distinct-key-type", "owned-struct-value", map[string]int{"Number": 13}, nil}}, map[string]any{"parent_state": migrationContextState(base), "child_state": migrationContextState(valued), "nil_shadow_state": migrationContextState(nilValue), "distinct_key_value": valued.Value(migrationOwnershipOtherKey("owned-value")), "equal_struct_key_value": valued.Value(migrationOwnershipStructKey{7, "owned"}), "go_only_pointer_key_same_value_identity": valued.Value(pointer) == payload, "distinct_pointer_key_value": valued.Value(pointerOther), "go_only_child_interface_differs": valued != base, "parent_unaffected_value": base.Value(migrationOwnershipValueKey)})
	for _, sample := range []struct {
		name string
		call func()
	}{
		{"WithValue_nil_parent", func() { _ = context.WithValue(nil, "k", "v") }},
		{"WithValue_nil_key", func() { _ = context.WithValue(base, nil, "v") }},
		{"WithValue_uncomparable_key", func() { _ = context.WithValue(base, []int{1}, "v") }},
		{"WithCancel_nil_parent", func() { _, cancel := context.WithCancel(nil); cancel() }},
		{"WithDeadline_nil_parent", func() { _, cancel := context.WithDeadline(nil, time.Unix(1, 0)); cancel() }},
		{"WithoutCancel_nil_parent", func() { _ = context.WithoutCancel(nil) }},
	} {
		add("standard/"+sample.name, map[string]any{"operation": sample.name}, map[string]any{"go_only_panic": migrationContextPanic(sample.call)})
	}
	for _, order := range []string{"child_first", "parent_first"} {
		parent, cancelParent := context.WithCancel(base)
		child, cancelChild := context.WithCancel(parent)
		sibling, cancelSibling := context.WithCancel(parent)
		before := map[string]any{"parent": migrationContextState(parent), "child": migrationContextState(child), "sibling": migrationContextState(sibling), "go_only_child_done_distinct": child.Done() != parent.Done()}
		if order == "child_first" {
			cancelChild()
		} else {
			cancelParent()
		}
		afterFirst := map[string]any{"parent": migrationContextState(parent), "child": migrationContextState(child), "sibling": migrationContextState(sibling)}
		cancelChild()
		cancelParent()
		cancelParent()
		cancelChild()
		migrationContextAwait(t, sibling.Done(), "sibling cancellation")
		afterBoth := map[string]any{"parent": migrationContextState(parent), "child": migrationContextState(child), "sibling": migrationContextState(sibling)}
		cancelSibling()
		add("standard/cancellation/"+order, map[string]any{"operation": "WithCancel", "order": order, "repeat_cancel": true}, map[string]any{"before": before, "after_first": afterFirst, "after_both": afterBoth})
	}
	past := time.Unix(1, 123456789).UTC()
	future := time.Date(2099, 1, 1, 0, 0, 0, 123456789, time.UTC)
	later := future.Add(time.Hour)
	for _, scenario := range []string{"expired_parent_later_child", "expired_child_live_parent", "earlier_parent_later_child", "later_parent_earlier_child", "cancel_before_future_deadline", "cancel_before_expired_child"} {
		parent, cancelParent := context.WithCancel(base)
		var deadlineParent context.Context = parent
		var stopParent context.CancelFunc = func() {}
		parentDeadline, childDeadline := "none", past
		switch scenario {
		case "expired_parent_later_child":
			deadlineParent, stopParent = context.WithDeadline(parent, past)
			parentDeadline = past.Format(time.RFC3339Nano)
			childDeadline = future
		case "earlier_parent_later_child":
			deadlineParent, stopParent = context.WithDeadline(parent, future)
			parentDeadline = future.Format(time.RFC3339Nano)
			childDeadline = later
		case "later_parent_earlier_child":
			deadlineParent, stopParent = context.WithDeadline(parent, later)
			parentDeadline = later.Format(time.RFC3339Nano)
			childDeadline = future
		case "cancel_before_future_deadline":
			childDeadline = future
		case "cancel_before_expired_child":
			cancelParent()
		}
		child, stopChild := context.WithDeadline(deadlineParent, childDeadline)
		before := map[string]any{"parent": migrationContextState(deadlineParent), "child": migrationContextState(child)}
		stopChild()
		cancelParent()
		stopParent()
		add("standard/deadline/"+scenario, map[string]any{"operation": "WithDeadline", "parent_deadline": parentDeadline, "child_requested_deadline": childDeadline.Format(time.RFC3339Nano), "scenario": scenario}, map[string]any{"before_cleanup": before, "after_cleanup": map[string]any{"parent": migrationContextState(deadlineParent), "child": migrationContextState(child)}})
	}
	origin := time.Now()
	interval := 10 * time.Millisecond
	timerDeadline := origin.Add(interval)
	timer, stopTimer := context.WithDeadline(base, timerDeadline)
	migrationContextAwait(t, timer.Done(), "real deadline timer")
	actualDeadline, hasDeadline := timer.Deadline()
	stopTimer()
	add("standard/deadline/real_timer_channel", map[string]any{"operation": "WithDeadline", "deadline_expression": "construction_origin + interval_ns", "interval_ns": int64(interval), "observation_phase": "after actual Done channel closes", "go_only_absolute_runtime_timestamp": "intentionally not compared; explicit relative construction expression is compared exactly"}, map[string]any{"done_nil": timer.Done() == nil, "done_closed": func() bool {
		select {
		case <-timer.Done():
			return true
		default:
			return false
		}
	}(), "has_deadline": hasDeadline, "go_only_deadline_minus_construction_origin_ns": actualDeadline.Sub(origin).Nanoseconds(), "error": migrationContextError(timer.Err()), "cause": migrationContextError(context.Cause(timer)), "value": timer.Value(migrationOwnershipValueKey)})
	return rows
}

func migrationContextDiagnosticRows(t *testing.T) []migrationContextRow {
	t.Helper()
	var rows []migrationContextRow
	for _, mode := range []string{"absent", "nil_context_absent", "nil_context_scoped", "nil_sink", "nil_sink_func", "zero_scope_direct", "parent_child_detached", "parent_child_detached_expired"} {
		events := []diagnostics.Event{}
		sink := diagnostics.SinkFunc(func(event diagnostics.Event) { events = append(events, event) })
		var ctx context.Context = context.WithValue(context.Background(), migrationOwnershipValueKey, "owned-parent")
		if mode == "nil_context_absent" || mode == "nil_context_scoped" {
			ctx = nil
		}
		if mode != "absent" && mode != "nil_context_absent" && mode != "zero_scope_direct" {
			var selected diagnostics.Sink = sink
			if mode == "nil_sink" {
				selected = nil
			}
			if mode == "nil_sink_func" {
				var fn diagnostics.SinkFunc
				selected = fn
			}
			ctx = diagnostics.WithScope(ctx, selected, diagnostics.ModuleFanboxMCP, 41)
		}
		before := migrationContextState(ctx)
		child := diagnostics.WithChildScope(ctx, diagnostics.ModuleFanboxSolver, 42)
		initial := migrationContextTypedEvent()
		diagnostics.Emit(ctx, initial)
		diagnostics.Emit(child, initial)
		explicit := initial
		explicit.Module = diagnostics.ModuleFanboxNetwork
		diagnostics.Emit(child, explicit)
		scope, present := diagnostics.ScopeFromContext(child)
		scope.Emit(initial)
		result := map[string]any{"before": before, "child": migrationContextState(child), "scope_lookup_present": present, "go_only_absent_child_same_interface": ctx == child, "input_event_after_emit": initial}
		if strings.HasPrefix(mode, "parent_child_detached") {
			deadline := time.Date(2099, 1, 1, 0, 0, 0, 123456789, time.UTC)
			if mode == "parent_child_detached_expired" {
				deadline = time.Unix(1, 123456789).UTC()
			}
			parent, cancelParent := context.WithDeadline(child, deadline)
			detached := context.WithoutCancel(parent)
			independent, cancelIndependent := context.WithCancel(detached)
			cancelParent()
			result["after_parent_cancel"] = map[string]any{"parent": migrationContextState(parent), "detached": migrationContextState(detached), "independent": migrationContextState(independent)}
			diagnostics.Emit(detached, initial)
			diagnostics.Emit(independent, explicit)
			cancelIndependent()
			result["after_independent_cancel"] = map[string]any{"parent": migrationContextState(parent), "detached": migrationContextState(detached), "independent": migrationContextState(independent)}
			diagnostics.Emit(independent, initial)
		}
		result["events"] = events
		rows = append(rows, migrationContextRow{"diagnostics/" + mode, map[string]any{"operation": "typed diagnostic scope ownership", "mode": mode, "parent_module": diagnostics.ModuleFanboxMCP, "parent_request_id": 41, "child_module": diagnostics.ModuleFanboxSolver, "child_request_id": 42, "event": initial}, result})
	}
	return rows
}

type migrationContextBody struct {
	reader *strings.Reader
	trace  *[]string
	closes int
	bytes  int
}

func (body *migrationContextBody) Read(buffer []byte) (int, error) {
	count, err := body.reader.Read(buffer)
	body.bytes += count
	return count, err
}
func (body *migrationContextBody) Close() error {
	body.closes++
	*body.trace = append(*body.trace, "body.close")
	return nil
}

type migrationContextTransport func(*http.Request) (*http.Response, error)

func (transport migrationContextTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	return transport(request)
}

func migrationContextSDKRows(t *testing.T) []migrationContextRow {
	t.Helper()
	var rows []migrationContextRow
	for _, mode := range []string{"scoped_ignoring", "scoped_precanceled_ignoring", "scoped_precanceled_observing", "scoped_parent_cancel_during_observing", "scoped_expired_observing", "detached_canceled_parent_ignoring", "detached_independent_cancel_during_observing", "absent_scope_ignoring", "nil_context"} {
		events := []diagnostics.Event{}
		trace := []string{}
		parent, cancelParent := context.WithCancel(context.WithValue(context.Background(), migrationOwnershipValueKey, "owned-sdk-value"))
		defer cancelParent()
		var ctx context.Context = parent
		if mode != "absent_scope_ignoring" && mode != "nil_context" {
			ctx = diagnostics.WithScope(ctx, diagnostics.SinkFunc(func(event diagnostics.Event) { events = append(events, event) }), diagnostics.ModuleFanboxMCP, 41)
			ctx = diagnostics.WithChildScope(ctx, diagnostics.ModuleFanboxSolver, 42)
		}
		var cancelIndependent context.CancelFunc = func() {}
		if strings.Contains(mode, "detached") {
			ctx = context.WithoutCancel(ctx)
			ctx, cancelIndependent = context.WithCancel(ctx)
		}
		defer cancelIndependent()
		if strings.Contains(mode, "precanceled") || strings.Contains(mode, "detached") {
			cancelParent()
		}
		if mode == "scoped_expired_observing" {
			var stop context.CancelFunc
			ctx, stop = context.WithDeadline(ctx, time.Unix(1, 123456789).UTC())
			defer stop()
		}
		if mode == "nil_context" {
			ctx = nil
		}
		before := migrationContextState(ctx)
		requests := []map[string]any{}
		bodyText := `<meta name="metadata" content='{"context":{"user":{"userId":42,"name":"owned user"}}}'>`
		body := &migrationContextBody{reader: strings.NewReader(bodyText), trace: &trace}
		entered := make(chan struct{})
		transport := migrationContextTransport(func(request *http.Request) (*http.Response, error) {
			trace = append(trace, "transport.enter")
			requests = append(requests, map[string]any{"method": request.Method, "url": request.URL.String(), "state": migrationContextState(request.Context()), "go_only_same_context_interface": request.Context() == ctx, "go_only_same_done_channel": request.Context().Done() == ctx.Done()})
			close(entered)
			diagnostics.Emit(request.Context(), diagnostics.Event{Kind: diagnostics.EventStarted, Operation: "owned transport entered"})
			if strings.Contains(mode, "observing") {
				migrationContextAwait(t, request.Context().Done(), "owned transport cancellation")
				trace = append(trace, "transport.context_done")
				return nil, request.Context().Err()
			}
			trace = append(trace, "transport.response")
			return &http.Response{StatusCode: 200, Header: make(http.Header), Body: body, Request: request, ContentLength: int64(len(bodyText))}, nil
		})
		client, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: "synthetic-owned-context-session"}, fanbox.Options{HTTPClient: &http.Client{Transport: transport}, UserAgent: "owned-context-agent"})
		if err != nil {
			t.Fatal(err)
		}
		var user fanbox.User
		if strings.Contains(mode, "during") {
			finished := make(chan struct{})
			go func() { defer close(finished); user, err = client.CurrentUser(ctx, fanbox.CurrentUserRequest{}) }()
			migrationContextAwait(t, entered, "owned transport entry")
			if strings.Contains(mode, "independent") {
				cancelIndependent()
			} else {
				cancelParent()
			}
			migrationContextAwait(t, finished, "owned SDK CurrentUser")
		} else {
			user, err = client.CurrentUser(ctx, fanbox.CurrentUserRequest{})
		}
		trace = append(trace, "sdk.return")
		rows = append(rows, migrationContextRow{"sdk_current_user/" + mode, map[string]any{"operation": "fanbox.Client.CurrentUser", "mode": mode, "parent_value": "owned-sdk-value", "parent_scope_request_id": 41, "child_scope_request_id": 42, "response_status": 200, "response_body": bodyText, "user_agent": "owned-context-agent", "transport_behavior": func() string {
			if strings.Contains(mode, "observing") {
				return "wait on actual request.Context.Done then return actual request.Context.Err"
			}
			return "return owned response without inspecting cancellation"
		}()}, map[string]any{"before": before, "after": migrationContextState(ctx), "parent_after": migrationContextState(parent), "requests": requests, "events": events, "trace": trace, "dto": fanbox.ToUserDTO(user), "error": migrationContextError(err), "response_body_close_calls": body.closes, "response_bytes_read": body.bytes}})
	}
	return rows
}

func migrationContextLifecycleRows(t *testing.T) []migrationContextRow {
	t.Helper()
	var rows []migrationContextRow
	for _, mode := range []string{"success", "parent_cancel_during_use", "already_expired_parent", "use_failure_close_failure", "use_panic", "nil_context"} {
		events := []diagnostics.Event{}
		trace := []string{}
		parent, cancelParent := context.WithCancel(context.WithValue(context.Background(), migrationOwnershipValueKey, "owned-lifecycle-value"))
		defer cancelParent()
		var ctx context.Context = diagnostics.WithScope(parent, diagnostics.SinkFunc(func(event diagnostics.Event) { events = append(events, event) }), diagnostics.ModuleFanboxMCP, 71)
		if mode == "already_expired_parent" {
			var stop context.CancelFunc
			ctx, stop = context.WithDeadline(ctx, time.Unix(1, 123456789).UTC())
			defer stop()
		}
		if mode == "nil_context" {
			ctx = nil
		}
		before := migrationContextState(ctx)
		var openContext, useContext context.Context
		var openedLease *lifecycle.Lease[int]
		var closeState, useState map[string]any
		closes := 0
		var returned error
		panicText := migrationContextPanic(func() {
			returned = lifecycle.Run(ctx, func(child context.Context) (*lifecycle.Lease[int], error) {
				trace = append(trace, "open")
				openContext = child
				diagnostics.Emit(child, diagnostics.Event{Kind: diagnostics.EventStarted, Operation: "owned lifecycle open"})
				openedLease = lifecycle.NewLease(37, func() error {
					trace = append(trace, "close")
					closes++
					closeState = migrationContextState(child)
					diagnostics.Emit(child, diagnostics.Event{Kind: diagnostics.EventCompleted, Operation: "owned lifecycle close"})
					if mode == "use_failure_close_failure" {
						return errors.New("owned close failure")
					}
					return nil
				})
				return openedLease, nil
			}, func(child context.Context, value int, attempt *lifecycle.Attempt) error {
				trace = append(trace, "use")
				useContext = child
				if value != 37 {
					t.Fatalf("owned lease value = %d", value)
				}
				attempt.Commit()
				attempt.Commit()
				if mode == "parent_cancel_during_use" {
					cancelParent()
				}
				useState = migrationContextState(child)
				diagnostics.Emit(child, diagnostics.Event{Kind: diagnostics.EventAccount, Operation: "owned lifecycle use", Count: value})
				if mode == "use_panic" {
					panic("owned lifecycle panic")
				}
				if mode == "use_failure_close_failure" {
					return fmt.Errorf("owned use failure: %w", context.Canceled)
				}
				return child.Err()
			})
		})
		trace = append(trace, "run.return")
		if openContext != nil {
			migrationContextAwait(t, openContext.Done(), "lifecycle child cleanup")
		}
		repeatedClose := openedLease.Close()
		rows = append(rows, migrationContextRow{"lifecycle/" + mode, map[string]any{"operation": "lifecycle.Run", "mode": mode, "value": 37, "parent_value": "owned-lifecycle-value", "scope_request_id": 71}, map[string]any{"before": before, "parent_after": migrationContextState(ctx), "use_state": useState, "close_state": closeState, "child_after_return": migrationContextState(openContext), "go_only_open_use_same_context_interface": openContext == useContext, "go_only_child_differs_from_parent": openContext != nil && openContext != ctx, "close_calls_after_repeat": closes, "repeated_close_error": migrationContextError(repeatedClose), "error": migrationContextError(returned), "go_only_panic": panicText, "events": events, "trace": trace}})
	}
	return rows
}

func TestMigrationFanboxContextOwnershipMatchesFrozenGo(t *testing.T) {
	const reference = "4b4426487ef18bed276706daec385e0d0a6979f9"
	sources := []migrationContextSource{
		{"go.mod", "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c"},
		{"go.sum", "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e"},
		{"sdk/fanbox/dto.go", "860f6d3f18d083526681cf86112dbbdb7faa382e1f9cac3e35b543f56c2df505"},
		{"sdk/fanbox/models.go", "1dd3928aa6752c51f114c678eef3b0a2b4784a275ec80fcdbbd61006407e4623"},
		{"internal/services/fanbox/protocol/cookie.go", "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9"},
		{"sdk/fanbox/fanbox.go", "2208576144b94b89efd57b6f054ee018268812d52d556fd0758e2ee322577542"},
		{"sdk/fanbox/ops.go", "868b3b68de07d7638af5f9dd3ab1be8f0c9751f222c05c7776cde092052a7c8d"},
		{"sdk/fanbox/errors.go", "523577a066e3d53ce9eced8c7afbe29f422a75ce6aca58bc3b5b6fbf8da59909"},
		{"sdk/error.go", "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041"},
		{"internal/services/fanbox/protocol/protocol.go", "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3"},
		{"internal/services/fanbox/protocol/identity.go", "10ba481b43e3ec7d0bdaa2defbbd629c4b5bb2aa169c5bff99c8e756de8143d9"},
		{"internal/shared/diagnostics/diagnostics.go", "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76"},
		{"internal/shared/lifecycle/lifecycle.go", "2c5524fc40cc2202ba06934e01369824909ad2bd7f0f43478ceab9e344298384"},
		{"internal/shared/lifecycle/lease.go", "9715a8f3592aa0269b6f85b1a52751f2e0f2947e34fb133836fb1ef07172b381"},
		{"internal/shared/lifecycle/attempt.go", "5f329d3433b77b30a33570f8425d9742e4ac1fb09012aee2a628f6f14578d614"},
	}
	standard := []migrationContextSource{
		{"src/context/context.go", "971f00ffa375b79d3f65e6da33cca3a493cc3fce091c589233abfb4283e29b7d"},
		{"src/net/http/client.go", "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9"},
		{"src/net/http/request.go", "c3257079994b4e4f74f01cef72508919983ba31f91fcf599d348e9d52cec1540"},
	}
	root := filepath.Join("..", "..")
	for _, source := range sources {
		current, err := os.ReadFile(filepath.Join(root, source.Path))
		if err != nil {
			t.Fatal(err)
		}
		command := exec.Command("git", "show", reference+":"+source.Path)
		command.Dir = root
		frozen, err := command.Output()
		if err != nil {
			t.Fatalf("read frozen %s: %v", source.Path, err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(frozen)) != source.SHA256 || !bytes.Equal(current, frozen) {
			t.Fatalf("%s differs from frozen Go", source.Path)
		}
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("Go version = %s", runtime.Version())
	}
	for _, source := range standard {
		current, err := os.ReadFile(filepath.Join(runtime.GOROOT(), source.Path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(current)) != source.SHA256 {
			t.Fatalf("%s differs from official pinned Go source", source.Path)
		}
	}
	fixture := migrationContextFixture{Reference: reference, GoVersion: runtime.Version(), Sources: sources, StandardSources: standard,
		Evidence: "Actual standard context.Context APIs, typed opt-in diagnostics, lifecycle.Run with owned leases, and actual fanbox.Client.CurrentUser through a no-network owned standard RoundTripper. Transport cancellation returns the request context's actual Err after its actual Done channel closes. Frozen production files and official Go 1.27.1 standard source hashes are guarded before every capture/replay. Observations are compared byte-for-byte without normalization.",
		Limitations: []string{
			"Go has standard context.Context; there is no sdk/context.go API. This fixture cannot establish parity for the lost Rust SDK context bridge.",
			"Go-only arbitrary comparable key types, interface and value identity, context concrete type, exact error-tree types, panic text and fixed absolute deadline timestamps are explicitly named; no Rust representation or API identity is assumed.",
			"The real timer case compares the exact deadline relative to its construction origin, with this expression declared in its input. Absolute runtime timestamp and scheduling latency are not compared; fixed past/future cases retain exact absolute timestamps. Every wait is bounded at two seconds.",
			"Synthetic CurrentUser response parses only a control identity and ownership outcome; identity/DTO/header/redirect/options cases already published elsewhere are not re-inventoried here.",
			"Injected transports can ignore cancellation and still return success. This source outcome must not be replaced by an unconditional context precheck or cancellation race.",
			"No live account, socket, browser, native transport, native cancellation, Rust implementation/build or non-Linux runtime is exercised. Context causes beyond standard Canceled/DeadlineExceeded, custom Context implementations and unrestricted concurrent schedules are outside this bounded slice.",
		}, Cases: []migrationContextRow{}}
	fixture.Cases = append(fixture.Cases, migrationContextStandardRows(t)...)
	fixture.Cases = append(fixture.Cases, migrationContextDiagnosticRows(t)...)
	fixture.Cases = append(fixture.Cases, migrationContextSDKRows(t)...)
	fixture.Cases = append(fixture.Cases, migrationContextLifecycleRows(t)...)
	encoded, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-context-ownership.json")
	if *migrationCaptureFanboxContext {
		if err := os.WriteFile(path, encoded, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	frozen, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(encoded, frozen) {
		t.Fatal("context ownership differs from exact frozen Go observation")
	}
}

var _ io.ReadCloser = (*migrationContextBody)(nil)

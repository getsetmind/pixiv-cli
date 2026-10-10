package pixiv

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httptrace"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateIdleNativeH1 = flag.Bool("migration-update-idle-native-h1", false, "capture default-owned SDK HTTP/1 idle retirement contracts")

const migrationIdleH1Bound = 10 * time.Second
const migrationIdleH1ResourceURL = "https://i.pximg.net/idle-native-h1.fixture"

type migrationIdleH1Result struct {
	Step            string          `json:"step"`
	Operation       string          `json:"operation"`
	Success         bool            `json:"success"`
	ID              int64           `json:"id,omitempty"`
	Status          int             `json:"status,omitempty"`
	Body            string          `json:"body,omitempty"`
	Error           json.RawMessage `json:"error,omitempty"`
	ContextCanceled bool            `json:"context_canceled,omitempty"`
	RawReadError    string          `json:"raw_read_error,omitempty"`
	CloseSuccess    bool            `json:"close_success,omitempty"`
}
type migrationIdleH1Wire struct {
	Step       string `json:"step"`
	Connection int    `json:"connection"`
	Method     string `json:"method"`
	Host       string `json:"host"`
	Target     string `json:"target"`
	Protocol   string `json:"protocol"`
}
type migrationIdleH1Idle struct {
	Step     string `json:"step"`
	Observed bool   `json:"observed"`
	Accepted bool   `json:"accepted"`
	RawError string `json:"raw_error,omitempty"`
}
type migrationIdleH1Closure struct {
	Barrier    string `json:"barrier"`
	Connection int    `json:"connection"`
}
type migrationIdleH1Row struct {
	Name      string                  `json:"name"`
	Schedule  []string                `json:"schedule"`
	PublicSDK []migrationIdleH1Result `json:"public_sdk"`
	Native    struct {
		CloseIdleCalls int                      `json:"close_idle_calls"`
		Requests       []migrationIdleH1Wire    `json:"requests"`
		IdleReturns    []migrationIdleH1Idle    `json:"idle_returns"`
		PeerClosures   []migrationIdleH1Closure `json:"peer_closures_at_barriers"`
	} `json:"native_http1_observations"`
}
type migrationIdleH1Plan struct {
	name, route, phase, body string
	release                  chan struct{}
	ready                    chan struct{}
	arrived                  chan migrationIdleH1Wire
	done                     chan struct{}
	once                     sync.Once
	started                  bool
}

func (p *migrationIdleH1Plan) unblock() { p.once.Do(func() { close(p.release) }) }

type migrationIdleH1Call struct {
	name        string
	ctx         context.Context
	cancel      context.CancelFunc
	done        chan migrationIdleH1Result
	result      migrationIdleH1Result
	idle        chan migrationIdleH1Idle
	firstByte   chan struct{}
	compareIdle bool
	traceMu     sync.Mutex
	trace       migrationIdleH1Idle
}
type migrationIdleH1Raw struct {
	Name        string                `json:"name"`
	IdleReturns []migrationIdleH1Idle `json:"raw_private_idle_returns"`
}
type migrationIdleH1PeerKey struct{}
type migrationIdleH1Harness struct {
	t           *testing.T
	client      *Client
	server      *httptest.Server
	base        http.RoundTripper
	mu          sync.Mutex
	plans       map[string][]*migrationIdleH1Plan
	allPlans    []*migrationIdleH1Plan
	calls       []*migrationIdleH1Call
	conns       map[net.Conn]int
	closeEvents map[int]chan struct{}
	requests    []migrationIdleH1Wire
	peerErrors  chan error
	rawIdle     []migrationIdleH1Idle
	wg          sync.WaitGroup
	row         migrationIdleH1Row
}

func migrationIdleH1Await[T any](t *testing.T, ch <-chan T, what string) T {
	t.Helper()
	timer := time.NewTimer(migrationIdleH1Bound)
	defer timer.Stop()
	select {
	case value := <-ch:
		return value
	case <-timer.C:
		t.Fatalf("timed out waiting for %s", what)
		var zero T
		return zero
	}
}
func migrationIdleH1Signal(t *testing.T, ch <-chan struct{}, what string) {
	t.Helper()
	migrationIdleH1Await(t, ch, what)
}

func migrationIdleH1New(t *testing.T, name string, schedule []string) *migrationIdleH1Harness {
	t.Helper()
	h := &migrationIdleH1Harness{t: t, plans: map[string][]*migrationIdleH1Plan{}, conns: map[net.Conn]int{}, closeEvents: map[int]chan struct{}{}, peerErrors: make(chan error, 16), row: migrationIdleH1Row{Name: name, Schedule: schedule}}
	h.row.Native.PeerClosures = []migrationIdleH1Closure{}
	h.server = httptest.NewUnstartedServer(http.HandlerFunc(h.serve))
	h.server.EnableHTTP2 = false
	h.server.Config.ConnContext = func(ctx context.Context, c net.Conn) context.Context {
		h.mu.Lock()
		id := len(h.conns) + 1
		h.conns[c] = id
		h.closeEvents[id] = make(chan struct{})
		h.mu.Unlock()
		return context.WithValue(ctx, migrationIdleH1PeerKey{}, id)
	}
	h.server.Config.ConnState = func(c net.Conn, state http.ConnState) {
		if state == http.StateClosed {
			h.mu.Lock()
			ch := h.closeEvents[h.conns[c]]
			close(ch)
			h.mu.Unlock()
		}
	}
	h.server.StartTLS()
	root := x509.NewCertPool()
	root.AddCert(h.server.Certificate())
	if len(h.server.Certificate().DNSNames) == 0 {
		t.Fatal("synthetic certificate has no DNS identity")
	}
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.Proxy = nil
	transport.ForceAttemptHTTP2 = false
	transport.Protocols = new(http.Protocols)
	transport.Protocols.SetHTTP1(true)
	transport.TLSNextProto = map[string]func(string, *tls.Conn) http.RoundTripper{}
	transport.TLSClientConfig = &tls.Config{RootCAs: root, ServerName: h.server.Certificate().DNSNames[0], NextProtos: []string{"http/1.1"}, MinVersion: tls.VersionTLS12}
	transport.DialTLSContext = nil
	transport.DialContext = func(ctx context.Context, network, address string) (net.Conn, error) {
		host, port, err := net.SplitHostPort(address)
		if err != nil || port != "443" || (host != "app-api.pixiv.net" && host != "i.pximg.net") {
			return nil, fmt.Errorf("unexpected logical fixture dial: %q", address)
		}
		if network != "tcp" {
			return nil, fmt.Errorf("unexpected fixture network: %q", network)
		}
		return (&net.Dialer{Timeout: migrationIdleH1Bound}).DialContext(ctx, network, h.server.Listener.Addr().String())
	}
	h.base = http.DefaultTransport
	http.DefaultTransport = transport
	t.Cleanup(func() {
		for _, call := range h.calls {
			call.cancel()
		}
		for _, plan := range h.allPlans {
			plan.unblock()
		}
		if h.client != nil {
			h.client.CloseIdleConnections()
		}
		h.server.CloseClientConnections()
		joined := make(chan struct{})
		go func() { h.wg.Wait(); close(joined) }()
		migrationIdleH1Signal(t, joined, "owned SDK request workers")
		h.server.Close()
		http.DefaultTransport = h.base
		transport.CloseIdleConnections()
	})
	var err error
	h.client, err = New("fixture-access")
	if err != nil {
		t.Fatal(err)
	}
	return h
}
func (h *migrationIdleH1Harness) plan(name string, id int64, phase string, resource bool) *migrationIdleH1Plan {
	h.t.Helper()
	route := "/v1/illust/detail?illust_id=" + strconv.FormatInt(id, 10)
	body := fmt.Sprintf(`{"illust":{"id":%d,"create_date":"2026-01-02T03:04:05Z","meta_single_page":{"original_image_url":%q}}}`, id, migrationIdleH1ResourceURL)
	if resource {
		route = "/idle-native-h1.fixture"
		body = "abc"
	}
	p := &migrationIdleH1Plan{name: name, route: route, phase: phase, body: body, release: make(chan struct{}), ready: make(chan struct{}), arrived: make(chan migrationIdleH1Wire, 1), done: make(chan struct{})}
	if phase == "" {
		p.unblock()
	}
	h.mu.Lock()
	h.plans[route] = append(h.plans[route], p)
	h.allPlans = append(h.allPlans, p)
	h.mu.Unlock()
	return p
}
func (h *migrationIdleH1Harness) serve(w http.ResponseWriter, r *http.Request) {
	h.mu.Lock()
	queue := h.plans[r.URL.RequestURI()]
	if len(queue) == 0 {
		h.mu.Unlock()
		h.peerErrors <- fmt.Errorf("unplanned request %s", r.URL.RequestURI())
		http.Error(w, "unplanned fixture", 500)
		return
	}
	p := queue[0]
	h.plans[p.route] = queue[1:]
	p.started = true
	wire := migrationIdleH1Wire{p.name, r.Context().Value(migrationIdleH1PeerKey{}).(int), r.Method, r.Host, r.URL.RequestURI(), r.Proto}
	h.requests = append(h.requests, wire)
	h.mu.Unlock()
	defer close(p.done)
	p.arrived <- wire
	if p.phase == "headers" {
		select {
		case <-p.release:
		case <-r.Context().Done():
			return
		}
	}
	w.Header().Set("Content-Type", "application/json")
	if r.Host == "i.pximg.net" {
		w.Header().Set("Content-Type", "application/octet-stream")
	}
	w.Header().Set("Content-Length", strconv.Itoa(len(p.body)))
	w.WriteHeader(200)
	if p.phase == "body" {
		if _, err := io.WriteString(w, p.body[:1]); err != nil {
			return
		}
		w.(http.Flusher).Flush()
		close(p.ready)
		select {
		case <-p.release:
		case <-r.Context().Done():
			return
		}
		_, _ = io.WriteString(w, p.body[1:])
	} else {
		_, _ = io.WriteString(w, p.body)
	}
}
func (h *migrationIdleH1Harness) call(name, operation string) *migrationIdleH1Call {
	h.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), migrationIdleH1Bound)
	c := &migrationIdleH1Call{name: name, cancel: cancel, done: make(chan migrationIdleH1Result, 1), idle: make(chan migrationIdleH1Idle, 1), firstByte: make(chan struct{}, 1), trace: migrationIdleH1Idle{Step: name}, result: migrationIdleH1Result{Step: name, Operation: operation}}
	c.ctx = httptrace.WithClientTrace(ctx, &httptrace.ClientTrace{GotFirstResponseByte: func() {
		select {
		case c.firstByte <- struct{}{}:
		default:
		}
	}, PutIdleConn: func(err error) {
		event := migrationIdleH1Idle{Step: name, Observed: true, Accepted: err == nil}
		if err != nil {
			event.RawError = err.Error()
		}
		c.traceMu.Lock()
		c.trace = event
		c.traceMu.Unlock()
		c.idle <- event
	}})
	h.calls = append(h.calls, c)
	return c
}
func (h *migrationIdleH1Harness) artwork(p *migrationIdleH1Plan, id int64) *migrationIdleH1Call {
	c := h.call(p.name, "Artwork")
	h.wg.Add(1)
	go func() {
		defer h.wg.Done()
		out := c.result
		art, err := h.client.Artwork(c.ctx, ArtworkRequest{ArtworkID: id})
		out.Success = err == nil
		out.ID = art.ID
		if err != nil {
			out.Error, _ = json.Marshal(err)
			out.ContextCanceled = errors.Is(err, context.Canceled)
		}
		c.done <- out
	}()
	return c
}
func (h *migrationIdleH1Harness) finish(c *migrationIdleH1Call, success bool) {
	h.t.Helper()
	c.result = migrationIdleH1Await(h.t, c.done, c.name+" SDK result")
	if c.result.Success != success {
		h.t.Fatalf("%s: unexpected SDK outcome %+v", c.name, c.result)
	}
}
func (h *migrationIdleH1Harness) arrived(p *migrationIdleH1Plan, connection int) {
	h.t.Helper()
	wire := migrationIdleH1Await(h.t, p.arrived, p.name+" peer request")
	if wire.Connection != connection || wire.Protocol != "HTTP/1.1" || wire.Method != "GET" {
		h.t.Fatalf("%s: unexpected wire %+v", p.name, wire)
	}
	if p.phase == "body" {
		migrationIdleH1Signal(h.t, p.ready, p.name+" flushed prefix")
		for _, call := range h.calls {
			if call.name == p.name {
				migrationIdleH1Signal(h.t, call.firstByte, p.name+" client response-byte trace")
				break
			}
		}
	}
}
func (h *migrationIdleH1Harness) idle(c *migrationIdleH1Call, accepted bool) {
	h.t.Helper()
	event := migrationIdleH1Await(h.t, c.idle, c.name+" idle-return trace")
	c.compareIdle = true
	if event.Accepted != accepted {
		h.t.Fatalf("%s: idle return %+v", c.name, event)
	}
}
func (h *migrationIdleH1Harness) closeIdle(n int) {
	for i := 0; i < n; i++ {
		h.client.CloseIdleConnections()
		h.row.Native.CloseIdleCalls++
	}
}
func (h *migrationIdleH1Harness) closed(id int, barrier string) {
	h.t.Helper()
	h.mu.Lock()
	ch := h.closeEvents[id]
	h.mu.Unlock()
	if ch == nil {
		h.t.Fatalf("connection %d was never accepted", id)
	}
	migrationIdleH1Signal(h.t, ch, fmt.Sprintf("peer connection %d shutdown at %s", id, barrier))
	h.row.Native.PeerClosures = append(h.row.Native.PeerClosures, migrationIdleH1Closure{barrier, id})
}
func (h *migrationIdleH1Harness) completeArtwork(name string, id int64, connection int) *migrationIdleH1Call {
	p := h.plan(name, id, "", false)
	c := h.artwork(p, id)
	h.arrived(p, connection)
	h.finish(c, true)
	h.idle(c, true)
	return c
}
func (h *migrationIdleH1Harness) resourceRef() sdk.ResourceRef {
	p := h.plan("metadata", 42, "", false)
	c := h.call("metadata", "ArtworkPages")
	pages, err := h.client.ArtworkPages(c.ctx, ArtworkPagesRequest{ArtworkID: 42})
	if err != nil || len(pages) != 1 {
		h.t.Fatalf("metadata result: %v, %d pages", err, len(pages))
	}
	c.result.Success = true
	c.result.ID = 42
	h.arrived(p, 1)
	h.idle(c, true)
	return pages[0].Image.Resource.Ref
}
func (h *migrationIdleH1Harness) openResource(p *migrationIdleH1Plan, ref sdk.ResourceRef, connection int) (*migrationIdleH1Call, *sdk.ResourceResponse) {
	c := h.call(p.name, "OpenResource")
	response, err := h.client.OpenResource(c.ctx, sdk.OpenResourceRequest{Ref: ref})
	if err != nil {
		h.t.Fatal(err)
	}
	c.result.Success = true
	c.result.Status = response.StatusCode
	h.arrived(p, connection)
	if response.StatusCode != 200 {
		h.t.Fatalf("resource status %d", response.StatusCode)
	}
	return c, response
}
func (h *migrationIdleH1Harness) readResource(c *migrationIdleH1Call, r *sdk.ResourceResponse, prefix string, canceled bool) {
	h.t.Helper()
	body, err := io.ReadAll(r.Body)
	c.result.Body = prefix + string(body)
	c.result.ContextCanceled = errors.Is(err, context.Canceled)
	if err != nil {
		c.result.RawReadError = err.Error()
	}
	if c.result.ContextCanceled != canceled || (!canceled && err != nil) {
		h.t.Fatalf("%s resource read: %v", c.name, err)
	}
	if err := r.Body.Close(); err != nil {
		h.t.Fatal(err)
	}
	c.result.CloseSuccess = true
}
func (h *migrationIdleH1Harness) prefix(r *sdk.ResourceResponse) string {
	h.t.Helper()
	p := make([]byte, 1)
	if _, err := io.ReadFull(r.Body, p); err != nil {
		h.t.Fatal(err)
	}
	return string(p)
}
func (h *migrationIdleH1Harness) snapshot() migrationIdleH1Row {
	h.t.Helper()
	for _, p := range h.allPlans {
		migrationIdleH1Signal(h.t, p.done, p.name+" peer handler join")
	}
	h.client.CloseIdleConnections()
	h.mu.Lock()
	ids := len(h.closeEvents)
	h.mu.Unlock()
	for id := 1; id <= ids; id++ {
		h.mu.Lock()
		ch := h.closeEvents[id]
		h.mu.Unlock()
		migrationIdleH1Signal(h.t, ch, fmt.Sprintf("final peer %d shutdown", id))
	}
	select {
	case err := <-h.peerErrors:
		h.t.Fatal(err)
	default:
	}
	h.row.PublicSDK = []migrationIdleH1Result{}
	h.row.Native.IdleReturns = []migrationIdleH1Idle{}
	for _, c := range h.calls {
		h.row.PublicSDK = append(h.row.PublicSDK, c.result)
		c.traceMu.Lock()
		h.rawIdle = append(h.rawIdle, c.trace)
		if c.compareIdle {
			h.row.Native.IdleReturns = append(h.row.Native.IdleReturns, c.trace)
		}
		c.traceMu.Unlock()
	}
	h.mu.Lock()
	h.row.Native.Requests = append([]migrationIdleH1Wire{}, h.requests...)
	h.mu.Unlock()
	return h.row
}

func TestMigrationIdleNativeH1RetirementMatchesFrozenGo(t *testing.T) {
	rows := []migrationIdleH1Row{}
	rawRows := []migrationIdleH1Raw{}
	collect := func(h *migrationIdleH1Harness) {
		rows = append(rows, h.snapshot())
		rawRows = append(rawRows, migrationIdleH1Raw{Name: h.row.Name, IdleReturns: h.rawIdle})
	}
	for _, phase := range []string{"headers", "body"} {
		t.Run("active-"+phase+"-retired-after-close", func(t *testing.T) {
			repeats := 1
			if phase == "body" {
				repeats = 2
			}
			h := migrationIdleH1New(t, "active-"+phase+"-retired-after-close", []string{"start Artwork 42 and wait for peer " + phase + " barrier", "CloseIdleConnections while request is active", "release response and await successful SDK result", "observe rejected idle return and peer connection shutdown", "Artwork 43 succeeds on a new connection"})
			p := h.plan("active", 42, phase, false)
			c := h.artwork(p, 42)
			h.arrived(p, 1)
			h.closeIdle(repeats)
			p.unblock()
			h.finish(c, true)
			h.idle(c, false)
			h.closed(1, "active response completed")
			h.completeArtwork("next", 43, 2)
			collect(h)
		})
	}
	t.Run("next-acquisition-resets-newly-idle-retirement", func(t *testing.T) {
		h := migrationIdleH1New(t, "next-acquisition-resets-newly-idle-retirement", []string{"Artwork 42 waits after flushed body prefix on connection 1", "CloseIdleConnections while 42 is active", "Artwork 43 acquires connection 2 and waits before headers", "release 42; its idle return is accepted", "Artwork 44 reuses connection 1 while 43 is active", "release 43; its idle return is accepted"})
		p := h.plan("active", 42, "body", false)
		c := h.artwork(p, 42)
		h.arrived(p, 1)
		h.closeIdle(1)
		q := h.plan("resetting", 43, "headers", false)
		d := h.artwork(q, 43)
		h.arrived(q, 2)
		p.unblock()
		h.finish(c, true)
		h.idle(c, true)
		h.completeArtwork("reused", 44, 1)
		q.unblock()
		h.finish(d, true)
		h.idle(d, true)
		collect(h)
	})
	t.Run("mixed-idle-and-active-repeated-close", func(t *testing.T) {
		h := migrationIdleH1New(t, "mixed-idle-and-active-repeated-close", []string{"warm connection 1 with Artwork 41", "Artwork 42 holds connection 1 after flushed prefix", "Artwork 43 completes and leaves connection 2 idle", "call CloseIdleConnections twice; peer 2 closes", "release 42; SDK succeeds and peer 1 closes", "Artwork 44 succeeds on connection 3"})
		h.completeArtwork("warm", 41, 1)
		p := h.plan("active", 42, "body", false)
		c := h.artwork(p, 42)
		h.arrived(p, 1)
		h.completeArtwork("idle", 43, 2)
		h.closeIdle(2)
		h.closed(2, "idle pool close")
		p.unblock()
		h.finish(c, true)
		h.idle(c, false)
		h.closed(1, "active response completed")
		h.completeArtwork("next", 44, 3)
		collect(h)
	})
	t.Run("resource-eof-after-close-retires-active-connection", func(t *testing.T) {
		h := migrationIdleH1New(t, "resource-eof-after-close-retires-active-connection", []string{"ArtworkPages resolves a synthetic resource on connection 1", "OpenResource returns headers and a prefix from connection 2", "CloseIdleConnections closes idle API connection 1", "release remaining body and read through EOF", "stream succeeds, its idle return is rejected, and peer 2 closes", "next OpenResource succeeds on connection 3; a further open reuses 3"})
		ref := h.resourceRef()
		p := h.plan("active-stream", 0, "body", true)
		c, r := h.openResource(p, ref, 2)
		prefix := h.prefix(r)
		h.closeIdle(1)
		h.closed(1, "idle API pool close")
		p.unblock()
		h.readResource(c, r, prefix, false)
		h.idle(c, false)
		h.closed(2, "stream EOF")
		for _, name := range []string{"next-stream", "reused-stream"} {
			q := h.plan(name, 0, "", true)
			d, s := h.openResource(q, ref, 3)
			h.readResource(d, s, "", false)
			h.idle(d, true)
		}
		collect(h)
	})
	t.Run("resource-early-close-discards-connection", func(t *testing.T) {
		h := migrationIdleH1New(t, "resource-early-close-discards-connection", []string{"resolve synthetic resource using ArtworkPages", "OpenResource returns after headers; read only the first body byte", "close unread body and observe peer 2 shutdown independently", "next OpenResource uses connection 3 and reads to EOF", "a further OpenResource reuses connection 3"})
		ref := h.resourceRef()
		p := h.plan("early-stream", 0, "body", true)
		c, r := h.openResource(p, ref, 2)
		c.result.Body = h.prefix(r)
		if err := r.Body.Close(); err != nil {
			t.Fatal(err)
		}
		c.result.CloseSuccess = true
		h.closed(2, "early Body.Close")
		p.unblock()
		for _, name := range []string{"next-stream", "reused-stream"} {
			q := h.plan(name, 0, "", true)
			d, s := h.openResource(q, ref, 3)
			h.readResource(d, s, "", false)
			h.idle(d, true)
		}
		collect(h)
	})
	for _, phase := range []string{"headers", "body"} {
		t.Run("cancel-active-"+phase+"-after-close", func(t *testing.T) {
			h := migrationIdleH1New(t, "cancel-active-"+phase+"-after-close", []string{"Artwork 42 reaches peer " + phase + " barrier", "CloseIdleConnections while active", "cancel request context and await terminal SDK error", "observe peer connection 1 shutdown", "Artwork 43 succeeds on a new connection"})
			p := h.plan("canceled", 42, phase, false)
			c := h.artwork(p, 42)
			h.arrived(p, 1)
			h.closeIdle(1)
			c.cancel()
			h.finish(c, false)
			if !c.result.ContextCanceled {
				t.Fatalf("cancellation was not preserved: %+v", c.result)
			}
			h.closed(1, "terminal cancellation")
			p.unblock()
			h.completeArtwork("next", 43, 2)
			collect(h)
		})
	}
	t.Run("resource-cancel-body-after-close", func(t *testing.T) {
		h := migrationIdleH1New(t, "resource-cancel-body-after-close", []string{"resolve synthetic resource using ArtworkPages", "OpenResource returns and first body byte is read", "CloseIdleConnections closes idle API connection 1", "cancel stream context and await context-canceled body read", "observe peer connection 2 shutdown", "next OpenResource succeeds on connection 3"})
		ref := h.resourceRef()
		p := h.plan("canceled-stream", 0, "body", true)
		c, r := h.openResource(p, ref, 2)
		prefix := h.prefix(r)
		h.closeIdle(1)
		h.closed(1, "idle API pool close")
		c.cancel()
		h.readResource(c, r, prefix, true)
		h.closed(2, "terminal stream cancellation")
		p.unblock()
		q := h.plan("next-stream", 0, "", true)
		d, s := h.openResource(q, ref, 3)
		h.readResource(d, s, "", false)
		h.idle(d, true)
		collect(h)
	})
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "client-idle-native-h1.json")
	if *migrationUpdateIdleNativeH1 {
		raw, err := json.MarshalIndent(rawRows, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		raw = append(raw, '\n')
		evidence := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "support", "client-idle-native-h1-evidence", "raw-private-idle-traces.json")
		if err := os.WriteFile(evidence, raw, 0o644); err != nil {
			t.Fatal(err)
		}
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
		at := 0
		for at < len(data) && at < len(want) && data[at] == want[at] {
			at++
		}
		start := max(0, at-160)
		t.Fatalf("default-owned SDK native HTTP/1 idle retirement differs at byte %d: got %q; want %q", at, data[start:min(len(data), at+160)], want[start:min(len(want), at+160)])
	}
}

package fanbox_test

import (
	"bufio"
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
	"reflect"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	fanboxcommands "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/fanbox"
	fanboxmcpcommand "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/fanbox/mcp"
	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	stdio "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver"
	fanboxmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/fanbox"
	facade "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/diagnostics"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	fanboxsdk "github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
	"github.com/modelcontextprotocol/go-sdk/mcp"
	"github.com/spf13/cobra"
)

var migrationCaptureFanboxReadTools = flag.Bool("migration-capture-fanbox-read-tools", false, "capture genuine FANBOX MCP read tools")

const migrationMCPReadPost = `{"id":"post-1","title":"fixture title 日本語","publishedDatetime":"2024-06-01T10:20:30.456+09:00","creatorId":"writer","feeRequired":500,"isRestricted":true,"isPinned":true,"restrictedFor":2,"commentCount":3,"body":{"text":"fixture private body canary","blocks":[{"type":"image","imageId":"image-1"},{"type":"file","fileId":"file-1"},{"type":"article"},{"type":"video"},{"type":"alien"}],"imageMap":{"image-1":{"id":"image-1","extension":"png","originalUrl":"https://downloads.fanbox.cc/image-1.png?signature=fixture-signed-secret","thumbnailUrl":"https://i.pximg.net/thumb.png"}},"fileMap":{"file-1":{"id":"file-1","name":"fixture.zip","extension":"zip","url":"https://i.pximg.net/file-1.zip?signature=fixture-signed-secret"}}}}`
const migrationMCPReadCreator = `{"creatorId":"writer","user":{"name":" Writer 日本語 ","iconUrl":"https://i.pximg.net/icon.png?signature=fixture-signed-secret"},"coverImageUrl":"https://downloads.fanbox.cc/cover.png?signature=fixture-signed-secret","hasAdultContent":true,"isFollowing":true,"plan":{"fee":700,"hasSupportingPlan":true}}`

type migrationMCPReadCall struct {
	Tool      string          `json:"tool"`
	Arguments json.RawMessage `json:"arguments"`
	Result    json.RawMessage `json:"result"`
	Error     string          `json:"error"`
}
type migrationMCPReadRow struct {
	Name          string                 `json:"name"`
	Scenario      string                 `json:"scenario"`
	Port          string                 `json:"port"`
	Config        string                 `json:"config"`
	Proxy         *string                `json:"proxy"`
	Calls         []migrationMCPReadCall `json:"calls"`
	Requests      []map[string]any       `json:"requests"`
	Responses     []map[string]any       `json:"responses"`
	Trace         []string               `json:"trace"`
	Diagnostics   []map[string]any       `json:"diagnostics"`
	Database      []map[string]any       `json:"database"`
	ConfigAfter   string                 `json:"config_after"`
	LeaseOpens    int                    `json:"lease_opens"`
	LeaseCloses   int                    `json:"lease_closes"`
	MaxLiveLeases int                    `json:"max_live_leases"`
}
type migrationMCPReadFixture struct {
	FrozenGo   string                `json:"frozen_go"`
	Sources    map[string]string     `json:"source_sha256"`
	Boundary   string                `json:"boundary"`
	Tools      json.RawMessage       `json:"tools_list"`
	Initialize json.RawMessage       `json:"initialize"`
	Cases      []migrationMCPReadRow `json:"cases"`
	Stdio      []map[string]any      `json:"stdio"`
}
type migrationMCPReadFiles struct{ path string }

func (f migrationMCPReadFiles) Path() (string, error)             { return f.path, nil }
func (f migrationMCPReadFiles) ReadFile(p string) ([]byte, error) { return os.ReadFile(p) }
func (f migrationMCPReadFiles) WritePrivateFile(p string, b []byte) error {
	return os.WriteFile(p, b, 0600)
}
func (f migrationMCPReadFiles) EnsurePrivateFile(p string, b []byte) error {
	if _, e := os.Stat(p); e == nil {
		return nil
	}
	return os.WriteFile(p, b, 0600)
}

type migrationMCPReadRepository struct {
	db *database.DB
	h  *migrationMCPReadHarness
}

func (r migrationMCPReadRepository) SaveFanboxCredential(c context.Context, a account.Account) error {
	return r.db.SaveFanboxCredential(c, a)
}
func (r migrationMCPReadRepository) RotateFanboxSession(c context.Context, id, rev int64, b []byte, at int64) error {
	return r.db.RotateFanboxSession(c, id, rev, b, at)
}
func (r migrationMCPReadRepository) ListFanbox(c context.Context) ([]account.Account, error) {
	r.h.trace("repository.list")
	return r.db.ListFanbox(c)
}
func (r migrationMCPReadRepository) GetFanbox(c context.Context, id int64) (account.Account, error) {
	r.h.trace(fmt.Sprintf("repository.get/%d", id))
	return r.db.GetFanbox(c, id)
}
func (r migrationMCPReadRepository) RemoveFanbox(c context.Context, id int64) error {
	return r.db.RemoveFanbox(c, id)
}

type migrationMCPReadDefaults struct {
	store settings.Store
	h     *migrationMCPReadHarness
}

func (d migrationMCPReadDefaults) ReadFanboxDefaultUserID() (int64, bool, error) {
	d.h.trace("defaults.read")
	return d.store.ReadFanboxDefaultUserID()
}
func (d migrationMCPReadDefaults) SetFanboxDefaultUserID(id int64) error {
	return d.store.SetFanboxDefaultUserID(id)
}
func (d migrationMCPReadDefaults) ClearFanboxDefaultUserID() error {
	return d.store.ClearFanboxDefaultUserID()
}

type migrationMCPReadHarness struct {
	t             *testing.T
	mu            sync.Mutex
	row           migrationMCPReadRow
	db            *database.DB
	files         migrationMCPReadFiles
	service       *account.Service
	facade        *facade.Facade
	clients       map[*fanboxsdk.Client]int
	live          int
	network       int
	routeCounts   map[string]int
	started       chan struct{}
	releaseFirst  chan struct{}
	releaseSecond chan struct{}
	completed     chan struct{}
}

func (h *migrationMCPReadHarness) trace(s string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.row.Trace = append(h.row.Trace, s)
}
func migrationMCPReadNewHarness(t *testing.T, name, scenario, port, config string, proxy *string) *migrationMCPReadHarness {
	t.Helper()
	dir := t.TempDir()
	db, e := database.Open(filepath.Join(dir, "auth"))
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = db.Close() })
	h := &migrationMCPReadHarness{t: t, db: db, files: migrationMCPReadFiles{filepath.Join(dir, "config.toml")}, clients: map[*fanboxsdk.Client]int{}, routeCounts: map[string]int{}, started: make(chan struct{}, 2), releaseFirst: make(chan struct{}), releaseSecond: make(chan struct{}), completed: make(chan struct{}, 8)}
	h.row = migrationMCPReadRow{Name: name, Scenario: scenario, Port: port, Config: config, Proxy: proxy, Calls: []migrationMCPReadCall{}, Requests: []map[string]any{}, Responses: []map[string]any{}, Trace: []string{}, Diagnostics: []map[string]any{}, Database: []map[string]any{}}
	if e = os.WriteFile(h.files.path, []byte(config), 0600); e != nil {
		t.Fatal(e)
	}
	if scenario != "no_accounts" {
		for _, id := range []int64{7, 9} {
			a := account.New(id, fmt.Sprintf("fixture-%d", id), "", []byte(fmt.Sprintf("fixture-session-secret-%d", id)))
			a.CredentialRevision = 1
			a.ValidatedAt = 30
			if e = db.SaveFanboxCredential(context.Background(), a); e != nil {
				t.Fatal(e)
			}
		}
		if _, e = db.DB().Exec("UPDATE fanbox_account SET created_at=11,updated_at=22"); e != nil {
			t.Fatal(e)
		}
	}
	store := settings.Store{Files: h.files}
	h.service = account.NewService(migrationMCPReadRepository{db, h}, migrationMCPReadDefaults{store, h})
	h.service.LoadOptionsFunc = func() (fanboxsdk.Options, error) {
		h.trace("options.load")
		if scenario == "options_error" {
			return fanboxsdk.Options{}, errors.New("owned options failure")
		}
		options := fanboxsdk.Options{HTTPClient: &http.Client{Transport: (*migrationMCPReadTransport)(h)}, UserAgent: "fixture-agent"}
		if scenario == "invalid_proxy" {
			options.ProxyURL = "http://fixture-user:fixture-password-secret@127.0.0.1:9"
		}
		return options, nil
	}
	h.facade = facade.NewFacadeWithCloseClient(h.service, func(c *fanboxsdk.Client) error {
		h.mu.Lock()
		id := h.clients[c]
		h.live--
		h.row.LeaseCloses++
		h.row.Trace = append(h.row.Trace, fmt.Sprintf("lease.close/%d", id))
		h.mu.Unlock()
		c.CloseIdleConnections()
		if strings.Contains(scenario, "lease_close_failure") {
			return errors.New("fixture lease close secret failure")
		}
		return nil
	})
	return h
}
func (h *migrationMCPReadHarness) server() *mcp.Server {
	ports := fanboxmcp.SDKPorts{}
	if h.row.Port != "none" {
		ports.OpenLease = func(ctx context.Context, a fanboxmcp.Account) (*lifecycle.Lease[*fanboxsdk.Client], error) {
			h.trace("port.open/proxy=" + migrationMCPReadProxy(a.HTTPSProxyOverride))
			if h.row.Port == "open_error" {
				return nil, errors.New("owned opener failure")
			}
			if h.row.Port == "nil_lease" {
				return nil, nil
			}
			lease, e := h.facade.Open(ctx, facade.OpenRequest{ProxyOverride: a.HTTPSProxyOverride})
			if e != nil {
				return nil, e
			}
			h.mu.Lock()
			h.row.LeaseOpens++
			id := h.row.LeaseOpens
			h.clients[lease.Value()] = id
			h.live++
			h.row.MaxLiveLeases = max(h.row.MaxLiveLeases, h.live)
			h.row.Trace = append(h.row.Trace, fmt.Sprintf("lease.open/%d", id))
			h.mu.Unlock()
			return lease, nil
		}
	}
	if h.row.Port == "raw" {
		ports.OpenLease = nil
		ports.Open = func(ctx context.Context, a fanboxmcp.Account) (*fanboxsdk.Client, error) {
			h.trace("raw.open/proxy=" + migrationMCPReadProxy(a.HTTPSProxyOverride))
			return h.service.OpenClientWithProxy(ctx, a.HTTPSProxyOverride)
		}
	}
	if h.row.Port == "raw_nil" {
		ports.OpenLease = nil
		ports.Open = func(context.Context, fanboxmcp.Account) (*fanboxsdk.Client, error) {
			h.trace("raw.open/nil")
			return nil, nil
		}
	}
	return fanboxmcp.NewWithProxy(ports, h.row.Proxy)
}
func migrationMCPReadProxy(p *string) string {
	if p == nil {
		return "nil"
	}
	return *p
}
func (h *migrationMCPReadHarness) event(e diagnostics.Event) {
	if e.Duration < 0 {
		h.t.Error("negative diagnostic duration")
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	h.row.Diagnostics = append(h.row.Diagnostics, map[string]any{"module": e.Module, "kind": e.Kind, "operation": e.Operation, "resource": e.Resource, "route": e.Route, "target": e.Target, "proxy": e.Proxy, "user_agent": e.UserAgent, "reason": e.Reason, "status": e.Status, "count": e.Count, "request_id": e.RequestID, "duration_nonnegative": e.Duration >= 0})
	if e.Module == diagnostics.ModuleFanboxMCPServer && (e.Kind == diagnostics.EventCompleted || e.Kind == diagnostics.EventFailed) {
		h.completed <- struct{}{}
	}
}
func (h *migrationMCPReadHarness) connect() (*mcp.ClientSession, func()) {
	h.t.Helper()
	ctx, cancel := context.WithCancel(diagnostics.WithScope(context.Background(), diagnostics.SinkFunc(h.event), diagnostics.ModuleFanboxCLI, 999))
	ct, st := mcp.NewInMemoryTransports()
	done := make(chan error, 1)
	server := h.server()
	go func() { done <- server.Run(ctx, st) }()
	var session *mcp.ClientSession
	var stopOnce sync.Once
	stop := func() {
		stopOnce.Do(func() {
			cancel()
			if session != nil {
				_ = session.Close()
			}
			select {
			case <-done:
			case <-time.After(5 * time.Second):
				h.t.Error("owned MCP server did not stop")
			}
		})
	}
	h.t.Cleanup(stop)
	client := mcp.NewClient(&mcp.Implementation{Name: "fixture-client", Version: "1"}, nil)
	var e error
	session, e = client.Connect(ctx, ct, nil)
	if e != nil {
		stop()
		h.t.Fatal(e)
	}
	return session, stop
}
func migrationMCPReadJSON(t *testing.T, x any) json.RawMessage {
	t.Helper()
	b, e := json.Marshal(x)
	if e != nil {
		t.Fatal(e)
	}
	return b
}
func (h *migrationMCPReadHarness) call(ctx context.Context, s *mcp.ClientSession, tool, args string) migrationMCPReadCall {
	h.t.Helper()
	c := migrationMCPReadCall{Tool: tool, Arguments: json.RawMessage(args)}
	r, e := s.CallTool(ctx, &mcp.CallToolParams{Name: tool, Arguments: json.RawMessage(args)})
	if e != nil {
		c.Error = e.Error()
		c.Result = json.RawMessage("null")
	} else {
		c.Result = migrationMCPReadJSON(h.t, r)
	}
	return c
}
func (h *migrationMCPReadHarness) addCall(s *mcp.ClientSession, tool, args string) {
	c := h.call(context.Background(), s, tool, args)
	h.row.Calls = append(h.row.Calls, c)
}
func (h *migrationMCPReadHarness) finish() migrationMCPReadRow {
	h.t.Helper()
	rows, e := h.db.ListFanbox(context.Background())
	if e != nil {
		h.t.Fatal(e)
	}
	for _, a := range rows {
		h.row.Database = append(h.row.Database, map[string]any{"user_id": a.UserID, "sort_order": a.SortOrder, "session": string(a.SessionIDCopy()), "credential_revision": a.CredentialRevision, "validated_at": a.ValidatedAt, "created_at": a.CreatedAt, "updated_at": a.UpdatedAt})
	}
	b, e := os.ReadFile(h.files.path)
	if e != nil {
		h.t.Fatal(e)
	}
	h.row.ConfigAfter = string(b)
	if h.live != 0 {
		h.t.Fatalf("%s leaked %d leases", h.row.Name, h.live)
	}
	for _, c := range h.row.Calls {
		for _, secret := range []string{"fixture-session-secret", "fixture-signed-secret", "fixture-password-secret", "fixture private body canary", "fixture lease close secret"} {
			if bytes.Contains(c.Result, []byte(secret)) {
				h.t.Fatalf("%s output leaked %s", h.row.Name, secret)
			}
		}
	}
	d := migrationMCPReadJSON(h.t, h.row.Diagnostics)
	for _, secret := range []string{"fixture-session-secret", "fixture-signed-secret", "fixture-password-secret", "fixture private body canary"} {
		if bytes.Contains(d, []byte(secret)) {
			h.t.Fatalf("%s diagnostics leaked %s", h.row.Name, secret)
		}
	}
	return h.row
}

type migrationMCPReadTransport migrationMCPReadHarness

func (r *migrationMCPReadTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	h := (*migrationMCPReadHarness)(r)
	h.mu.Lock()
	h.network++
	n := h.network
	h.routeCounts[req.URL.Path]++
	routeN := h.routeCounts[req.URL.Path]
	scenario := h.row.Scenario
	h.row.Requests = append(h.row.Requests, map[string]any{"method": req.Method, "url": req.URL.String(), "cookie": req.Header.Get("Cookie"), "accept": req.Header.Get("Accept"), "origin": req.Header.Get("Origin"), "referer": req.Header.Get("Referer"), "user_agent": req.Header.Get("User-Agent")})
	h.row.Trace = append(h.row.Trace, fmt.Sprintf("request/%d", n))
	h.mu.Unlock()
	if scenario == "cancel_reuse" && n == 1 {
		h.started <- struct{}{}
		if marker := os.Getenv("PIXIV_FANBOX_READ_STDIO_MARKER"); marker != "" {
			if e := os.WriteFile(marker, []byte("started"), 0600); e != nil {
				return nil, e
			}
		}
		<-req.Context().Done()
		h.trace("request.canceled/1")
		return nil, req.Context().Err()
	}
	if scenario == "independent_accounts" && req.URL.Path == "/" {
		h.started <- struct{}{}
		release := h.releaseFirst
		if strings.Contains(req.Header.Get("Cookie"), "secret-9") {
			release = h.releaseSecond
		}
		select {
		case <-release:
		case <-req.Context().Done():
			return nil, req.Context().Err()
		}
	}
	if scenario == "eof_pending" && n == 1 {
		if marker := os.Getenv("PIXIV_FANBOX_READ_STDIO_MARKER"); marker != "" {
			if e := os.WriteFile(marker, []byte("started"), 0600); e != nil {
				return nil, e
			}
		}
		select {
		case <-req.Context().Done():
			h.trace("request.canceled/1")
			return nil, req.Context().Err()
		case <-time.After(200 * time.Millisecond):
			h.trace("owned.pending.request.released/1")
		}
	}
	if scenario == "transport_error" || ((scenario == "partial_transport_error" || scenario == "creators_partial_error") && req.URL.Path == "/page2") {
		h.trace(fmt.Sprintf("transport.error/%d", n))
		return nil, errors.New("fixture-session-secret unsafe transport")
	}
	body, kind, status := h.response(req, routeN)
	if scenario == "malformed" {
		body = "{"
	}
	if scenario == "decode_close_failure" {
		body = "{"
	}
	if scenario == "status401" {
		status = 401
	}
	if scenario == "status403" {
		status = 403
	}
	if scenario == "status500" {
		status = 500
	}
	if scenario == "unsafe_asset" {
		body = strings.ReplaceAll(body, "https://downloads.fanbox.cc/image-1.png?signature=fixture-signed-secret", "https://evil.invalid/image.png?signature=fixture-signed-secret")
	}
	if scenario == "bad_date" {
		body = strings.ReplaceAll(body, "2024-06-01T10:20:30.456+09:00", "invalid-time")
	}
	if scenario == "post_nil_body" {
		body = `{"body":{"post":{"id":"post-1","title":"no body","publishedDatetime":"2024-01-01T00:00:00Z","creatorId":"writer","body":null}}}`
	}
	if scenario == "post_empty_images" {
		body = `{"body":{"post":{"id":"post-1","publishedDatetime":"","creatorId":"writer","body":{"text":"fixture private body canary","images":[]}}}}`
	}
	closeFailure := (strings.Contains(scenario, "body_close_failure") && (!strings.HasPrefix(scenario, "media_") || kind == "image/png")) || scenario == "decode_close_failure"
	readFailure := scenario == "read_failure" || (scenario == "media_read_failure" && kind == "image/png")
	if kind == "image/png" {
		if scenario == "media304" {
			status = 304
		}
		if scenario == "media401" {
			status = 401
		}
	}
	h.mu.Lock()
	h.row.Responses = append(h.row.Responses, map[string]any{"request": n, "status": status, "content_type": kind, "body": body, "close_failure": closeFailure, "read_failure": readFailure})
	h.mu.Unlock()
	header := http.Header{"Content-Type": {kind}}
	if kind == "image/png" {
		header.Set("Content-Length", "4")
		header.Set("Set-Cookie", "fixture-session-secret")
		header.Set("X-Unsafe", "fixture-signed-secret")
	}
	if scenario == "media_missing_headers" && kind == "image/png" {
		header.Del("Content-Type")
		header.Del("Content-Length")
	}
	b := &migrationMCPReadBody{h: h, n: n, reader: strings.NewReader(body), closeFail: closeFailure, readFail: readFailure}
	return &http.Response{StatusCode: status, Header: header, Body: b}, nil
}
func (h *migrationMCPReadHarness) response(req *http.Request, routeN int) (string, string, int) {
	p := req.URL.Path
	scenario := h.row.Scenario
	if req.URL.Host == "downloads.fanbox.cc" || req.URL.Host == "i.pximg.net" {
		return "PNG!", "image/png", 200
	}
	if p == "/" {
		id := 7
		if strings.Contains(req.Header.Get("Cookie"), "secret-9") {
			id = 9
		}
		return fmt.Sprintf(`<html><head><meta name="metadata" content='{"context":{"user":{"userId":%d,"name":"fixture user","creatorId":"writer"}}}'></head></html>`, id), "text/html", 200
	}
	if p == "/creator.get" {
		return `{"body":` + migrationMCPReadCreator + `}`, "application/json", 200
	}
	if p == "/post.info" {
		post := migrationMCPReadPost
		if scenario == "rotated_refs" && routeN > 1 {
			post = strings.ReplaceAll(post, "image-1.png?signature=fixture-signed-secret", "rotated-image-1.png?signature=fixture-signed-secret")
		}
		return `{"body":{"post":` + post + `}}`, "application/json", 200
	}
	if p == "/tag.getFeatured" {
		return `{"body":{"tags":[{"tag":" fanart 日本語 ","url":"https://www.fanbox.cc/@writer/posts/tag/fanart"},{"tag":"no URL"}]}}`, "application/json", 200
	}
	isCreator := p == "/plan.listSupporting" || p == "/creator.listFollowing" || strings.Contains(scenario, "creators_")
	items := []string{migrationMCPReadPost}
	if isCreator {
		items = []string{`{"creatorId":"writer","user":{"name":"Writer","iconUrl":"https://i.pximg.net/icon.png"}}`}
	}
	next := ""
	switch scenario {
	case "list_truncated", "list_all", "list_page2", "list_past_end":
		items = append(items, strings.ReplaceAll(items[0], "post-1", "post-2"))
		if isCreator {
			items[1] = `{"creatorId":"writer2"}`
		}
	case "list_next", "list_empty_then_data", "partial_transport_error", "partial_malformed", "list_cycle", "creators_all", "creators_partial_error":
		if p != "/page2" {
			next = "https://api.fanbox.cc/page2?fixture=next"
		}
		if scenario == "list_cycle" {
			next = "https://api.fanbox.cc/page2?fixture=next"
		}
		if scenario == "list_empty_then_data" && p != "/page2" {
			items = nil
		}
		if scenario == "partial_malformed" && p == "/page2" {
			return "{", "application/json", 200
		}
	}
	if scenario == "list_empty" {
		items = nil
	}
	if scenario == "unsafe_continuation" {
		next = "https://evil.invalid/page2?fixture=secret"
	}
	if p == "/page2" {
		for i := range items {
			items[i] = strings.ReplaceAll(items[i], "post-1", "post-next")
		}
	}
	key := "posts"
	if p == "/post.listHome" || p == "/post.listSupporting" {
		key = "items"
	}
	if isCreator {
		key = "plans"
		if p == "/creator.listFollowing" {
			key = "creators"
		}
	}
	return fmt.Sprintf(`{"body":{"%s":[%s],"nextUrl":%q}}`, key, strings.Join(items, ","), next), "application/json", 200
}

type migrationMCPReadBody struct {
	h                   *migrationMCPReadHarness
	n                   int
	reader              *strings.Reader
	closeFail, readFail bool
}

func (b *migrationMCPReadBody) Read(p []byte) (int, error) {
	if b.readFail {
		b.h.trace(fmt.Sprintf("body.read/%d/0/failure", b.n))
		return 0, errors.New("fixture-signed-secret unsafe read")
	}
	n, e := b.reader.Read(p)
	text := "nil"
	if e != nil {
		text = e.Error()
	}
	b.h.trace(fmt.Sprintf("body.read/%d/%d/%s", b.n, n, text))
	return n, e
}
func (b *migrationMCPReadBody) Close() error {
	b.h.trace(fmt.Sprintf("body.close/%d", b.n))
	if b.closeFail {
		return errors.New("fixture-signed-secret unsafe close")
	}
	return nil
}

func TestMigrationFanboxReadToolsFrozenGo(t *testing.T) {
	for path, want := range migrationMCPReadSources {
		b, e := os.ReadFile(filepath.Join("../../..", path))
		if e != nil {
			t.Fatal(e)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(b)); got != want {
			t.Fatalf("frozen source drift %s: %s", path, got)
		}
	}
	fixture := migrationMCPReadFixture{FrozenGo: "4b4426487ef18bed276706daec385e0d0a6979f9", Sources: migrationMCPReadSources, Boundary: "actual eleven registered tools and github.com/modelcontextprotocol/go-sdk v0.8.0 MCP sessions; owned SQLite/settings -> saved account.Service -> Facade.Open -> public SDK/OpenWith injected fallible HTTP transport; owned command leaf -> production RunStdio (not cmd/pixiv bootstrap, native transport, or platform proof)", Cases: []migrationMCPReadRow{}}
	h := migrationMCPReadNewHarness(t, "tools_list", "success", "lease", "", nil)
	s, stop := h.connect()
	list, e := s.ListTools(context.Background(), nil)
	if e != nil {
		t.Fatal(e)
	}
	if len(list.Tools) != 11 {
		t.Fatalf("tools=%d", len(list.Tools))
	}
	sort.Slice(list.Tools, func(i, j int) bool { return list.Tools[i].Name < list.Tools[j].Name })
	fixture.Tools = migrationMCPReadJSON(t, list)
	fixture.Initialize = migrationMCPReadJSON(t, s.InitializeResult())
	stop()
	fixture.Cases = append(fixture.Cases, h.finish())
	type testcase struct {
		name, tool, args, scenario, port, config string
		proxy                                    *string
	}
	cases := []testcase{
		{name: "current_user", tool: "fanbox_current_user", args: `{}`},
		{name: "creator", tool: "fanbox_creator", args: `{"creator_id":" writer "}`},
		{name: "creators_default_supporting", tool: "fanbox_creators", args: `{}`},
		{name: "creators_following", tool: "fanbox_creators", args: `{"kind":"following"}`},
		{name: "creator_tags", tool: "fanbox_creator_tags", args: `{"creator_id":" writer "}`},
		{name: "creator_posts", tool: "fanbox_creator_posts", args: `{"creator_id":" writer "}`},
		{name: "tagged_posts", tool: "fanbox_tagged_posts", args: `{"creator_id":" writer ","tag":" fanart 日本語 "}`},
		{name: "post", tool: "fanbox_post", args: `{"post_id":" post-1 "}`},
		{name: "home", tool: "fanbox_home", args: `{}`},
		{name: "supporting", tool: "fanbox_supporting", args: `{}`},
		{name: "resolve_post", tool: "fanbox_resolve_url", args: `{"url":"https://www.fanbox.cc/@writer/posts/123"}`},
		{name: "resolve_creator", tool: "fanbox_resolve_url", args: `{"url":"https://writer.fanbox.cc/ignored/path"}`},
		{name: "resolve_tag", tool: "fanbox_resolve_url", args: `{"url":"https://www.fanbox.cc/@writer/posts/tag/fanart"}`},
		{name: "resolve_bad_host", tool: "fanbox_resolve_url", args: `{"url":"https://example.invalid/path"}`},
		{name: "post_nil_body", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "post_nil_body"},
		{name: "post_empty_images", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "post_empty_images"},
		{name: "post_bad_date", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "bad_date"},
		{name: "post_unsafe_asset", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "unsafe_asset"},
		{name: "post_lease_close_failure_ignored", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "lease_close_failure"},
		{name: "post_api_body_close_failure", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "body_close_failure"},
		{name: "post_decode_and_close_failure", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "decode_close_failure"},
		{name: "post_body_read_failure", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "read_failure"},
		{name: "current_user_body_close_failure", tool: "fanbox_current_user", args: `{}`, scenario: "body_close_failure"},
		{name: "current_user_status401", tool: "fanbox_current_user", args: `{}`, scenario: "status401"},
		{name: "creator_status403", tool: "fanbox_creator", args: `{"creator_id":"writer"}`, scenario: "status403"},
		{name: "home_status500", tool: "fanbox_home", args: `{}`, scenario: "status500"},
		{name: "creator_schema_missing", tool: "fanbox_creator", args: `{}`},
		{name: "post_schema_missing", tool: "fanbox_post", args: `{}`},
		{name: "tags_schema_missing", tool: "fanbox_creator_tags", args: `{}`},
		{name: "tagged_schema_missing_tag", tool: "fanbox_tagged_posts", args: `{"creator_id":"writer"}`},
		{name: "resolve_schema_missing", tool: "fanbox_resolve_url", args: `{}`},
		{name: "resource_schema_missing", tool: "fanbox_open_resource", args: `{}`},
		{name: "creator_empty", tool: "fanbox_creator", args: `{"creator_id":""}`},
		{name: "creator_whitespace", tool: "fanbox_creator", args: `{"creator_id":"  "}`},
		{name: "creator_posts_empty_and_bad_page", tool: "fanbox_creator_posts", args: `{"creator_id":"","page":0}`},
		{name: "creator_tags_empty", tool: "fanbox_creator_tags", args: `{"creator_id":""}`},
		{name: "post_empty", tool: "fanbox_post", args: `{"post_id":""}`},
		{name: "post_whitespace", tool: "fanbox_post", args: `{"post_id":"  "}`},
		{name: "tagged_empty_creator", tool: "fanbox_tagged_posts", args: `{"creator_id":"","tag":"tag"}`},
		{name: "tagged_empty_tag", tool: "fanbox_tagged_posts", args: `{"creator_id":"writer","tag":""}`},
		{name: "resolve_whitespace", tool: "fanbox_resolve_url", args: `{"url":"  "}`},
		{name: "resource_invalid_ref", tool: "fanbox_open_resource", args: `{"ref":"not-a-ref"}`},
		{name: "creators_invalid_kind", tool: "fanbox_creators", args: `{"kind":"everything"}`},
		{name: "creators_kind_and_plan_error_order", tool: "fanbox_creators", args: `{"kind":"everything","limit":-1}`},
		{name: "post_schema_wrong_type", tool: "fanbox_post", args: `{"post_id":123}`},
		{name: "post_schema_null", tool: "fanbox_post", args: `{"post_id":null}`},
		{name: "home_schema_fraction", tool: "fanbox_home", args: `{"limit":1.5}`},
		{name: "home_schema_string", tool: "fanbox_home", args: `{"page":"2","limit":1}`},
		{name: "home_schema_boolean", tool: "fanbox_home", args: `{"limit":true}`},
		{name: "home_explicit_nulls", tool: "fanbox_home", args: `{"page":null,"limit":null}`},
		{name: "home_array_arguments", tool: "fanbox_home", args: `[]`},
		{name: "home_null_arguments", tool: "fanbox_home", args: `null`},
		{name: "home_scalar_arguments", tool: "fanbox_home", args: `42`},
		{name: "unknown_tool", tool: "fanbox_missing", args: `{}`},
		{name: "home_limit_negative", tool: "fanbox_home", args: `{"limit":-1}`},
		{name: "home_page_negative", tool: "fanbox_home", args: `{"page":-1,"limit":1}`},
		{name: "home_page_without_limit", tool: "fanbox_home", args: `{"page":1}`},
		{name: "home_page_limit_zero", tool: "fanbox_home", args: `{"page":2,"limit":0}`},
		{name: "home_page_overflow", tool: "fanbox_home", args: `{"page":4611686018427387904,"limit":4}`},
		{name: "home_schema_int_overflow", tool: "fanbox_home", args: `{"limit":9223372036854775808}`},
		{name: "home_schema_max_int_rounded_overflow", tool: "fanbox_home", args: `{"page":9223372036854775807,"limit":2}`},
		{name: "home_default_empty", tool: "fanbox_home", args: `{}`, scenario: "list_empty"},
		{name: "home_default_has_next", tool: "fanbox_home", args: `{}`, scenario: "list_next"},
		{name: "home_default_skips_empty", tool: "fanbox_home", args: `{}`, scenario: "list_empty_then_data"},
		{name: "home_positive_truncated_without_next", tool: "fanbox_home", args: `{"limit":1}`, scenario: "list_truncated"},
		{name: "home_positive_truncated_with_next", tool: "fanbox_home", args: `{"limit":1}`, scenario: "list_next"},
		{name: "home_zero_all_pages", tool: "fanbox_home", args: `{"limit":0}`, scenario: "list_next"},
		{name: "home_zero_one_page", tool: "fanbox_home", args: `{"limit":0}`, scenario: "list_all"},
		{name: "home_page2", tool: "fanbox_home", args: `{"page":2,"limit":1}`, scenario: "list_page2"},
		{name: "home_past_end", tool: "fanbox_home", args: `{"page":3,"limit":1}`, scenario: "list_past_end"},
		{name: "home_cross_upstream_page", tool: "fanbox_home", args: `{"page":2,"limit":1}`, scenario: "list_next"},
		{name: "home_partial_error_discards_posts", tool: "fanbox_home", args: `{"limit":0}`, scenario: "partial_transport_error"},
		{name: "home_partial_malformed_discards_posts", tool: "fanbox_home", args: `{"limit":0}`, scenario: "partial_malformed"},
		{name: "home_cursor_cycle_discards_posts", tool: "fanbox_home", args: `{"limit":0}`, scenario: "list_cycle"},
		{name: "home_unsafe_continuation", tool: "fanbox_home", args: `{"limit":0}`, scenario: "unsafe_continuation"},
		{name: "creators_zero_all_pages", tool: "fanbox_creators", args: `{"limit":0}`, scenario: "creators_all"},
		{name: "creators_partial_error_discards_results", tool: "fanbox_creators", args: `{"limit":0}`, scenario: "creators_partial_error"},
		{name: "creator_posts_zero_all_pages", tool: "fanbox_creator_posts", args: `{"creator_id":"writer","limit":0}`, scenario: "list_next"},
		{name: "tagged_posts_positive_page2", tool: "fanbox_tagged_posts", args: `{"creator_id":"writer","tag":"art","page":2,"limit":1}`, scenario: "list_next"},
		{name: "supporting_default_skips_empty", tool: "fanbox_supporting", args: `{}`, scenario: "list_empty_then_data"},
		{name: "resolve_no_accounts_still_opens", tool: "fanbox_resolve_url", args: `{"url":"https://writer.fanbox.cc"}`, scenario: "no_accounts"},
		{name: "post_missing_default", tool: "fanbox_post", args: `{"post_id":"post-1"}`, config: "[fanbox.auth]\ndefault_user_id = 99\n"},
		{name: "post_invalid_config", tool: "fanbox_post", args: `{"post_id":"post-1"}`, config: "[fanbox.auth]\ndefault_user_id = [\n"},
		{name: "post_options_error", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "options_error"},
		{name: "post_invalid_proxy_redacted", tool: "fanbox_post", args: `{"post_id":"post-1"}`, scenario: "invalid_proxy"},
		{name: "post_port_missing", tool: "fanbox_post", args: `{"post_id":"post-1"}`, port: "none"},
		{name: "post_port_failure", tool: "fanbox_post", args: `{"post_id":"post-1"}`, port: "open_error"},
		{name: "post_port_nil_lease", tool: "fanbox_post", args: `{"post_id":"post-1"}`, port: "nil_lease"},
		{name: "post_raw_adapter", tool: "fanbox_post", args: `{"post_id":"post-1"}`, port: "raw"},
		{name: "post_raw_nil_client", tool: "fanbox_post", args: `{"post_id":"post-1"}`, port: "raw_nil"},
	}
	for _, tool := range []string{"fanbox_current_user", "fanbox_creator", "fanbox_creators", "fanbox_creator_tags", "fanbox_creator_posts", "fanbox_tagged_posts", "fanbox_post", "fanbox_home", "fanbox_supporting", "fanbox_resolve_url", "fanbox_open_resource"} {
		args := map[string]any{"unexpected": true}
		switch tool {
		case "fanbox_creator", "fanbox_creator_tags", "fanbox_creator_posts":
			args["creator_id"] = "writer"
		case "fanbox_tagged_posts":
			args["creator_id"] = "writer"
			args["tag"] = "art"
		case "fanbox_post":
			args["post_id"] = "post-1"
		case "fanbox_resolve_url":
			args["url"] = "https://writer.fanbox.cc"
		case "fanbox_open_resource":
			args["ref"] = "not-a-ref"
		}
		cases = append(cases, testcase{name: tool + "_rejects_unknown_field", tool: tool, args: string(migrationMCPReadJSON(t, args))})
	}
	for _, entry := range cases {
		if entry.scenario == "" {
			entry.scenario = "success"
		}
		if entry.port == "" {
			entry.port = "lease"
		}
		t.Run(entry.name, func(t *testing.T) {
			h := migrationMCPReadNewHarness(t, entry.name, entry.scenario, entry.port, entry.config, entry.proxy)
			s, stop := h.connect()
			h.addCall(s, entry.tool, entry.args)
			stop()
			fixture.Cases = append(fixture.Cases, h.finish())
		})
	}
	for _, tool := range []string{"fanbox_current_user", "fanbox_creator", "fanbox_creators", "fanbox_creator_tags", "fanbox_creator_posts", "fanbox_tagged_posts", "fanbox_post", "fanbox_home", "fanbox_supporting"} {
		args := `{}`
		switch tool {
		case "fanbox_creator", "fanbox_creator_tags", "fanbox_creator_posts":
			args = `{"creator_id":"writer"}`
		case "fanbox_tagged_posts":
			args = `{"creator_id":"writer","tag":"art"}`
		case "fanbox_post":
			args = `{"post_id":"post-1"}`
		}
		t.Run(tool+"_transport_failure", func(t *testing.T) {
			h := migrationMCPReadNewHarness(t, tool+"_transport_failure", "transport_error", "lease", "", nil)
			s, stop := h.connect()
			h.addCall(s, tool, args)
			stop()
			fixture.Cases = append(fixture.Cases, h.finish())
		})
	}
	for _, scenario := range []string{"rotated_refs", "media_body_close_failure_lease_close_failure", "media_read_failure", "media_missing_headers", "media304", "media401"} {
		t.Run(scenario, func(t *testing.T) {
			h := migrationMCPReadNewHarness(t, scenario, scenario, "lease", "", nil)
			s, stop := h.connect()
			h.addCall(s, "fanbox_post", `{"post_id":"post-1"}`)
			var r struct {
				Structured struct {
					Assets []struct {
						Resource struct {
							Ref string `json:"ref"`
						} `json:"resource"`
					} `json:"assets"`
				} `json:"structuredContent"`
			}
			if e := json.Unmarshal(h.row.Calls[0].Result, &r); e != nil {
				t.Fatal(e)
			}
			if len(r.Structured.Assets) != 2 {
				t.Fatalf("assets=%s", h.row.Calls[0].Result)
			}
			for _, a := range r.Structured.Assets {
				h.addCall(s, "fanbox_open_resource", string(migrationMCPReadJSON(t, map[string]any{"ref": a.Resource.Ref})))
			}
			h.addCall(s, "fanbox_creator", `{"creator_id":"writer"}`)
			var creator struct {
				Structured struct {
					Icon, Cover struct {
						Ref string `json:"ref"`
					}
				} `json:"structuredContent"`
			}
			if e := json.Unmarshal(h.row.Calls[3].Result, &creator); e != nil {
				t.Fatal(e)
			}
			for _, ref := range []string{creator.Structured.Icon.Ref, creator.Structured.Cover.Ref} {
				h.addCall(s, "fanbox_open_resource", string(migrationMCPReadJSON(t, map[string]any{"ref": ref})))
			}
			stop()
			fixture.Cases = append(fixture.Cases, h.finish())
		})
	}
	ref, e := sdk.NewResourceRef("fanbox", json.RawMessage(`{"k":"post_image","c":"writer","p":"post-1","a":"image-1"}`))
	if e != nil {
		t.Fatal(e)
	}
	foreign, e := sdk.NewResourceRef("pixiv", json.RawMessage(`{"k":"post_image"}`))
	if e != nil {
		t.Fatal(e)
	}
	for _, entry := range []testcase{
		{name: "resource_lowercase_head", tool: "fanbox_open_resource", args: string(migrationMCPReadJSON(t, map[string]any{"ref": ref.String(), "method": "head"}))},
		{name: "resource_invalid_method", tool: "fanbox_open_resource", args: string(migrationMCPReadJSON(t, map[string]any{"ref": ref.String(), "method": "POST"}))},
		{name: "resource_method_spaces", tool: "fanbox_open_resource", args: string(migrationMCPReadJSON(t, map[string]any{"ref": ref.String(), "method": " GET "}))},
		{name: "resource_foreign_ref", tool: "fanbox_open_resource", args: string(migrationMCPReadJSON(t, map[string]any{"ref": foreign.String()}))},
		{name: "resource_transport_failure", tool: "fanbox_open_resource", args: string(migrationMCPReadJSON(t, map[string]any{"ref": ref.String()})), scenario: "transport_error"},
		{name: "resource_metadata_failure", tool: "fanbox_open_resource", args: string(migrationMCPReadJSON(t, map[string]any{"ref": ref.String()})), scenario: "malformed"},
	} {
		if entry.scenario == "" {
			entry.scenario = "success"
		}
		t.Run(entry.name, func(t *testing.T) {
			h := migrationMCPReadNewHarness(t, entry.name, entry.scenario, "lease", "", nil)
			s, stop := h.connect()
			h.addCall(s, entry.tool, entry.args)
			stop()
			fixture.Cases = append(fixture.Cases, h.finish())
		})
	}
	t.Run("selected_account_changed_between_tools", func(t *testing.T) {
		proxy := ""
		h := migrationMCPReadNewHarness(t, "selected_account_changed_between_tools", "success", "lease", "", &proxy)
		s, stop := h.connect()
		h.addCall(s, "fanbox_current_user", `{}`)
		if e := os.WriteFile(h.files.path, []byte("[fanbox.auth]\ndefault_user_id = 9\n"), 0600); e != nil {
			t.Fatal(e)
		}
		h.trace("owned.config.select/9")
		h.addCall(s, "fanbox_current_user", `{}`)
		h.addCall(s, "fanbox_resolve_url", `{"url":"https://writer.fanbox.cc"}`)
		stop()
		fixture.Cases = append(fixture.Cases, h.finish())
	})
	t.Run("cancel_reuse", func(t *testing.T) {
		h := migrationMCPReadNewHarness(t, "cancel_reuse", "cancel_reuse", "lease", "", nil)
		s, stop := h.connect()
		ctx, cancel := context.WithCancel(context.Background())
		result := make(chan migrationMCPReadCall, 1)
		go func() { result <- h.call(ctx, s, "fanbox_home", `{}`) }()
		migrationMCPReadAwait(t, h.started)
		cancel()
		select {
		case c := <-result:
			h.row.Calls = append(h.row.Calls, c)
		case <-time.After(5 * time.Second):
			t.Fatal("cancel caller stuck")
		}
		migrationMCPReadAwait(t, h.completed)
		h.addCall(s, "fanbox_home", `{}`)
		stop()
		fixture.Cases = append(fixture.Cases, h.finish())
	})
	t.Run("independent_accounts", func(t *testing.T) {
		h := migrationMCPReadNewHarness(t, "independent_accounts", "independent_accounts", "lease", "", nil)
		s, stop := h.connect()
		result := make(chan migrationMCPReadCall, 2)
		go func() { result <- h.call(context.Background(), s, "fanbox_current_user", `{}`) }()
		migrationMCPReadAwait(t, h.started)
		if e := os.WriteFile(h.files.path, []byte("[fanbox.auth]\ndefault_user_id = 9\n"), 0600); e != nil {
			t.Fatal(e)
		}
		h.trace("owned.config.select/9")
		go func() { result <- h.call(context.Background(), s, "fanbox_current_user", `{}`) }()
		migrationMCPReadAwait(t, h.started)
		close(h.releaseFirst)
		migrationMCPReadAwait(t, h.completed)
		close(h.releaseSecond)
		migrationMCPReadAwait(t, h.completed)
		for i := 0; i < 2; i++ {
			select {
			case c := <-result:
				h.row.Calls = append(h.row.Calls, c)
			case <-time.After(5 * time.Second):
				t.Fatal("independent caller stuck")
			}
		}
		sort.Slice(h.row.Calls, func(i, j int) bool { return string(h.row.Calls[i].Result) < string(h.row.Calls[j].Result) })
		stop()
		row := h.finish()
		if row.MaxLiveLeases != 2 {
			t.Fatalf("concurrency=%d", row.MaxLiveLeases)
		}
		fixture.Cases = append(fixture.Cases, row)
	})
	fixture.Stdio = migrationMCPReadStdio(t)
	path := filepath.Join("../../..", "crates/pixiv-mcp/tests/fixtures/fanbox-read-tools.json")
	actual := migrationMCPReadJSON(t, fixture)
	if *migrationCaptureFanboxReadTools {
		var pretty bytes.Buffer
		if e := json.Indent(&pretty, actual, "", "  "); e != nil {
			t.Fatal(e)
		}
		pretty.WriteByte('\n')
		if e := os.WriteFile(path, pretty.Bytes(), 0644); e != nil {
			t.Fatal(e)
		}
		t.Logf("captured %d cases, %d tools, %d stdio rows", len(fixture.Cases), len(list.Tools), len(fixture.Stdio))
		return
	}
	expected, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	var want, got any
	if e = json.Unmarshal(expected, &want); e != nil {
		t.Fatal(e)
	}
	if e = json.Unmarshal(actual, &got); e != nil {
		t.Fatal(e)
	}
	if !reflect.DeepEqual(want, got) {
		b, _ := json.MarshalIndent(fixture, "", "  ")
		out := filepath.Join(os.TempDir(), "fanbox-mcp-read-tools", "actual.json")
		_ = os.WriteFile(out, b, 0600)
		t.Fatalf("genuine Go MCP fixture mismatch; observation %s", out)
	}
}
func migrationMCPReadAwait(t *testing.T, c <-chan struct{}) {
	t.Helper()
	select {
	case <-c:
	case <-time.After(5 * time.Second):
		t.Fatal("owned schedule timed out")
	}
}

func TestMigrationFanboxReadToolsStdioChild(t *testing.T) {
	mode := os.Getenv("PIXIV_FANBOX_READ_STDIO_CHILD")
	if mode == "" {
		return
	}
	scenario := "success"
	if mode == "cancel_reuse" {
		scenario = "cancel_reuse"
	}
	if mode == "eof_pending" {
		scenario = "eof_pending"
	}
	h := migrationMCPReadNewHarness(t, "stdio_"+mode, scenario, "lease", "", nil)
	ctx, cancel := context.WithCancel(diagnostics.WithScope(context.Background(), diagnostics.SinkFunc(h.event), diagnostics.ModuleFanboxCLI, 0))
	defer cancel()
	data := fanboxcommands.Data{Reader: os.Stdin, Writer: os.Stdout, WrapUsage: func(e error) error { return e }, ServiceFactory: func() (*facade.Facade, error) { h.trace("command.service"); return h.facade, nil }, RunMCPServer: func(cmd *cobra.Command, _ *facade.Facade, proxy *string) error {
		h.trace("command.run_stdio/proxy=" + migrationMCPReadProxy(proxy))
		return stdio.RunStdio(cmd.Context(), h.server())
	}}
	leaf := fanboxmcpcommand.New(data)
	cmd := fanboxcommands.New(data, fanboxcommands.CommandSet{MCP: leaf})
	cmd.SetArgs([]string{"mcp"})
	cmd.SetIn(os.Stdin)
	cmd.SetOut(os.Stdout)
	cmd.SetErr(os.Stderr)
	cmd.SetContext(ctx)
	e := cmd.Execute()
	commandError := ""
	if e != nil {
		commandError = e.Error()
	}
	row := h.finish()
	report := map[string]any{"command_error": commandError, "observation": row}
	path := os.Getenv("PIXIV_FANBOX_READ_STDIO_REPORT")
	if e := os.WriteFile(path, migrationMCPReadJSON(t, report), 0600); e != nil {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(4)
	}
	if e := os.Stdout.Close(); e != nil && !errors.Is(e, os.ErrClosed) {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(5)
	}
}
func migrationMCPReadStdio(t *testing.T) []map[string]any {
	t.Helper()
	rows := []map[string]any{}
	for _, mode := range []string{"normal_eof", "cancel_reuse", "eof_pending"} {
		t.Run("stdio_"+mode, func(t *testing.T) {
			exe, e := os.Executable()
			if e != nil {
				t.Fatal(e)
			}
			dir := t.TempDir()
			report := filepath.Join(dir, "report.json")
			marker := filepath.Join(dir, "request.started")
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, exe, "-test.run=^TestMigrationFanboxReadToolsStdioChild$")
			cmd.Env = append(os.Environ(), "PIXIV_FANBOX_READ_STDIO_CHILD="+mode, "PIXIV_FANBOX_READ_STDIO_REPORT="+report, "PIXIV_FANBOX_READ_STDIO_MARKER="+marker)
			input, e := cmd.StdinPipe()
			if e != nil {
				t.Fatal(e)
			}
			output, e := cmd.StdoutPipe()
			if e != nil {
				t.Fatal(e)
			}
			var stderr bytes.Buffer
			cmd.Stderr = &stderr
			if e = cmd.Start(); e != nil {
				_ = input.Close()
				_ = output.Close()
				t.Fatal(e)
			}
			waited := false
			defer func() {
				_ = input.Close()
				if !waited {
					cancel()
					_ = output.Close()
					_ = cmd.Wait()
					waited = true
				} else {
					_ = output.Close()
				}
			}()
			scanner := bufio.NewScanner(output)
			scanner.Buffer(make([]byte, 4096), 2*1024*1024)
			frames := []json.RawMessage{}
			sent := []json.RawMessage{}
			send := func(line string) {
				sent = append(sent, json.RawMessage(line))
				if _, e := io.WriteString(input, line+"\n"); e != nil {
					t.Fatal(e)
				}
			}
			receive := func() {
				if !scanner.Scan() {
					t.Fatalf("stdio frame missing: %v", scanner.Err())
				}
				b := append([]byte(nil), scanner.Bytes()...)
				if !json.Valid(b) {
					t.Fatalf("nonprotocol stdout: %s", b)
				}
				frames = append(frames, json.RawMessage(b))
			}
			send(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"owned-stdio","version":"1"}}}`)
			receive()
			send(`{"jsonrpc":"2.0","method":"notifications/initialized"}`)
			if mode == "normal_eof" {
				send(`{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}`)
				receive()
				send(`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fanbox_post","arguments":{"post_id":"post-1"}}}`)
				receive()
				send(`{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"fanbox_post","arguments":{"post_id":"post-1","extra":true}}}`)
				receive()
				send(`{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"fanbox_resolve_url","arguments":{"url":"https://writer.fanbox.cc"}}}`)
				receive()
			} else {
				send(`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fanbox_home","arguments":{}}}`)
				deadline := time.Now().Add(5 * time.Second)
				for {
					if _, e := os.Stat(marker); e == nil {
						break
					}
					if time.Now().After(deadline) {
						_ = input.Close()
						t.Fatal("owned stdio transport did not reach SDK")
					}
					time.Sleep(time.Millisecond)
				}
				if mode == "cancel_reuse" {
					send(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"owned cancellation"}}`)
					receive()
					send(`{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fanbox_home","arguments":{}}}`)
					receive()
				}
			}
			_ = input.Close()
			for scanner.Scan() {
				b := append([]byte(nil), scanner.Bytes()...)
				if !json.Valid(b) {
					t.Fatalf("nonprotocol EOF stdout: %s", b)
				}
				frames = append(frames, json.RawMessage(b))
			}
			if e := scanner.Err(); e != nil {
				t.Fatal(e)
			}
			e = cmd.Wait()
			waited = true
			exit := 0
			if e != nil {
				var ee *exec.ExitError
				if errors.As(e, &ee) {
					exit = ee.ExitCode()
				} else {
					t.Fatal(e)
				}
			}
			if ctx.Err() != nil {
				t.Fatalf("owned stdio child timed out: %v", ctx.Err())
			}
			if exit != 0 {
				t.Fatalf("owned stdio child exit=%d stderr=%s", exit, stderr.String())
			}
			b, e := os.ReadFile(report)
			if e != nil {
				t.Fatal(e)
			}
			var observation any
			if e = json.Unmarshal(b, &observation); e != nil {
				t.Fatal(e)
			}
			rows = append(rows, map[string]any{"name": mode, "sent": sent, "received": frames, "exit_code": exit, "stderr": stderr.String(), "report": observation})
		})
	}
	return rows
}

var migrationMCPReadSources = map[string]string{
	"sdk/cursor.go":                          "23da658da9f659711c62fed9df917a76906629c9f5dc2424acb5ef09a579f1ca",
	"sdk/error.go":                           "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
	"go.mod":                                 "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
	"go.sum":                                 "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
	"internal/cli/commands/fanbox/fanbox.go": "31d3642468868be03c1bd2b8c7ee02f96c9dcdf461d8f573c03e511a8a4e5468",
	"internal/cli/commands/fanbox/mcp/mcp.go":                         "afce033c7a6c10acf21319a73a7821ed863c8d416dade3a68a420c8bf3a42f63",
	"internal/cli/composition.go":                                     "61606c01971156607d6e632cbf417a63c8e7f02b4c673172ee3db9392dc0f9fc",
	"internal/config/settings/auth.go":                                "78729a90d7a71fe5793f469be5770d1acb565ac2945bf252b893dfcec392981e",
	"internal/config/settings/store.go":                               "c5b418cd50e17dce27d4e0f5f499e4d293e0527e07d335e80da9a5fe2711feec",
	"internal/mcpserver/fanbox/fanbox.go":                             "2408ff85a5307821b1b9e55efa6bd7f5082018625e1569c828690cdf1dedaa59",
	"internal/mcpserver/fanbox/internal/runtime/runtime.go":           "49b2df1d5f1e4d32638af07b73c40a013251f57a4a265c253d76552640bc63cc",
	"internal/mcpserver/fanbox/tools/creator/creator.go":              "084a14b5cd6b31dccc1ee1dce80a0001c3228585398a34f5828ffc991adba691",
	"internal/mcpserver/fanbox/tools/creatorposts/creatorposts.go":    "66884c29d20e4fd15847a74a8b6f4d2e18c6bff27df48f0257d87b08c813816e",
	"internal/mcpserver/fanbox/tools/creators/creators.go":            "d8ddbd2635122159f3f2316502981deab6e7c491ea20befd08c2a418d9c6a276",
	"internal/mcpserver/fanbox/tools/creatortags/creatortags.go":      "f3fc20a606e7d3cf346d8ead694005282229d1efe9864dea9f6f564483ffb26e",
	"internal/mcpserver/fanbox/tools/currentuser/currentuser.go":      "815bae96e54295337913df3078414444e587f64ee632670466da95453eef7d51",
	"internal/mcpserver/fanbox/tools/home/home.go":                    "8d6f29a303625fc13d50b8d3b653071130444a976055cb9d361696e831fa906b",
	"internal/mcpserver/fanbox/tools/openresource/openresource.go":    "eb33ee2809f6e7c330ba709d1592a0929e900ef9cd89c54b2ae47057c2c1e13c",
	"internal/mcpserver/fanbox/tools/post/post.go":                    "3eea3d2ce2bb9b78f7566da44e14f9e8a6e6f680510bffac166ab278891908a6",
	"internal/mcpserver/fanbox/tools/resolveurl/resolveurl.go":        "ba1b51c6ff2ed1eff7997a3e8413535cf53d8dd2c6087a8b748f424ad920982c",
	"internal/mcpserver/fanbox/tools/supporting/supporting.go":        "84ed302e2a26ba687ce428ebbc6d55b79388c0edcff49b0583f3ded99de36ecd",
	"internal/mcpserver/fanbox/tools/taggedposts/taggedposts.go":      "901f2fd9f8cef08e1c9f326c78de135fd5ebe89a50966e5ea479a9f089b905ec",
	"internal/mcpserver/stdio.go":                                     "c48f34ffdcc0f92eadea6766aeb331ceab9899d982dcb59565900aa4adaeb46e",
	"internal/services/fanbox/account/accounts.go":                    "261ae6e5339086f205d7662e435ac664e626d8800cd9937789867bfc82b473bc",
	"internal/services/fanbox/endpoint/creator/creator.go":            "4c3d0eb263ed83952020fafad34f3d9e549f64635637cb0120b67943741ff2b1",
	"internal/services/fanbox/endpoint/creator/creators/creators.go":  "34208cfd9b3692a036405261e3f74a63b3f0d7086d73cba5e93b0fa68827042c",
	"internal/services/fanbox/endpoint/creator/tags/tags.go":          "db5a2ce05d9818bc4d055d852c3cc5cfafde73b15ff9a1006487c163640a945c",
	"internal/services/fanbox/endpoint/post/home/home.go":             "8383a06d3395dd1324eadf59e9ab9a2ed31a02794fae426aa3a6e196fe370c46",
	"internal/services/fanbox/endpoint/post/info/info.go":             "261a376310ac0286526815ae5b02c9b32f932c65c7474f284cc0688386faa5dd",
	"internal/services/fanbox/endpoint/post/post.go":                  "54624ebc7ea54392664e41c7c510b530f15c7969a5292a973b58deb177bd45eb",
	"internal/services/fanbox/endpoint/post/posts/posts.go":           "3e405c604934c91d2a235a17ea3ca2421914ae80994852fa34eceaef0480417a",
	"internal/services/fanbox/endpoint/post/supporting/supporting.go": "ccdc0ef6d59cf99f871a4e610307b5677fb6b6a89d3394bb9e73040d19670fef",
	"internal/services/fanbox/endpoint/post/wire/wire.go":             "30e4ea2119269d052ec6d31447c2cfef59a992b186c652e631d1c488b2df51aa",
	"internal/services/fanbox/fanbox.go":                              "694071130c0a6ad5c1ae174df2dead5ca6d929f9a8826e09c1ab86b5fecfe9e7",
	"internal/services/fanbox/protocol/protocol.go":                   "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3",
	"internal/services/fanbox/resource/resource.go":                   "d8f6e9373033ef93edf4b0c3c9e1bf917892433dc5dbdd2dde6cc67fafcd5be3",
	"internal/shared/diagnostics/diagnostics.go":                      "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76",
	"internal/shared/lifecycle/lease.go":                              "9715a8f3592aa0269b6f85b1a52751f2e0f2947e34fb133836fb1ef07172b381",
	"internal/storage/database/repository.go":                         "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
	"sdk/fanbox/cursor.go":                                            "4014f8d1bdfe3363d3cb94fe2df9eb15f296e3e3489ed27420dda8b7e6684797",
	"sdk/fanbox/doc.go":                                               "e81fec5b31f6993bfc7e1c28023a90682cd583e07f097630bfef8290c5d5d652",
	"sdk/fanbox/dto.go":                                               "860f6d3f18d083526681cf86112dbbdb7faa382e1f9cac3e35b543f56c2df505",
	"sdk/fanbox/errors.go":                                            "523577a066e3d53ce9eced8c7afbe29f422a75ce6aca58bc3b5b6fbf8da59909",
	"sdk/fanbox/fanbox.go":                                            "2208576144b94b89efd57b6f054ee018268812d52d556fd0758e2ee322577542",
	"sdk/fanbox/models.go":                                            "1dd3928aa6752c51f114c678eef3b0a2b4784a275ec80fcdbbd61006407e4623",
	"sdk/fanbox/ops.go":                                               "868b3b68de07d7638af5f9dd3ab1be8f0c9751f222c05c7776cde092052a7c8d",
	"sdk/fanbox/reference.go":                                         "0e0929ca8091097f627f33c28c3042676d4773d2ecf7d8c5f2b482c521951568",
	"sdk/fanbox/request.go":                                           "deea7897788f1013297fc3cf03a7b71ff0d893335d6434d1908feb2245ece263",
	"sdk/fanbox/resource.go":                                          "f4d2f8fd3f9787aecbba5fc026179d1f90483cd94cddd0421e95f05be631c8f1",
	"sdk/ref.go":                                                      "9c7a7ff2dc8a91904d2a343268f4445a094ed7bb7c93c17971908280d4efaf8e",
	"sdk/resource.go":                                                 "0ce748e4141e33c9fd9dfdfaa8d5a0dbd3176146718eb398fc40c970e375e446",
}

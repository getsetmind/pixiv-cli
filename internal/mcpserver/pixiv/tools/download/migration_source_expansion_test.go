package download

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
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	goruntime "runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver"
	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/ugoira/staticlib"
	pixivservice "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateSourceExpansion = flag.Bool("migration-update-download-source-expansion", false, "capture frozen Go MCP user/bookmark download expansion")

type sourceExpansionCase struct {
	ugoiraWorkflowCase
	DefaultID    int64                         `json:"default_user_id"`
	AccountID    int64                         `json:"account_override"`
	ConfigBefore string                        `json:"config_before"`
	ConfigAfter  string                        `json:"config_after"`
	Responses    []sourceExpansionHTTPResponse `json:"responses"`
}

type sourceExpansionHTTPResponse struct {
	URL        string `json:"url"`
	Status     int    `json:"status"`
	RetryAfter string `json:"retry_after"`
	Error      string `json:"transport_error"`
}

type sourceExpansionReply struct {
	Status     int             `json:"status"`
	RetryAfter string          `json:"retry_after,omitempty"`
	Error      string          `json:"transport_error,omitempty"`
	Body       json.RawMessage `json:"body"`
}

type sourceExpansionStore struct{ path string }

func (s sourceExpansionStore) Path() (string, error)              { return s.path, nil }
func (sourceExpansionStore) ReadFile(path string) ([]byte, error) { return os.ReadFile(path) }
func (sourceExpansionStore) WritePrivateFile(path string, body []byte) error {
	return os.WriteFile(path, body, 0600)
}
func (sourceExpansionStore) EnsurePrivateFile(path string, body []byte) error {
	if _, err := os.Stat(path); err == nil {
		return nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	return os.WriteFile(path, body, 0600)
}

type sourceExpansionGate struct {
	inner *pool.Gate
	mu    *sync.Mutex
	row   *sourceExpansionCase
}

func (g sourceExpansionGate) Acquire(ctx context.Context) error {
	if err := g.inner.Acquire(ctx); err != nil {
		return err
	}
	g.mu.Lock()
	g.row.Acquired++
	g.mu.Unlock()
	return nil
}
func (g sourceExpansionGate) Release() {
	g.mu.Lock()
	g.row.Released++
	g.mu.Unlock()
	g.inner.Release()
}

func sourceExpansionReplies() map[string]sourceExpansionReply {
	replies := map[string]sourceExpansionReply{}
	list := func(path string, query url.Values, ids []int64, next string) {
		items := []any{}
		for _, id := range ids {
			items = append(items, map[string]any{"id": id, "title": "List title must not be saved", "type": "synthetic_list_kind", "page_count": 99, "create_date": "2026-10-08T12:34:56+09:00", "user": map[string]any{"id": 99, "name": "List author must not be saved"}, "meta_single_page": map[string]any{"original_image_url": "https://i.pximg.net/list-must-not-be-used.png"}})
		}
		var nextValue any
		if next != "" {
			nextValue = next
		}
		body, _ := json.Marshal(map[string]any{"illusts": items, "next_url": nextValue})
		replies[path+"?"+query.Encode()] = sourceExpansionReply{Status: 200, Body: body}
	}
	user := func(id int64, kind string, offset int, ids []int64, next int) {
		query := url.Values{"user_id": {fmt.Sprint(id)}, "type": {kind}}
		if offset > 0 {
			query.Set("offset", fmt.Sprint(offset))
		}
		nextURL := ""
		if next > 0 {
			q := url.Values{"user_id": {fmt.Sprint(id)}, "type": {kind}, "offset": {fmt.Sprint(next)}}
			nextURL = "https://app-api.pixiv.net/v1/user/illusts?" + q.Encode()
		}
		list("/v1/user/illusts", query, ids, nextURL)
	}
	bookmark := func(id int64, max int, ids []int64, next int) {
		query := url.Values{"user_id": {fmt.Sprint(id)}, "restrict": {"public"}}
		if max > 0 {
			query.Set("max_bookmark_id", fmt.Sprint(max))
		}
		nextURL := ""
		if next > 0 {
			q := url.Values{"user_id": {fmt.Sprint(id)}, "restrict": {"public"}, "max_bookmark_id": {fmt.Sprint(next)}}
			nextURL = "https://app-api.pixiv.net/v1/user/bookmarks/illust?" + q.Encode()
		}
		list("/v1/user/bookmarks/illust", query, ids, nextURL)
	}
	for _, id := range []int64{7, 8, 9, 10, 11, 12, 14, 15, 16, 17, 18} {
		for _, kind := range []string{"illust", "manga", "ugoira"} {
			user(id, kind, 0, nil, 0)
		}
	}
	user(7, "illust", 0, []int64{44, 42, 44}, 3)
	user(7, "illust", 3, []int64{42}, 0)
	user(7, "manga", 0, nil, 3)
	user(7, "manga", 3, []int64{44}, 0)
	user(7, "ugoira", 0, []int64{71, 42}, 3)
	user(7, "ugoira", 3, []int64{71}, 0)
	bookmark(7, 0, nil, 90)
	bookmark(7, 90, []int64{71, 44, 42, 44}, 80)
	bookmark(7, 80, []int64{42}, 0)
	user(8, "illust", 0, []int64{42}, 3)
	user(8, "manga", 0, []int64{44}, 0)
	user(8, "ugoira", 0, []int64{71}, 0)
	failed := sourceExpansionReply{Status: 404, Body: json.RawMessage(`{"error":{"message":"owned fixture list failure"}}`)}
	replies["/v1/user/illusts?offset=3&type=illust&user_id=8"] = failed
	for _, kind := range []string{"illust", "manga", "ugoira"} {
		replies["/v1/user/illusts?type="+kind+"&user_id=9"] = failed
	}
	rate := sourceExpansionReply{Status: 429, RetryAfter: "120", Body: json.RawMessage(`{"error":{"message":"owned fixture rate failure"}}`)}
	replies["/v1/user/illusts?type=illust&user_id=10"] = rate
	replies["/v1/user/illusts?type=illust&user_id=11"] = sourceExpansionReply{Status: 200, Error: "context canceled", Body: json.RawMessage(`{}`)}
	user(11, "manga", 0, []int64{42}, 0)
	user(12, "illust", 0, nil, 3)
	user(12, "illust", 3, []int64{42}, 6)
	user(12, "illust", 6, nil, 0)
	user(14, "illust", 0, []int64{42}, 3)
	user(14, "illust", 3, []int64{44}, 0)
	bookmark(14, 0, []int64{42}, 90)
	bookmark(14, 90, []int64{44}, 0)
	user(15, "manga", 0, []int64{44}, 0)
	user(16, "illust", 0, []int64{45, 42}, 0)
	user(17, "manga", 0, []int64{46}, 0)
	user(18, "illust", 0, []int64{42}, 3)
	user(18, "illust", 3, []int64{0}, 0)
	bookmark(8, 0, []int64{42}, 90)
	replies["/v1/user/bookmarks/illust?max_bookmark_id=90&restrict=public&user_id=8"] = failed
	replies["/v1/user/bookmarks/illust?restrict=public&user_id=9"] = rate
	return replies
}

func sourceExpansionSetup(t *testing.T, row *sourceExpansionCase, root string, pending bool) (*runtime.App, func()) {
	t.Helper()
	dest := filepath.Join(root, "downloads")
	if err := os.MkdirAll(dest, 0700); err != nil {
		t.Fatal(err)
	}
	row.Responses = []sourceExpansionHTTPResponse{}
	row.ManagerRequests = []staticArtworkRequest{}
	row.Requests = []directWireRequest{}
	row.Files = []directFile{}
	row.Directories = []string{}
	row.Opens = []int64{}
	row.Batches = []json.RawMessage{}
	row.States = []leaseAccountState{}
	row.InitialFiles = []directFile{}
	if row.DefaultID == 0 {
		row.DefaultID = 43
	}
	configPath := filepath.Join(root, "config.toml")
	config := fmt.Sprintf("# preserve fixture comment\n[download]\npath=%q\nfilename_template=%q\ndirectory_template=%q\n[account_pool]\nenabled=true\nstrategy='round_robin'\n[pixiv.auth]\ndefault_user_id=%d\n[unrelated]\nkeep='untouched'\n", dest, row.Filename, row.Directory, row.DefaultID)
	if err := os.WriteFile(configPath, []byte(config), 0600); err != nil {
		t.Fatal(err)
	}
	row.ConfigBefore = strings.ReplaceAll(config, dest, "<ROOT>")
	store := settings.Store{Files: sourceExpansionStore{configPath}}
	snapshot, err := store.Current()
	if err != nil {
		t.Fatal(err)
	}
	configuration, err := snapshot.Runtime()
	if err != nil {
		t.Fatal(err)
	}
	db, err := database.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []int64{42, 43} {
		if err = db.SavePixivCredential(context.Background(), account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
			t.Fatal(err)
		}
	}
	if err = db.SetAllPixivSchedulable(context.Background(), true); err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	replies := sourceExpansionReplies()
	rateCounts := map[string]int{}
	httpClient := &http.Client{Transport: directTransport(func(r *http.Request) (*http.Response, error) {
		mu.Lock()
		row.Requests = append(row.Requests, directWireRequest{r.URL.String(), r.Method, r.Header.Get("Referer"), r.Header.Get("Authorization"), r.Header.Get("Cookie")})
		mu.Unlock()
		status := 200
		header := http.Header{"Content-Type": {"application/json"}}
		var body io.ReadCloser
		switch r.URL.Host {
		case "oauth.secure.pixiv.net":
			raw, _ := io.ReadAll(r.Body)
			form, _ := url.ParseQuery(string(raw))
			id, _ := strconv.ParseInt(strings.TrimPrefix(strings.TrimPrefix(form.Get("refresh_token"), "fixture-refresh-"), "fixture-rotated-"), 10, 64)
			mu.Lock()
			row.Opens = append(row.Opens, id)
			mu.Unlock()
			raw = []byte(fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id))
			if row.Name == "failed-refresh-no-source-read" {
				status = 429
				header.Set("Retry-After", "120")
				raw = []byte(`{"error":{"message":"owned fixture refresh failed"}}`)
			}
			body = io.NopCloser(bytes.NewReader(raw))
		case "app-api.pixiv.net":
			switch r.URL.Path {
			case "/v1/user/illusts", "/v1/user/bookmarks/illust":
				reply, ok := replies[r.URL.Path+"?"+r.URL.RawQuery]
				if !ok {
					return nil, fmt.Errorf("unexpected list query %s", r.URL.String())
				}
				if reply.Error != "" {
					mu.Lock()
					row.Responses = append(row.Responses, sourceExpansionHTTPResponse{r.URL.String(), 0, "", "context canceled"})
					mu.Unlock()
					return nil, context.Canceled
				}
				status = reply.Status
				if reply.RetryAfter != "" {
					rateCounts[r.URL.String()]++
					if rateCounts[r.URL.String()]%2 == 1 {
						header.Set("Retry-After", "0")
					} else {
						header.Set("Retry-After", reply.RetryAfter)
					}
				}
				body = io.NopCloser(bytes.NewReader(reply.Body))
				if pending && r.URL.Query().Get("user_id") == "14" && (r.URL.Query().Get("offset") == "3" || r.URL.Query().Get("max_bookmark_id") == "90") {
					body = &sourceExpansionPendingBody{ctx: r.Context(), root: dest}
				}
			case "/v1/illust/detail":
				id, _ := strconv.ParseInt(r.URL.Query().Get("illust_id"), 10, 64)
				raw := staticArtworkMetadata(id)
				if id == 71 {
					raw = ugoiraWorkflowArtwork(id)
				}
				if id == 45 {
					status = 404
					raw = []byte(`{"error":{"message":"fixture missing artwork"}}`)
				}
				body = io.NopCloser(bytes.NewReader(raw))
			case "/v1/ugoira/metadata":
				id, _ := strconv.ParseInt(r.URL.Query().Get("illust_id"), 10, 64)
				body = io.NopCloser(bytes.NewReader(ugoiraWorkflowMetadata(id)))
			default:
				return nil, fmt.Errorf("unexpected API operation %s", r.URL.Path)
			}
		case "i.pximg.net":
			header.Set("Content-Type", "image/png")
			raw := ugoiraWorkflowPNG(t, 0)
			if strings.HasSuffix(r.URL.Path, ".zip") {
				header.Set("Content-Type", "application/zip")
				raw = ugoiraWorkflowArchive(t, 71)
			}
			if strings.Contains(r.URL.Path, "46_p1") {
				status = 503
				raw = []byte("unavailable")
			}
			if strings.Contains(r.URL.Path, "list-must-not-be-used") {
				return nil, errors.New("list metadata reached media save")
			}
			body = io.NopCloser(bytes.NewReader(raw))
		default:
			return nil, fmt.Errorf("unexpected synthetic host %s", r.URL.Host)
		}
		mu.Lock()
		row.Responses = append(row.Responses, sourceExpansionHTTPResponse{r.URL.String(), status, header.Get("Retry-After"), ""})
		mu.Unlock()
		return &http.Response{StatusCode: status, Header: header, Body: body, Request: r}, nil
	})}
	facade := pixivservice.New(pixivservice.Dependencies{Accounts: account.NewService(db, store), Gate: sourceExpansionGate{pool.NewGate(), &mu, row}, LoadPoolConfig: func() (pixivservice.PoolConfig, error) {
		mu.Lock()
		row.PoolLoads++
		mu.Unlock()
		return pixivservice.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
	}, Pool: func(pixivservice.PoolConfig) (pixivservice.PoolExecutor, error) {
		mu.Lock()
		row.PoolFactories++
		mu.Unlock()
		return nil, errors.New("unexpected pool construction")
	}, CloseClient: func(c *pixiv.Client) error {
		mu.Lock()
		row.Closes++
		mu.Unlock()
		c.CloseIdleConnections()
		return nil
	}})
	configured := downloader.NewManager(nil, configuration.DownloadPath, configuration.FilenameTemplate)
	configured.SetDirectoryTemplate(configuration.DirectoryTemplate)
	app := runtime.NewApp(configured, func(c *pixiv.Client) runtime.DownloadManager {
		manager := downloader.NewManager(c, configuration.DownloadPath, configuration.FilenameTemplate)
		manager.SetDirectoryTemplate(configuration.DirectoryTemplate)
		return &observedUgoiraManager{Manager: manager, row: &row.ugoiraWorkflowCase, root: dest, mu: &mu}
	}, runtime.SDKPorts{OpenLease: func(ctx context.Context, a runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
		return facade.Open(ctx, pixivservice.Request{UserID: a.UserID, Options: pixiv.Options{HTTPClient: httpClient}})
	}, Execute: func(context.Context, runtime.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
		mu.Lock()
		row.ExecuteCalls++
		mu.Unlock()
		return errors.New("unexpected pooled execution")
	}}, runtime.Account{UserID: row.AccountID})
	finish := func() {
		for _, id := range []int64{42, 43} {
			state, e := db.GetPixiv(context.Background(), id)
			if e != nil {
				t.Fatal(e)
			}
			row.States = append(row.States, leaseAccountState{id, state.CredentialRevision, state.PoolFrozenUntil != nil, state.PoolLastSelected})
		}
		row.Files, row.Directories = ugoiraWorkflowDisk(t, dest)
		raw, e := os.ReadFile(configPath)
		if e != nil {
			t.Fatal(e)
		}
		row.ConfigAfter = strings.ReplaceAll(string(raw), dest, "<ROOT>")
		if row.ConfigBefore != row.ConfigAfter {
			t.Fatal("download changed saved configuration")
		}
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}
		sourceExpansionOrder(row)
	}
	return app, finish
}

func sourceExpansionOrder(row *sourceExpansionCase) {
	firstMedia := len(row.Requests)
	for i, request := range row.Requests {
		if request.URL != "https://oauth.secure.pixiv.net/auth/token" && !strings.Contains(request.URL, "/v1/user/") {
			firstMedia = i
			break
		}
	}
	for _, request := range row.Requests[firstMedia:] {
		if strings.Contains(request.URL, "/v1/user/") {
			panic("list request began after media acquisition")
		}
	}
	firstResponse := len(row.Responses)
	for i, response := range row.Responses {
		if response.URL != "https://oauth.secure.pixiv.net/auth/token" && !strings.Contains(response.URL, "/v1/user/") {
			firstResponse = i
			break
		}
	}
	sort.SliceStable(row.Responses[firstResponse:], func(a, b int) bool { return row.Responses[firstResponse+a].URL < row.Responses[firstResponse+b].URL })
	sort.SliceStable(row.Requests[firstMedia:], func(a, b int) bool { return row.Requests[firstMedia+a].URL < row.Requests[firstMedia+b].URL })
}

func captureSourceExpansionCase(t *testing.T, row *sourceExpansionCase, listTool bool) json.RawMessage {
	t.Helper()
	root := t.TempDir()
	app, finish := sourceExpansionSetup(t, row, root, false)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
	Register(app, server)
	ct, st := mcp.NewInMemoryTransports()
	stopped := make(chan struct{})
	go func() { defer close(stopped); _ = server.Run(ctx, st) }()
	conn, err := ct.Connect(ctx)
	if err != nil {
		t.Fatal(err)
	}
	send := func(raw []byte) {
		t.Helper()
		message, e := jsonrpc.DecodeMessage(raw)
		if e != nil {
			t.Fatal(e)
		}
		if e = conn.Write(ctx, message); e != nil {
			t.Fatal(e)
		}
	}
	read := func() json.RawMessage {
		t.Helper()
		message, e := conn.Read(ctx)
		if e != nil {
			t.Fatal(e)
		}
		raw, e := jsonrpc.EncodeMessage(message)
		if e != nil {
			t.Fatal(e)
		}
		return raw
	}
	send([]byte(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`))
	_ = read()
	send([]byte(`{"jsonrpc":"2.0","method":"notifications/initialized"}`))
	var tool json.RawMessage
	if listTool {
		send([]byte(`{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}`))
		var listed struct {
			Result struct {
				Tools []json.RawMessage `json:"tools"`
			} `json:"result"`
		}
		if err = json.Unmarshal(read(), &listed); err != nil {
			t.Fatal(err)
		}
		for _, raw := range listed.Result.Tools {
			var name struct{ Name string }
			_ = json.Unmarshal(raw, &name)
			if name.Name == "download" {
				tool = raw
			}
		}
		if tool == nil {
			t.Fatal("missing registered download")
		}
	}
	raw, err := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": map[string]any{"name": "download", "arguments": row.Arguments}})
	if err != nil {
		t.Fatal(err)
	}
	send(raw)
	row.Response = normalizeDirectJSON(t, read(), filepath.Join(root, "downloads"))
	_ = conn.Close()
	cancel()
	<-stopped
	finish()
	verifySourceExpansionCase(t, row, filepath.Join(root, "downloads"))
	return tool
}

func verifySourceExpansionCase(t *testing.T, row *sourceExpansionCase, root string) {
	t.Helper()
	if row.PoolLoads != 0 || row.PoolFactories != 0 || row.ExecuteCalls != 0 {
		t.Fatalf("download entered pool: %s", row.Name)
	}
	if row.Acquired != row.Released {
		t.Fatalf("lease did not release gate: %s", row.Name)
	}
	if len(row.Opens) > 0 && (len(row.Opens) != 1 || row.Closes != 1 && row.Name != "failed-refresh-no-source-read") {
		t.Fatalf("download did not use one lease: %s", row.Name)
	}
	if row.Name == "failed-refresh-no-source-read" && (len(row.Requests) != 1 || row.Closes != 0) {
		t.Fatal("failed refresh reached list or returned a client")
	}
	if strings.Contains(row.Name, "bookmark-failure") && (len(row.ManagerRequests) != 0 || len(row.Files) != 0) {
		t.Fatalf("fatal bookmark failure acquired queued media: %s", row.Name)
	}
	for _, request := range row.Requests {
		if strings.Contains(request.URL, "i.pximg.net") && (request.Authorization != "" || request.Cookie != "" || request.Referer != "https://app-api.pixiv.net/") {
			t.Fatalf("resource credential boundary: %+v", request)
		}
		if strings.Contains(request.URL, "/v1/user/bookmarks/illust") {
			parsed, _ := url.Parse(request.URL)
			if parsed.Query().Get("restrict") != "public" || parsed.Query().Get("tag") != "" {
				t.Fatal("bookmark query selected private/tagged data")
			}
		}
	}
	var result struct {
		Result struct {
			Content []struct {
				Type string `json:"type"`
			} `json:"content"`
			StructuredContent downloadOut `json:"structuredContent"`
		} `json:"result"`
	}
	if err := json.Unmarshal(row.Response, &result); err != nil {
		t.Fatal(err)
	}
	if want, ok := map[string]int{
		"home-default-all-visual-pages": 5, "localized-artworks-query-fragment": 5, "public-bookmarks-ignore-private-tag": 5,
		"combined-dedup-repeat-users-direct-opaque": 7, "configured-templates-pages": 2, "static-quality-regular": 1, "later-user-failure-retains-prefix": 5,
		"all-user-failures-then-other-source": 1, "retryable-user-failure-no-pool-replay": 1, "client-cancel-lookalike-no-parent-cancel": 1,
		"empty-pages-continue": 1, "list-detail-failure": 1, "list-media-partial-failure": 1, "later-invalid-producer-page-retains-prefix": 1,
		"invalid-options-before-lease": 0, "failed-refresh-no-source-read": 0, "missing-saved-default": 0, "explicit-account-leaves-default-unchanged": 1,
	}[row.Name]; ok && len(row.Files) != want {
		t.Fatalf("connected fixture published %d files, want %d: %s", len(row.Files), want, row.Name)
	}
	if want, ok := map[string]int{
		"home-default-all-visual-pages": 0, "localized-artworks-query-fragment": 0, "public-bookmarks-ignore-private-tag": 0,
		"combined-dedup-repeat-users-direct-opaque": 0, "configured-templates-pages": 2, "static-quality-regular": 0,
		"later-user-failure-retains-prefix": 1, "all-user-failures-then-other-source": 3, "retryable-user-failure-no-pool-replay": 1,
		"client-cancel-lookalike-no-parent-cancel": 1, "first-bookmark-failure-aborts-pid-direct-ref": 0,
		"later-bookmark-failure-retains-user-failures-aborts-media": 1, "earlier-invalid-source-fatal-bookmark-failure": 1,
		"empty-pages-continue": 0, "list-detail-failure": 1, "list-media-partial-failure": 1, "later-invalid-producer-page-retains-prefix": 1,
		"invalid-options-before-lease": 0, "failed-refresh-no-source-read": 0, "missing-saved-default": 0, "explicit-account-leaves-default-unchanged": 0,
	}[row.Name]; ok && len(result.Result.StructuredContent.Failures) != want {
		t.Fatalf("connected fixture failures %d, want %d: %s", len(result.Result.StructuredContent.Failures), want, row.Name)
	}
	if row.Name == "home-default-all-visual-pages" || row.Name == "public-bookmarks-ignore-private-tag" {
		ids := []int64{}
		for _, item := range result.Result.StructuredContent.Items {
			ids = append(ids, item.IllustID)
		}
		a, _ := json.Marshal(ids)
		if string(a) != "[42,44,71]" {
			t.Fatalf("all visual kinds were not detail-refetched and saved: %s", a)
		}
	}
	grouped := []downloadFileOut{}
	for _, item := range result.Result.StructuredContent.Items {
		grouped = append(grouped, item.Files...)
		if item.IllustID > 0 && item.URL != fmt.Sprintf("https://www.pixiv.net/artworks/%d", item.IllustID) {
			t.Fatal("artwork URL not canonical")
		}
		if strings.Contains(item.Title, "List title") || strings.Contains(item.Author, "List author") {
			t.Fatal("list metadata replaced detail")
		}
	}
	files := result.Result.StructuredContent.Files
	if len(files) != len(grouped) || len(files) != len(row.Files) {
		t.Fatalf("grouped/flat/disk files mismatch: %s", row.Name)
	}
	for i, file := range files {
		a, _ := json.Marshal(file)
		b, _ := json.Marshal(grouped[i])
		if !bytes.Equal(a, b) {
			t.Fatal("grouped/flat files differ")
		}
		path := strings.ReplaceAll(file.Path, "<ROOT>", root)
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if file.SizeBytes != info.Size() || file.MIMEType != downloader.MimeTypeForFile(path) || !strings.HasPrefix(file.FileURI, "file://<ROOT>/") {
			t.Fatalf("published local metadata: %+v", file)
		}
	}
	for _, content := range result.Result.Content {
		if content.Type != "text" {
			t.Fatal("nontext download content")
		}
	}
	if strings.Contains(string(row.Response), "fixture-access") || strings.Contains(string(row.Response), "signature=private") {
		t.Fatal("response leaked credential or resource query")
	}
}

func TestMigrationMCPDownloadSourceExpansionMatchesFrozenContract(t *testing.T) {
	if goruntime.GOOS != "linux" || goruntime.GOARCH != "amd64" {
		t.Skip("native default ugoira fixture uses tracked linux/amd64 staticlib; other targets remain pending")
	}
	base := filepath.Join("..", "..", "..", "..", "..")
	hashes := map[string]string{}
	for path, digest := range ugoiraWorkflowHashes {
		hashes[path] = digest
	}
	for path, digest := range map[string]string{
		"internal/shared/pagination/pagination.go":                      "0dfb923e82eb59991f7958b68d547e67efd44b440da1085e7e8191cc8bffe952",
		"internal/config/settings/store.go":                             "c5b418cd50e17dce27d4e0f5f499e4d293e0527e07d335e80da9a5fe2711feec",
		"internal/config/settings/auth.go":                              "78729a90d7a71fe5793f469be5770d1acb565ac2945bf252b893dfcec392981e",
		"internal/services/pixiv/account/accounts.go":                   "129d83a89a09bc8a4dbd926ce19e9551fedf048a4c1fb9f71d551ee3089fad91",
		"internal/services/pixiv/pool/gate.go":                          "33387af051432f6a6e6a8d0a8bbbcc88a5035ef6a65cbd30846f3ea9a70990a6",
		"internal/services/pixiv/endpoint/artwork/timeline/timeline.go": "ae2f702af7e44ab6548772986127522e036f1347929d8bb60821c45e780ce557",
		"internal/services/pixiv/endpoint/artwork/bookmark/bookmark.go": "8ad7e3837413ce05fbfed524f8665e4da157b178253c4c5a681fd47dc73e7725",
	} {
		hashes[path] = digest
	}
	for path, want := range hashes {
		raw, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(raw)) != want {
			t.Fatalf("reference source changed: %s", path)
		}
	}
	digest, err := staticlib.CalculateRustSourceDigest(filepath.Join(base, "internal", "media", "ugoira", "rust"), filepath.Join(base, "third_party", "rust", "quantette-0.6.0"))
	if err != nil || digest != ugoiraSourceDigest {
		t.Fatalf("native source identity: %s, %v", digest, err)
	}
	opaque, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"artwork","id":42,"p":0,"v":"original"}`))
	if err != nil {
		t.Fatal(err)
	}
	makeCase := func(name string, args any) sourceExpansionCase {
		return sourceExpansionCase{ugoiraWorkflowCase: ugoiraWorkflowCase{staticArtworkCase: staticArtworkCase{Name: name, Arguments: args}}}
	}
	home := "https://www.pixiv.net/users/7"
	bookmarks := "https://www.pixiv.net/users/7/bookmarks/artworks?restrict=private&rest=hide&tag=ignored#fragment"
	rows := []sourceExpansionCase{
		makeCase("home-default-all-visual-pages", map[string]any{"src": home}),
		makeCase("localized-artworks-query-fragment", map[string]any{"src": "https://pixiv.net/en/users/7/artworks?tag=ignored#fragment", "ugoira_mode": "zip"}),
		makeCase("public-bookmarks-ignore-private-tag", map[string]any{"src": bookmarks, "ugoira_mode": "raw"}),
		makeCase("combined-dedup-repeat-users-direct-opaque", map[string]any{"srcs": []string{"71", home, "https://www.pixiv.net/artworks/42", home, bookmarks, "https://i.pximg.net/direct.png", opaque.String()}, "ugoira_mode": "zip"}),
		makeCase("configured-templates-pages", map[string]any{"src": home, "pages": "3,1,1", "ugoira_mode": "zip"}),
		makeCase("static-quality-regular", map[string]any{"src": "https://www.pixiv.net/users/15", "pages": "2", "quality": " regular ", "delivery": " local_path "}),
		makeCase("later-user-failure-retains-prefix", map[string]any{"src": "https://www.pixiv.net/en/users/8/artworks?private=1", "ugoira_mode": "zip"}),
		makeCase("all-user-failures-then-other-source", map[string]any{"srcs": []string{"https://www.pixiv.net/users/9", "42"}}),
		makeCase("retryable-user-failure-no-pool-replay", map[string]any{"srcs": []string{"https://www.pixiv.net/users/10", "42"}}),
		makeCase("client-cancel-lookalike-no-parent-cancel", map[string]any{"src": "https://www.pixiv.net/users/11"}),
		makeCase("first-bookmark-failure-aborts-pid-direct-ref", map[string]any{"srcs": []string{"42", "https://i.pximg.net/direct.png", opaque.String(), "https://www.pixiv.net/users/9/bookmarks/artworks"}}),
		makeCase("later-bookmark-failure-retains-user-failures-aborts-media", map[string]any{"srcs": []string{"https://www.pixiv.net/users/8", "42", "https://i.pximg.net/direct.png", opaque.String(), "https://www.pixiv.net/users/8/bookmarks/artworks"}, "ugoira_mode": "zip"}),
		makeCase("earlier-invalid-source-fatal-bookmark-failure", map[string]any{"srcs": []string{"invalid-owned-source", "https://www.pixiv.net/users/9/bookmarks/artworks"}}),
		makeCase("empty-pages-continue", map[string]any{"src": "https://www.pixiv.net/users/12"}),
		makeCase("list-detail-failure", map[string]any{"src": "https://www.pixiv.net/users/16"}),
		makeCase("list-media-partial-failure", map[string]any{"src": "https://www.pixiv.net/users/17"}),
		makeCase("later-invalid-producer-page-retains-prefix", map[string]any{"src": "https://www.pixiv.net/users/18"}),
		makeCase("invalid-options-before-lease", map[string]any{"src": home, "pages": "1,,2"}),
		makeCase("failed-refresh-no-source-read", map[string]any{"src": home}),
		makeCase("missing-saved-default", map[string]any{"src": home}),
		makeCase("explicit-account-leaves-default-unchanged", map[string]any{"src": "https://www.pixiv.net/users/12"}),
	}
	rows[4].Filename = "{author_id}_{id}_{num}_{date}_{tags}"
	rows[4].Directory = "{author}/{date}/{id}/{num}"
	rows[19].DefaultID = 99
	rows[20].AccountID = 42
	var tool json.RawMessage
	for i := range rows {
		t.Run(rows[i].Name, func(t *testing.T) {
			captured := captureSourceExpansionCase(t, &rows[i], i == 0)
			if i == 0 {
				tool = captured
			}
		})
	}
	metadata := map[string]json.RawMessage{}
	for _, id := range []int64{42, 44, 45, 46} {
		metadata[fmt.Sprint(id)] = staticArtworkMetadata(id)
	}
	metadata["71"] = ugoiraWorkflowArtwork(71)
	cancellations := []sourceExpansionCancellation{}
	for _, kind := range []string{"user", "bookmark"} {
		t.Run("stdio-cancel-"+kind, func(t *testing.T) { cancellations = append(cancellations, captureSourceExpansionCancellation(t, kind)) })
	}
	output := struct {
		Source              string                          `json:"source"`
		SourceHashes        map[string]string               `json:"source_hashes"`
		Native              map[string]string               `json:"native_provenance"`
		RequestOrder        string                          `json:"request_order"`
		ObservationBoundary []string                        `json:"observation_boundary"`
		Pending             []string                        `json:"pending"`
		Metadata            map[string]json.RawMessage      `json:"artwork_metadata"`
		UgoiraMetadata      json.RawMessage                 `json:"ugoira_metadata"`
		ArchiveHex          string                          `json:"archive_hex"`
		StaticHex           string                          `json:"static_hex"`
		Replies             map[string]sourceExpansionReply `json:"list_responses"`
		Tool                json.RawMessage                 `json:"tool"`
		Cancellation        []sourceExpansionCancellation   `json:"stdio_cancellation"`
		Cases               []sourceExpansionCase           `json:"cases"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", hashes, map[string]string{"platform": "linux/amd64", "source_digest": digest, "artifact_sha256": ugoiraNativeHash}, "OAuth and sequential list request prefix retained; concurrent detail/metadata/media suffix sorted by URL; media starts only after every source expands", []string{"requests, saved account revisions/freeze/selection, configuration bytes, RPC response, and disk bytes use genuine public SDK/saved boundaries", "manager_requests/manager_batches and close/gate/pool counters are Go internal callback observations; Rust object drop counts are not proof of public SDK idle-close ownership"}, []string{"random recommendation download remains separate", "native other five platforms, SDK injected-client ownership/explicit Context, HTTP2/Accept and broader cancellation schedules remain pending"}, metadata, ugoiraWorkflowMetadata(71), fmt.Sprintf("%x", ugoiraWorkflowArchive(t, 71)), fmt.Sprintf("%x", ugoiraWorkflowPNG(t, 0)), sourceExpansionReplies(), tool, cancellations, rows}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_source_expansion.json")
	if *updateSourceExpansion {
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP source expansion differs from frozen Go contract")
	}
}

type sourceExpansionPendingBody struct {
	ctx  context.Context
	root string
	once sync.Once
}

func (b *sourceExpansionPendingBody) Read([]byte) (int, error) {
	b.once.Do(func() {
		files, temporary := 0, 0
		_ = filepath.WalkDir(b.root, func(_ string, e os.DirEntry, err error) error {
			if err == nil && !e.IsDir() {
				if strings.HasPrefix(e.Name(), ".atomic-write-") {
					temporary++
				} else {
					files++
				}
			}
			return err
		})
		_ = json.NewEncoder(os.Stderr).Encode(map[string]any{"event": "waiting", "files": files, "temporary": temporary})
	})
	<-b.ctx.Done()
	return 0, b.ctx.Err()
}
func (*sourceExpansionPendingBody) Close() error { return nil }

type sourceExpansionCancellation struct {
	Kind                  string              `json:"kind"`
	Arguments             any                 `json:"arguments"`
	Response              json.RawMessage     `json:"response"`
	NextResponse          json.RawMessage     `json:"next_response"`
	FilesBeforeCancel     int                 `json:"files_before_cancel"`
	TemporaryBeforeCancel int                 `json:"temporary_before_cancel"`
	Observed              sourceExpansionCase `json:"observed"`
}

func TestMigrationMCPSourceExpansionCancellationChild(t *testing.T) {
	root := os.Getenv("PIXIV_SOURCE_EXPANSION_CHILD")
	if root == "" {
		return
	}
	row := sourceExpansionCase{ugoiraWorkflowCase: ugoiraWorkflowCase{staticArtworkCase: staticArtworkCase{Name: "stdio-cancel-" + os.Getenv("PIXIV_SOURCE_EXPANSION_KIND")}}}
	app, finish := sourceExpansionSetup(t, &row, root, true)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
	Register(app, server)
	if err := mcpserver.RunStdio(ctx, server); err != nil {
		t.Fatal(err)
	}
	finish()
	_ = json.NewEncoder(os.Stderr).Encode(map[string]any{"event": "completed", "observed": row})
}
func captureSourceExpansionCancellation(t *testing.T, kind string) sourceExpansionCancellation {
	t.Helper()
	root := t.TempDir()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationMCPSourceExpansionCancellationChild$")
	command.Env = append(os.Environ(), "PIXIV_SOURCE_EXPANSION_CHILD="+root, "PIXIV_SOURCE_EXPANSION_KIND="+kind)
	stdin, err := command.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	stdout, err := command.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	stderr, err := command.StderrPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err = command.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = command.Process.Kill() })
	events := make(chan json.RawMessage, 8)
	go func() {
		scanner := bufio.NewScanner(stderr)
		scanner.Buffer(make([]byte, 4096), 1024*1024)
		for scanner.Scan() {
			raw := append([]byte(nil), scanner.Bytes()...)
			var event map[string]any
			if json.Unmarshal(raw, &event) == nil && event["event"] != nil {
				events <- raw
			}
		}
		close(events)
	}()
	write := func(raw string) {
		t.Helper()
		if _, e := io.WriteString(stdin, raw+"\n"); e != nil {
			t.Fatal(e)
		}
	}
	scanner := bufio.NewScanner(stdout)
	scanner.Buffer(make([]byte, 4096), 1024*1024)
	read := func() json.RawMessage {
		t.Helper()
		if !scanner.Scan() {
			t.Fatalf("child stdout ended: %v", scanner.Err())
		}
		raw := append([]byte(nil), scanner.Bytes()...)
		if !json.Valid(raw) {
			t.Fatalf("non-RPC stdout: %q", raw)
		}
		return raw
	}
	write(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`)
	_ = read()
	write(`{"jsonrpc":"2.0","method":"notifications/initialized"}`)
	source := "https://www.pixiv.net/users/14"
	if kind == "bookmark" {
		source += "/bookmarks/artworks?restrict=private&tag=ignored"
	}
	row := sourceExpansionCancellation{Kind: kind, Arguments: map[string]any{"srcs": []string{"https://www.pixiv.net/users/9", "42", "https://i.pximg.net/direct.png", source}, "ugoira_mode": "zip"}}
	raw, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": map[string]any{"name": "download", "arguments": row.Arguments}})
	write(string(raw))
	select {
	case raw := <-events:
		var event struct {
			Event     string `json:"event"`
			Files     int    `json:"files"`
			Temporary int    `json:"temporary"`
		}
		if err = json.Unmarshal(raw, &event); err != nil || event.Event != "waiting" {
			t.Fatalf("list-read cancellation event: %s, %v", raw, err)
		}
		row.FilesBeforeCancel = event.Files
		row.TemporaryBeforeCancel = event.Temporary
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	if row.FilesBeforeCancel != 0 || row.TemporaryBeforeCancel != 0 {
		t.Fatal("media acquired before expansion completed")
	}
	write(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture"}}`)
	row.Response = normalizeDirectJSON(t, read(), filepath.Join(root, "downloads"))
	write(`{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"download","arguments":{"src":"42"}}}`)
	row.NextResponse = normalizeDirectJSON(t, read(), filepath.Join(root, "downloads"))
	_ = stdin.Close()
	completed := false
	for !completed {
		var raw json.RawMessage
		select {
		case message, ok := <-events:
			if !ok {
				t.Fatal("child stderr ended before completion")
			}
			raw = message
		case <-ctx.Done():
			t.Fatal(ctx.Err())
		}
		var event struct {
			Event    string              `json:"event"`
			Observed sourceExpansionCase `json:"observed"`
		}
		if err = json.Unmarshal(raw, &event); err != nil {
			t.Fatal(err)
		}
		if event.Event == "completed" {
			row.Observed = event.Observed
			completed = true
		}
	}
	if err = command.Wait(); err != nil {
		t.Fatal(err)
	}
	if !completed || len(row.Observed.Opens) != 2 || row.Observed.Closes != 2 || row.Observed.Acquired != 2 || row.Observed.Released != 2 || len(row.Observed.Files) != 1 {
		t.Fatalf("canceled saved lease was not reusable: %+v", row.Observed)
	}
	if row.Observed.PoolLoads != 0 || row.Observed.PoolFactories != 0 || row.Observed.ExecuteCalls != 0 {
		t.Fatal("stdio download entered pool")
	}
	return row
}

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
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateRandomWorkflow = flag.Bool("migration-update-download-random-workflow", false, "capture frozen Go saved random recommendation download workflow")

type randomAccountRequest struct {
	UserID int64   `json:"user_id"`
	Proxy  *string `json:"https_proxy_override"`
}
type randomWorkflowCase struct {
	sourceExpansionCase
	DetailOverrides map[string]json.RawMessage `json:"detail_overrides,omitempty"`
	Candidates      []int64                    `json:"candidate_ids"`
	Recommendations []sourceExpansionReply     `json:"recommendation_responses"`
	RefreshFailure  string                     `json:"refresh_failure"`
	RemovePublished bool                       `json:"remove_published_before_output"`
	Proxy           *string                    `json:"https_proxy_override"`
	AccountRequests []randomAccountRequest     `json:"account_requests"`
}
type randomWorkflowManager struct {
	observedUgoiraManager
	removePublished bool
}

func (m *randomWorkflowManager) Download(ctx context.Context, request downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	batch, err := m.observedUgoiraManager.Download(ctx, request)
	if m.removePublished && len(batch.Items) > 0 && len(batch.Items[0].Files) > 0 {
		if e := os.Remove(batch.Items[0].Files[0].Path); e != nil {
			return batch, e
		}
	}
	return batch, err
}
func randomRecommendation(ids []int64, next any) sourceExpansionReply {
	items := []any{}
	for _, id := range ids {
		items = append(items, map[string]any{"id": id, "title": "List title must not be saved", "type": "synthetic_list_kind", "page_count": 99, "create_date": "2026-10-08T12:34:56+09:00", "user": map[string]any{"id": 99, "name": "List author must not be saved"}, "image_urls": map[string]any{"large": "https://i.pximg.net/list-must-not-be-used.png"}, "meta_single_page": map[string]any{"original_image_url": "https://i.pximg.net/list-must-not-be-used.png"}})
	}
	body, _ := json.Marshal(map[string]any{"illusts": items, "next_url": next})
	return sourceExpansionReply{Status: 200, Body: body}
}
func randomWorkflowCaseFor(name string, ids []int64, args any) randomWorkflowCase {
	return randomWorkflowCase{sourceExpansionCase: sourceExpansionCase{ugoiraWorkflowCase: ugoiraWorkflowCase{staticArtworkCase: staticArtworkCase{Name: name, Arguments: args}}}, Candidates: ids, Recommendations: []sourceExpansionReply{randomRecommendation(ids, nil)}, AccountRequests: []randomAccountRequest{}}
}
func randomWorkflowSetup(t *testing.T, random *randomWorkflowCase, root string, pending string) (*runtime.App, func()) {
	t.Helper()
	row := &random.sourceExpansionCase
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
	recommendationCount := 0
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
			if random.RefreshFailure == "http" {
				status = 429
				header.Set("Retry-After", "120")
				raw = []byte(`{"error":{"message":"owned fixture refresh failed"}}`)
			}
			if random.RefreshFailure == "identity" {
				raw = []byte(fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id+1))
			}
			if random.RefreshFailure == "cas" {
				if err := db.RotatePixivCredentials(context.Background(), id, 1, []byte(fmt.Sprintf("fixture-competing-%d", id))); err != nil {
					return nil, err
				}
			}
			if random.RefreshFailure == "persist" {
				if _, err := db.DB().Exec(`CREATE TRIGGER fixture_rotation_failure BEFORE UPDATE OF refresh_token ON pixiv_account BEGIN SELECT RAISE(FAIL, 'owned fixture persist failure'); END`); err != nil {
					return nil, err
				}
			}
			body = io.NopCloser(bytes.NewReader(raw))
		case "app-api.pixiv.net":
			switch r.URL.Path {
			case "/v1/illust/recommended":
				if r.URL.RawQuery != "" {
					return nil, fmt.Errorf("random followed a page or added query: %s", r.URL)
				}
				recommendationCount++
				if recommendationCount > len(random.Recommendations) {
					return nil, errors.New("unexpected recommendation replay")
				}
				reply := random.Recommendations[recommendationCount-1]
				if reply.Error != "" {
					mu.Lock()
					row.Responses = append(row.Responses, sourceExpansionHTTPResponse{r.URL.String(), 0, "", reply.Error})
					mu.Unlock()
					return nil, context.Canceled
				}
				status = reply.Status
				if reply.RetryAfter != "" {
					header.Set("Retry-After", reply.RetryAfter)
				}
				body = io.NopCloser(bytes.NewReader(reply.Body))
				if pending == "recommendation" && recommendationCount == 1 {
					body = &sourceExpansionPendingBody{ctx: r.Context(), root: dest}
				}
			case "/v1/illust/detail":
				id, _ := strconv.ParseInt(r.URL.Query().Get("illust_id"), 10, 64)
				raw := staticArtworkMetadata(id)
				if override, ok := random.DetailOverrides[fmt.Sprint(id)]; ok {
					raw = override
				}
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
			if strings.Contains(r.URL.Path, "47_p1") {
				mu.Lock()
				row.Responses = append(row.Responses, sourceExpansionHTTPResponse{r.URL.String(), 0, "", "context canceled"})
				mu.Unlock()
				return nil, context.Canceled
			}
			if strings.Contains(r.URL.Path, "46_p1") {
				status = 503
				raw = []byte("unavailable")
			}
			if strings.Contains(r.URL.Path, "list-must-not-be-used") {
				return nil, errors.New("list metadata reached media save")
			}
			body = io.NopCloser(bytes.NewReader(raw))
			if pending == "media" && strings.Contains(r.URL.Path, "44_p1") {
				body = &sourceExpansionPendingBody{ctx: r.Context(), root: dest}
			}
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
		if row.Name == "ignored-client-close-error" {
			return errors.New("owned fixture close failure")
		}
		return nil
	}})
	configured := downloader.NewManager(nil, configuration.DownloadPath, configuration.FilenameTemplate)
	configured.SetDirectoryTemplate(configuration.DirectoryTemplate)
	app := runtime.NewApp(configured, func(c *pixiv.Client) runtime.DownloadManager {
		manager := downloader.NewManager(c, configuration.DownloadPath, configuration.FilenameTemplate)
		manager.SetDirectoryTemplate(configuration.DirectoryTemplate)
		return &randomWorkflowManager{observedUgoiraManager: observedUgoiraManager{Manager: manager, row: &row.ugoiraWorkflowCase, root: dest, mu: &mu}, removePublished: random.RemovePublished}
	}, runtime.SDKPorts{OpenLease: func(ctx context.Context, a runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
		mu.Lock()
		random.AccountRequests = append(random.AccountRequests, randomAccountRequest{a.UserID, a.HTTPSProxyOverride})
		mu.Unlock()
		return facade.Open(ctx, pixivservice.Request{UserID: a.UserID, Options: pixiv.Options{HTTPClient: httpClient}})
	}, Execute: func(context.Context, runtime.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
		mu.Lock()
		row.ExecuteCalls++
		mu.Unlock()
		return errors.New("unexpected pooled execution")
	}}, runtime.Account{UserID: row.AccountID, HTTPSProxyOverride: random.Proxy})
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
		randomWorkflowOrder(row)
	}
	return app, finish
}

func randomWorkflowOrder(row *sourceExpansionCase) {
	order := func(length int, getURL func(int) string, less func(int, int) bool, swap func(int, int)) {
		start := 0
		attempt := 0
		for start < length {
			end := start + 1
			for end < length && getURL(end) != "https://oauth.secure.pixiv.net/auth/token" {
				end++
			}
			media := start
			for media < end && (getURL(media) == "https://oauth.secure.pixiv.net/auth/token" || getURL(media) == "https://app-api.pixiv.net/v1/illust/recommended") {
				media++
			}
			for i := media; i < end; i++ {
				if getURL(i) == "https://app-api.pixiv.net/v1/illust/recommended" {
					panic("recommendations began after media within one RPC")
				}
			}
			if attempt < len(row.ManagerRequests) {
				ids := map[int64]bool{}
				for _, id := range row.ManagerRequests[attempt].IDs {
					ids[id] = true
				}
				if len(ids) > 1 {
					sort.Sort(randomOrder{end - media, func(a, b int) bool { return less(media+a, media+b) }, func(a, b int) { swap(media+a, media+b) }})
				}
			}
			start = end
			attempt++
		}
	}
	order(len(row.Requests), func(i int) string { return row.Requests[i].URL }, func(a, b int) bool { return row.Requests[a].URL < row.Requests[b].URL }, func(a, b int) { row.Requests[a], row.Requests[b] = row.Requests[b], row.Requests[a] })
	order(len(row.Responses), func(i int) string { return row.Responses[i].URL }, func(a, b int) bool { return row.Responses[a].URL < row.Responses[b].URL }, func(a, b int) { row.Responses[a], row.Responses[b] = row.Responses[b], row.Responses[a] })
	for i := range row.ManagerRequests {
		sort.Slice(row.ManagerRequests[i].IDs, func(a, b int) bool { return row.ManagerRequests[i].IDs[a] < row.ManagerRequests[i].IDs[b] })
	}
}

type randomOrder struct {
	length int
	less   func(int, int) bool
	swap   func(int, int)
}

func (o randomOrder) Len() int           { return o.length }
func (o randomOrder) Less(a, b int) bool { return o.less(a, b) }
func (o randomOrder) Swap(a, b int)      { o.swap(a, b) }

func captureRandomWorkflowCase(t *testing.T, row *randomWorkflowCase) {
	t.Helper()
	root := t.TempDir()
	app, finish := randomWorkflowSetup(t, row, root, "")
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
	raw, err := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": map[string]any{"name": "download_random_from_recommendation", "arguments": row.Arguments}})
	if err != nil {
		t.Fatal(err)
	}
	send(raw)
	row.Response = normalizeDirectJSON(t, read(), filepath.Join(root, "downloads"))
	_ = conn.Close()
	cancel()
	<-stopped
	finish()
	verifyRandomWorkflowCase(t, row, filepath.Join(root, "downloads"))
}
func verifyRandomWorkflowCase(t *testing.T, row *randomWorkflowCase, root string) {
	t.Helper()
	if row.PoolLoads != 0 || row.PoolFactories != 0 || row.ExecuteCalls != 0 {
		t.Fatal("random download entered pool/replay")
	}
	if row.Acquired != 1 || row.Released != 1 {
		t.Fatal("random lease gate ownership")
	}
	if len(row.Opens) > 1 {
		t.Fatal("random download refreshed twice")
	}
	if row.RefreshFailure != "" {
		if len(row.Requests) != 1 || row.Closes != 0 || len(row.ManagerRequests) != 0 {
			t.Fatal("failed rotation reached recommendations/media")
		}
	} else if len(row.Opens) > 0 && row.Closes != 1 {
		t.Fatal("single client not closed")
	}
	for _, request := range row.Requests {
		if strings.Contains(request.URL, "/v1/illust/recommended") && request.URL != "https://app-api.pixiv.net/v1/illust/recommended" {
			t.Fatal("random followed continuation or added list query")
		}
		if strings.Contains(request.URL, "i.pximg.net") && (request.Authorization != "" || request.Cookie != "" || request.Referer != "https://app-api.pixiv.net/") {
			t.Fatalf("resource credential boundary: %+v", request)
		}
	}
	if len(row.ManagerRequests) > 0 {
		want := 5
		if args, ok := row.Arguments.(map[string]any); ok && args["count"] != nil {
			want = args["count"].(int)
		}
		if want > len(row.Candidates) {
			want = len(row.Candidates)
		}
		if len(row.ManagerRequests) != 1 || len(row.ManagerRequests[0].IDs) != want {
			t.Fatalf("shuffle/truncate/clamp selected %v, want cardinality %d", row.ManagerRequests, want)
		}
		remaining := map[int64]int{}
		for _, id := range row.Candidates {
			remaining[id]++
		}
		for _, id := range row.ManagerRequests[0].IDs {
			remaining[id]--
			if remaining[id] < 0 {
				t.Fatal("selected an entry outside candidate multiset")
			}
		}
		request := row.ManagerRequests[0]
		if request.Path != "<ROOT>" || request.Filename != row.Filename || request.Directory != row.Directory {
			t.Fatal("runtime templates/options were not forwarded")
		}
	}
	var result struct {
		Result struct {
			IsError bool `json:"isError"`
			Content []struct {
				Type string `json:"type"`
			}
			StructuredContent downloadOut `json:"structuredContent"`
		}
		Error any
	}
	if err := json.Unmarshal(row.Response, &result); err != nil {
		t.Fatal(err)
	}
	if result.Error != nil {
		t.Fatal("handler failure became protocol error")
	}
	out := result.Result.StructuredContent
	lastID := int64(0)
	grouped := []downloadFileOut{}
	for _, item := range out.Items {
		if item.IllustID <= lastID {
			t.Fatal("manager public item output not sorted/deduplicated")
		}
		lastID = item.IllustID
		if item.URL != fmt.Sprintf("https://www.pixiv.net/artworks/%d", item.IllustID) || strings.Contains(item.Title, "List title") || strings.Contains(item.Author, "List author") {
			t.Fatal("recommendation metadata bypassed detail refetch")
		}
		grouped = append(grouped, item.Files...)
	}
	a, _ := json.Marshal(grouped)
	b, _ := json.Marshal(out.Files)
	if !bytes.Equal(a, b) {
		t.Fatal("grouped/flat file order differs")
	}
	if !row.RemovePublished && len(out.Files) != len(row.Files) {
		t.Fatal("RPC and saved file count differ")
	}
	for _, file := range out.Files {
		path := strings.ReplaceAll(file.Path, "<ROOT>", root)
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if file.SizeBytes != info.Size() || file.MIMEType != downloader.MimeTypeForFile(path) || !strings.HasPrefix(file.FileURI, "file://<ROOT>/") {
			t.Fatal("saved file metadata differs")
		}
	}
	for _, content := range result.Result.Content {
		if content.Type != "text" {
			t.Fatal("random emitted nontext content")
		}
	}
	if strings.Contains(string(row.Response), "fixture-access") || strings.Contains(string(row.Response), "fixture=private") {
		t.Fatal("RPC leaked credentials or raw resource query")
	}
}
func randomWorkflowHashes() map[string]string {
	hashes := map[string]string{}
	for path, digest := range ugoiraWorkflowHashes {
		hashes[path] = digest
	}
	for path, digest := range map[string]string{
		"go.mod":              "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum":              "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"sdk/pixiv/pixiv.go":  "daefb42f9f90359f5ce3d326d18df7afe8c10615750d88fef049cce78224d317",
		"sdk/pixiv/errors.go": "9a1830393129ca195ef4d57c0bda17f219f0f750c7293abf5520aa888bfe81a8",
		"internal/services/pixiv/endpoint/artwork/recommended/recommended.go": "3167adbfc47d50d458484a98a5dbe64694d3a39a845014f64ec6e5fac04c3688",
		"internal/storage/database/repository.go":                             "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
		"internal/services/pixiv/account/pixiv.go":                            "683353360af6bd9700bdffe5ee6b531800f93228de1afac87542b23f96d5857c",
		"internal/config/settings/store.go":                                   "c5b418cd50e17dce27d4e0f5f499e4d293e0527e07d335e80da9a5fe2711feec",
		"internal/config/settings/auth.go":                                    "78729a90d7a71fe5793f469be5770d1acb565ac2945bf252b893dfcec392981e",
		"internal/services/pixiv/account/accounts.go":                         "129d83a89a09bc8a4dbd926ce19e9551fedf048a4c1fb9f71d551ee3089fad91",
		"internal/services/pixiv/pool/gate.go":                                "33387af051432f6a6e6a8d0a8bbbcc88a5035ef6a65cbd30846f3ea9a70990a6",
	} {
		hashes[path] = digest
	}
	return hashes
}
func guardRandomWorkflowSource(t *testing.T, base string) string {
	t.Helper()
	for path, want := range randomWorkflowHashes() {
		raw, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(raw)) != want {
			t.Fatalf("worktree reference changed: %s", path)
		}
		command := exec.Command("git", "show", "4b4426487ef18bed276706daec385e0d0a6979f9:"+path)
		command.Dir = base
		frozen, err := command.Output()
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(frozen)) != want {
			t.Fatalf("literal digest does not match frozen git object: %s", path)
		}
	}
	digest, err := staticlib.CalculateRustSourceDigest(filepath.Join(base, "internal", "media", "ugoira", "rust"), filepath.Join(base, "third_party", "rust", "quantette-0.6.0"))
	if err != nil || digest != ugoiraSourceDigest {
		t.Fatalf("native source identity: %s, %v", digest, err)
	}
	command := exec.Command("git", "diff", "--quiet", "4b4426487ef18bed276706daec385e0d0a6979f9", "--", "internal/media/ugoira/rust", "third_party/rust/quantette-0.6.0", ":!third_party/rust/quantette-0.6.0/rustfmt.toml")
	command.Dir = base
	if err = command.Run(); err != nil {
		t.Fatalf("frozen native tracked assets changed: %v", err)
	}
	return digest
}
func mutateRandomRecommendation(t *testing.T, row *randomWorkflowCase, mutate func(map[string]any)) {
	t.Helper()
	var value map[string]any
	if err := json.Unmarshal(row.Recommendations[0].Body, &value); err != nil {
		t.Fatal(err)
	}
	mutate(value)
	raw, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	row.Recommendations[0].Body = raw
}
func TestMigrationMCPDownloadRandomWorkflowMatchesFrozenContract(t *testing.T) {
	if goruntime.GOOS != "linux" || goruntime.GOARCH != "amd64" {
		t.Skip("default ugoira fixture requires tracked genuine linux/amd64 staticlib; other native targets remain pending")
	}
	base := filepath.Join("..", "..", "..", "..", "..")
	digest := guardRandomWorkflowSource(t, base)
	makeCase := randomWorkflowCaseFor
	rows := []randomWorkflowCase{
		makeCase("singleton-default-count", []int64{42}, map[string]any{}),
		makeCase("default-five-distinct", []int64{1, 2, 3, 4, 5}, map[string]any{}),
		makeCase("null-count-default-five", []int64{1, 2, 3, 4, 5}, map[string]any{"count": nil}),
		makeCase("count-one-singleton", []int64{42}, map[string]any{"count": 1}),
		makeCase("count-twenty-all-distinct", []int64{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20}, map[string]any{"count": 20}),
		makeCase("count-clamps-to-short-list", []int64{44, 42}, map[string]any{"count": 20}),
		makeCase("duplicate-entry-count-no-backfill", []int64{42, 42, 42, 42, 42, 42, 42, 42}, map[string]any{"count": 3}),
		makeCase("all-visual-kinds-detail-refetch", []int64{71, 44, 42, 48, 44}, map[string]any{}),
		makeCase("first-page-valid-next-never-followed", []int64{42}, map[string]any{}),
		makeCase("configured-templates-pages", []int64{44}, map[string]any{"pages": "3,1,1", "ugoira_mode": "zip"}),
		makeCase("partial-detail-failure-independent-item", []int64{45, 42}, map[string]any{}),
		makeCase("partial-media-failure", []int64{46}, map[string]any{}),
		makeCase("resource-context-lookalike", []int64{47}, map[string]any{}),
		makeCase("empty-first-page", nil, map[string]any{}),
		makeCase("missing-saved-default", []int64{42}, map[string]any{}),
		makeCase("explicit-account-leaves-default-unchanged", []int64{42}, map[string]any{}),
		makeCase("ignored-client-close-error", []int64{42}, map[string]any{}),
		makeCase("configured-directory-operation-error", []int64{42}, map[string]any{}),
		makeCase("configured-filename-business-error", []int64{42}, map[string]any{}),
		makeCase("published-file-removed-before-output-stat", []int64{42}, map[string]any{}),
	}
	rows[8].Recommendations[0] = randomRecommendation([]int64{42}, "https://app-api.pixiv.net/v1/illust/recommended?offset=1&include_ranking_illusts=1&viewed[0]=999")
	rows[9].Filename = "{author_id}_{id}_{num}_{date}_{tags}"
	rows[9].Directory = "{author}/{date}/{id}/{num}"
	rows[14].DefaultID = 99
	rows[15].AccountID = 42
	proxy := "http://owned-proxy.invalid:8080"
	rows[15].Proxy = &proxy
	rows[17].Directory = "../{id}"
	rows[18].Filename = "{unknown}"
	rows[19].RemovePublished = true
	rows = append(rows, makeCase("default-five-duplicate-entries", []int64{42, 42, 42, 42, 42, 42, 42, 42}, map[string]any{}), makeCase("null-five-duplicate-entries", []int64{42, 42, 42, 42, 42, 42, 42, 42}, map[string]any{"count": nil}))
	for _, quality := range []string{"regular", "small", "thumb", "mini"} {
		rows = append(rows, makeCase("static-quality-"+quality, []int64{44}, map[string]any{"pages": "2", "quality": quality}))
	}
	for _, mode := range []string{"apng", "zip", "raw"} {
		rows = append(rows, makeCase("ugoira-mode-"+mode, []int64{71}, map[string]any{"ugoira_mode": mode}))
	}
	for _, status := range []int{401, 403, 404, 429} {
		row := makeCase(fmt.Sprintf("recommendation-http-%d-no-pool-replay", status), []int64{42}, map[string]any{})
		row.Recommendations[0] = sourceExpansionReply{Status: status, Body: json.RawMessage(`{"error":{"message":"owned recommendation failure"}}`)}
		rows = append(rows, row)
	}
	retry := makeCase("recommendation-retry-after-zero-same-client", []int64{42}, map[string]any{})
	retry.Recommendations = []sourceExpansionReply{{Status: 429, RetryAfter: "0", Body: json.RawMessage(`{"error":{"message":"owned retry"}}`)}, randomRecommendation([]int64{42}, nil)}
	rows = append(rows, retry)
	transport := makeCase("recommendation-transport-context-lookalike", []int64{42}, map[string]any{})
	transport.Recommendations[0].Error = "context canceled"
	rows = append(rows, transport)
	for _, failure := range []string{"http", "identity", "cas", "persist"} {
		row := makeCase("refresh-"+failure+"-stops-before-list", []int64{42}, map[string]any{})
		row.RefreshFailure = failure
		rows = append(rows, row)
	}
	for _, next := range []struct {
		name  string
		value any
	}{{"foreign-host", "https://untrusted.invalid/v1/illust/recommended?offset=1"}, {"wrong-path", "https://app-api.pixiv.net/v1/illust/related?offset=1"}, {"unknown-param", "https://app-api.pixiv.net/v1/illust/recommended?unknown=1"}, {"empty", ""}, {"wrong-type", 9}} {
		row := makeCase("invalid-next-"+next.name+"-before-selection", []int64{42}, map[string]any{"count": 1})
		row.Recommendations[0] = randomRecommendation(row.Candidates, next.value)
		rows = append(rows, row)
	}
	for _, kind := range []string{"id", "date", "cover", "dto-type"} {
		row := makeCase("invalid-unselected-"+kind+"-before-selection", []int64{42, 44}, map[string]any{"count": 1})
		mutateRandomRecommendation(t, &row, func(value map[string]any) {
			item := value["illusts"].([]any)[1].(map[string]any)
			switch kind {
			case "id":
				item["id"] = 0
			case "date":
				item["create_date"] = "bad"
			case "cover":
				item["image_urls"] = map[string]any{"large": "https://i.pximg.net/%zz"}
			case "dto-type":
				item["page_count"] = "three"
			}
		})
		rows = append(rows, row)
	}
	statIdentity := makeCase("png-identity-file-removed-before-output-stat", []int64{42}, map[string]any{})
	statIdentity.RemovePublished = true
	statIdentity.DetailOverrides = map[string]json.RawMessage{"42": json.RawMessage(bytes.ReplaceAll(staticArtworkMetadata(42), []byte("42_p0.jpg"), []byte("42_p0.png")))}
	rows = append(rows, statIdentity)
	for i := range rows {
		t.Run(rows[i].Name, func(t *testing.T) { captureRandomWorkflowCase(t, &rows[i]) })
	}
	metadata := map[string]json.RawMessage{}
	for _, row := range rows {
		for _, id := range row.Candidates {
			metadata[fmt.Sprint(id)] = staticArtworkMetadata(id)
		}
	}
	metadata["71"] = ugoiraWorkflowArtwork(71)
	cancellations := []randomWorkflowCancellation{}
	for _, kind := range []string{"recommendation", "media"} {
		t.Run("stdio-cancel-"+kind, func(t *testing.T) { cancellations = append(cancellations, captureRandomWorkflowCancellation(t, kind)) })
	}
	output := struct {
		Source              string                       `json:"source"`
		SourceHashes        map[string]string            `json:"source_hashes"`
		Native              map[string]string            `json:"native_provenance"`
		RequestOrder        string                       `json:"request_order"`
		ObservationBoundary []string                     `json:"observation_boundary"`
		Pending             []string                     `json:"pending"`
		Metadata            map[string]json.RawMessage   `json:"artwork_metadata"`
		UgoiraMetadata      json.RawMessage              `json:"ugoira_metadata"`
		ArchiveHex          string                       `json:"archive_hex"`
		StaticHex           string                       `json:"static_hex"`
		Cancellation        []randomWorkflowCancellation `json:"stdio_cancellation"`
		Cases               []randomWorkflowCase         `json:"cases"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", randomWorkflowHashes(), map[string]string{"platform": "linux/amd64", "source_digest": digest, "artifact_sha256": ugoiraNativeHash}, "Each RPC retains exact OAuth/recommendation prefix. Only multiple distinct selected artworks use URL-sorted concurrent detail/metadata/media suffix multiset; singleton media sequence and public RPC item/file order are never sorted.", []string{"manager_requests.ids is an explicitly sorted selected-entry multiset, preserving duplicates after raw cardinality/subset assertions; random draw/order is never a golden", "count below distinct list length uses a separate runtime invariant, not exact selected IDs", "manager batches and gate/close/pool callbacks are private Go observations, not native Rust SDK idle-close ownership", "valid Account HTTPSProxyOverride is forwarded at the embedding boundary; owned synthetic HTTPClient does not prove proxy routing", "stat error removes an owned genuinely published file after real Manager.Download; this schedule is distinct from save failure"}, []string{"all RNG distribution/entropy failures and concurrent sampling schedules", "other five native targets, full native IO/race and encoder cancellation", "public SDK Context/ownership/idle-close, HTTP2/Accept, signal restoration and Windows repeated cleanup"}, metadata, ugoiraWorkflowMetadata(71), fmt.Sprintf("%x", ugoiraWorkflowArchive(t, 71)), fmt.Sprintf("%x", ugoiraWorkflowPNG(t, 0)), cancellations, rows}
	original48 := output
	original48.Cases = original48.Cases[:48]
	projection, projectionErr := json.MarshalIndent(original48, "", "  ")
	if projectionErr != nil {
		t.Fatal(projectionErr)
	}
	projection = append(projection, '\n')
	if fmt.Sprintf("%x", sha256.Sum256(projection)) != "49fdad6ed0c5a575dc4161195cb1740befce5bdcc3ba2a9d40da74479c8b32fa" {
		t.Fatal("original frozen48 input/expectation projection changed")
	}
	if t.Failed() {
		return
	}
	raw, err := json.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	raw = append(raw, '\n')
	path := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_random_workflow.json")
	if *updateRandomWorkflow {
		if err = os.WriteFile(path, raw, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(raw, want) {
		t.Fatal("saved random recommendation workflow differs from frozen Go")
	}
}
func TestMigrationMCPDownloadRandomSubsetKeepsCardinalityAndCandidateEntries(t *testing.T) {
	for _, test := range []struct {
		name      string
		arguments map[string]any
		count     int
	}{{"count-two", map[string]any{"count": 2}, 2}, {"omitted-default-five", map[string]any{}, 5}, {"null-default-five", map[string]any{"count": nil}, 5}} {
		t.Run(test.name, func(t *testing.T) {
			row := randomWorkflowCaseFor(test.name, []int64{1, 2, 3, 4, 5, 6}, test.arguments)
			captureRandomWorkflowCase(t, &row)
			if len(row.ManagerRequests) != 1 || len(row.ManagerRequests[0].IDs) != test.count {
				t.Fatal("selected candidate cardinality")
			}
			var value struct {
				Result struct {
					StructuredContent downloadOut `json:"structuredContent"`
				}
			}
			if err := json.Unmarshal(row.Response, &value); err != nil {
				t.Fatal(err)
			}
			if len(value.Result.StructuredContent.Items) != test.count || len(value.Result.StructuredContent.Files) != test.count {
				t.Fatal("requested distinct sample cardinality not publicly saved")
			}
			for index, item := range value.Result.StructuredContent.Items {
				if item.IllustID < 1 || item.IllustID > 6 || index > 0 && item.IllustID <= value.Result.StructuredContent.Items[index-1].IllustID {
					t.Fatal("public sample subset/order/dedup")
				}
			}
		})
	}
}

type randomWorkflowCancellation struct {
	Kind                  string             `json:"kind"`
	Arguments             any                `json:"arguments"`
	Response              json.RawMessage    `json:"response"`
	NextResponse          json.RawMessage    `json:"next_response"`
	FilesBeforeCancel     int                `json:"files_before_cancel"`
	TemporaryBeforeCancel int                `json:"temporary_before_cancel"`
	Observed              randomWorkflowCase `json:"observed"`
}

func TestMigrationMCPRandomWorkflowCancellationChild(t *testing.T) {
	root := os.Getenv("PIXIV_RANDOM_WORKFLOW_CHILD")
	if root == "" {
		return
	}
	kind := os.Getenv("PIXIV_RANDOM_WORKFLOW_KIND")
	id := int64(42)
	if kind == "media" {
		id = 44
	}
	row := randomWorkflowCaseFor("stdio-cancel-"+kind, []int64{id}, map[string]any{})
	row.Recommendations = append(row.Recommendations, randomRecommendation([]int64{42}, nil))
	app, finish := randomWorkflowSetup(t, &row, root, kind)
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
func captureRandomWorkflowCancellation(t *testing.T, kind string) randomWorkflowCancellation {
	t.Helper()
	root := t.TempDir()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationMCPRandomWorkflowCancellationChild$")
	command.Env = append(os.Environ(), "PIXIV_RANDOM_WORKFLOW_CHILD="+root, "PIXIV_RANDOM_WORKFLOW_KIND="+kind)
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
	row := randomWorkflowCancellation{Kind: kind, Arguments: map[string]any{}}
	raw, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": map[string]any{"name": "download_random_from_recommendation", "arguments": row.Arguments}})
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
	wantFiles, wantTemp := 0, 0
	if kind == "media" {
		wantFiles, wantTemp = 1, 1
	}
	if row.FilesBeforeCancel != wantFiles || row.TemporaryBeforeCancel != wantTemp {
		t.Fatalf("cancellation was not at owned body boundary: files=%d temp=%d", row.FilesBeforeCancel, row.TemporaryBeforeCancel)
	}
	write(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture"}}`)
	row.Response = normalizeDirectJSON(t, read(), filepath.Join(root, "downloads"))
	write(`{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"download_random_from_recommendation","arguments":{}}}`)
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
			Event    string             `json:"event"`
			Observed randomWorkflowCase `json:"observed"`
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
	if !completed || len(row.Observed.Opens) != 2 || row.Observed.Closes != 2 || row.Observed.Acquired != 2 || row.Observed.Released != 2 || len(row.Observed.Files) != wantFiles+1 {
		t.Fatalf("canceled saved lease was not reusable: %+v", row.Observed)
	}
	if row.Observed.PoolLoads != 0 || row.Observed.PoolFactories != 0 || row.Observed.ExecuteCalls != 0 {
		t.Fatal("stdio download entered pool")
	}
	return row
}

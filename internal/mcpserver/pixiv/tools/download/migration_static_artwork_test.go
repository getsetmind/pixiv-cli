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
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver"
	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	pixivservice "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateStaticArtwork = flag.Bool("migration-update-download-static", false, "capture frozen Go static artwork MCP contract")

type staticArtworkCase struct {
	ManagerRequests []staticArtworkRequest `json:"manager_requests"`
	Name            string                 `json:"name"`
	Arguments       any                    `json:"arguments"`
	Filename        string                 `json:"filename_template"`
	Directory       string                 `json:"directory_template"`
	Response        json.RawMessage        `json:"response"`
	Requests        []directWireRequest    `json:"requests"`
	Files           []directFile           `json:"files"`
	Directories     []string               `json:"directories"`
	Opens           []int64                `json:"opens"`
	Closes          int                    `json:"closes"`
	PoolLoads       int                    `json:"pool_loads"`
	PoolFactories   int                    `json:"pool_factories"`
	ExecuteCalls    int                    `json:"execute_calls"`
	Acquired        int                    `json:"acquired"`
	Released        int                    `json:"released"`
}

type staticArtworkRequest struct {
	IDs       []int64                    `json:"ids"`
	Pages     []int                      `json:"pages"`
	Quality   downloader.DownloadQuality `json:"quality"`
	Mode      downloader.UgoiraFormat    `json:"ugoira_mode"`
	Path      string                     `json:"path"`
	Filename  string                     `json:"filename_template"`
	Directory string                     `json:"directory_template"`
}
type observedStaticManager struct {
	*downloader.Manager
	observe func(downloader.DownloadRequest)
}

func (m *observedStaticManager) Download(ctx context.Context, r downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	m.observe(r)
	return m.Manager.Download(ctx, r)
}

func TestMigrationMCPDownloadStaticArtworkMatchesFrozenContract(t *testing.T) {
	hashes := map[string]string{
		"internal/mcpserver/pixiv/tools/download/download.go":  "60cd7f8fcecfe27c185a9df4469dd8eaa0117e87586c047dfdf911e3400a11e5",
		"internal/mcpserver/pixiv/internal/runtime/runtime.go": "a6ca224d1949e7716277677f81f80c1133eb0b91e196dc64e04d29fad2c7fe79",
		"internal/media/downloader/downloader.go":              "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
		"internal/media/downloader/mime.go":                    "aa7e39c8d974c81214b0d61d2f0264803f68db1c9c9f50b14c34eecd2ce3eb64",
		"internal/media/downloader/filename/filename.go":       "e703ba7787b54a752cd9a697ca3e44a496e65618a48e29df7a5d41715d29dedb",
		"sdk/pixiv/reference.go":                               "7d467e3ae306fbd3d920f86330d80e1c6bd64787db6e56869fe76e77be468abc",
		"sdk/pixiv/ops_artwork.go":                             "f8aa00684b84463c6ba82b18db87d4c2e403a0dcaa048f3c3282f6555fd7445c",
		"sdk/pixiv/map_artwork.go":                             "45fe0a1d6b081ce2842ce536492b5a431bcf02940d94a29400b06cd8c441bb22",
		"sdk/pixiv/resource.go":                                "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
		"internal/services/pixiv/facade.go":                    "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
	}
	base := filepath.Join("..", "..", "..", "..", "..")
	for path, want := range hashes {
		body, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != want {
			t.Fatalf("reference source changed: %s", path)
		}
	}
	opaque, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"artwork","id":42,"p":0,"v":"original"}`))
	if err != nil {
		t.Fatal(err)
	}
	rows := []staticArtworkCase{
		{Name: "runtime-config-default-template", Filename: "{author} - {title}_{id}", Arguments: map[string]any{"src": "42"}},
		{Name: "mixed-static-opaque-direct", Arguments: map[string]any{"srcs": []string{"https://i.pximg.net/direct.jpg", opaque.String(), "42"}}},
		{Name: "pid-default", Arguments: map[string]any{"src": "42"}},
		{Name: "localized-artwork-url", Arguments: map[string]any{"src": "https://www.pixiv.net/en/artworks/42"}},
		{Name: "unrecognized-legacy-url-forbidden-direct", Arguments: map[string]any{"src": "https://www.pixiv.net/member_illust.php?mode=medium&illust_id=42"}},
		{Name: "canonical-dedup-srcs", Arguments: map[string]any{"srcs": []string{" +42 ", "https://www.pixiv.net/artworks/42", "42"}}},
		{Name: "multipage-default", Arguments: map[string]any{"src": "44"}},
		{Name: "selected-pages", Arguments: map[string]any{"src": "44", "pages": "3,1-2,2", "quality": " original ", "delivery": " local_path ", "ugoira_mode": " raw "}},
		{Name: "configured-templates", Filename: "{author_id}_{id}_{num}_{date}_{tags}", Directory: "{author}/{date}/{id}/{num}", Arguments: map[string]any{"src": "44", "pages": "3,1"}},
		{Name: "configured-filename-implicit-pages", Filename: "{id}-{title}", Arguments: map[string]any{"src": "44", "pages": "2"}},
		{Name: "one-artwork-business-failure", Arguments: map[string]any{"srcs": []string{"42", "45"}}},
		{Name: "all-business-failure", Arguments: map[string]any{"src": "45"}},
		{Name: "partial-page-business-failure", Arguments: map[string]any{"src": "46"}},
		{Name: "partial-page-operation-error", Arguments: map[string]any{"src": "47"}},
		{Name: "missing-page", Arguments: map[string]any{"src": "44", "pages": "4"}},
		{Name: "configured-directory-operation-error", Directory: "../{id}", Arguments: map[string]any{"src": "42"}},
		{Name: "configured-filename-business-error", Filename: "{unknown}", Arguments: map[string]any{"src": "42"}},
		{Name: "invalid-pages-before-lease", Arguments: map[string]any{"src": "42", "pages": "1,,2"}},
		{Name: "invalid-quality-before-lease", Arguments: map[string]any{"src": "42", "quality": "large"}},
		{Name: "invalid-mode-before-lease", Arguments: map[string]any{"src": "42", "ugoira_mode": "webm"}},
		{Name: "invalid-delivery-before-lease", Arguments: map[string]any{"src": "42", "delivery": "image_content"}},
		{Name: "both-sources-before-lease", Arguments: map[string]any{"src": "42", "srcs": []string{"44"}}},
		{Name: "unknown-option-before-lease", Arguments: map[string]any{"src": "42", "filename_template": "override"}},
	}
	for _, quality := range []string{"regular", "small", "thumb", "mini"} {
		rows = append(rows, staticArtworkCase{Name: "quality-" + quality, Arguments: map[string]any{"src": "44", "pages": "2", "quality": quality}})
	}
	rows = append(rows, staticArtworkCase{Name: "unknown-upstream-kind-static", Arguments: map[string]any{"src": "48"}})
	rows = append(rows, staticArtworkCase{Name: "published-date-crosses-utc-day", Filename: "{date}-{id}", Directory: "{date}", Arguments: map[string]any{"src": "49"}})
	var tool json.RawMessage
	for i := range rows {
		row := &rows[i]
		root := t.TempDir()
		dest := filepath.Join(root, "downloads")
		if err := os.Mkdir(dest, 0700); err != nil {
			t.Fatal(err)
		}
		row.ManagerRequests = []staticArtworkRequest{}
		row.Requests = []directWireRequest{}
		row.Files = []directFile{}
		row.Directories = []string{}
		row.Opens = []int64{}
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		db, err := database.Open(root)
		if err != nil {
			t.Fatal(err)
		}
		for _, id := range []int64{42, 43} {
			if err = db.SavePixivCredential(ctx, account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
				t.Fatal(err)
			}
		}
		if err = db.SetAllPixivSchedulable(ctx, true); err != nil {
			t.Fatal(err)
		}
		var mu sync.Mutex
		httpClient := &http.Client{Transport: directTransport(func(r *http.Request) (*http.Response, error) {
			mu.Lock()
			row.Requests = append(row.Requests, directWireRequest{r.URL.String(), r.Method, r.Header.Get("Referer"), r.Header.Get("Authorization"), r.Header.Get("Cookie")})
			mu.Unlock()
			status := 200
			header := http.Header{"Content-Type": {"application/json"}}
			body := []byte(`{}`)
			switch r.URL.Host {
			case "oauth.secure.pixiv.net":
				raw, _ := io.ReadAll(r.Body)
				form, _ := url.ParseQuery(string(raw))
				id, _ := strconv.ParseInt(strings.TrimPrefix(form.Get("refresh_token"), "fixture-refresh-"), 10, 64)
				row.Opens = append(row.Opens, id)
				body = []byte(fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id))
			case "app-api.pixiv.net":
				if r.URL.Path != "/v1/illust/detail" {
					return nil, fmt.Errorf("unexpected API operation %s", r.URL.Path)
				}
				id, _ := strconv.ParseInt(r.URL.Query().Get("illust_id"), 10, 64)
				if id == 45 {
					status = 404
					body = []byte(`{"error":{"message":"fixture missing artwork"}}`)
				} else {
					body = staticArtworkMetadata(id)
				}
			case "i.pximg.net":
				if strings.Contains(r.URL.Path, "46_p1") {
					status = 503
					body = []byte("unavailable")
				} else if strings.Contains(r.URL.Path, "47_p1") {
					return nil, context.Canceled
				} else {
					header.Set("Content-Type", "image/png")
					body = []byte("\x89PNG\r\n\x1a\nfixture")
				}
			default:
				return nil, fmt.Errorf("unexpected synthetic host %s", r.URL.Host)
			}
			return &http.Response{StatusCode: status, Header: header, Body: io.NopCloser(bytes.NewReader(body)), Request: r}, nil
		})}
		gate := &leaseGate{}
		facade := pixivservice.New(pixivservice.Dependencies{Accounts: account.NewService(db, leaseDefaults{}), Gate: gate, LoadPoolConfig: func() (pixivservice.PoolConfig, error) {
			row.PoolLoads++
			return pixivservice.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
		}, Pool: func(pixivservice.PoolConfig) (pixivservice.PoolExecutor, error) {
			row.PoolFactories++
			return nil, errors.New("unexpected pool")
		}, CloseClient: func(c *pixiv.Client) error { row.Closes++; c.CloseIdleConnections(); return nil }})
		configured := downloader.NewManager(nil, dest, row.Filename)
		configured.SetDirectoryTemplate(row.Directory)
		app := runtime.NewApp(configured, func(c *pixiv.Client) runtime.DownloadManager {
			m := downloader.NewManager(c, dest, row.Filename)
			m.SetDirectoryTemplate(row.Directory)
			return &observedStaticManager{Manager: m, observe: func(r downloader.DownloadRequest) {
				row.ManagerRequests = append(row.ManagerRequests, staticArtworkRequest{r.IllustIDs, r.Pages, r.Quality, r.UgoiraFormat, strings.ReplaceAll(r.DownloadPath, dest, "<ROOT>"), r.FilenameTemplate, r.DirectoryTemplate})
			}}
		}, runtime.SDKPorts{OpenLease: func(ctx context.Context, _ runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
			return facade.Open(ctx, pixivservice.Request{Options: pixiv.Options{HTTPClient: httpClient}})
		}, Execute: func(context.Context, runtime.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
			row.ExecuteCalls++
			return errors.New("unexpected pool execution")
		}}, runtime.Account{})
		server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
		Register(app, server)
		ct, st := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, st) }()
		conn, err := ct.Connect(ctx)
		if err != nil {
			t.Fatal(err)
		}
		send := func(raw []byte) {
			m, e := jsonrpc.DecodeMessage(raw)
			if e != nil {
				t.Fatal(e)
			}
			if e = conn.Write(ctx, m); e != nil {
				t.Fatal(e)
			}
		}
		read := func() json.RawMessage {
			m, e := conn.Read(ctx)
			if e != nil {
				t.Fatal(e)
			}
			raw, e := jsonrpc.EncodeMessage(m)
			if e != nil {
				t.Fatal(e)
			}
			return raw
		}
		send([]byte(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`))
		_ = read()
		send([]byte(`{"jsonrpc":"2.0","method":"notifications/initialized"}`))
		if i == 0 {
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
				var n struct{ Name string }
				_ = json.Unmarshal(raw, &n)
				if n.Name == "download" {
					tool = raw
				}
			}
			if tool == nil {
				t.Fatal("missing registered download")
			}
		}
		raw, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": map[string]any{"name": "download", "arguments": row.Arguments}})
		send(raw)
		row.Response = normalizeDirectJSON(t, read(), dest)
		_ = conn.Close()
		cancel()
		_ = db.Close()
		row.Acquired, row.Released = gate.acquired, gate.released
		err = filepath.WalkDir(dest, func(path string, entry os.DirEntry, walkErr error) error {
			if walkErr != nil {
				return walkErr
			}
			if path == dest {
				return nil
			}
			relative, e := filepath.Rel(dest, path)
			if e != nil {
				return e
			}
			relative = filepath.ToSlash(relative)
			if entry.IsDir() {
				row.Directories = append(row.Directories, relative)
				return nil
			}
			if strings.HasPrefix(entry.Name(), ".atomic-write-") {
				return errors.New("temporary destination leaked")
			}
			body, e := os.ReadFile(path)
			if e != nil {
				return e
			}
			row.Files = append(row.Files, directFile{relative, fmt.Sprintf("%x", body)})
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
		if row.Name == "one-artwork-business-failure" {
			sort.Slice(row.Requests, func(a, b int) bool { return row.Requests[a].URL < row.Requests[b].URL })
		}
		for _, r := range row.Requests {
			if strings.Contains(r.URL, "i.pximg.net") && (r.Authorization != "" || r.Cookie != "" || r.Referer != "https://app-api.pixiv.net/") {
				t.Fatalf("resource credential boundary: %+v", r)
			}
		}
		if row.PoolLoads != 0 || row.PoolFactories != 0 || row.ExecuteCalls != 0 {
			t.Fatalf("MCP entered CLI pool: %+v", row)
		}
		if len(row.Opens) > 0 && (len(row.Opens) != 1 || row.Opens[0] != 43 || row.Closes != 1 || row.Acquired != 1 || row.Released != 1) {
			t.Fatalf("default lease lifecycle: %+v", row)
		}
		var actual struct {
			Result struct {
				IsError bool `json:"isError"`
				Content []struct {
					Type string `json:"type"`
				} `json:"content"`
				StructuredContent downloadOut `json:"structuredContent"`
			} `json:"result"`
		}
		if err = json.Unmarshal(row.Response, &actual); err != nil {
			t.Fatal(err)
		}
		out := actual.Result.StructuredContent
		var grouped []downloadFileOut
		for _, item := range out.Items {
			grouped = append(grouped, item.Files...)
			if item.IllustID > 0 && item.URL != fmt.Sprintf("https://www.pixiv.net/artworks/%d", item.IllustID) {
				t.Fatalf("noncanonical artwork URL: %+v", item)
			}
		}
		if len(grouped) != len(out.Files) || len(out.Files) != len(row.Files) {
			t.Fatalf("grouped/flat/disk count mismatch: %s", row.Name)
		}
		for index, file := range out.Files {
			a, _ := json.Marshal(file)
			b, _ := json.Marshal(grouped[index])
			if !bytes.Equal(a, b) {
				t.Fatalf("grouped/flat file mismatch: %s", row.Name)
			}
			if file.MIMEType != "image/png" || file.SizeBytes != 15 || !strings.HasPrefix(file.FileURI, "file://<ROOT>/") {
				t.Fatalf("published file shape: %+v", file)
			}
		}
		for _, content := range actual.Result.Content {
			if content.Type != "text" {
				t.Fatalf("nontext content: %s", content.Type)
			}
		}
		for _, request := range row.ManagerRequests {
			if request.Filename != row.Filename || request.Directory != row.Directory || request.Path != "<ROOT>" {
				t.Fatalf("configured defaults not forwarded: %+v", request)
			}
		}
		if strings.Contains(row.Name, "before-lease") && len(row.Opens) != 0 {
			t.Fatalf("validation opened lease: %s", row.Name)
		}
	}
	output := struct {
		Source       string                     `json:"source"`
		SourceHashes map[string]string          `json:"source_hashes"`
		RequestOrder string                     `json:"request_order"`
		Pending      []string                   `json:"pending"`
		Cancellation staticArtworkCancellation  `json:"stdio_cancellation"`
		Metadata     map[string]json.RawMessage `json:"metadata"`
		Tool         json.RawMessage            `json:"tool"`
		Cases        []staticArtworkCase        `json:"cases"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", hashes, "single-artwork request sequence retained; concurrent multi-artwork case sorted by URL", []string{"user/bookmark and ugoira successful workflows remain accepted and are not replaced by static failures", "random recommendations", "native platform path/publication IO", "additional static notification/interrupt/deadline schedules beyond one owned stdio page-prefix cancellation"}, captureStaticArtworkCancellation(t), map[string]json.RawMessage{"42": staticArtworkMetadata(42), "44": staticArtworkMetadata(44), "46": staticArtworkMetadata(46), "47": staticArtworkMetadata(47), "48": staticArtworkMetadata(48), "49": staticArtworkMetadata(49)}, tool, rows}
	data, err := json.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_static.json")
	if *updateStaticArtwork {
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
		t.Fatal("static artwork MCP differs from frozen Go contract")
	}
}

func staticArtworkMetadata(id int64) []byte {
	count := 1
	kind := "illust"
	if id >= 44 {
		count = 3
		kind = "manga"
	}
	if id == 48 {
		count = 1
		kind = "synthetic_new_kind"
	}
	date := "2026-10-08T12:34:56+09:00"
	if id == 49 {
		count = 1
		kind = "illust"
		date = "2026-10-08T00:30:00+09:00"
	}
	artwork := map[string]any{"id": id, "title": "Fixture / title", "type": kind, "page_count": count, "create_date": date, "user": map[string]any{"id": 7, "name": "Artist / name"}, "tags": []any{map[string]any{"name": " one "}, map[string]any{"name": "two"}}}
	image := func(page int) string {
		return fmt.Sprintf("https://i.pximg.net/img-original/img/2026/10/08/12/34/56/%d_p%d.jpg?fixture=private", id, page)
	}
	if count == 1 {
		artwork["meta_single_page"] = map[string]any{"original_image_url": image(0)}
	} else {
		pages := []any{}
		for p := 0; p < count; p++ {
			pages = append(pages, map[string]any{"image_urls": map[string]any{"original": image(p)}})
		}
		artwork["meta_pages"] = pages
	}
	raw, _ := json.Marshal(map[string]any{"illust": artwork})
	return raw
}

type staticArtworkCancellation struct {
	Response              json.RawMessage `json:"response"`
	Files                 []directFile    `json:"files"`
	PublishedBeforeCancel int             `json:"published_before_cancel"`
	TemporaryBeforeCancel int             `json:"temporary_before_cancel"`
	Closes                int             `json:"closes"`
}
type staticCancelBody struct {
	ctx  context.Context
	root string
	once sync.Once
}

func (b *staticCancelBody) Read([]byte) (int, error) {
	b.once.Do(func() {
		published, temporary := 0, 0
		_ = filepath.WalkDir(b.root, func(_ string, e os.DirEntry, err error) error {
			if err == nil && !e.IsDir() {
				if strings.HasPrefix(e.Name(), ".atomic-write-") {
					temporary++
				} else {
					published++
				}
			}
			return err
		})
		_ = json.NewEncoder(os.Stderr).Encode(map[string]any{"event": "waiting", "published": published, "temporary": temporary})
	})
	<-b.ctx.Done()
	return 0, b.ctx.Err()
}
func (*staticCancelBody) Close() error { return nil }
func TestMigrationMCPStaticArtworkCancellationChild(t *testing.T) {
	root := os.Getenv("PIXIV_STATIC_ARTWORK_CHILD")
	if root == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: directTransport(func(r *http.Request) (*http.Response, error) {
		header := http.Header{"Content-Type": {"application/json"}}
		var body io.ReadCloser = io.NopCloser(bytes.NewReader(staticArtworkMetadata(44)))
		if r.URL.Host == "i.pximg.net" {
			header.Set("Content-Type", "image/png")
			body = io.NopCloser(bytes.NewReader([]byte("\x89PNG\r\n\x1a\nfixture")))
			if strings.Contains(r.URL.Path, "44_p1") {
				body = &staticCancelBody{ctx: r.Context(), root: root}
			}
		} else if r.URL.Host != "app-api.pixiv.net" {
			return nil, fmt.Errorf("unexpected host %s", r.URL.Host)
		}
		return &http.Response{StatusCode: 200, Header: header, Body: body, Request: r}, nil
	})}})
	if err != nil {
		t.Fatal(err)
	}
	manager := downloader.NewManager(client, root, "")
	app := runtime.NewApp(manager, nil, runtime.SDKPorts{OpenLease: func(context.Context, runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
		return lifecycle.NewLease(client, func() error {
			client.CloseIdleConnections()
			_ = json.NewEncoder(os.Stderr).Encode(map[string]any{"event": "closed"})
			return nil
		}), nil
	}}, runtime.Account{})
	server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
	Register(app, server)
	if err = mcpserver.RunStdio(ctx, server); err != nil {
		t.Fatal(err)
	}
}
func captureStaticArtworkCancellation(t *testing.T) staticArtworkCancellation {
	t.Helper()
	root := t.TempDir()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationMCPStaticArtworkCancellationChild$")
	command.Env = append(os.Environ(), "PIXIV_STATIC_ARTWORK_CHILD="+root)
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
	events := make(chan map[string]any, 8)
	go func() {
		scanner := bufio.NewScanner(stderr)
		for scanner.Scan() {
			var event map[string]any
			if json.Unmarshal(scanner.Bytes(), &event) == nil && event["event"] != nil {
				events <- event
			}
		}
		close(events)
	}()
	write := func(raw string) {
		if _, e := io.WriteString(stdin, raw+"\n"); e != nil {
			t.Fatal(e)
		}
	}
	scanner := bufio.NewScanner(stdout)
	read := func() []byte {
		if !scanner.Scan() {
			t.Fatalf("child stdio ended: %v", scanner.Err())
		}
		return append([]byte(nil), scanner.Bytes()...)
	}
	write(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`)
	_ = read()
	write(`{"jsonrpc":"2.0","method":"notifications/initialized"}`)
	write(`{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":{"src":"44"}}}`)
	var row staticArtworkCancellation
	row.Files = []directFile{}
	select {
	case event := <-events:
		if event["event"] != "waiting" {
			t.Fatalf("unexpected event: %+v", event)
		}
		row.PublishedBeforeCancel = int(event["published"].(float64))
		row.TemporaryBeforeCancel = int(event["temporary"].(float64))
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	if row.PublishedBeforeCancel != 1 || row.TemporaryBeforeCancel != 1 {
		t.Fatalf("cancellation did not reach published prefix/live temp: %+v", row)
	}
	write(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture"}}`)
	row.Response = normalizeDirectJSON(t, read(), root)
	_ = stdin.Close()
	if err = command.Wait(); err != nil {
		t.Fatal(err)
	}
	for event := range events {
		if event["event"] == "closed" {
			row.Closes++
		}
	}
	if row.Closes != 1 {
		t.Fatalf("cancellation lease closes=%d", row.Closes)
	}
	err = filepath.WalkDir(root, func(path string, e os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if e.IsDir() {
			return nil
		}
		if strings.HasPrefix(e.Name(), ".atomic-write-") {
			return errors.New("cancellation temporary file leaked")
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		row.Files = append(row.Files, directFile{filepath.ToSlash(relative), fmt.Sprintf("%x", body)})
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(row.Files) != 1 {
		t.Fatalf("cancellation prefix files=%d", len(row.Files))
	}
	return row
}

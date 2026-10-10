package download

import (
	"archive/zip"
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"image"
	"image/color"
	"image/png"
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

	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver"
	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/ugoira/staticlib"
	pixivservice "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateUgoiraWorkflow = flag.Bool("migration-update-download-ugoira", false, "capture frozen Go ugoira MCP workflow")

const ugoiraSourceDigest = "4318464a846c6a6701f1868adb48c0ccab67e0e4eb1a5a21e6f8c4fde89bbb02"
const ugoiraNativeHash = "b29ec70678bb7fb7401c63996940c8ff60c94802cdd891ade100641a1dabb784"

var ugoiraWorkflowHashes = map[string]string{
	"internal/mcpserver/pixiv/tools/download/download.go":                          "60cd7f8fcecfe27c185a9df4469dd8eaa0117e87586c047dfdf911e3400a11e5",
	"internal/mcpserver/pixiv/internal/runtime/runtime.go":                         "a6ca224d1949e7716277677f81f80c1133eb0b91e196dc64e04d29fad2c7fe79",
	"internal/mcpserver/stdio.go":                                                  "c48f34ffdcc0f92eadea6766aeb331ceab9899d982dcb59565900aa4adaeb46e",
	"internal/media/downloader/downloader.go":                                      "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
	"internal/media/downloader/ugoira_archive.go":                                  "8e2c3586290a2745a774705586caf6f6d06a3add04b7161d19285cd127a61c34",
	"internal/media/downloader/mime.go":                                            "aa7e39c8d974c81214b0d61d2f0264803f68db1c9c9f50b14c34eecd2ce3eb64",
	"internal/media/downloader/filename/filename.go":                               "e703ba7787b54a752cd9a697ca3e44a496e65618a48e29df7a5d41715d29dedb",
	"sdk/pixiv/reference.go":                                                       "7d467e3ae306fbd3d920f86330d80e1c6bd64787db6e56869fe76e77be468abc",
	"sdk/pixiv/ops_artwork.go":                                                     "f8aa00684b84463c6ba82b18db87d4c2e403a0dcaa048f3c3282f6555fd7445c",
	"sdk/pixiv/map_artwork.go":                                                     "45fe0a1d6b081ce2842ce536492b5a431bcf02940d94a29400b06cd8c441bb22",
	"sdk/pixiv/resource.go":                                                        "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
	"internal/services/pixiv/facade.go":                                            "99523f209e13554508cb7c991e47876efff2d1c5208382843a9969b4e51ee525",
	"internal/media/ugoira/ugoira.go":                                              "44fdf6867e56bc9beaa6f8be703d259cf92f55856f3329741b6ec3b8f49f3494",
	"internal/media/ugoira/rust.go":                                                "c5bec9ab428207fa279b305ba80c215e90f92d3cf3513a47270fd5199d906e3a",
	"internal/media/ugoira/rust_link_linux_amd64.go":                               "f5abe7d18597608de0e1078e7b0a5e01610cd4aa05e340882b41afc7f8f3beba",
	"internal/media/ugoira/rust/staticlib/manifest.json":                           "3624075e2e7b80457ec453c5e7468fdb1cc6efbf4840e4350422ffefd067c80b",
	"internal/media/ugoira/rust/staticlib/x86_64-unknown-linux-gnu/libugoira_rs.a": "b29ec70678bb7fb7401c63996940c8ff60c94802cdd891ade100641a1dabb784",
}

type ugoiraWorkflowCase struct {
	staticArtworkCase
	Batches      []json.RawMessage   `json:"manager_batches"`
	States       []leaseAccountState `json:"account_states"`
	InitialFiles []directFile        `json:"initial_files"`
}

type observedUgoiraManager struct {
	*downloader.Manager
	row  *ugoiraWorkflowCase
	root string
	mu   *sync.Mutex
}

func (m *observedUgoiraManager) Download(ctx context.Context, request downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	m.mu.Lock()
	m.row.ManagerRequests = append(m.row.ManagerRequests, staticArtworkRequest{request.IllustIDs, request.Pages, request.Quality, request.UgoiraFormat, strings.ReplaceAll(request.DownloadPath, m.root, "<ROOT>"), request.FilenameTemplate, request.DirectoryTemplate})
	m.mu.Unlock()
	batch, err := m.Manager.Download(ctx, request)
	items := make([]any, 0, len(batch.Items))
	for _, item := range batch.Items {
		files := make([]any, 0, len(item.Files))
		for _, file := range item.Files {
			files = append(files, map[string]any{"path": strings.ReplaceAll(file.Path, m.root, "<ROOT>"), "page": file.Page, "bytes": file.Bytes})
		}
		items = append(items, map[string]any{"illust_id": item.IllustID, "title": item.Title, "author": item.Author, "type": item.Type, "quality": item.Quality, "frames": item.Frames, "frame_report": item.FrameReport, "files": files})
	}
	failures := make([]any, 0, len(batch.Failures))
	for _, failure := range batch.Failures {
		failures = append(failures, map[string]any{"illust_id": failure.IllustID, "message": failure.Message, "code": failure.Code, "path": strings.ReplaceAll(failure.Path, m.root, "<ROOT>"), "missing": failure.Missing, "sdk_reason": sdk.ReasonOf(failure.Cause)})
	}
	operation := ""
	if err != nil {
		operation = err.Error()
	}
	raw, marshalErr := json.Marshal(map[string]any{"items": items, "failures": failures, "warnings": batch.Warnings, "operation_error": operation})
	if marshalErr != nil {
		panic(marshalErr)
	}
	m.mu.Lock()
	m.row.Batches = append(m.row.Batches, raw)
	m.mu.Unlock()
	return batch, err
}

func TestMigrationMCPDownloadUgoiraMatchesFrozenContract(t *testing.T) {
	if goruntime.GOOS != "linux" || goruntime.GOARCH != "amd64" {
		t.Skip("frozen native bytes require genuine tracked linux/amd64 Rust staticlib; other five targets remain pending")
	}
	base := filepath.Join("..", "..", "..", "..", "..")
	for path, want := range ugoiraWorkflowHashes {
		body, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != want {
			t.Fatalf("reference source changed: %s", path)
		}
	}
	digest, err := staticlib.CalculateRustSourceDigest(filepath.Join(base, "internal", "media", "ugoira", "rust"), filepath.Join(base, "third_party", "rust", "quantette-0.6.0"))
	if err != nil || digest != ugoiraSourceDigest {
		t.Fatalf("genuine Rust source identity: %s, %v", digest, err)
	}
	rows := []ugoiraWorkflowCase{}
	add := func(name string, id int64, mode string) {
		rows = append(rows, ugoiraWorkflowCase{staticArtworkCase: staticArtworkCase{Name: name, Arguments: map[string]any{"src": fmt.Sprint(id), "ugoira_mode": mode}}})
	}
	add("default-genuine-native-gif", 71, "")
	add("explicit-gif", 71, "gif")
	add("genuine-native-apng", 71, "apng")
	add("archive-original-preferred", 71, "zip")
	add("raw-is-zip-alias", 71, "raw")
	add("archive-medium-fallback", 72, "zip")
	add("native-medium-fallback", 72, "apng")
	add("archive-missing-frame-quarantined", 73, "zip")
	add("archive-undeclared-ordered-warning", 74, "raw")
	add("archive-corrupt-quarantined", 75, "zip")
	add("archive-duplicate-quarantined", 76, "zip")
	add("archive-unsafe-parent-prefix-quarantined", 77, "zip")
	add("archive-empty-quarantined", 78, "zip")
	add("metadata-no-archive-sdk-error", 79, "zip")
	add("metadata-duplicate-frame-sdk-error", 80, "raw")
	add("native-corrupt-image-preserves-old-output", 81, "gif")
	rows[len(rows)-1].Filename = "fixed"
	rows[len(rows)-1].Directory = "{id}"
	add("native-missing-frame-failure", 73, "apng")
	add("native-corrupt-archive-failure", 75, "gif")
	add("metadata-upstream-failure", 83, "gif")
	add("archive-resource-http-failure", 84, "raw")
	add("native-resource-http-failure", 84, "gif")
	add("archive-resource-read-failure", 85, "zip")
	add("native-resource-read-failure", 85, "apng")
	add("configured-filename-fallback-gif", 71, "gif")
	rows[len(rows)-1].Filename = "{unknown}"
	add("configured-filename-fallback-zip", 71, "zip")
	rows[len(rows)-1].Filename = "{unknown}"
	add("configured-empty-filename-fallback", 87, "zip")
	rows[len(rows)-1].Filename = "{tags}"
	add("configured-templates", 71, "apng")
	rows[len(rows)-1].Filename = "{author_id}_{id}_{num}_{date}_{tags}"
	rows[len(rows)-1].Directory = "{author}/{date}/{id}/{num}"
	add("published-date-local-day", 88, "zip")
	rows[len(rows)-1].Filename = "{date}-{id}"
	rows[len(rows)-1].Directory = "{date}"
	add("ugoira-page-selection-after-lease", 71, "raw")
	rows[len(rows)-1].Arguments.(map[string]any)["pages"] = "1"
	for _, quality := range []string{"regular", "small", "thumb", "mini"} {
		add("ugoira-"+quality+"-unsupported-after-lease", 71, "zip")
		rows[len(rows)-1].Arguments.(map[string]any)["quality"] = quality
	}
	for _, entry := range []struct{ name, field, value string }{{"invalid-pages-before-lease", "pages", "1,,2"}, {"invalid-quality-before-lease", "quality", "large"}, {"invalid-mode-before-lease", "ugoira_mode", "webm"}, {"invalid-delivery-before-lease", "delivery", "image_content"}, {"unknown-option-before-lease", "filename_template", "override"}} {
		add(entry.name, 71, "gif")
		rows[len(rows)-1].Arguments.(map[string]any)[entry.field] = entry.value
	}
	rows = append(rows, ugoiraWorkflowCase{staticArtworkCase: staticArtworkCase{Name: "mixed-static-direct-ugoira-prefix-failure", Arguments: map[string]any{"srcs": []string{"https://i.pximg.net/prefix.png", "42", "71", "73"}, "ugoira_mode": "zip"}}})
	add("configured-directory-operation-error", 71, "zip")
	rows[len(rows)-1].Directory = "../{id}"
	add("resource-context-cancel-operation-error", 86, "gif")
	var tool json.RawMessage
	for index := range rows {
		t.Run(rows[index].Name, func(t *testing.T) {
			captured := captureUgoiraWorkflowCase(t, &rows[index], index == 0)
			if index == 0 {
				tool = captured
			}
		})
	}
	metadata := map[string]json.RawMessage{}
	archives := map[string]string{}
	for _, id := range []int64{71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 83, 84, 85, 86, 87, 88} {
		metadata[fmt.Sprint(id)] = ugoiraWorkflowMetadata(id)
		archives[fmt.Sprint(id)] = fmt.Sprintf("%x", ugoiraWorkflowArchive(t, id))
	}
	output := struct {
		Source            string                     `json:"source"`
		SourceHashes      map[string]string          `json:"source_hashes"`
		Native            map[string]string          `json:"native_provenance"`
		RequestOrder      string                     `json:"request_order"`
		ObservedOmissions []string                   `json:"observed_mcp_omissions"`
		Pending           []string                   `json:"pending"`
		Metadata          map[string]json.RawMessage `json:"ugoira_metadata"`
		Archives          map[string]string          `json:"archive_hex"`
		Tool              json.RawMessage            `json:"tool"`
		Cancellation      ugoiraWorkflowCancellation `json:"stdio_cancellation"`
		Cases             []ugoiraWorkflowCase       `json:"cases"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", ugoiraWorkflowHashes, map[string]string{"platform": "linux/amd64", "source_digest": digest, "target": "x86_64-unknown-linux-gnu", "artifact_sha256": ugoiraNativeHash}, "single-artwork sequence retained; mixed parallel-artwork requests sorted by URL", []string{"archive item quality/frames/frame_report omitted from MCP item; files expose only quality/frames", "failure code/path/missing present in manager batch and quarantine disk artifacts but omitted from MCP failure"}, []string{"other five native staticlib target link/run", "native encode-in-progress and encode-gate cancellation schedules", "user/bookmark expansion, visual-record execution, random recommendation", "native platform path/replacement IO failures, HTTP2/Accept, SDK lifecycle and signal restoration"}, metadata, archives, tool, captureUgoiraWorkflowCancellation(t), rows}
	data, err := json.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_ugoira.json")
	if *updateUgoiraWorkflow {
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
		t.Fatal("ugoira MCP workflow differs from frozen Go contract")
	}
}

func captureUgoiraWorkflowCase(t *testing.T, row *ugoiraWorkflowCase, listTool bool) json.RawMessage {
	t.Helper()
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
	row.Batches = []json.RawMessage{}
	row.States = []leaseAccountState{}
	row.InitialFiles = []directFile{}
	if row.Name == "native-corrupt-image-preserves-old-output" {
		old := ugoiraWorkflowPNG(t, 0)
		relative := "81/fixed.gif"
		if err := os.MkdirAll(filepath.Join(dest, "81"), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dest, filepath.FromSlash(relative)), old, 0600); err != nil {
			t.Fatal(err)
		}
		row.InitialFiles = append(row.InitialFiles, directFile{relative, fmt.Sprintf("%x", old)})
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	db, err := database.Open(root)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
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
			mu.Lock()
			row.Opens = append(row.Opens, id)
			mu.Unlock()
			body = []byte(fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id))
		case "app-api.pixiv.net":
			id, _ := strconv.ParseInt(r.URL.Query().Get("illust_id"), 10, 64)
			switch r.URL.Path {
			case "/v1/illust/detail":
				body = ugoiraWorkflowArtwork(id)
			case "/v1/ugoira/metadata":
				if id == 83 {
					status = 404
					body = []byte(`{"error":{"message":"owned fixture metadata absent"}}`)
				} else {
					body = ugoiraWorkflowMetadata(id)
				}
			default:
				return nil, fmt.Errorf("unexpected API operation %s", r.URL.Path)
			}
		case "i.pximg.net":
			header.Set("Content-Type", "application/zip")
			id, _ := strconv.ParseInt(strings.TrimSuffix(filepath.Base(r.URL.Path), ".zip"), 10, 64)
			if strings.HasSuffix(r.URL.Path, ".png") || strings.HasSuffix(r.URL.Path, ".jpg") {
				header.Set("Content-Type", "image/png")
				body = ugoiraWorkflowPNG(t, 0)
			} else {
				body = ugoiraWorkflowArchive(t, id)
				if id == 84 {
					status = 503
				}
				if id == 85 {
					return &http.Response{StatusCode: status, Header: header, Body: &failedDirectBody{r.URL.String()}, Request: r}, nil
				}
				if id == 86 {
					return nil, context.Canceled
				}
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
		manager := downloader.NewManager(c, dest, row.Filename)
		manager.SetDirectoryTemplate(row.Directory)
		return &observedUgoiraManager{Manager: manager, row: row, root: dest, mu: &mu}
	}, runtime.SDKPorts{OpenLease: func(ctx context.Context, _ runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
		return facade.Open(ctx, pixivservice.Request{Options: pixiv.Options{HTTPClient: httpClient}})
	}, Execute: func(context.Context, runtime.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
		row.ExecuteCalls++
		return errors.New("unexpected pool execution")
	}}, runtime.Account{})
	server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
	Register(app, server)
	ct, st := mcp.NewInMemoryTransports()
	stopped := make(chan struct{})
	go func() { defer close(stopped); _ = server.Run(ctx, st) }()
	conn, err := ct.Connect(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = conn.Close(); cancel(); <-stopped }()
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
	row.Response = normalizeDirectJSON(t, read(), dest)
	row.Acquired, row.Released = gate.acquired, gate.released
	for _, id := range []int64{42, 43} {
		state, e := db.GetPixiv(ctx, id)
		if e != nil {
			t.Fatal(e)
		}
		row.States = append(row.States, leaseAccountState{id, state.CredentialRevision, state.PoolFrozenUntil != nil, state.PoolLastSelected})
	}
	row.Files, row.Directories = ugoiraWorkflowDisk(t, dest)
	if row.Name == "mixed-static-direct-ugoira-prefix-failure" {
		sort.Slice(row.Requests, func(a, b int) bool { return row.Requests[a].URL < row.Requests[b].URL })
	}
	for _, request := range row.Requests {
		if strings.Contains(request.URL, "i.pximg.net") && (request.Authorization != "" || request.Cookie != "" || request.Referer != "https://app-api.pixiv.net/") {
			t.Fatalf("opaque resource credential boundary: %+v", request)
		}
	}
	if row.PoolLoads != 0 || row.PoolFactories != 0 || row.ExecuteCalls != 0 {
		t.Fatalf("MCP entered CLI pool: %+v", row)
	}
	if len(row.Opens) > 0 && (len(row.Opens) != 1 || row.Opens[0] != 43 || row.Closes != 1 || row.Acquired != 1 || row.Released != 1) {
		t.Fatalf("default lease lifecycle: %+v", row)
	}
	if strings.Contains(row.Name, "before-lease") && (len(row.Opens) != 0 || row.Acquired != 0 || len(row.ManagerRequests) != 0) {
		t.Fatalf("validation opened lease: %s", row.Name)
	}
	for _, request := range row.ManagerRequests {
		if request.Filename != row.Filename || request.Directory != row.Directory || request.Path != "<ROOT>" {
			t.Fatalf("configured defaults not forwarded: %+v", request)
		}
	}
	if len(row.InitialFiles) > 0 && (len(row.Files) != 1 || row.Files[0] != row.InitialFiles[0]) {
		t.Fatalf("native failure changed old output: %+v", row.Files)
	}
	verifyUgoiraWorkflowResponse(t, row, dest)
	return tool
}

func verifyUgoiraWorkflowResponse(t *testing.T, row *ugoiraWorkflowCase, root string) {
	t.Helper()
	var actual struct {
		Result struct {
			Content []struct {
				Type string `json:"type"`
			} `json:"content"`
			StructuredContent downloadOut `json:"structuredContent"`
		} `json:"result"`
	}
	if err := json.Unmarshal(row.Response, &actual); err != nil {
		t.Fatal(err)
	}
	grouped := []downloadFileOut{}
	for _, item := range actual.Result.StructuredContent.Items {
		grouped = append(grouped, item.Files...)
		if item.IllustID > 0 && item.URL != fmt.Sprintf("https://www.pixiv.net/artworks/%d", item.IllustID) {
			t.Fatalf("noncanonical artwork URL: %+v", item)
		}
	}
	files := actual.Result.StructuredContent.Files
	if len(files) != len(grouped) {
		t.Fatal("grouped/flat file count mismatch")
	}
	for index, file := range files {
		a, _ := json.Marshal(file)
		b, _ := json.Marshal(grouped[index])
		if !bytes.Equal(a, b) {
			t.Fatal("grouped/flat file mismatch")
		}
		path := strings.ReplaceAll(file.Path, "<ROOT>", root)
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if file.SizeBytes != info.Size() || file.MIMEType != downloader.MimeTypeForFile(path) || !strings.HasPrefix(file.FileURI, "file://<ROOT>/") {
			t.Fatalf("published local file metadata: %+v", file)
		}
		if file.Page != 1 {
			t.Fatalf("ugoira/static page contract: %+v", file)
		}
	}
	for _, content := range actual.Result.Content {
		if content.Type != "text" {
			t.Fatalf("nontext content %s", content.Type)
		}
	}
	if strings.Contains(string(row.Response), "signature=private") || strings.Contains(string(row.Response), "fixture-access") {
		t.Fatal("wire response leaked resource query or credential")
	}
}

func ugoiraWorkflowDisk(t *testing.T, root string) ([]directFile, []string) {
	t.Helper()
	files := []directFile{}
	directories := []string{}
	err := filepath.WalkDir(root, func(path string, entry os.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if path == root {
			return nil
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		relative = filepath.ToSlash(relative)
		if entry.IsDir() {
			directories = append(directories, relative)
			return nil
		}
		if strings.HasPrefix(entry.Name(), ".atomic-write-") || strings.HasPrefix(entry.Name(), ".ugoira-") || strings.HasPrefix(entry.Name(), "ugoira-") {
			return fmt.Errorf("temporary ugoira file leaked: %s", relative)
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		files = append(files, directFile{relative, fmt.Sprintf("%x", body)})
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return files, directories
}

func ugoiraWorkflowArtwork(id int64) []byte {
	if id == 42 {
		return staticArtworkMetadata(id)
	}
	tags := []any{map[string]any{"name": " one "}, map[string]any{"name": "two"}}
	if id == 87 {
		tags = []any{}
	}
	date := "2026-10-08T12:34:56+09:00"
	if id == 88 {
		date = "2026-10-08T00:30:00+09:00"
	}
	raw, _ := json.Marshal(map[string]any{"illust": map[string]any{"id": id, "title": "Owned / ugoira", "type": "ugoira", "page_count": 1, "create_date": date, "user": map[string]any{"id": 7, "name": "Artist / name"}, "tags": tags}})
	return raw
}
func ugoiraWorkflowMetadata(id int64) []byte {
	urls := map[string]string{"medium": fmt.Sprintf("https://i.pximg.net/medium/%d.zip?signature=private", id), "original": fmt.Sprintf("https://i.pximg.net/original/%d.zip?signature=private", id)}
	if id == 72 {
		delete(urls, "original")
	}
	if id == 79 {
		urls = map[string]string{}
	}
	frames := []any{map[string]any{"file": "000000.png", "delay": 41}, map[string]any{"file": "000001.png", "delay": 79}}
	if id == 80 {
		frames[1] = map[string]any{"file": "000000.png", "delay": 79}
	}
	raw, _ := json.Marshal(map[string]any{"ugoira_metadata": map[string]any{"zip_urls": urls, "frames": frames}})
	return raw
}
func ugoiraWorkflowPNG(t *testing.T, index int) []byte {
	t.Helper()
	canvas := image.NewNRGBA(image.Rect(0, 0, 2, 2))
	shades := []color.NRGBA{{R: 240, G: 20, B: 40, A: 255}, {R: 20, G: 80, B: 240, A: 255}}
	for y := 0; y < 2; y++ {
		for x := 0; x < 2; x++ {
			canvas.SetNRGBA(x, y, shades[(index+x+y)%2])
		}
	}
	var body bytes.Buffer
	if err := png.Encode(&body, canvas); err != nil {
		t.Fatal(err)
	}
	return body.Bytes()
}
func ugoiraWorkflowArchive(t *testing.T, id int64) []byte {
	t.Helper()
	names := []string{"000001.png", "000000.png"}
	switch id {
	case 73:
		names = []string{"000000.png"}
	case 74:
		names = []string{"ignored/", "__MACOSX/side.png", "extra-b.png", "000001.png", "extra-a.png", "000000.png"}
	case 76:
		names = []string{"000000.png", "000000.png", "000001.png"}
	case 77:
		names = []string{"000000.png", "..foo.png", "000001.png"}
	case 78:
		names = []string{"ignored/", "__MACOSX/side.png"}
	}
	var body bytes.Buffer
	archive := zip.NewWriter(&body)
	for index, name := range names {
		entry, err := archive.CreateHeader(&zip.FileHeader{Name: name, Method: zip.Store})
		if err != nil {
			t.Fatal(err)
		}
		data := ugoiraWorkflowPNG(t, index%2)
		if strings.HasSuffix(name, "/") {
			data = nil
		}
		if id == 81 {
			data = data[:12]
		}
		if _, err = entry.Write(data); err != nil {
			t.Fatal(err)
		}
	}
	if err := archive.Close(); err != nil {
		t.Fatal(err)
	}
	data := body.Bytes()
	if id == 75 {
		data = data[:len(data)-30]
	}
	return data
}

type ugoiraWorkflowCancellation struct {
	Response                     json.RawMessage `json:"response"`
	Files                        []directFile    `json:"files"`
	Directories                  []string        `json:"directories"`
	PublishedBeforeCancel        int             `json:"published_before_cancel"`
	ArchiveTemporaryBeforeCancel int             `json:"archive_temporary_before_cancel"`
	AtomicTemporaryBeforeCancel  int             `json:"atomic_temporary_before_cancel"`
	Closes                       int             `json:"closes"`
}

type ugoiraCancelBody struct {
	ctx  context.Context
	root string
	once sync.Once
}

func (body *ugoiraCancelBody) Read([]byte) (int, error) {
	body.once.Do(func() {
		for {
			entries, err := os.ReadDir(body.root)
			if err != nil {
				panic(err)
			}
			prefix := false
			for _, entry := range entries {
				if !entry.IsDir() && strings.HasSuffix(entry.Name(), "_42.png") {
					prefix = true
				}
			}
			if prefix {
				break
			}
			select {
			case <-body.ctx.Done():
				return
			case <-time.After(time.Millisecond):
			}
		}
		published, archives, atomic := 0, 0, 0
		_ = filepath.WalkDir(body.root, func(_ string, entry os.DirEntry, err error) error {
			if err == nil && !entry.IsDir() {
				switch {
				case strings.HasPrefix(entry.Name(), ".atomic-write-"):
					atomic++
				case strings.HasPrefix(entry.Name(), "ugoira-"):
					archives++
				default:
					published++
				}
			}
			return err
		})
		_ = json.NewEncoder(os.Stderr).Encode(map[string]any{"event": "waiting", "published": published, "archive_temporary": archives, "atomic_temporary": atomic})
	})
	<-body.ctx.Done()
	return 0, body.ctx.Err()
}
func (*ugoiraCancelBody) Close() error { return nil }

func TestMigrationMCPUgoiraCancellationChild(t *testing.T) {
	root := os.Getenv("PIXIV_UGOIRA_WORKFLOW_CHILD")
	if root == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: directTransport(func(request *http.Request) (*http.Response, error) {
		header := http.Header{"Content-Type": {"application/json"}}
		var body io.ReadCloser
		switch request.URL.Host {
		case "app-api.pixiv.net":
			if request.URL.Path == "/v1/illust/detail" {
				id, _ := strconv.ParseInt(request.URL.Query().Get("illust_id"), 10, 64)
				body = io.NopCloser(bytes.NewReader(ugoiraWorkflowArtwork(id)))
			} else if request.URL.Path == "/v1/ugoira/metadata" {
				body = io.NopCloser(bytes.NewReader(ugoiraWorkflowMetadata(71)))
			} else {
				return nil, fmt.Errorf("unexpected API operation %s", request.URL.Path)
			}
		case "i.pximg.net":
			if strings.HasSuffix(request.URL.Path, ".png") || strings.HasSuffix(request.URL.Path, ".jpg") {
				header.Set("Content-Type", "image/png")
				body = io.NopCloser(bytes.NewReader(ugoiraWorkflowPNG(t, 0)))
			} else {
				header.Set("Content-Type", "application/zip")
				body = &ugoiraCancelBody{ctx: request.Context(), root: root}
			}
		default:
			return nil, fmt.Errorf("unexpected owned host %s", request.URL.Host)
		}
		return &http.Response{StatusCode: 200, Header: header, Body: body, Request: request}, nil
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

func captureUgoiraWorkflowCancellation(t *testing.T) ugoiraWorkflowCancellation {
	t.Helper()
	root := t.TempDir()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationMCPUgoiraCancellationChild$")
	command.Env = append(os.Environ(), "PIXIV_UGOIRA_WORKFLOW_CHILD="+root)
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
	defer command.Process.Kill()
	events := make(chan map[string]any, 8)
	scanDone := make(chan error, 1)
	go func() {
		defer close(events)
		scanner := bufio.NewScanner(stderr)
		for scanner.Scan() {
			var event map[string]any
			if json.Unmarshal(scanner.Bytes(), &event) == nil && event["event"] != nil {
				events <- event
			}
		}
		scanDone <- scanner.Err()
	}()
	lines := make(chan []byte, 8)
	outDone := make(chan error, 1)
	go func() {
		defer close(lines)
		scanner := bufio.NewScanner(stdout)
		scanner.Buffer(make([]byte, 4096), 1024*1024)
		for scanner.Scan() {
			lines <- append([]byte(nil), scanner.Bytes()...)
		}
		outDone <- scanner.Err()
	}()
	write := func(raw string) {
		t.Helper()
		if _, e := io.WriteString(stdin, raw+"\n"); e != nil {
			t.Fatal(e)
		}
	}
	read := func() []byte {
		t.Helper()
		select {
		case raw, ok := <-lines:
			if !ok {
				t.Fatal("child stdio ended")
			}
			return raw
		case <-ctx.Done():
			t.Fatal(ctx.Err())
			return nil
		}
	}
	write(`{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"0"}}}`)
	_ = read()
	write(`{"jsonrpc":"2.0","method":"notifications/initialized"}`)
	write(`{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"download","arguments":{"srcs":["42","71"],"ugoira_mode":"apng"}}}`)
	var row ugoiraWorkflowCancellation
	select {
	case event := <-events:
		if event["event"] != "waiting" {
			t.Fatalf("unexpected cancellation event %+v", event)
		}
		row.PublishedBeforeCancel = int(event["published"].(float64))
		row.ArchiveTemporaryBeforeCancel = int(event["archive_temporary"].(float64))
		row.AtomicTemporaryBeforeCancel = int(event["atomic_temporary"].(float64))
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	if row.PublishedBeforeCancel != 1 || row.ArchiveTemporaryBeforeCancel != 1 || row.AtomicTemporaryBeforeCancel != 1 {
		t.Fatalf("cancellation lacked published prefix and archive/save temporaries: %+v", row)
	}
	write(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture"}}`)
	row.Response = normalizeDirectJSON(t, read(), root)
	_ = stdin.Close()
	for event := range events {
		if event["event"] == "closed" {
			row.Closes++
		}
	}
	if err = <-scanDone; err != nil {
		t.Fatal(err)
	}
	for range lines {
	}
	if err = <-outDone; err != nil {
		t.Fatal(err)
	}
	if err = command.Wait(); err != nil {
		t.Fatal(err)
	}
	row.Files, row.Directories = ugoiraWorkflowDisk(t, root)
	if row.Closes != 1 || len(row.Files) != 1 || !strings.HasSuffix(row.Files[0].Name, "_42.png") {
		t.Fatalf("cancellation lost prefix, leaked temporary, or lease: %+v", row)
	}
	return row
}

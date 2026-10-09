package download

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
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	"github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateDownloadDirect = flag.Bool("migration-update-download-direct", false, "capture frozen Go direct download MCP contract")

type directManager struct{ path string }

func (d *directManager) SetDownloadPath(string) error { return nil }
func (d *directManager) DownloadPath() string         { return d.path }
func (d *directManager) Download(context.Context, downloader.DownloadRequest) (downloader.DownloadBatchResult, error) {
	return downloader.DownloadBatchResult{}, errors.New("unexpected artwork manager call")
}

type directTransport func(*http.Request) (*http.Response, error)

func (f directTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type failedDirectBody struct{ source string }

func (b *failedDirectBody) Read([]byte) (int, error) {
	return 0, fmt.Errorf("read failed %s signature=private", b.source)
}
func (*failedDirectBody) Close() error { return nil }

type directWireRequest struct {
	URL           string `json:"url"`
	Method        string `json:"method"`
	Referer       string `json:"referer"`
	Authorization string `json:"authorization"`
	Cookie        string `json:"cookie"`
}
type directFile struct {
	Name string `json:"name"`
	Hex  string `json:"hex"`
}
type directCase struct {
	Name      string              `json:"name"`
	Arguments any                 `json:"arguments"`
	Cancel    bool                `json:"cancel,omitempty"`
	Response  json.RawMessage     `json:"response"`
	Requests  []directWireRequest `json:"requests"`
	Files     []directFile        `json:"files"`
	Opens     int                 `json:"opens"`
}

func TestMigrationMCPDownloadDirectMatchesFrozenContract(t *testing.T) {
	sources := map[string]string{
		"internal/mcpserver/pixiv/tools/download/download.go": "60cd7f8fcecfe27c185a9df4469dd8eaa0117e87586c047dfdf911e3400a11e5",
		"internal/media/downloader/downloader.go":             "2ea84cf1ab3eaf8bec1b3dc5b2f5162ba48071b0a7ae5e2a950043dcc4867979",
		"internal/media/downloader/mime.go":                   "aa7e39c8d974c81214b0d61d2f0264803f68db1c9c9f50b14c34eecd2ce3eb64",
		"sdk/pixiv/resource.go":                               "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
	}
	for path, want := range sources {
		body, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "..", filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != want {
			t.Fatalf("reference source changed: %s", path)
		}
	}
	const source = "https://i.pximg.net/img/asset.png?signature=private"
	rows := []directCase{
		{Name: "default-success", Arguments: map[string]any{"src": source}},
		{Name: "srcs-duplicate-query", Arguments: map[string]any{"srcs": []string{source, source, source + "&v=2"}}},
		{Name: "trimmed-source-and-options", Arguments: map[string]any{"src": "  " + source + "  ", "pages": "3,1-2,2", "quality": " mini ", "ugoira_mode": " raw ", "delivery": " local_path "}},
		{Name: "partial-read-failure", Arguments: map[string]any{"srcs": []string{source, "https://i.pximg.net/read.png?signature=private"}}},
		{Name: "partial-cancel", Arguments: map[string]any{"srcs": []string{source, "https://i.pximg.net/cancel.png?signature=private"}}, Cancel: true},
		{Name: "cancel-before-publication", Arguments: map[string]any{"src": "https://i.pximg.net/cancel.png?signature=private"}, Cancel: true},
		{Name: "forbidden-host", Arguments: map[string]any{"src": "https://example.invalid/asset.png?signature=private"}},
		{Name: "missing-basename", Arguments: map[string]any{"src": "https://i.pximg.net/"}},
		{Name: "unsafe-basename", Arguments: map[string]any{"src": "https://i.pximg.net/a%2Fb%3F.png?signature=private"}},
		{Name: "signature-mime", Arguments: map[string]any{"src": "https://i.pximg.net/asset.jpg?signature=private"}},
		{Name: "invalid-source", Arguments: map[string]any{"src": "private-invalid-source"}},
		{Name: "empty-source-list", Arguments: map[string]any{"srcs": []string{}}},
		{Name: "null-source-list", Arguments: map[string]any{"srcs": nil}},
		{Name: "wrong-source-list-element", Arguments: map[string]any{"srcs": []any{42}}},
		{Name: "array-arguments", Arguments: []any{}},
		{Name: "http-error", Arguments: map[string]any{"src": "https://i.pximg.net/status.png?signature=private"}},
		{Name: "missing-source", Arguments: map[string]any{}},
		{Name: "both-sources", Arguments: map[string]any{"src": source, "srcs": []string{source}}},
		{Name: "unsupported-delivery", Arguments: map[string]any{"src": source, "delivery": "image_content"}},
		{Name: "invalid-pages", Arguments: map[string]any{"src": source, "pages": "1,,2"}},
		{Name: "invalid-quality", Arguments: map[string]any{"src": source, "quality": "large"}},
		{Name: "invalid-ugoira-mode", Arguments: map[string]any{"src": source, "ugoira_mode": "webm"}},
		{Name: "unknown-field", Arguments: map[string]any{"src": source, "concurrency": 1}},
		{Name: "wrong-source-type", Arguments: map[string]any{"src": 42}},
		{Name: "null-source", Arguments: map[string]any{"src": nil}},
	}
	var tool json.RawMessage
	for i := range rows {
		row := &rows[i]
		root := t.TempDir()
		row.Requests = []directWireRequest{}
		row.Files = []directFile{}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		started := make(chan struct{})
		var mutex sync.Mutex
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: directTransport(func(r *http.Request) (*http.Response, error) {
			mutex.Lock()
			row.Requests = append(row.Requests, directWireRequest{r.URL.String(), r.Method, r.Header.Get("Referer"), r.Header.Get("Authorization"), r.Header.Get("Cookie")})
			mutex.Unlock()
			if r.URL.Path == "/cancel.png" {
				close(started)
				<-r.Context().Done()
				return nil, r.Context().Err()
			}
			var body io.ReadCloser = io.NopCloser(bytes.NewReader([]byte("\x89PNG\r\n\x1a\nfixture")))
			if r.URL.Path == "/read.png" {
				body = &failedDirectBody{r.URL.String()}
			}
			status := 200
			if r.URL.Path == "/status.png" {
				status = 503
			}
			return &http.Response{StatusCode: status, Header: http.Header{"Content-Type": {"image/png"}}, Body: body, Request: r}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		app := runtime.NewApp(&directManager{root}, nil, runtime.SDKPorts{Open: func(runtime.Account) (*pixiv.Client, error) { row.Opens++; return client, nil }}, runtime.Account{})
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
		raw, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": map[string]any{"name": "download", "arguments": row.Arguments}})
		send(raw)
		if row.Cancel {
			select {
			case <-started:
			case <-ctx.Done():
				t.Fatal(ctx.Err())
			}
			send([]byte(`{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7,"reason":"fixture"}}`))
		}
		row.Response = normalizeDirectJSON(t, read(), root)
		_ = conn.Close()
		cancel()
		client.CloseIdleConnections()
		entries, err := os.ReadDir(root)
		if err != nil {
			t.Fatal(err)
		}
		for _, entry := range entries {
			if strings.HasPrefix(entry.Name(), ".atomic-write-") {
				t.Fatal("temporary destination leaked")
			}
			if entry.IsDir() {
				t.Fatal("unexpected directory")
			}
			body, e := os.ReadFile(filepath.Join(root, entry.Name()))
			if e != nil {
				t.Fatal(e)
			}
			row.Files = append(row.Files, directFile{entry.Name(), fmt.Sprintf("%x", body)})
		}
		if len(row.Requests) > 0 {
			for _, request := range row.Requests {
				if request.Authorization != "" || request.Cookie != "" || request.Referer != "https://app-api.pixiv.net/" {
					t.Fatalf("credential boundary: %+v", request)
				}
			}
		}
	}
	output := struct {
		Source       string            `json:"source"`
		SourceHashes map[string]string `json:"source_hashes"`
		Pending      []string          `json:"pending"`
		Tool         json.RawMessage   `json:"tool"`
		Cases        []directCase      `json:"cases"`
	}{"4b4426487ef18bed276706daec385e0d0a6979f9", sources, []string{"artwork PID/URL and visual record success", "user/bookmark expansion", "static page/quality/naming/publication", "ugoira archive/conversion", "random recommendations", "native platform paths and IO"}, tool, rows}
	data, err := json.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "..", "crates", "pixiv-mcp", "tests", "fixtures", "download_direct.json")
	if *updateDownloadDirect {
		if err = os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("direct download differs from frozen Go contract")
	}
}
func normalizeDirectJSON(t *testing.T, raw []byte, root string) json.RawMessage {
	t.Helper()
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		t.Fatal(err)
	}
	var visit func(any) any
	visit = func(v any) any {
		switch x := v.(type) {
		case string:
			return strings.ReplaceAll(x, root, "<ROOT>")
		case []any:
			for i := range x {
				x[i] = visit(x[i])
			}
		case map[string]any:
			for k, v := range x {
				x[k] = visit(v)
			}
		}
		return v
	}
	data, err := json.Marshal(visit(value))
	if err != nil {
		t.Fatal(err)
	}
	return data
}

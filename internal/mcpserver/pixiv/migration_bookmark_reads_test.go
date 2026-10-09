package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationMCPBookmarkReadsAllowDiagnosticOrder = flag.Bool("migration-mcp-bookmark-reads-allow-diagnostic-order", false, "allow observed fixed Go first-violation alternatives only for two multi-invalid aggregate inputs")

var migrationUpdateMCPBookmarkReads = flag.Bool("migration-update-mcp-bookmark-reads", false, "capture fixed Go MCP bookmark reads contracts")

func TestMigrationMCPBookmarkReadsMatchesFrozenContracts(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	type row struct {
		NoIdentity bool              `json:"no_identity,omitempty"`
		Name       string            `json:"name"`
		ToolName   string            `json:"tool_name"`
		Arguments  any               `json:"arguments"`
		Bodies     []json.RawMessage `json:"bodies"`
		Result     json.RawMessage   `json:"result"`
		Calls      int               `json:"calls"`
		Requests   int               `json:"requests"`
		Paths      []string          `json:"paths"`
		Queries    []string          `json:"queries"`
		RPCCode    int               `json:"rpc_code"`
		RPCError   string            `json:"rpc_error"`
	}
	var rows []row
	for _, tool := range []string{"bookmark_detail", "novel_bookmark_detail"} {
		field := "illust_id"
		if tool == "novel_bookmark_detail" {
			field = "novel_id"
		}
		body := json.RawMessage(`{"bookmark_detail":{"is_bookmarked":true,"restrict":"private","tags":[{"name":"猫","is_registered":true},{"name":"ignored","is_registered":false},{"name":"猫","is_registered":true}]}}`)
		for i, args := range []any{map[string]any{field: 22}, map[string]any{}, map[string]any{field: 0}, map[string]any{field: -1}, map[string]any{field: nil}, map[string]any{field: 1.5}, map[string]any{field: "22"}, map[string]any{field: int64(9223372036854775807)}, map[string]any{field: 22, "extra": 1}, nil, 7, "invalid", []any{}, true} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-argument-%d", tool, i), ToolName: tool, Arguments: args, Bodies: []json.RawMessage{body}})
		}
		for i, payload := range []string{`{}`, `{"bookmark_detail":null}`, `{"bookmark_detail":{"is_bookmarked":false,"restrict":"private","tags":[{"name":"ignored","is_registered":true}]}}`, `{"bookmark_detail":{"restrict":"public","tags":[]}}`, `{"bookmark_detail":{"restrict":"unexpected","tags":[{"name":"","is_registered":true},null]}}`, `{"bookmark_detail":{"tags":[1]}}`} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-body-%d", tool, i), ToolName: tool, Arguments: map[string]any{field: 22}, Bodies: []json.RawMessage{json.RawMessage(payload)}})
		}
	}
	for _, tool := range []string{"bookmark_tags", "novel_bookmark_tags", "bookmark_tags_all"} {
		first := json.RawMessage(`{"bookmark_tags":[{"name":"猫","count":2},{"name":"猫","count":3},{"name":"blue","count":9007199254740993}],"next_url":null}`)
		bodies := []json.RawMessage{first}
		if tool == "bookmark_tags_all" {
			bodies = append(bodies, first)
		}
		for i, args := range []any{map[string]any{}, map[string]any{"user_id": 7}, map[string]any{"user_id": 0}, map[string]any{"user_id": -1}, map[string]any{"user_id": nil}, map[string]any{"user_id": 1.5}, map[string]any{"user_id": "42"}, map[string]any{"user_id": int64(9223372036854775807)}, map[string]any{"restrict": "public"}, map[string]any{"restrict": "private"}, map[string]any{"restrict": "wrong"}, map[string]any{"restrict": nil}, map[string]any{"extra": true}, map[string]any{"limit": 0}, map[string]any{"limit": 1}, map[string]any{"limit": 2}, map[string]any{"limit": 3}, map[string]any{"limit": 4}, map[string]any{"limit": 6}, map[string]any{"page": 2, "limit": 2}, map[string]any{"page": 3, "limit": 2}, map[string]any{"page": 4, "limit": 2}, map[string]any{"page": 2}, map[string]any{"page": 0}, map[string]any{"limit": -1}, map[string]any{"page": 0, "user_id": -1}, map[string]any{"limit": -1, "user_id": -1}, map[string]any{"limit": int64(9223372036854775807)}, map[string]any{"page": int64(9223372036854775807), "limit": 2}, map[string]any{"page": 1.5}, map[string]any{"limit": "2"}, map[string]any{"page": nil}, map[string]any{"limit": nil}, nil, 7, "invalid", []any{}, true} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-argument-%d", tool, i), ToolName: tool, Arguments: args, Bodies: bodies})
		}
		for i, payload := range []string{`{}`, `{"bookmark_tags":null}`, `{"bookmark_tags":[]}`, `{"bookmark_tags":[null]}`, `{"bookmark_tags":[{"name":""}]}`, `{"bookmark_tags":[{"name":"negative","count":-1}]}`, `{"bookmark_tags":[{"name":"missing count"}]}`} {
			bs := []json.RawMessage{json.RawMessage(payload)}
			if tool == "bookmark_tags_all" {
				bs = append(bs, first)
			}
			rows = append(rows, row{Name: fmt.Sprintf("%s-body-%d", tool, i), ToolName: tool, Arguments: map[string]any{"limit": 0}, Bodies: bs})
		}
		for i, payload := range []string{`{"bookmark_tags":[{"name":"same","count":2},{"name":"same","count":2}]}`, `{"bookmark_tags":[{"name":"max","count":9223372036854775807},{"name":"min","count":-9223372036854775808}]}`} {
			bs := []json.RawMessage{json.RawMessage(payload)}
			if tool == "bookmark_tags_all" {
				bs = append(bs, json.RawMessage(payload))
			}
			rows = append(rows, row{Name: fmt.Sprintf("%s-boundary-%d", tool, i), ToolName: tool, Arguments: map[string]any{"limit": 0}, Bodies: bs})
		}
		for i, args := range []any{map[string]any{}, map[string]any{"page": 0}, map[string]any{"limit": -1}} {
			rows = append(rows, row{Name: fmt.Sprintf("%s-no-identity-%d", tool, i), ToolName: tool, Arguments: args, Bodies: bodies, NoIdentity: true})
		}
		if tool != "novel_bookmark_tags" {
			continuing := json.RawMessage(`{"bookmark_tags":[{"name":"first","count":1},{"name":"first","count":2}],"next_url":"https://app-api.pixiv.net/v1/user/bookmark-tags/illust?user_id=42&offset=30"}`)
			for i, args := range []map[string]any{{}, {"limit": 0}, {"page": 2, "limit": 2}, {"limit": 3}, {"limit": 4}, {"limit": 5}} {
				bs := []json.RawMessage{continuing, first}
				if tool == "bookmark_tags_all" {
					bs = append(bs, first)
				}
				rows = append(rows, row{Name: fmt.Sprintf("%s-continuation-%d", tool, i), ToolName: tool, Arguments: args, Bodies: bs})
			}
		}
	}

	var err error
	var data []byte
	tools := map[string]json.RawMessage{}
	for index := range rows {
		row := &rows[index]
		client, _, err := pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}`)), Request: req}, nil
			}
			bodyIndex := row.Requests
			row.Requests++
			row.Queries = append(row.Queries, req.URL.RawQuery)
			row.Paths = append(row.Paths, req.URL.Path)
			if bodyIndex >= len(row.Bodies) {
				t.Fatalf("unexpected HTTP request %d", row.Requests)
			}
			endpoint := "/v1/user/bookmark-tags/illust"
			switch row.ToolName {
			case "bookmark_detail":
				endpoint = "/v2/illust/bookmark/detail"
			case "novel_bookmark_detail":
				endpoint = "/v2/novel/bookmark/detail"
			case "novel_bookmark_tags":
				endpoint = "/v1/user/bookmark-tags/novel"
			case "bookmark_tags_all":
				if strings.Contains(req.URL.Path, "/novel") {
					endpoint = "/v1/user/bookmark-tags/novel"
				}
			}
			if req.Method != http.MethodGet || req.URL.Path != endpoint || req.Header.Get("Authorization") != "Bearer fixture-access" {
				t.Fatalf("unexpected request %s", req.URL)
			}

			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[bodyIndex])), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		if row.NoIdentity {
			client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(*http.Request) (*http.Response, error) {
				t.Error("missing identity must fail before content requests")
				return nil, fmt.Errorf("unexpected request")
			})}})
			if err != nil {
				t.Fatal(err)
			}
		}
		server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			row.Calls++
			_, err := invoke(ctx, client)
			return err
		}}, pixivmcp.Account{})
		ctx, cancel := context.WithCancel(context.Background())
		clientTransport, serverTransport := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, serverTransport) }()
		capture := &migrationBookmarkReadCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, migrationBookmarkReadCaptureTransport{clientTransport, capture}, nil)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		if index == 0 {
			listing, err := session.ListTools(context.Background(), nil)
			if err != nil {
				t.Fatal(err)
			}
			for _, item := range listing.Tools {
				if item.Name == "bookmark_detail" || item.Name == "novel_bookmark_detail" || item.Name == "bookmark_tags" || item.Name == "novel_bookmark_tags" || item.Name == "bookmark_tags_all" {
					tools[item.Name], err = json.Marshal(item)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
		}
		_, callErr := session.CallTool(context.Background(), &mcp.CallToolParams{Name: row.ToolName, Arguments: row.Arguments})
		if callErr != nil {
			row.RPCError = callErr.Error()
		}
		capture.mu.Lock()
		row.Result = capture.result
		row.RPCCode = capture.code
		capture.mu.Unlock()
		if strings.HasSuffix(row.Name, "-argument-0") {
			var result struct {
				IsError    bool `json:"isError"`
				Structured struct {
					Bookmarked   bool              `json:"bookmarked"`
					Tags         []string          `json:"tags"`
					BookmarkTags []json.RawMessage `json:"bookmark_tags"`
				} `json:"structuredContent"`
			}
			if row.RPCError != "" {
				t.Fatalf("%s expected successful RPC: %s", row.Name, row.RPCError)
			}
			if err := json.Unmarshal(row.Result, &result); err != nil {
				t.Fatal(err)
			}
			if result.IsError {
				t.Fatalf("%s expected success: %s", row.Name, row.Result)
			}
			if strings.Contains(row.ToolName, "detail") {
				if !result.Structured.Bookmarked || len(result.Structured.Tags) != 2 {
					t.Fatalf("malformed detail fixture: %s", row.Result)
				}
			} else if len(result.Structured.BookmarkTags) != 3 {
				t.Fatalf("malformed tag fixture: %s", row.Result)
			}
		}

		expected := -1
		if !strings.Contains(row.ToolName, "detail") {
			total := 3
			if row.ToolName == "bookmark_tags_all" {
				total = 6
			}
			for suffix, count := range map[string]int{"-argument-13": total, "-argument-14": 1, "-argument-15": 2, "-argument-16": 3, "-argument-17": min(total, 4), "-argument-18": total, "-argument-19": min(max(total-2, 0), 2), "-argument-20": min(max(total-4, 0), 2), "-argument-21": 0, "-boundary-0": 1, "-boundary-1": 2, "-continuation-0": 2, "-continuation-1": 5, "-continuation-2": 2, "-continuation-3": 3, "-continuation-4": 4, "-continuation-5": 5} {
				if strings.HasSuffix(row.Name, suffix) {
					expected = count
				}
			}
			if row.ToolName == "bookmark_tags_all" {
				if strings.HasSuffix(row.Name, "-boundary-0") {
					expected = 4
				}
				if strings.HasSuffix(row.Name, "-boundary-1") {
					expected = 4
				}
				if strings.HasSuffix(row.Name, "-continuation-1") {
					expected = 8
				}
			}
		}
		if expected >= 0 {
			var result struct {
				IsError    bool `json:"isError"`
				Structured struct {
					Tags []json.RawMessage `json:"bookmark_tags"`
				} `json:"structuredContent"`
			}
			if row.RPCError != "" {
				t.Fatalf("%s intended successful window: %s", row.Name, row.RPCError)
			}
			if err := json.Unmarshal(row.Result, &result); err != nil {
				t.Fatal(err)
			}
			if result.IsError || len(result.Structured.Tags) != expected {
				t.Fatalf("%s wanted %d tags, got %s", row.Name, expected, row.Result)
			}
		}

		_ = session.Close()
		cancel()
	}
	if len(tools) != 5 {
		t.Fatal("bookmark reads tools were not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-bookmark-reads.json")
	if *migrationUpdateMCPBookmarkReads {
		if err := os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}

	if *migrationMCPBookmarkReadsAllowDiagnosticOrder {
		var frozen struct {
			Cases []row `json:"cases"`
		}
		if err := json.Unmarshal(want, &frozen); err != nil {
			t.Fatal(err)
		}
		for i := range rows {
			if rows[i].Name != "bookmark_tags_all-argument-25" && rows[i].Name != "bookmark_tags_all-argument-26" {
				continue
			}
			prefix := `calling "tools/call": invalid params: validating "arguments": validating root: validating /properties/`
			user := prefix + `user_id: minimum: -1/1 is less than 1.000000`
			other := prefix + `page: minimum: 0/1 is less than 1.000000`
			if rows[i].Name == "bookmark_tags_all-argument-26" {
				other = prefix + `limit: minimum: -1/1 is less than 0.000000`
			}
			if rows[i].RPCError != user && rows[i].RPCError != other {
				t.Fatalf("unobserved diagnostic alternative: %s", rows[i].RPCError)
			}
			if frozen.Cases[i].Name != rows[i].Name || (frozen.Cases[i].RPCError != user && frozen.Cases[i].RPCError != other) {
				t.Fatal("unexpected frozen alternative")
			}
			t.Logf("%s actual first violation: %s", rows[i].Name, rows[i].RPCError)
			rows[i].RPCError = frozen.Cases[i].RPCError
		}
		data, err = json.MarshalIndent(struct {
			Tools map[string]json.RawMessage `json:"tools"`
			Cases []row                      `json:"cases"`
		}{tools, rows}, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		data = append(data, '\n')
	}
	if !bytes.Equal(data, want) {
		for line, got := range strings.Split(string(data), "\n") {
			wantLines := strings.Split(string(want), "\n")
			if line >= len(wantLines) || got != wantLines[line] {
				t.Fatalf("MCP bookmark reads differ at line %d: got %s want %s", line+1, got, wantLines[line])
			}
		}
	}
}

type migrationBookmarkReadCapture struct {
	mu     sync.Mutex
	result json.RawMessage
	code   int
}
type migrationBookmarkReadCaptureTransport struct {
	mcp.Transport
	capture *migrationBookmarkReadCapture
}
type migrationBookmarkReadCaptureConnection struct {
	mcp.Connection
	capture *migrationBookmarkReadCapture
}

func (t migrationBookmarkReadCaptureTransport) Connect(ctx context.Context) (mcp.Connection, error) {
	c, e := t.Transport.Connect(ctx)
	if e != nil {
		return nil, e
	}
	return migrationBookmarkReadCaptureConnection{c, t.capture}, nil
}
func (c migrationBookmarkReadCaptureConnection) Read(ctx context.Context) (jsonrpc.Message, error) {
	message, err := c.Connection.Read(ctx)
	if err == nil {
		if _, ok := message.(*jsonrpc.Response); ok {
			data, e := jsonrpc.EncodeMessage(message)
			if e != nil {
				return nil, e
			}
			var out struct {
				Result json.RawMessage `json:"result"`
				Error  struct {
					Code int `json:"code"`
				} `json:"error"`
			}
			if e = json.Unmarshal(data, &out); e != nil {
				return nil, e
			}
			c.capture.mu.Lock()
			c.capture.result = out.Result
			c.capture.code = out.Error.Code
			c.capture.mu.Unlock()
		}
	}
	return message, err
}

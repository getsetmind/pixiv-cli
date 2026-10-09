package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPCommentReads = flag.Bool("migration-update-mcp-comment-reads", false, "capture fixed Go MCP comment read contracts")

func TestMigrationMCPCommentReadsMatchesFrozenContracts(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	type row struct {
		UnknownIdentity bool              `json:"unknown_identity,omitempty"`
		Name            string            `json:"name"`
		ToolName        string            `json:"tool_name"`
		Arguments       any               `json:"arguments"`
		Bodies          []json.RawMessage `json:"-"`
		BodyKeys        []string          `json:"body_keys"`
		Result          json.RawMessage   `json:"result"`
		Calls           int               `json:"calls"`
		Requests        int               `json:"requests"`
		Queries         []string          `json:"queries"`
		RPCError        string            `json:"rpc_error"`
	}
	var rows []row
	for _, toolName := range []string{"illust_comments", "novel_comments"} {
		entity, key, version := "illust", "illust_id", "v3"
		if toolName == "novel_comments" {
			entity, key, version = "novel", "novel_id", "v2"
		}
		item := `{"id":22,"comment":"日本語 fixture","date":"2024-01-02T03:04:05+00:00","user":{"id":7,"name":"artist"},"parent_comment":{"id":21,"comment":"parent","date":"2024-01-02T03:04:05+00:00","user":{"id":8},"parent_comment":{"id":20,"comment":"root","date":"2024-01-02T03:04:05+00:00","user":{"id":9}}}}`
		first := json.RawMessage(fmt.Sprintf(`{"comments":[%s,%s],"next_url":"https://app-api.pixiv.net/%s/%s/comments?%s=42&offset=30","total_comments":9,"comment_access_control":2}`, item, item, version, entity, key))
		second := json.RawMessage(`{"comments":[{"id":23,"comment":"second","date":"2024-01-02T03:04:05+00:00","user":{"id":7}}],"next_url":null,"total_comments":99,"access_control":{"can_comment":true,"is_locked":false}}`)
		add := func(name string, args any, bodies ...json.RawMessage) {
			if bodies == nil {
				bodies = []json.RawMessage{first, second}
			}
			rows = append(rows, row{Name: toolName + "-" + name, ToolName: toolName, Arguments: args, Bodies: bodies})
		}
		for index, args := range []any{
			map[string]any{"id": 42}, map[string]any{"id": 42, "limit": 0}, map[string]any{"id": 42, "limit": 1}, map[string]any{"id": 42, "page": 2, "limit": 1}, map[string]any{"id": 42, "page": 2},
			map[string]any{}, map[string]any{"id": 0}, map[string]any{"id": -1}, map[string]any{"id": nil}, map[string]any{"id": "42"}, map[string]any{"id": 1.5}, map[string]any{"id": 42, "page": 1.5}, map[string]any{"id": 42, "limit": "2"}, map[string]any{"id": 42, "extra": true}, map[string]any{"id": 42, "stamp_id": 1},
			map[string]any{"id": 42, "page": 0}, map[string]any{"id": 42, "limit": -1}, map[string]any{"id": 42, "page": nil}, map[string]any{"id": 42, "limit": nil}, map[string]any{"id": 42, "limit": int64(9223372036854775807)}, map[string]any{"id": 42, "page": int64(9223372036854775807), "limit": 2}, nil, 7, "invalid", []any{}, true,
		} {
			add(fmt.Sprintf("argument-%d", index), args)
		}
		add("empty-stop", map[string]any{"id": 42}, json.RawMessage(fmt.Sprintf(`{"comments":[],"next_url":"https://app-api.pixiv.net/%s/%s/comments?%s=42&offset=30"}`, version, entity, key)), second)
		add("metadata-later", map[string]any{"id": 42, "limit": 0}, json.RawMessage(fmt.Sprintf(`{"comments":[%s],"next_url":"https://app-api.pixiv.net/%s/%s/comments?%s=42&offset=30"}`, item, version, entity, key)), second)
		add("missing-list", map[string]any{"id": 42}, json.RawMessage(`{}`))
		add("null-list", map[string]any{"id": 42}, json.RawMessage(`{"comments":null}`))
		add("later-malformed", map[string]any{"id": 42, "limit": 0}, first, json.RawMessage(`{}`))
		add("invalid-parent", map[string]any{"id": 42}, json.RawMessage(`{"comments":[{"id":1,"parent_comment":{"id":0}}]}`))
		add("no-metadata", map[string]any{"id": 42}, json.RawMessage(`{"comments":[],"next_url":null}`))
		add("parentless-value-dedup", map[string]any{"id": 42}, json.RawMessage(`{"comments":[{"id":24,"comment":"equal","date":"2024-01-02T03:04:05Z","user":{"id":7}},{"id":24,"comment":"equal","date":"2024-01-02T03:04:05Z","user":{"id":7}},{"id":24,"comment":"different","date":"2024-01-02T03:04:05Z","user":{"id":7}}],"total_comments":0,"comment_access_control":0}`))

		add("runtime-image-url-distinct", map[string]any{"id": 42}, json.RawMessage(`{"comments":[{"id":24,"comment":"same","date":"2024-01-02T03:04:05Z","user":{"id":7,"profile_image_urls":{"medium":"https://i.pximg.net/one.jpg"}}},{"id":24,"comment":"same","date":"2024-01-02T03:04:05Z","user":{"id":7,"profile_image_urls":{"medium":"https://i.pximg.net/two.jpg"}}}]}`))
		add("raw-string-key-collision", map[string]any{"id": 42}, json.RawMessage(`{"comments":[{"id":24,"comment":"same","date":"2024-01-02T03:04:05.123400Z","user":{"id":7,"name":"a b","account":"c"}},{"id":24,"comment":"same","date":"2024-01-02T03:04:05.123400Z","user":{"id":7,"name":"a","account":"b c"}}]}`))

	}
	sharedBodies := map[string]json.RawMessage{}
	bodyIndex := map[string]string{}
	for i := range rows {
		rows[i].BodyKeys = []string{}
		for _, body := range rows[i].Bodies {
			key, ok := bodyIndex[string(body)]
			if !ok {
				key = fmt.Sprintf("body-%02d", len(sharedBodies))
				bodyIndex[string(body)] = key
				sharedBodies[key] = body
			}
			rows[i].BodyKeys = append(rows[i].BodyKeys, key)
		}
	}
	var err error
	var data []byte
	tools := map[string]json.RawMessage{}
	for index := range rows {
		row := &rows[index]
		client, _, err := pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(func() string {
					return `{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":42}}`
				}())), Request: req}, nil
			}
			bodyIndex := row.Requests
			row.Requests++
			row.Queries = append(row.Queries, req.URL.RawQuery)
			if bodyIndex >= len(row.Bodies) {
				return nil, fmt.Errorf("unexpected HTTP request %d", row.Requests)
			}
			entity, version := "illust", "v3"
			if row.ToolName == "novel_comments" {
				entity, version = "novel", "v2"
			}
			if req.Method != "GET" || req.URL.Path != "/"+version+"/"+entity+"/comments" || req.Header.Get("Authorization") != "Bearer fixture-access" {
				return nil, fmt.Errorf("unexpected request %s", req.URL)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[bodyIndex])), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		if row.UnknownIdentity {
			client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
				row.Requests++
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
		capture := &migrationMCPCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, migrationMCPCaptureTransport{clientTransport, capture}, nil)
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
				if strings.HasSuffix(item.Name, "_comments") {
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
		capture.mu.Unlock()
		if strings.HasSuffix(row.Name, "argument-0") {
			var result struct {
				IsError    bool `json:"isError"`
				Structured struct {
					Comments []json.RawMessage `json:"comments"`
				} `json:"structuredContent"`
			}
			if err := json.Unmarshal(row.Result, &result); err != nil {
				t.Fatal(err)
			}
			if result.IsError || len(result.Structured.Comments) != 2 || row.Requests != 1 {
				t.Fatalf("default read failed: %s", row.Result)
			}
		}
		_ = session.Close()
		cancel()
	}
	if len(tools) != 2 {
		t.Fatal("comment tools were not registered")
	}
	data, err = json.MarshalIndent(struct {
		Bodies map[string]json.RawMessage `json:"bodies"`
		Tools  map[string]json.RawMessage `json:"tools"`
		Cases  []row                      `json:"cases"`
	}{sharedBodies, tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-comment-reads.json")
	if *migrationUpdateMCPCommentReads {
		if err := os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP comment reads differ from fixed Go reference")
	}
}

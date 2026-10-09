package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
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

var migrationUpdateMCPBookmarkFollow = flag.Bool("migration-update-mcp-bookmark-follow", false, "capture fixed Go MCP bookmark/follow contracts")

func TestMigrationMCPBookmarkFollowMatchesFrozenSchemaResultsValidationAndForms(t *testing.T) {
	type row struct {
		Name      string          `json:"name"`
		Tool      string          `json:"tool"`
		Arguments any             `json:"arguments"`
		Status    int             `json:"status"`
		Body      string          `json:"body"`
		Result    json.RawMessage `json:"result"`
		Calls     int             `json:"calls"`
		Requests  []string        `json:"requests"`
		RPCError  string          `json:"rpc_error"`
	}
	var rows []row
	tools := map[string]json.RawMessage{}
	for _, name := range []string{"add_bookmark", "remove_bookmark", "add_novel_bookmark", "remove_novel_bookmark", "follow_user", "unfollow_user"} {
		field := "illust_id"
		if strings.Contains(name, "novel") {
			field = "novel_id"
		} else if strings.Contains(name, "user") {
			field = "user_id"
		}
		args := []any{map[string]any{field: 42}, map[string]any{}, map[string]any{field: 0}, map[string]any{field: -1}, map[string]any{field: int64(9223372036854775807)}, map[string]any{field: "42"}, map[string]any{field: nil}, map[string]any{field: 1.5}, map[string]any{field: 42, "extra": true}, []any{}, nil}
		if strings.HasPrefix(name, "add_") || name == "follow_user" {
			for _, restrict := range []any{"", "public", "private", "friends", "PUBLIC", " public ", nil, 1} {
				args = append(args, map[string]any{field: 42, "restrict": restrict})
			}
		}
		if strings.HasPrefix(name, "add_") {
			for _, tags := range []any{[]string{"tag", "", "日本語", "tag", "a&b +", "line\nfeed"}, []string{}, nil, []any{1}, "tag"} {
				args = append(args, map[string]any{field: 42, "tags": tags})
			}
		}
		for i, arg := range args {
			rows = append(rows, row{Name: name + ":" + string(rune('a'+i)), Tool: name, Arguments: arg, Status: 200, Body: "not JSON"})
		}
		for _, status := range []int{201, 204, 299, 300, 400, 401, 403, 404, 410, 429, 500, 503} {
			rows = append(rows, row{Name: name + ":status", Tool: name, Arguments: map[string]any{field: 42}, Status: status, Body: "fixture private body"})
		}
	}
	for i := range rows {
		row := &rows[i]
		row.Requests = []string{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "POST" || req.URL.Host != "app-api.pixiv.net" || req.URL.RawQuery != "" || req.Header.Get("Authorization") != "Bearer fixture-access" || req.Header.Get("Content-Type") != "application/x-www-form-urlencoded" {
				t.Fatal("unexpected mutation request")
			}
			if err := req.ParseForm(); err != nil {
				t.Fatal(err)
			}
			row.Requests = append(row.Requests, "POST "+req.URL.Path+" "+req.PostForm.Encode())
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"text/plain"}, "Retry-After": {"0"}}, Body: io.NopCloser(strings.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		server := pixivmcp.NewWithSDK(nil, &fakeDownloads{}, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			row.Calls++
			committed, err := invoke(ctx, client)
			if !committed {
				t.Fatal("mutation must suppress replay")
			}
			return err
		}}, pixivmcp.Account{})
		ctx, cancel := context.WithCancel(context.Background())
		ct, st := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, st) }()
		capture := &migrationMCPCapture{}
		session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(ctx, migrationMCPCaptureTransport{ct, capture}, nil)
		if err != nil {
			cancel()
			t.Fatal(err)
		}
		if i == 0 {
			list, err := session.ListTools(ctx, nil)
			if err != nil {
				t.Fatal(err)
			}
			for _, tool := range list.Tools {
				for _, name := range []string{"add_bookmark", "remove_bookmark", "add_novel_bookmark", "remove_novel_bookmark", "follow_user", "unfollow_user"} {
					if tool.Name == name {
						tools[name], err = json.Marshal(tool)
						if err != nil {
							t.Fatal(err)
						}
					}
				}
			}
		}
		_, err = session.CallTool(ctx, &mcp.CallToolParams{Name: row.Tool, Arguments: row.Arguments})
		if err != nil {
			row.RPCError = err.Error()
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
	}
	if len(tools) != 6 {
		t.Fatal("missing mutation tools")
	}
	data, err := json.MarshalIndent(struct {
		Tools map[string]json.RawMessage `json:"tools"`
		Cases []row                      `json:"cases"`
	}{tools, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-bookmark-follow.json")
	if *migrationUpdateMCPBookmarkFollow {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("MCP mutations differ from fixed Go reference")
	}
}

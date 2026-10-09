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
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPUserDetail = flag.Bool("migration-update-mcp-user-detail", false, "capture fixed Go MCP detail contracts")

func TestMigrationMCPUserDetailMatchesFrozenSchemaRecordsAndValidation(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "user-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	var inputs []struct {
		Name string          `json:"name"`
		ID   int64           `json:"id"`
		Body json.RawMessage `json:"body"`
	}
	if err := json.Unmarshal(data, &inputs); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Name      string          `json:"name"`
		Arguments map[string]any  `json:"arguments"`
		Body      json.RawMessage `json:"body"`
		Result    json.RawMessage `json:"result"`
		Calls     int             `json:"calls"`
		Requests  int             `json:"requests"`
		RPCError  string          `json:"rpc_error"`
	}
	var rows []row
	for _, in := range inputs {
		if in.Name == "rich" || in.Name == "section:profile:map[]" || len(in.Name) >= 8 && in.Name[:8] == "missing:" {
			rows = append(rows, row{Name: in.Name, Arguments: map[string]any{"user_id": in.ID}, Body: in.Body})
		}
	}
	for index, args := range []map[string]any{{}, {"user_id": 0}, {"user_id": -1}, {"user_id": 42, "url": "https://www.pixiv.net/user/show.php?id=42"}, {"user_id": int64(9223372036854775807)}, {"user_id": "42"}, {"user_id": nil}, {"user_id": 1.5}} {
		rows = append(rows, row{Name: "argument-" + string(rune('a'+index)), Arguments: args, Body: inputs[0].Body})
	}
	var tool json.RawMessage
	for index := range rows {
		row := &rows[index]
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
			row.Requests++
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
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
				if item.Name == "user_detail" {
					tool, err = json.Marshal(item)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
		}
		_, callErr := session.CallTool(context.Background(), &mcp.CallToolParams{Name: "user_detail", Arguments: row.Arguments})
		if callErr != nil {
			row.RPCError = callErr.Error()
		}
		capture.mu.Lock()
		row.Result = capture.result
		capture.mu.Unlock()
		_ = session.Close()
		cancel()
	}
	if len(tool) == 0 {
		t.Fatal("user_detail was not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tool  json.RawMessage `json:"tool"`
		Cases []row           `json:"cases"`
	}{tool, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-user-detail.json")
	if *migrationUpdateMCPUserDetail {
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
		t.Fatal("MCP detail differs from fixed Go reference")
	}
}

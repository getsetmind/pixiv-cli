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
	"sync"
	"testing"

	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/jsonrpc"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var migrationUpdateMCPDetail = flag.Bool("migration-update-mcp-detail", false, "capture fixed Go MCP detail contracts")

type migrationMCPTransport func(*http.Request) (*http.Response, error)

func (f migrationMCPTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type migrationMCPCapture struct {
	mu     sync.Mutex
	result json.RawMessage
}
type migrationMCPCaptureTransport struct {
	mcp.Transport
	capture *migrationMCPCapture
}
type migrationMCPCaptureConnection struct {
	mcp.Connection
	capture *migrationMCPCapture
}

func (t migrationMCPCaptureTransport) Connect(ctx context.Context) (mcp.Connection, error) {
	connection, err := t.Transport.Connect(ctx)
	if err != nil {
		return nil, err
	}
	return migrationMCPCaptureConnection{connection, t.capture}, nil
}
func (c migrationMCPCaptureConnection) Read(ctx context.Context) (jsonrpc.Message, error) {
	message, err := c.Connection.Read(ctx)
	if err == nil {
		if _, ok := message.(*jsonrpc.Response); ok {
			data, encodeErr := jsonrpc.EncodeMessage(message)
			if encodeErr != nil {
				return nil, encodeErr
			}
			var response struct {
				Result json.RawMessage `json:"result"`
			}
			if decodeErr := json.Unmarshal(data, &response); decodeErr != nil {
				return nil, decodeErr
			}
			c.capture.mu.Lock()
			c.capture.result = response.Result
			c.capture.mu.Unlock()
		}
	}
	return message, err
}

func TestMigrationMCPArtworkDetailMatchesFrozenSchemaRecordsAndValidation(t *testing.T) {
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "artwork-detail.json"))
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
		if in.ID > 0 {
			rows = append(rows, row{Name: in.Name, Arguments: map[string]any{"illust_id": in.ID}, Body: in.Body})
		}
	}
	for index, args := range []map[string]any{
		{}, {"illust_id": 0}, {"illust_id": -1}, {"illust_id": 42, "url": "https://www.pixiv.net/artworks/42"},
		{"illust_id": 42, "url": " "}, {"illust_id": 0, "url": "https://www.pixiv.net/artworks/42"},
		{"url": "42"}, {"url": "https://www.pixiv.net/artworks/42"}, {"url": " https://www.pixiv.net/en/artworks/+0042?x=y "},
		{"url": "https://www.pixiv.net/users/42"}, {"url": "https://evil.invalid/artworks/42"}, {"url": "https://www.pixiv.net/artworks/0"},
		{"url": "https://www.pixiv.net/artworks/9223372036854775807"}, {"illust_id": int64(9223372036854775807)},
		{"url": "https://www.pixiv.net/novel/show.php?id=42"}, {"url": "https://www.pixiv.net/artworks/42#bookmark"},
	} {
		rows = append(rows, row{Name: "reference-" + string(rune('a'+index)), Arguments: args, Body: inputs[0].Body})
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
				if item.Name == "illust_detail" {
					tool, err = json.Marshal(item)
					if err != nil {
						t.Fatal(err)
					}
				}
			}
		}
		_, callErr := session.CallTool(context.Background(), &mcp.CallToolParams{Name: "illust_detail", Arguments: row.Arguments})
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
		t.Fatal("illust_detail was not registered")
	}
	data, err = json.MarshalIndent(struct {
		Tool  json.RawMessage `json:"tool"`
		Cases []row           `json:"cases"`
	}{tool, rows}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "mcp-detail.json")
	if *migrationUpdateMCPDetail {
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

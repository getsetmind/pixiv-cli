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
	"reflect"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixivmcp "github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv"
	pixivapp "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateMCPCommentMutationsPool = flag.Bool("migration-update-mcp-comment-mutations-pool", false, "capture saved MCP comment mutation pool contracts")

func TestMigrationMCPCommentMutationsPoolNeverReplaysCommittedFailure(t *testing.T) {
	type state struct {
		ID       int64  `json:"id"`
		Revision int64  `json:"revision"`
		Token    string `json:"token"`
		Frozen   bool   `json:"frozen"`
		Selected bool   `json:"selected"`
	}
	type row struct {
		Tool      string          `json:"tool"`
		Arguments map[string]any  `json:"arguments"`
		Result    json.RawMessage `json:"result"`
		Opens     []int64         `json:"opens"`
		Requests  []int64         `json:"requests"`
		Closes    int             `json:"closes"`
		States    []state         `json:"states"`
		Reusable  bool            `json:"reusable"`
	}
	rows := []row{}
	for _, name := range []string{"create_artwork_comment", "reply_artwork_comment", "delete_artwork_comment", "stamp_artwork_comment", "create_novel_comment", "reply_novel_comment", "delete_novel_comment", "stamp_novel_comment"} {
		t.Run(name, func(t *testing.T) {
			ctx := context.Background()
			db, err := database.Open(t.TempDir())
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
			field := "illust_id"
			if strings.Contains(name, "novel") {
				field = "novel_id"
			}
			args := map[string]any{field: 42, "comment": "fixture"}
			if strings.HasPrefix(name, "reply_") {
				args["parent_comment_id"] = 9
			}
			if strings.HasPrefix(name, "stamp_") {
				delete(args, "comment")
				args["stamp_id"] = 7
			}
			if strings.HasPrefix(name, "delete_") {
				args = map[string]any{"comment_id": 22}
			}
			r := row{Tool: name, Arguments: args, Opens: []int64{}, Requests: []int64{}, States: []state{}}
			var mu sync.Mutex
			transport := migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
				mu.Lock()
				defer mu.Unlock()
				status, payload := 429, `{}`
				header := http.Header{"Content-Type": {"application/json"}, "Retry-After": {"120"}}
				if req.URL.Host == "oauth.secure.pixiv.net" {
					if err := req.ParseForm(); err != nil {
						return nil, err
					}
					token := req.Form.Get("refresh_token")
					id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
					if err != nil {
						return nil, err
					}
					r.Opens = append(r.Opens, id)
					status = 200
					payload = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id)
				} else {
					token := req.Header.Get("Authorization")
					id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
					if err != nil {
						return nil, err
					}
					r.Requests = append(r.Requests, id)
					stored, err := db.GetPixiv(ctx, id)
					if err != nil {
						return nil, err
					}
					if stored.CredentialRevision != 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", id) {
						t.Error("mutation ran before refresh was saved")
					}
					if req.Method != "POST" {
						t.Fatal("mutation must POST")
					}
				}
				return &http.Response{StatusCode: status, Header: header, Body: io.NopCloser(strings.NewReader(payload)), Request: req}, nil
			})
			gate := pool.NewGate()
			facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, nil), Gate: gate, LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
				return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
			}, Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
				return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
			}, CloseClient: func(*pixiv.Client) error { mu.Lock(); defer mu.Unlock(); r.Closes++; return nil }})
			server := pixivmcp.NewWithSDK(nil, nil, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				return facade.Use(ctx, pixivapp.Request{UserID: 99, Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, invoke)
			}}, pixivmcp.Account{})
			runCtx, cancel := context.WithCancel(ctx)
			ct, st := mcp.NewInMemoryTransports()
			done := make(chan error, 1)
			go func() { done <- server.Run(runCtx, st) }()
			capture := &migrationMCPCapture{}
			session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(runCtx, migrationMCPCaptureTransport{ct, capture}, nil)
			if err != nil {
				cancel()
				t.Fatal(err)
			}
			result, err := session.CallTool(runCtx, &mcp.CallToolParams{Name: name, Arguments: args})
			if err != nil || !result.IsError {
				t.Fatalf("rate limit must be MCP tool error: %+v %v", result, err)
			}
			capture.mu.Lock()
			r.Result = append(json.RawMessage(nil), capture.result...)
			capture.mu.Unlock()
			_ = session.Close()
			cancel()
			select {
			case <-done:
			case <-time.After(5 * time.Second):
				t.Fatal("MCP did not close")
			}
			probe, stop := context.WithTimeout(ctx, time.Second)
			if err := gate.Acquire(probe); err == nil {
				r.Reusable = true
				gate.Release()
			}
			stop()
			for _, id := range []int64{42, 43} {
				a, err := db.GetPixiv(ctx, id)
				if err != nil {
					t.Fatal(err)
				}
				r.States = append(r.States, state{id, a.CredentialRevision, string(a.RefreshTokenCopy()), a.PoolFrozenUntil != nil && *a.PoolFrozenUntil > time.Now().Unix(), a.PoolLastSelected})
			}
			if !reflect.DeepEqual(r.Opens, []int64{42}) || !reflect.DeepEqual(r.Requests, []int64{42}) || r.Closes != 1 || !r.Reusable {
				t.Fatalf("committed mutation replayed or leaked lease: %+v", r)
			}
			rows = append(rows, r)
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-comment-mutations-pool.json")
	if *updateMCPCommentMutationsPool {
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
		t.Fatal("saved comment mutation pool differs from Go reference")
	}
}

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

var updateMCPUserRelationshipsPool = flag.Bool("migration-update-mcp-user-relationships-pool", false, "capture saved account MCP pool replay contracts")

func TestMigrationMCPUserRelationshipsPoolPreservesResolvedTargetAndReplay(t *testing.T) {
	type state struct {
		ID       int64  `json:"id"`
		Revision int64  `json:"revision"`
		Token    string `json:"token"`
		Frozen   bool   `json:"frozen"`
		Selected bool   `json:"selected"`
	}
	type row struct {
		ToolName string            `json:"tool_name"`
		Explicit bool              `json:"explicit"`
		Targets  []int64           `json:"targets"`
		Mode     string            `json:"mode"`
		Body     json.RawMessage   `json:"body"`
		Results  []json.RawMessage `json:"results"`
		Opens    []int64           `json:"opens"`
		Requests []int64           `json:"requests"`
		Closes   int               `json:"closes"`
		States   []state           `json:"states"`
		Reusable bool              `json:"reusable"`
	}
	var data []byte
	var err error
	rows := []row{}
	for _, toolName := range []string{"user_following", "user_followers", "related_users", "blocked_users"} {
		for _, explicit := range []bool{false, true} {
			field := "user_previews"
			endpoint := map[string]string{"user_following": "/v1/user/following", "user_followers": "/v1/user/follower", "related_users": "/v1/user/related", "blocked_users": "/v2/user/list"}[toolName]
			body := json.RawMessage(fmt.Sprintf(`{"%s":[{"user":{"id":22,"name":"first metadata"},"illusts":[],"novels":[]}],"next_url":"https://app-api.pixiv.net%s?user_id=42&offset=30"}`, field, endpoint))
			if toolName == "related_users" {
				body = json.RawMessage(strings.ReplaceAll(string(body), "user_id=42&", "seed_user_id=42&"))
			}
			for _, mode := range []string{"replay_success", "replay_malformed"} {
				t.Run(mode, func(t *testing.T) {
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
					r := row{ToolName: toolName, Explicit: explicit, Targets: []int64{}, Mode: mode, Body: body, Results: []json.RawMessage{}, Opens: []int64{}, Requests: []int64{}, States: []state{}}
					throttled := int64(42)
					replayed := int64(43)
					if !explicit {
						throttled, replayed = 43, 42
					}
					var mu sync.Mutex
					transport := migrationMCPTransport(func(req *http.Request) (*http.Response, error) {
						mu.Lock()
						defer mu.Unlock()
						payload, status := string(body), 200
						header := http.Header{"Content-Type": {"application/json"}}
						if req.URL.Host == "oauth.secure.pixiv.net" {
							if err := req.ParseForm(); err != nil {
								return nil, err
							}
							token := req.Form.Get("refresh_token")
							id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
							if err != nil {
								return nil, err
							}
							stored, err := db.GetPixiv(ctx, id)
							if err != nil {
								return nil, err
							}
							if token != string(stored.RefreshTokenCopy()) {
								t.Error("refresh did not use persisted token")
							}
							r.Opens = append(r.Opens, id)
							payload = fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id)
						} else {
							token := req.Header.Get("Authorization")
							id, err := strconv.ParseInt(token[strings.LastIndex(token, "-")+1:], 10, 64)
							if err != nil {
								return nil, err
							}
							stored, err := db.GetPixiv(ctx, id)
							if err != nil {
								return nil, err
							}
							if stored.CredentialRevision < 2 || string(stored.RefreshTokenCopy()) != fmt.Sprintf("fixture-rotated-%d", id) {
								t.Error("content requested before rotation persisted")
							}
							r.Requests = append(r.Requests, id)
							targetKey := "user_id"
							if toolName == "related_users" {
								targetKey = "seed_user_id"
							}
							target, err := strconv.ParseInt(req.URL.Query().Get(targetKey), 10, 64)
							if err != nil {
								t.Fatal(err)
							}
							r.Targets = append(r.Targets, target)
							if req.URL.Path != endpoint {
								t.Fatalf("unexpected endpoint %s", req.URL)
							}
							count := 0
							for _, requested := range r.Requests {
								if requested == id {
									count++
								}
							}
							if id == throttled && count > 1 {
								status = 429
								if count%2 == 0 {
									header.Set("Retry-After", "0")
								} else {
									header.Set("Retry-After", "120")
								}
							} else if req.URL.Query().Get("offset") != "" {
								if mode == "replay_malformed" && id == replayed {
									payload = `{}`
								} else {
									payload = fmt.Sprintf(`{"%s":[{"user":{"id":23},"illusts":[],"novels":[]}],"next_url":null}`, field)
								}
							} else if id == throttled {
								payload = strings.ReplaceAll(payload, "first metadata", "discarded account42 metadata")
							}

						}
						return &http.Response{StatusCode: status, Header: header, Body: io.NopCloser(strings.NewReader(payload)), Request: req}, nil
					})
					gate := pool.NewGate()
					facade := pixivapp.New(pixivapp.Dependencies{Accounts: account.NewService(db, nil), Gate: gate,
						LoadPoolConfig: func() (pixivapp.PoolConfig, error) {
							return pixivapp.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
						},
						Pool: func(c pixivapp.PoolConfig) (pixivapp.PoolExecutor, error) {
							return pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: c.Enabled, Strategy: settings.AccountPoolStrategy(c.Strategy)}, State: db, Now: time.Now}, nil
						},
						CloseClient: func(*pixiv.Client) error { mu.Lock(); defer mu.Unlock(); r.Closes++; return nil },
					})
					server := pixivmcp.NewWithSDK(nil, nil, pixivmcp.SDKPorts{Execute: func(ctx context.Context, _ pixivmcp.Account, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
						return facade.Use(ctx, pixivapp.Request{UserID: 99, Options: pixiv.Options{HTTPClient: &http.Client{Transport: transport}}}, invoke)
					}}, pixivmcp.Account{})
					runCtx, cancel := context.WithCancel(ctx)
					clientTransport, serverTransport := mcp.NewInMemoryTransports()
					done := make(chan error, 1)
					go func() { done <- server.Run(runCtx, serverTransport) }()
					capture := &migrationMCPCapture{}
					session, err := mcp.NewClient(&mcp.Implementation{Name: "migration", Version: "0"}, nil).Connect(runCtx, migrationMCPCaptureTransport{clientTransport, capture}, nil)
					if err != nil {
						cancel()
						t.Fatal(err)
					}
					for range 2 {
						args := map[string]any{"limit": 0}
						if explicit {
							args["user_id"] = 7
						}
						_, _ = session.CallTool(runCtx, &mcp.CallToolParams{Name: toolName, Arguments: args})
						capture.mu.Lock()
						r.Results = append(r.Results, append(json.RawMessage(nil), capture.result...))
						capture.mu.Unlock()
					}
					_ = session.Close()
					cancel()
					select {
					case <-done:
					case <-time.After(5 * time.Second):
						t.Fatal("MCP pool session did not stop")
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
					rows = append(rows, r)
				})
			}
		}
	}
	if t.Failed() {
		return
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-user-relationships-pool.json")
	if *updateMCPUserRelationshipsPool {
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
		t.Fatal("MCP pool replay differs from fixed Go reference")
	}
}

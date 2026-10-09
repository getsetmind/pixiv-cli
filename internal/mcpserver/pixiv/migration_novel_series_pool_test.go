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

var updateMCPNovelSeriesPool = flag.Bool("migration-update-mcp-novel-series-pool", false, "capture saved account MCP pool replay contracts")

func TestMigrationMCPNovelSeriesPoolReplaysCompleteCollection(t *testing.T) {
	type state struct {
		ID       int64  `json:"id"`
		Revision int64  `json:"revision"`
		Token    string `json:"token"`
		Frozen   bool   `json:"frozen"`
		Selected bool   `json:"selected"`
	}
	type row struct {
		Mode     string            `json:"mode"`
		Body     json.RawMessage   `json:"body"`
		Results  []json.RawMessage `json:"results"`
		Opens    []int64           `json:"opens"`
		Requests []int64           `json:"requests"`
		Closes   int               `json:"closes"`
		States   []state           `json:"states"`
		Reusable bool              `json:"reusable"`
	}
	body := json.RawMessage(`{"novel_series_detail":{"id":21,"title":"first metadata","user":{"id":7}},"novels":[{"id":22,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":"https://app-api.pixiv.net/v2/novel/series?series_id=21&last_order=9"}`)
	var data []byte
	var err error
	rows := []row{}
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
			r := row{Mode: mode, Body: body, Results: []json.RawMessage{}, Opens: []int64{}, Requests: []int64{}, States: []state{}}
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
					count := 0
					for _, requested := range r.Requests {
						if requested == id {
							count++
						}
					}
					if id == 42 && count > 1 {
						status = 429
						if count%2 == 0 {
							header.Set("Retry-After", "0")
						} else {
							header.Set("Retry-After", "120")
						}
					} else if req.URL.Query().Get("last_order") != "" {
						if mode == "replay_malformed" && id == 43 {
							payload = `{}`
						} else {
							payload = `{"novel_series_detail":{"id":21,"title":"later metadata","user":{"id":7}},"novels":[{"id":23,"user":{"id":7},"create_date":"2024-01-02T03:04:05+00:00"}],"next_url":null}`
						}
					} else if id == 42 {
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
				_, _ = session.CallTool(runCtx, &mcp.CallToolParams{Name: "novel_series", Arguments: map[string]any{"series_id": 21, "limit": 0}})
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
	if t.Failed() {
		return
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "mcp-novel-series-pool.json")
	if *updateMCPNovelSeriesPool {
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

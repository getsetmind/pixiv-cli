package download

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/mcpserver/pixiv/internal/runtime"
	pixivservice "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var updateSingleLease = flag.Bool("migration-update-download-single-lease", false, "capture frozen Go single-lease MCP download")

type leaseDefaults struct{}

func (leaseDefaults) ReadPixivDefaultUserID() (int64, bool, error) { return 43, true, nil }
func (leaseDefaults) SetPixivDefaultUserID(int64) error {
	return errors.New("unexpected default mutation")
}
func (leaseDefaults) ClearPixivDefaultUserID() error {
	return errors.New("unexpected default mutation")
}

type leaseGate struct{ acquired, released int }

func (g *leaseGate) Acquire(context.Context) error { g.acquired++; return nil }
func (g *leaseGate) Release()                      { g.released++ }

type leaseAccountState struct {
	ID       int64 `json:"id"`
	Revision int64 `json:"revision"`
	Frozen   bool  `json:"frozen"`
	Selected bool  `json:"selected"`
}
type singleLeaseCase struct {
	Name          string              `json:"name"`
	Opens         []int64             `json:"opens"`
	Resources     int                 `json:"resources"`
	PoolLoads     int                 `json:"pool_loads"`
	PoolFactories int                 `json:"pool_factories"`
	ExecuteCalls  int                 `json:"execute_calls"`
	Closes        int                 `json:"closes"`
	Acquired      int                 `json:"acquired"`
	Released      int                 `json:"released"`
	Result        json.RawMessage     `json:"result"`
	States        []leaseAccountState `json:"states"`
}

func TestMigrationMCPDownloadUsesSingleDefaultLeaseEvenWhenPoolEnabled(t *testing.T) {
	rows := []singleLeaseCase{{Name: "default43-pool42"}, {Name: "retryable-open-failure"}, {Name: "ignored-deferred-close-error"}}
	for index := range rows {
		row := &rows[index]
		row.Opens = []int64{}
		root := t.TempDir()
		dest := filepath.Join(root, "downloads")
		if err := os.Mkdir(dest, 0o700); err != nil {
			t.Fatal(err)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		db, err := database.Open(root)
		if err != nil {
			t.Fatal(err)
		}
		for _, id := range []int64{42, 43} {
			if err = db.SavePixivCredential(ctx, account.New(id, "fixture", []byte(fmt.Sprintf("fixture-refresh-%d", id)))); err != nil {
				t.Fatal(err)
			}
		}
		if err = db.SetAllPixivSchedulable(ctx, true); err != nil {
			t.Fatal(err)
		}
		httpClient := &http.Client{Transport: directTransport(func(request *http.Request) (*http.Response, error) {
			status := 200
			header := http.Header{"Content-Type": {"application/json"}}
			body := []byte(`{}`)
			if request.URL.Host == "oauth.secure.pixiv.net" {
				raw, _ := io.ReadAll(request.Body)
				form, _ := url.ParseQuery(string(raw))
				id, _ := strconv.ParseInt(form.Get("refresh_token")[len("fixture-refresh-"):], 10, 64)
				row.Opens = append(row.Opens, id)
				if row.Name == "retryable-open-failure" {
					status = 429
					header.Set("Retry-After", "120")
				} else {
					body = []byte(fmt.Sprintf(`{"access_token":"fixture-access-%d","refresh_token":"fixture-rotated-%d","expires_in":3600,"user":{"id":%d}}`, id, id, id))
				}
			} else {
				row.Resources++
				if request.URL.Host != "i.pximg.net" {
					t.Errorf("unexpected resource host %s", request.URL.Host)
				}
				header.Set("Content-Type", "image/png")
				body = []byte("\x89PNG\r\n\x1a\nfixture")
			}
			return &http.Response{StatusCode: status, Header: header, Body: io.NopCloser(bytes.NewReader(body)), Request: request}, nil
		})}
		gate := &leaseGate{}
		facade := pixivservice.New(pixivservice.Dependencies{Accounts: account.NewService(db, leaseDefaults{}), Gate: gate, LoadPoolConfig: func() (pixivservice.PoolConfig, error) {
			row.PoolLoads++
			return pixivservice.PoolConfig{Enabled: true, Strategy: "round_robin"}, nil
		}, Pool: func(pixivservice.PoolConfig) (pixivservice.PoolExecutor, error) {
			row.PoolFactories++
			return nil, errors.New("unexpected pool construction")
		}, CloseClient: func(client *pixiv.Client) error {
			row.Closes++
			client.CloseIdleConnections()
			if row.Name == "ignored-deferred-close-error" {
				return errors.New("fixture close failed")
			}
			return nil
		}})
		app := runtime.NewApp(&directManager{dest}, nil, runtime.SDKPorts{OpenLease: func(ctx context.Context, _ runtime.Account) (*lifecycle.Lease[*pixiv.Client], error) {
			return facade.Open(ctx, pixivservice.Request{Options: pixiv.Options{HTTPClient: httpClient}})
		}, Execute: func(context.Context, runtime.Account, func(context.Context, *pixiv.Client) (bool, error)) error {
			row.ExecuteCalls++
			return errors.New("unexpected pooled execution")
		}}, runtime.Account{})
		server := mcp.NewServer(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
		Register(app, server)
		ct, st := mcp.NewInMemoryTransports()
		go func() { _ = server.Run(ctx, st) }()
		client := mcp.NewClient(&mcp.Implementation{Name: "fixture", Version: "0"}, nil)
		session, err := client.Connect(ctx, ct, nil)
		if err != nil {
			t.Fatal(err)
		}
		result, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "download", Arguments: map[string]any{"src": "https://i.pximg.net/asset.png"}})
		if err != nil {
			t.Fatal(err)
		}
		raw, err := json.Marshal(result)
		if err != nil {
			t.Fatal(err)
		}
		row.Result = normalizeDirectJSON(t, raw, dest)
		row.Acquired, row.Released = gate.acquired, gate.released
		for _, id := range []int64{42, 43} {
			a, err := db.GetPixiv(ctx, id)
			if err != nil {
				t.Fatal(err)
			}
			row.States = append(row.States, leaseAccountState{id, a.CredentialRevision, a.PoolFrozenUntil != nil, a.PoolLastSelected})
		}
		_ = session.Close()
		cancel()
		_ = db.Close()
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "..", "..", "crates", "pixiv-mcp", "tests", "fixtures", "download_single_lease.json")
	if *updateSingleLease {
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
		t.Fatal("single-lease MCP download differs from frozen Go")
	}
}

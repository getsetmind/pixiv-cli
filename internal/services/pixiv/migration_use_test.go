package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	pixiv "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	sdkpixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"os"
	"path/filepath"
	"testing"
	"time"
)

var updateUse = flag.Bool("migration-update-use", false, "capture client use and pool replay contracts")

type migrationUseRow struct {
	Mode      string     `json:"mode"`
	Opened    []int64    `json:"opened"`
	Options   []string   `json:"options"`
	Frozen    [][2]int64 `json:"frozen"`
	Loads     int        `json:"loads"`
	Factories int        `json:"factories"`
	Uses      int        `json:"uses"`
	Closes    int        `json:"closes"`
	Message   string     `json:"message"`
	Reason    string     `json:"reason"`
	Canceled  bool       `json:"canceled"`
	Panic     bool       `json:"panic"`
	Reusable  bool       `json:"reusable"`
}
type migrationUseState struct{ row *migrationUseRow }

func (s migrationUseState) SelectPixiv(_ context.Context, _ int64, attempted []int64, chooser account.Chooser) (account.Account, error) {
	id := int64(42)
	if len(attempted) > 0 {
		id = 43
	}
	selected, err := chooser(account.PoolSnapshot{Candidates: []account.PoolCandidate{{UserID: id, SortOrder: 1, Eligible: true, Schedulable: true}}})
	return account.New(selected, "synthetic", nil), err
}
func (s migrationUseState) Freeze(_ context.Context, id, until int64) error {
	s.row.Frozen = append(s.row.Frozen, [2]int64{id, until})
	return nil
}

func TestMigrationUsePreservesCommitCloseAndPoolReplay(t *testing.T) {
	var rows []migrationUseRow
	for _, mode := range []string{"nil_context", "nil_callback", "missing_loader", "loader_error", "disabled", "missing_factory", "factory_error", "nil_pool", "enabled", "retry", "committed_retry", "use_error", "close_error", "use_close_error", "close_rate_retry", "close_canceled", "use_panic", "use_close_rate", "committed_close_rate", "use_close_canceled"} {
		row := migrationUseRow{Mode: mode}
		accountRow := migrationSessionCase{}
		gate := pool.NewGate()
		rate := func() error {
			return sdk.NewError("pixiv", "Artwork", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{Safe: true, HasAfter: true, After: time.Unix(1120, 0)}))
		}
		deps := pixiv.Dependencies{Accounts: &migrationSessionAccounts{row: &accountRow}, Gate: gate,
			LoadPoolConfig: func() (pixiv.PoolConfig, error) {
				row.Loads++
				if mode == "loader_error" {
					return pixiv.PoolConfig{}, errors.New("synthetic config failure")
				}
				return pixiv.PoolConfig{Enabled: mode != "disabled", Strategy: "round_robin"}, nil
			},
			Pool: func(c pixiv.PoolConfig) (pixiv.PoolExecutor, error) {
				row.Factories++
				if mode == "factory_error" {
					return nil, errors.New("synthetic factory failure")
				}
				if mode == "nil_pool" {
					return nil, nil
				}
				return pool.Scheduler{Config: config.AccountPoolConfig{Enabled: c.Enabled, Strategy: config.AccountPoolStrategy(c.Strategy)}, State: migrationUseState{&row}, Now: func() time.Time { return time.Unix(1000, 0) }}, nil
			},
			CloseClient: func(*sdkpixiv.Client) error {
				row.Closes++
				if mode == "close_error" || mode == "use_close_error" {
					return errors.New("synthetic close failure")
				}
				if (mode == "close_rate_retry" || mode == "use_close_rate" || mode == "committed_close_rate") && row.Closes == 1 {
					return rate()
				}
				if mode == "close_canceled" || mode == "use_close_canceled" {
					return context.Canceled
				}
				return nil
			},
		}
		if mode == "missing_loader" {
			deps.LoadPoolConfig = nil
		}
		if mode == "missing_factory" {
			deps.Pool = nil
		}
		ctx := context.Background()
		if mode == "nil_context" {
			ctx = nil
		}
		callback := func(context.Context, *sdkpixiv.Client) (bool, error) {
			row.Uses++
			if mode == "use_panic" {
				panic("synthetic use panic")
			}
			if mode == "committed_retry" {
				return true, rate()
			}
			if mode == "retry" && row.Uses == 1 {
				return false, rate()
			}
			if mode == "use_error" || mode == "use_close_error" || mode == "use_close_rate" || mode == "use_close_canceled" {
				return false, errors.New("synthetic use failure")
			}
			return mode == "committed_close_rate", nil
		}
		if mode == "nil_callback" {
			callback = nil
		}
		func() {
			defer func() {
				if recover() != nil {
					row.Panic = true
				}
			}()
			err := pixiv.NewFacade(deps).Use(ctx, pixiv.Request{UserID: 99, Options: sdkpixiv.Options{AcceptLanguage: "synthetic-language"}}, callback)
			if err != nil {
				row.Message = err.Error()
				row.Canceled = errors.Is(err, context.Canceled)
				var classified *sdk.Error
				if errors.As(err, &classified) {
					row.Reason = string(classified.Reason)
				}
			}
		}()
		row.Opened, row.Options = accountRow.Opened, accountRow.Options
		probe, cancel := context.WithTimeout(context.Background(), time.Second)
		if err := gate.Acquire(probe); err == nil {
			row.Reusable = true
			gate.Release()
		}
		cancel()
		rows = append(rows, row)
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "use.json")
	if *updateUse {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("client use differs from fixed Go reference")
	}
}

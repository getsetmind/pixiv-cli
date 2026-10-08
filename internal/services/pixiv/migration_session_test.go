package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"time"

	pixiv "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv"
	"github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	sdkpixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var updateSessions = flag.Bool("migration-update-sessions", false, "update account client session contracts")

type migrationSessionCase struct {
	Mode        string   `json:"mode"`
	UserID      int64    `json:"user_id"`
	Opened      []int64  `json:"opened"`
	Options     []string `json:"options"`
	Error       string   `json:"error"`
	CloseErrors []string `json:"close_errors"`
	CloseCalls  int      `json:"close_calls"`
	HasLease    bool     `json:"has_lease"`
	Held        bool     `json:"held"`
	Reusable    bool     `json:"reusable"`
	Panic       string   `json:"panic"`
	Canceled    bool     `json:"canceled"`
	Reason      string   `json:"reason"`
}

type migrationSessionAccounts struct{ row *migrationSessionCase }

func (a *migrationSessionAccounts) OpenClientWith(ctx context.Context, options sdkpixiv.Options) (*sdkpixiv.Client, error) {
	return a.OpenAccountClientWith(ctx, 0, options)
}
func (a *migrationSessionAccounts) OpenAccountClientWith(_ context.Context, id int64, options sdkpixiv.Options) (*sdkpixiv.Client, error) {
	a.row.Opened = append(a.row.Opened, id)
	a.row.Options = append(a.row.Options, options.AcceptLanguage)
	switch a.row.Mode {
	case "open_panic":
		panic("synthetic opener panic")
	case "nil_client":
		return nil, nil
	case "open_error":
		return nil, errors.New("synthetic open failure")
	case "partial_error", "partial_close_error", "partial_close_canceled", "partial_close_sdk":
		return &sdkpixiv.Client{}, errors.New("synthetic open failure")
	}
	return &sdkpixiv.Client{}, nil
}

func TestMigrationSessionsPreserveClientGateOwnershipAndPartialFailure(t *testing.T) {
	var cases []migrationSessionCase
	for _, mode := range []string{"nil_context", "missing_accounts", "missing_gate", "zero_gate", "occupied_canceled", "default", "explicit", "negative", "open_error", "partial_error", "partial_close_error", "partial_close_canceled", "partial_close_sdk", "nil_client", "close_error", "open_panic", "close_panic"} {
		row := migrationSessionCase{Mode: mode}
		if mode == "explicit" {
			row.UserID = 42
		}
		if mode == "negative" {
			row.UserID = -2
		}
		gate := pool.NewGate()
		deps := pixiv.Dependencies{Accounts: &migrationSessionAccounts{row: &row}, Gate: gate, CloseClient: func(*sdkpixiv.Client) error {
			row.CloseCalls++
			switch mode {
			case "close_error", "partial_close_error":
				return errors.New("synthetic close failure")
			case "partial_close_canceled":
				return context.Canceled
			case "partial_close_sdk":
				return sdk.NewError("pixiv", "close", sdk.RateLimited)
			case "close_panic":
				panic("synthetic closer panic")
			}
			return nil
		}}
		if mode == "missing_accounts" {
			deps.Accounts = nil
		}
		if mode == "missing_gate" {
			deps.Gate = nil
		}
		if mode == "zero_gate" {
			gate = &pool.Gate{}
			deps.Gate = gate
		}
		ctx, cancel := context.WithCancel(context.Background())
		if mode == "nil_context" {
			ctx = nil
		}
		if mode == "occupied_canceled" {
			if err := gate.Acquire(ctx); err != nil {
				t.Fatal(err)
			}
			cancel()
		}
		var lease *lifecycle.Lease[*sdkpixiv.Client]
		var err error
		func() {
			defer func() {
				if value := recover(); value != nil {
					row.Panic = value.(string)
				}
			}()
			lease, err = pixiv.New(deps).Open(ctx, pixiv.Request{UserID: row.UserID, Options: sdkpixiv.Options{AcceptLanguage: "ja-JP"}})
		}()
		if err != nil {
			row.Error = err.Error()
		}
		row.Canceled = errors.Is(err, context.Canceled)
		var classified *sdk.Error
		if errors.As(err, &classified) {
			row.Reason = string(classified.Reason)
		}
		row.HasLease = lease != nil
		if lease != nil {
			probe, stop := context.WithTimeout(context.Background(), time.Millisecond)
			row.Held = errors.Is(gate.Acquire(probe), context.DeadlineExceeded)
			stop()
			if !row.Held {
				t.Fatal("lease did not retain gate")
			}
			for range 3 {
				func() {
					defer func() {
						if value := recover(); value != nil {
							row.Panic = value.(string)
						}
					}()
					err := lease.Close()
					text := ""
					if err != nil {
						text = err.Error()
					}
					row.CloseErrors = append(row.CloseErrors, text)
				}()
			}
		}
		if mode == "occupied_canceled" {
			gate.Release()
		}
		if mode != "zero_gate" {
			probe, stop := context.WithTimeout(context.Background(), 10*time.Second)
			if err = gate.Acquire(probe); err != nil {
				stop()
				t.Fatal(err)
			}
			gate.Release()
			stop()
			row.Reusable = true
		}
		cancel()
		cases = append(cases, row)
	}
	encoded, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "sessions.json")
	if *updateSessions {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, encoded) {
		t.Fatal("account client session contracts differ")
	}
}

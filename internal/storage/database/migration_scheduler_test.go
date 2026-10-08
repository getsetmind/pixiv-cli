package database

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	pool "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
	"github.com/FlanChanXwO/pixiv-cli/internal/shared/lifecycle"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var updateSchedulerContract = flag.Bool("migration-update-scheduler", false, "update scheduler replay contracts")

func TestMigrationSchedulerPendingAttemptCommitAndCancellation(t *testing.T) {
	for _, committed := range []bool{false, true} {
		t.Run(fmt.Sprint(committed), func(t *testing.T) {
			db, err := Open(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			defer db.Close()
			if err = db.SavePixivCredential(context.Background(), account.New(1, "synthetic", []byte("synthetic-token"))); err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			started := make(chan *lifecycle.Attempt, 1)
			done := make(chan error, 1)
			base := time.Unix(1000, 250_000_000).UTC()
			row := &migrationSchedulerCase{}
			scheduler := pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: true, Strategy: settings.AccountPoolStrategyRoundRobin}, State: &migrationSchedulerState{db: db, row: row}, Now: func() time.Time { return base }}
			go func() {
				done <- scheduler.Run(ctx, func(ctx context.Context, _ int64, attempt *lifecycle.Attempt) error {
					started <- attempt
					<-ctx.Done()
					return sdk.NewError("pixiv", "read", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{Safe: true, HasAfter: true, After: base.Add(time.Second)}))
				})
			}()
			var attempt *lifecycle.Attempt
			select {
			case attempt = <-started:
			case err := <-done:
				t.Fatalf("attempt did not start: %v", err)
			case <-time.After(10 * time.Second):
				t.Fatal("attempt did not start")
			}
			if committed {
				attempt.Commit()
				attempt.Commit()
			}
			cancel()
			select {
			case err = <-done:
			case <-time.After(10 * time.Second):
				t.Fatal("canceled attempt did not finish")
			}
			if committed {
				var typed *sdk.Error
				if !errors.As(err, &typed) || typed.Reason != sdk.RateLimited || errors.Is(err, context.Canceled) {
					t.Fatalf("committed result: %v", err)
				}
			} else if !errors.Is(err, context.Canceled) {
				t.Fatalf("uncommitted result: %v", err)
			}
			if len(row.Selects) != 1 || len(row.Freezes) != 0 {
				t.Fatalf("unsafe replay: selects=%v freezes=%v", row.Selects, row.Freezes)
			}
		})
	}
}

type migrationSchedulerCase struct {
	Name           string           `json:"name"`
	Mode           string           `json:"mode"`
	Commit         bool             `json:"commit"`
	Cancel         string           `json:"cancel"`
	Once           bool             `json:"once"`
	State          string           `json:"state"`
	Enabled        bool             `json:"enabled"`
	Strategy       string           `json:"strategy"`
	StepNS         int64            `json:"step_ns"`
	MissingAttempt bool             `json:"missing_attempt"`
	Error          map[string]any   `json:"error"`
	Attempts       []int64          `json:"attempts"`
	Selects        [][]int64        `json:"selects"`
	Clocks         []int64          `json:"clocks"`
	Freezes        [][]int64        `json:"freezes"`
	Rows           []map[string]any `json:"rows"`
}

type migrationSchedulerState struct {
	db  *DB
	row *migrationSchedulerCase
}

func (s *migrationSchedulerState) SelectPixiv(ctx context.Context, now int64, attempted []int64, chooser account.Chooser) (account.Account, error) {
	s.row.Selects = append(s.row.Selects, append([]int64{}, attempted...))
	switch s.row.State {
	case "storage_error":
		return account.Account{}, errors.New("synthetic storage error")
	case "unknown":
		return account.Account{}, &account.PoolSelectionError{Kind: "unknown"}
	case "sentinel":
		return account.Account{}, pool.ErrAccountPoolExhausted
	case "sentinel_after":
		if len(attempted) > 0 {
			return account.Account{}, pool.ErrAccountPoolExhausted
		}
	case "invalid_uid":
		return account.New(0, "", nil), nil
	case "repeated_uid":
		return account.New(1, "", nil), nil
	}
	return s.db.SelectPixiv(ctx, now, attempted, chooser)
}
func (s *migrationSchedulerState) Freeze(ctx context.Context, id, until int64) error {
	s.row.Freezes = append(s.row.Freezes, []int64{id, until})
	if s.row.State == "freeze_error" {
		return errors.New("synthetic freeze error")
	}
	return s.db.Freeze(ctx, id, until)
}

func TestMigrationSchedulerPreservesReplayCommitCancellationAndExhaustion(t *testing.T) {
	var cases []migrationSchedulerCase
	for _, mode := range []string{"rate", "unsafe", "missing_after", "past", "wrong_reason", "plain", "wrapped", "cancel_raw", "deadline_raw", "success"} {
		for _, commit := range []bool{false, true} {
			for _, cancel := range []string{"", "during"} {
				for _, once := range []bool{false, true} {
					cases = append(cases, migrationSchedulerCase{Name: fmt.Sprintf("%s/%t/%s/%t", mode, commit, cancel, once), Mode: mode, Commit: commit, Cancel: cancel, Once: once, Enabled: true, Strategy: "round_robin"})
				}
			}
		}
	}
	for _, state := range []string{"empty", "disabled", "frozen", "nil", "storage_error", "unknown", "sentinel", "sentinel_after", "invalid_uid", "repeated_uid", "freeze_error"} {
		cases = append(cases, migrationSchedulerCase{Name: "state/" + state, State: state, Mode: "rate", Enabled: true, Strategy: "round_robin"})
	}
	cases = append(cases,
		migrationSchedulerCase{Name: "disabled config", Mode: "rate", Strategy: "round_robin"},
		migrationSchedulerCase{Name: "missing attempt", MissingAttempt: true, Enabled: true, Strategy: "round_robin"},
		migrationSchedulerCase{Name: "canceled before", Cancel: "before", Enabled: true, Strategy: "round_robin"},
		migrationSchedulerCase{Name: "deadline before", Cancel: "deadline", Enabled: true, Strategy: "round_robin"},
		migrationSchedulerCase{Name: "unsupported strategy", Mode: "rate", Enabled: true, Strategy: "unsupported"},
		migrationSchedulerCase{Name: "advancing clock", Mode: "rate", Enabled: true, Strategy: "round_robin", StepNS: 900_000_000},
	)
	base := time.Unix(1000, 250_000_000).UTC()
	for i := range cases {
		row := &cases[i]
		db, err := Open(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		ctx, cancel := context.WithCancel(context.Background())
		if row.Cancel == "before" {
			cancel()
		}
		if row.Cancel == "deadline" {
			var stop context.CancelFunc
			ctx, stop = context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
			defer stop()
		}
		if row.State != "empty" {
			for _, id := range []int64{1, 2, 3} {
				if err = db.SavePixivCredential(context.Background(), account.New(id, "synthetic", []byte("synthetic-token"))); err != nil {
					t.Fatal(err)
				}
			}
		}
		if row.State == "disabled" {
			if err = db.SetAllPixivSchedulable(context.Background(), false); err != nil {
				t.Fatal(err)
			}
		}
		if row.State == "frozen" {
			for _, id := range []int64{1, 2, 3} {
				if err = db.Freeze(context.Background(), id, 1500); err != nil {
					t.Fatal(err)
				}
			}
		}
		state := &migrationSchedulerState{db: db, row: row}
		scheduler := pool.Scheduler{Config: settings.AccountPoolConfig{Enabled: row.Enabled, Strategy: settings.AccountPoolStrategy(row.Strategy)}, State: state}
		if row.State == "nil" {
			scheduler.State = nil
		}
		tick := 0
		scheduler.Now = func() time.Time {
			now := base.Add(time.Duration(tick) * time.Duration(row.StepNS))
			tick++
			row.Clocks = append(row.Clocks, now.UnixNano())
			return now
		}
		var attempt func(context.Context, int64, *lifecycle.Attempt) error
		if !row.MissingAttempt {
			attempt = func(_ context.Context, id int64, commit *lifecycle.Attempt) error {
				row.Attempts = append(row.Attempts, id)
				if row.Commit {
					commit.Commit()
					commit.Commit()
				}
				if row.Cancel == "during" {
					cancel()
				}
				if row.Once && len(row.Attempts) > 1 {
					return nil
				}
				reason := sdk.RateLimited
				if row.Mode == "wrong_reason" {
					reason = sdk.UpstreamUnavailable
				}
				retry := sdk.RetryAdvice{Safe: row.Mode != "unsafe", HasAfter: row.Mode != "missing_after", After: base.Add(3500 * time.Millisecond)}
				if row.Mode == "past" {
					retry.After = base
				}
				typed := sdk.NewError("pixiv", "read", reason, sdk.WithDetail("synthetic_attempt"), sdk.WithRetry(retry))
				switch row.Mode {
				case "success":
					return nil
				case "plain":
					return errors.New("synthetic attempt error")
				case "wrapped":
					return fmt.Errorf("wrapped: %w", typed)
				case "cancel_raw":
					return context.Canceled
				case "deadline_raw":
					return context.DeadlineExceeded
				default:
					return typed
				}
			}
		}
		err = scheduler.Run(ctx, attempt)
		row.Error = map[string]any{"text": "", "exhausted": false, "canceled": false, "deadline": false, "classified": nil}
		if err != nil {
			row.Error["text"] = err.Error()
			row.Error["exhausted"] = errors.Is(err, pool.ErrAccountPoolExhausted)
			row.Error["canceled"] = errors.Is(err, context.Canceled)
			row.Error["deadline"] = errors.Is(err, context.DeadlineExceeded)
			var typed *sdk.Error
			if errors.As(err, &typed) {
				var after any
				if typed.Retry.HasAfter {
					after = typed.Retry.After.UnixNano()
				}
				row.Error["classified"] = map[string]any{"reason": typed.Reason, "product": typed.Product, "operation": typed.Operation, "detail": typed.Detail, "safe": typed.Retry.Safe, "after_ns": after}
			}
		}
		accounts, err := db.ListPixiv(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		row.Rows = []map[string]any{}
		for _, a := range accounts {
			row.Rows = append(row.Rows, map[string]any{"id": a.UserID, "schedulable": a.Schedulable, "frozen": a.PoolFrozenUntil, "selected": a.PoolLastSelected})
		}
		cancel()
		db.Close()
	}
	encoded, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "scheduler.json")
	if *updateSchedulerContract {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(encoded, want) {
		t.Fatal("scheduler replay contracts differ")
	}
}

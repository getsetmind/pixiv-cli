package database

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

	settings "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	pool "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/pool"
)

var updatePoolContract = flag.Bool("migration-update-pool", false, "update account pool storage contracts")

func TestMigrationPoolChooserDefaultRandomAndEmptySnapshot(t *testing.T) {
	chosen, err := pool.Choose(account.PoolSnapshot{Candidates: []account.PoolCandidate{{UserID: 42, SortOrder: 1}}}, settings.AccountPoolStrategyRandom, nil)
	if err != nil || chosen != 42 {
		t.Fatalf("default random: %d, %v", chosen, err)
	}
	until := int64(700)
	_, err = pool.Choose(account.PoolSnapshot{EarliestFrozenUntil: &until}, settings.AccountPoolStrategy("unsupported"), nil)
	var selection *account.PoolSelectionError
	if !errors.As(err, &selection) || selection.Kind != account.PoolSelectionExhausted || selection.EarliestFrozenUntil == nil || *selection.EarliestFrozenUntil != 700 {
		t.Fatalf("empty selection: %v", err)
	}
}

type migrationPoolOperation struct {
	Action      string                `json:"action"`
	IDs         []int64               `json:"ids"`
	Enabled     bool                  `json:"enabled"`
	Now         int64                 `json:"now"`
	Until       int64                 `json:"until"`
	Strategy    string                `json:"strategy"`
	Random      int                   `json:"random"`
	Error       string                `json:"error"`
	Kind        string                `json:"kind"`
	Earliest    *int64                `json:"earliest"`
	Snapshot    *account.PoolSnapshot `json:"snapshot"`
	Status      *account.PoolStatus   `json:"status"`
	Selected    map[string]any        `json:"selected"`
	Rows        []map[string]any      `json:"rows"`
	RandomSizes []int                 `json:"random_sizes"`
}

func TestMigrationPoolPreservesSelectionFreezeMembershipAndRollback(t *testing.T) {
	ops := []migrationPoolOperation{
		{Action: "status", Now: 100}, {Action: "select", Now: 100, Strategy: "nil"},
		{Action: "all", Enabled: false},
		{Action: "seed", IDs: []int64{1, 2, 3}},
		{Action: "select", Now: 100, Strategy: "round_robin"}, {Action: "select", Now: 100, Strategy: "round_robin"}, {Action: "select", Now: 100, Strategy: "round_robin"}, {Action: "select", Now: 100, Strategy: "round_robin"},
		{Action: "freeze", IDs: []int64{2}, Until: 200}, {Action: "freeze", IDs: []int64{2}, Until: 150}, {Action: "freeze", IDs: []int64{999}, Until: 200},
		{Action: "members"}, {Action: "members", IDs: []int64{1, 1}}, {Action: "members", IDs: []int64{0}}, {Action: "members", IDs: []int64{1, 999}},
		{Action: "members", IDs: []int64{2}, Enabled: false}, {Action: "status", Now: 100},
		{Action: "select", Now: 100, Strategy: "random", Random: 1},
		{Action: "all", Enabled: false}, {Action: "select", Now: 200, Strategy: "round_robin"}, {Action: "status", Now: 200},
		{Action: "all", Enabled: true},
		{Action: "freeze", IDs: []int64{1}, Until: 400}, {Action: "freeze", IDs: []int64{2}, Until: 300}, {Action: "freeze", IDs: []int64{3}, Until: 350},
		{Action: "select", Now: 200, Strategy: "round_robin"}, {Action: "members", IDs: []int64{2}, Enabled: false},
		{Action: "select", Now: 200, Strategy: "round_robin"}, {Action: "status", Now: 200},
		{Action: "select", Now: 350, Strategy: "nil"}, {Action: "select", Now: 350, Strategy: "outside"},
		{Action: "select", Now: 350, Strategy: "chooser_error"}, {Action: "select", Now: 350, Strategy: "unsupported"},
		{Action: "select", Now: 350, Strategy: "random", Random: -1}, {Action: "select", Now: 350, Strategy: "random", Random: 99},
		{Action: "select", Now: 350, Strategy: "random_error"},
		{Action: "select", Now: 350, Strategy: "round_robin"},
		{Action: "select", Now: 350, Strategy: "round_robin", IDs: []int64{3}},
		{Action: "select", Now: 350, Strategy: "round_robin", IDs: []int64{-1, 3, 3}},
		{Action: "remove", IDs: []int64{3}}, {Action: "select", Now: 400, Strategy: "round_robin"},
		{Action: "members", IDs: []int64{2}, Enabled: true}, {Action: "select", Now: 400, Strategy: "round_robin"},
		{Action: "members", IDs: []int64{2}, Enabled: false}, {Action: "select", Now: 400, Strategy: "round_robin"},
		{Action: "freeze", IDs: []int64{1}, Until: 999}, {Action: "freeze", IDs: []int64{1}, Until: 500},
		{Action: "freeze", IDs: []int64{2}, Until: -1}, {Action: "freeze", IDs: []int64{0}, Until: 100},
		{Action: "status", Now: 500}, {Action: "status", Now: 999},
	}
	db, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	ctx := context.Background()
	for i := range ops {
		op := &ops[i]
		if _, err := db.DB().Exec("UPDATE pixiv_account SET created_at=11,updated_at=22"); err != nil {
			t.Fatal(err)
		}
		start := time.Now().Unix()
		err = nil
		switch op.Action {
		case "seed":
			for _, id := range op.IDs {
				if err = db.SavePixivCredential(ctx, account.New(id, "synthetic", []byte("synthetic-token"))); err != nil {
					t.Fatal(err)
				}
			}
		case "members":
			err = db.SetPixivSchedulable(ctx, op.IDs, op.Enabled)
		case "all":
			err = db.SetAllPixivSchedulable(ctx, op.Enabled)
		case "freeze":
			err = db.Freeze(ctx, op.IDs[0], op.Until)
		case "remove":
			err = db.RemovePixiv(ctx, op.IDs[0])
		case "status":
			var status account.PoolStatus
			status, err = db.ListPixivPoolStatus(ctx, op.Now)
			if err == nil {
				op.Status = &status
			}
		case "select":
			var chooser account.Chooser
			if op.Strategy != "nil" {
				chooser = func(snapshot account.PoolSnapshot) (int64, error) {
					op.Snapshot = &snapshot
					if op.Strategy == "outside" {
						return 99, nil
					}
					if op.Strategy == "chooser_error" {
						return 0, errors.New("synthetic chooser error")
					}
					strategy := op.Strategy
					if strategy == "random_error" {
						strategy = "random"
					}
					return pool.Choose(snapshot, settings.AccountPoolStrategy(strategy), func(size int) (int, error) {
						op.RandomSizes = append(op.RandomSizes, size)
						if op.Strategy == "random_error" {
							return 0, errors.New("synthetic random error")
						}
						return op.Random, nil
					})
				}
			}
			var selected account.Account
			selected, err = db.SelectPixiv(ctx, op.Now, op.IDs, chooser)
			if err == nil {
				op.Selected = migrationPoolAccountState(t, selected, start, time.Now().Unix(), op.Now)
			}
		}
		end := time.Now().Unix()
		if err != nil {
			op.Error = err.Error()
			var selection *account.PoolSelectionError
			if errors.As(err, &selection) {
				op.Kind = string(selection.Kind)
				op.Earliest = selection.EarliestFrozenUntil
			}
		}
		rows, err := db.ListPixiv(ctx)
		if err != nil {
			t.Fatal(err)
		}
		op.Rows = []map[string]any{}
		for _, row := range rows {
			op.Rows = append(op.Rows, migrationPoolAccountState(t, row, start, end, op.Now))
		}
	}
	encoded, err := json.MarshalIndent(ops, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "pool.json")
	if *updatePoolContract {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(encoded, want) {
		t.Fatal("pool storage contracts differ")
	}
}

func migrationPoolAccountState(t *testing.T, a account.Account, start, end, now int64) map[string]any {
	updated := a.UpdatedAt
	if updated == now {
		a.UpdatedAt = 22
	}
	state := migrationAccountState(t, a, start, end)
	if updated == now {
		state["updated"] = now
	}
	return state
}

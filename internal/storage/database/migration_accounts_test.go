package database

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
)

var updateAccountsContract = flag.Bool("migration-update-accounts", false, "update account repository contracts")

func TestMigrationAccountsConcurrentRevisionConflict(t *testing.T) {
	db, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	ctx := context.Background()
	if err = db.SavePixivCredential(ctx, account.New(1, "synthetic", []byte("synthetic-first"))); err != nil {
		t.Fatal(err)
	}
	start := make(chan struct{})
	outcomes := make(chan error, 8)
	for i := 0; i < 8; i++ {
		go func() { <-start; outcomes <- db.RotatePixivCredentials(ctx, 1, 1, []byte("synthetic-next")) }()
	}
	close(start)
	successes, conflicts := 0, 0
	for i := 0; i < 8; i++ {
		err := <-outcomes
		if err == nil {
			successes++
		} else if errors.Is(err, account.ErrCredentialConflict) {
			conflicts++
		} else {
			t.Fatal(err)
		}
	}
	if successes != 1 || conflicts != 7 {
		t.Fatalf("successes=%d conflicts=%d", successes, conflicts)
	}
	got, err := db.GetPixiv(ctx, 1)
	if err != nil || got.CredentialRevision != 2 || string(got.RefreshTokenCopy()) != "synthetic-next" {
		t.Fatalf("rotation state: %+v, %v", got, err)
	}
}

func TestMigrationAccountsGoRustCredentialInteroperability(t *testing.T) {
	if !*rustDatabaseContract {
		t.Skip("enable -migration-rust-database for file interoperability")
	}
	dir := t.TempDir()
	db, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	premium := true
	checked, frozen := int64(123), int64(321)
	a := account.New(7, "go-user", []byte("synthetic-go-first"))
	a.PremiumStatus = &premium
	a.PremiumCheckedAt = &checked
	a.PoolFrozenUntil = &frozen
	a.PoolLastSelected = true
	if err = db.SavePixivCredential(ctx, a); err != nil {
		t.Fatal(err)
	}
	before, err := db.GetPixiv(ctx, 7)
	if err != nil {
		t.Fatal(err)
	}
	db.Close()
	run := func(phase string) {
		t.Helper()
		command := exec.Command("cargo", "test", "--target-dir", migrationRustTargetDirectory(), "-p", "pixiv-app", "--test", "accounts", "--locked", "accounts_exchange_with_go_repository", "--", "--ignored", "--exact")
		command.Dir = filepath.Join("..", "..", "..")
		command.Env = append(os.Environ(), "PIXIV_MIGRATION_DATABASE_DIRECTORY="+dir, "PIXIV_MIGRATION_ACCOUNT_PHASE="+phase)
		output, err := command.CombinedOutput()
		if err != nil {
			t.Fatalf("Rust %s: %v\n%s", phase, err, output)
		}
		if !bytes.Contains(output, []byte("1 passed; 0 failed")) {
			t.Fatalf("Rust helper did not execute: %s", output)
		}
	}
	run("write")
	db, err = Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	got, err := db.GetPixiv(ctx, 7)
	if err != nil {
		t.Fatal(err)
	}
	if got.CredentialRevision != 2 || string(got.RefreshTokenCopy()) != "synthetic-rust-rotated" || got.Username != "rust-metadata" || got.PremiumStatus == nil || *got.PremiumStatus || got.PremiumCheckedAt == nil || *got.PremiumCheckedAt != 333 || got.PoolFrozenUntil == nil || *got.PoolFrozenUntil != 321 || !got.PoolLastSelected || got.CreatedAt != before.CreatedAt {
		t.Fatalf("Go read of Rust state: %+v", got)
	}
	added, err := db.GetPixiv(ctx, 9)
	if err != nil || added.SortOrder != 2 || added.CredentialRevision != 1 || string(added.RefreshTokenCopy()) != "synthetic-rust-new" {
		t.Fatalf("Rust added account: %+v, %v", added, err)
	}
	if err = db.RotatePixivCredentials(ctx, 7, 2, []byte("synthetic-go-rotated")); err != nil {
		t.Fatal(err)
	}
	if err = db.RemovePixiv(ctx, 9); err != nil {
		t.Fatal(err)
	}
	db.Close()
	run("read")
}

type migrationAccountInput struct {
	ID          int64  `json:"id"`
	Order       int64  `json:"order"`
	Name        string `json:"name"`
	Token       string `json:"token"`
	Revision    int64  `json:"revision"`
	Premium     *bool  `json:"premium"`
	Checked     *int64 `json:"checked"`
	Frozen      *int64 `json:"frozen"`
	Selected    bool   `json:"selected"`
	Schedulable bool   `json:"schedulable"`
}

type migrationAccountOperation struct {
	Action   string                  `json:"action"`
	Accounts []migrationAccountInput `json:"accounts"`
	ID       int64                   `json:"id"`
	Revision int64                   `json:"revision"`
	Token    string                  `json:"token"`
	Name     string                  `json:"name"`
	Premium  *bool                   `json:"premium"`
	Checked  *int64                  `json:"checked"`
	Error    string                  `json:"error"`
	Kind     string                  `json:"kind"`
	Rows     []map[string]any        `json:"rows"`
	Get      map[string]any          `json:"get"`
}

func TestMigrationAccountsPreserveCredentialsMetadataAndAtomicChanges(t *testing.T) {
	truth, falsity := true, false
	checked, frozen := int64(123), int64(321)
	seed := migrationAccountInput{ID: 7, Order: 4, Name: "synthetic", Token: "synthetic-first", Revision: 88, Premium: &truth, Checked: &checked, Frozen: &frozen, Selected: true}
	other := migrationAccountInput{ID: 9, Name: "second", Token: "synthetic-second"}
	newAccount := migrationAccountInput{ID: 11, Name: "third", Token: "synthetic-third"}
	reimport := migrationAccountInput{ID: 7, Order: 99, Name: "renamed", Token: "synthetic-reimport", Revision: 999, Premium: &falsity, Schedulable: true}
	conflict := migrationAccountInput{ID: 13, Order: 4, Name: "conflict", Token: "synthetic-conflict"}
	marker := migrationAccountInput{ID: 15, Token: "synthetic-marker", Selected: true}
	ops := []migrationAccountOperation{
		{Action: "list"}, {Action: "get", ID: 7}, {Action: "get", ID: -1},
		{Action: "save", Accounts: []migrationAccountInput{{ID: 0, Token: "synthetic"}}},
		{Action: "save", Accounts: []migrationAccountInput{{ID: 7}}},
		{Action: "save", Accounts: []migrationAccountInput{seed}},
		{Action: "save", Accounts: []migrationAccountInput{other}},
		{Action: "get", ID: 7},
		{Action: "save", Accounts: []migrationAccountInput{reimport}},
		{Action: "metadata", ID: 7, Name: "metadata", Premium: &falsity, Checked: &checked},
		{Action: "metadata", ID: 7, Name: "cleared"},
		{Action: "metadata", ID: 999}, {Action: "metadata", ID: 0},
		{Action: "rotate", ID: 7, Revision: 2, Token: "synthetic-rotated"},
		{Action: "rotate", ID: 7, Revision: 2, Token: "synthetic-stale"},
		{Action: "rotate", ID: 999, Revision: 1, Token: "synthetic-missing"},
		{Action: "rotate", ID: 7, Revision: 0, Token: "synthetic-invalid"},
		{Action: "rotate", ID: 0, Revision: 1, Token: "synthetic-invalid"},
		{Action: "rotate", ID: 7, Revision: 3},
		{Action: "batch"},
		{Action: "batch", Accounts: []migrationAccountInput{newAccount, newAccount}},
		{Action: "batch", Accounts: []migrationAccountInput{newAccount, {ID: 0, Token: "synthetic"}}},
		{Action: "batch", Accounts: []migrationAccountInput{reimport, conflict}},
		{Action: "batch", Accounts: []migrationAccountInput{newAccount, marker}},
		{Action: "batch", Accounts: []migrationAccountInput{reimport, newAccount}},
		{Action: "remove", ID: 9}, {Action: "remove", ID: 9},
		{Action: "save", Accounts: []migrationAccountInput{{ID: 20, Order: -7, Token: "synthetic-new"}}},
		{Action: "remove", ID: 7}, {Action: "get", ID: 7},
		{Action: "save", Accounts: []migrationAccountInput{marker}},
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
		inputs := make([]account.Account, 0, len(op.Accounts))
		for _, input := range op.Accounts {
			a := account.New(input.ID, input.Name, []byte(input.Token))
			a.SortOrder = input.Order
			a.CredentialRevision = input.Revision
			a.PremiumStatus = input.Premium
			a.PremiumCheckedAt = input.Checked
			a.PoolFrozenUntil = input.Frozen
			a.PoolLastSelected = input.Selected
			a.Schedulable = input.Schedulable
			inputs = append(inputs, a)
		}
		err = nil
		var got account.Account
		switch op.Action {
		case "list":
		case "get":
			got, err = db.GetPixiv(ctx, op.ID)
		case "save":
			err = db.SavePixivCredential(ctx, inputs[0])
		case "batch":
			err = db.SavePixivCredentials(ctx, inputs)
		case "metadata":
			err = db.UpdatePixivMetadata(ctx, op.ID, op.Name, op.Premium, op.Checked)
		case "rotate":
			err = db.RotatePixivCredentials(ctx, op.ID, op.Revision, []byte(op.Token))
		case "remove":
			err = db.RemovePixiv(ctx, op.ID)
		}
		end := time.Now().Unix()
		if err != nil {
			op.Error = err.Error()
			op.Kind = "storage"
			if errors.Is(err, account.ErrNotFound) {
				op.Kind = "not_found"
			}
			if errors.Is(err, account.ErrCredentialConflict) {
				op.Kind = "credential_conflict"
			}
			if op.Error == "database: invalid pixiv account" || op.Error == "database: invalid rotation input" || op.Error == "database: invalid pixiv metadata input" {
				op.Kind = "invalid"
			}
			if op.Error == "database: duplicate pixiv account 11" {
				op.Kind = "duplicate"
			}
		} else if op.Action == "get" {
			op.Get = migrationAccountState(t, got, start, end)
		}
		rows, err := db.ListPixiv(ctx)
		if err != nil {
			t.Fatal(err)
		}
		op.Rows = []map[string]any{}
		for _, row := range rows {
			op.Rows = append(op.Rows, migrationAccountState(t, row, start, end))
		}
	}
	encoded, err := json.MarshalIndent(ops, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "accounts.json")
	if *updateAccountsContract {
		if err = os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(encoded, want) {
		t.Fatal("account repository contracts differ")
	}
}

func migrationAccountState(t *testing.T, a account.Account, start, end int64) map[string]any {
	t.Helper()
	normalize := func(value int64) int64 {
		if value == 11 || value == 22 {
			return value
		}
		if value < start || value > end {
			t.Fatalf("timestamp outside operation window: %d", value)
		}
		return -1
	}
	return map[string]any{"id": a.UserID, "order": a.SortOrder, "name": a.Username, "token": string(a.RefreshTokenCopy()), "revision": a.CredentialRevision, "premium": a.PremiumStatus, "checked": a.PremiumCheckedAt, "frozen": a.PoolFrozenUntil, "selected": a.PoolLastSelected, "schedulable": a.Schedulable, "created": normalize(a.CreatedAt), "updated": normalize(a.UpdatedAt)}
}

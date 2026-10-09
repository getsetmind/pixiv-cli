package pixiv_test

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"github.com/FlanChanXwO/pixiv-cli/internal/storage/database"
)

var updateAuthViews = flag.Bool("migration-update-auth-views", false, "update local auth account view contracts")

type authViewRepository struct {
	account.Repository
	calls   []string
	pool    account.PoolStatus
	rows    []account.Account
	poolErr error
	listErr error
}

func (r *authViewRepository) ListPixivPoolStatus(_ context.Context, _ int64) (account.PoolStatus, error) {
	r.calls = append(r.calls, "pool")
	return r.pool, r.poolErr
}
func (r *authViewRepository) ListPixiv(_ context.Context) ([]account.Account, error) {
	r.calls = append(r.calls, "list")
	return r.rows, r.listErr
}

func TestMigrationAuthViewsSnapshotAndErrorOrdering(t *testing.T) {
	frozen := int64(4102444800)
	premium := true
	row := account.New(42, "synthetic", nil)
	row.PremiumStatus = &premium
	row.Schedulable = true
	row.PoolFrozenUntil = &frozen
	r := &authViewRepository{rows: []account.Account{row}, pool: account.PoolStatus{Accounts: []account.PoolCandidate{{UserID: 42, Schedulable: false, Eligible: false, PoolLastSelected: true}}}}
	defaults := &pixivTestDefaults{}
	result, err := account.NewService(r, defaults).ListAccounts(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(r.calls, []string{"pool", "list", "list"}) {
		t.Fatalf("calls %v", r.calls)
	}
	if len(result) != 1 || !result[0].Default || result[0].Schedulable || result[0].PoolFrozenUntil != nil || !result[0].PoolLastSelected || !result[0].PoolStatusKnown || result[0].Premium == nil || !*result[0].Premium {
		t.Fatalf("summary %+v", result)
	}
	r.calls = nil
	r.poolErr = errors.New("pool failed")
	r.listErr = errors.New("list failed")
	if _, err = account.NewService(r, nil).ListAccounts(context.Background()); err.Error() != "pool failed" || !reflect.DeepEqual(r.calls, []string{"pool"}) {
		t.Fatalf("pool priority %v %v", err, r.calls)
	}
	r.calls = nil
	r.poolErr = nil
	if _, err = account.NewService(r, nil).ListAccounts(context.Background()); err.Error() != "list failed" {
		t.Fatal(err)
	}
	r.listErr = nil
	if _, err = account.NewService(r, nil).ListAccounts(context.Background()); err.Error() != "pixiv default account store is not configured" {
		t.Fatal(err)
	}
	r.rows = nil
	if result, err = account.NewService(r, nil).ListAccounts(context.Background()); err != nil || len(result) != 0 {
		t.Fatalf("empty %v %v", result, err)
	}
}

func TestMigrationAuthViewsLocalSummariesAndAtomicPoolChanges(t *testing.T) {
	db, err := database.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	ctx := context.Background()
	for _, id := range []int64{42, 7} {
		if err = db.SavePixivCredential(ctx, account.New(id, "synthetic", []byte("synthetic-token"))); err != nil {
			t.Fatal(err)
		}
	}
	if _, err = db.DB().Exec("UPDATE pixiv_account SET premium_status=1,premium_checked_at=123,pool_frozen_until=4102444800,pool_last_selected=1 WHERE user_id=7"); err != nil {
		t.Fatal(err)
	}
	defaults := &pixivTestDefaults{}
	service := account.NewService(db, defaults)
	records := []map[string]any{}
	capture := func(name string) {
		rows, e := service.ListAccounts(ctx)
		if e != nil {
			t.Fatal(e)
		}
		records = append(records, map[string]any{"name": name, "accounts": rows})
	}
	capture("implicit_default")
	defaults.userID = 7
	defaults.ok = true
	capture("explicit_default")
	defaults.userID = 999
	capture("missing_explicit_default")
	for _, op := range []struct {
		name string
		ids  []int64
	}{{"empty", nil}, {"zero", []int64{0}}, {"duplicate", []int64{42, 42}}, {"unknown", []int64{42, 999}}} {
		e := service.SetPoolSchedulable(ctx, op.ids, false)
		if e == nil {
			t.Fatal(op.name)
		}
		stored, _ := db.GetPixiv(ctx, 42)
		if !stored.Schedulable {
			t.Fatal("partial update")
		}
		records = append(records, map[string]any{"name": op.name, "error": e.Error()})
	}
	if err = service.SetPoolSchedulable(ctx, []int64{7}, false); err != nil {
		t.Fatal(err)
	}
	capture("disabled_frozen")
	if err = service.SetAllPoolSchedulable(ctx, false); err != nil {
		t.Fatal(err)
	}
	capture("all_disabled")
	if err = service.SetAllPoolSchedulable(ctx, true); err != nil {
		t.Fatal(err)
	}
	capture("all_enabled")
	if _, err = db.DB().Exec("UPDATE pixiv_account SET pool_frozen_until=1,updated_at=22 WHERE user_id=7"); err != nil {
		t.Fatal(err)
	}
	capture("expired_freeze")
	var updated int64
	if err = db.DB().QueryRow("SELECT updated_at FROM pixiv_account WHERE user_id=7").Scan(&updated); err != nil || updated != 22 {
		t.Fatalf("expiry updated_at %d %v", updated, err)
	}
	canceled, cancel := context.WithCancel(ctx)
	cancel()
	if _, err = service.ListAccounts(canceled); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if err = service.SetAllPoolSchedulable(canceled, false); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	records = append(records, map[string]any{"name": "canceled", "error": "context canceled"})
	if err = service.SetPoolSchedulable(canceled, []int64{0}, false); err == nil || err.Error() != "database: account pool user id must be positive" {
		t.Fatalf("validation precedes cancellation: %v", err)
	}
	body, err := json.MarshalIndent(records, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join("..", "..", "..", "..", "docs", "migration", "contracts", "auth-account-views.json")
	if *updateAuthViews {
		if err = os.WriteFile(path, body, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(body, want) {
		t.Fatal("auth account view contract changed")
	}
}

func TestMigrationAuthViewsMembershipRollbackPreservesMetadata(t *testing.T) {
	db, err := database.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	ctx := context.Background()
	for _, id := range []int64{42, 7} {
		if err = db.SavePixivCredential(ctx, account.New(id, "synthetic", []byte("synthetic-token"))); err != nil {
			t.Fatal(err)
		}
	}
	if _, err = db.DB().Exec("UPDATE pixiv_account SET premium_status=1,premium_checked_at=123,pool_frozen_until=4102444800,pool_last_selected=1,created_at=11,updated_at=22 WHERE user_id=7"); err != nil {
		t.Fatal(err)
	}
	if _, err = db.DB().Exec("CREATE TRIGGER reject_membership BEFORE UPDATE OF schedulable ON pixiv_account WHEN NEW.user_id=7 BEGIN SELECT RAISE(ABORT,'synthetic rejection'); END"); err != nil {
		t.Fatal(err)
	}
	service := account.NewService(db, &pixivTestDefaults{})
	for _, all := range []bool{false, true} {
		if all {
			err = service.SetAllPoolSchedulable(ctx, false)
		} else {
			err = service.SetPoolSchedulable(ctx, []int64{42, 7}, false)
		}
		if err == nil {
			t.Fatal("trigger must abort")
		}
		rows, e := db.ListPixiv(ctx)
		if e != nil {
			t.Fatal(e)
		}
		for _, row := range rows {
			if !row.Schedulable || row.CredentialRevision != 1 || string(row.RefreshTokenCopy()) != "synthetic-token" {
				t.Fatal("membership rollback changed credentials")
			}
		}
		second := rows[1]
		if second.PremiumStatus == nil || !*second.PremiumStatus || second.PremiumCheckedAt == nil || *second.PremiumCheckedAt != 123 || second.PoolFrozenUntil == nil || *second.PoolFrozenUntil != 4102444800 || !second.PoolLastSelected || second.CreatedAt != 11 || second.UpdatedAt != 22 {
			t.Fatal("membership rollback changed metadata")
		}
	}
}

type authViewDefaults struct {
	*pixivTestDefaults
	reads int
}

func (d *authViewDefaults) ReadPixivDefaultUserID() (int64, bool, error) {
	d.reads++
	return d.pixivTestDefaults.ReadPixivDefaultUserID()
}

func TestMigrationAuthViewsReadsDefaultForEachSummary(t *testing.T) {
	rows := []account.Account{account.New(42, "first", nil), account.New(7, "second", nil)}
	for _, explicit := range []bool{false, true} {
		repository := &authViewRepository{rows: rows}
		defaults := &authViewDefaults{pixivTestDefaults: &pixivTestDefaults{userID: 7, ok: explicit}}
		result, err := account.NewService(repository, defaults).ListAccounts(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if defaults.reads != 2 {
			t.Fatalf("default reads %d", defaults.reads)
		}
		want := []string{"pool", "list"}
		if !explicit {
			want = append(want, "list", "list")
		}
		if !reflect.DeepEqual(repository.calls, want) {
			t.Fatalf("calls %v", repository.calls)
		}
		if result[0].Default == explicit || result[1].Default != explicit {
			t.Fatal("default selection")
		}
	}
}

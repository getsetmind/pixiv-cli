package database

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"testing"
	"time"

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/fanbox/account"
	"modernc.org/sqlite"
)

var captureFanboxSavedRepository = flag.Bool("migration-capture-fanbox-saved-repository", false, "capture frozen FANBOX saved repository contracts")

type migrationFanboxRepositoryCase struct {
	Name   string           `json:"name"`
	Input  map[string]any   `json:"input"`
	Result any              `json:"result"`
	Error  map[string]any   `json:"error"`
	Rows   []map[string]any `json:"rows"`
}

func TestMigrationFanboxSavedRepositoryFrozenGo(t *testing.T) {
	sources := map[string]string{
		"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/storage/database/repository.go":                                  "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
		"internal/storage/database/database.go":                                    "2937e7cbd131e2aa064494b1a7482ef25bf285a5b4c20e734e44d6f1d8aac61e",
		"internal/storage/database/migrate.go":                                     "5a464652668c61e04e4439adcf9717fc949211d60ac311b494ae1d37cff12c96",
		"internal/storage/database/migrations/0001_initial.sql":                    "9b4f2e2c94a19a0958720dc824897b2eb6b47249b465ff097c49b1ff6ae60d37",
		"internal/storage/database/migrations/0002_fanbox_creator_id_not_null.sql": "ff4f91ea6c57b68e6b311ba7eb07beac191c5f2c3be2eae5a828c33cc91207d7",
		"internal/storage/database/migrations/0003_pixiv_account_schedulable.sql":  "21e7b42d42cf46939fc73d4a75115724b722d2e013208a46af85c730d3e37dcd",
		"internal/services/fanbox/account/fanbox.go":                               "0e7af911bd41245607d7bb0e1dc0206776785ddb88cd45f9344f927d2c9b72a8",
	}
	for path, want := range sources {
		body, err := os.ReadFile(filepath.Join("..", "..", "..", path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(body)); got != want {
			t.Fatalf("frozen source changed: %s: %s", path, got)
		}
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("unexpected Go version: %s", runtime.Version())
	}
	build, ok := debug.ReadBuildInfo()
	if !ok {
		t.Fatal("Go dependency provenance unavailable")
	}
	driver := ""
	for _, dep := range build.Deps {
		if dep.Path == "modernc.org/sqlite" {
			driver = dep.Version
		}
	}
	if driver != "v1.40.1" {
		t.Fatalf("unexpected SQLite dependency: %s", driver)
	}
	var cases []migrationFanboxRepositoryCase
	var sqliteVersion string
	newDB := func() *DB {
		db, err := Open(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = db.Close() })
		if err = db.DB().QueryRow("SELECT sqlite_version()").Scan(&sqliteVersion); err != nil {
			t.Fatal(err)
		}
		return db
	}
	ctx := context.Background()
	db := newDB()
	for _, input := range []struct {
		name, action                   string
		id, order, revision, validated int64
		nameValue, creator, session    string
	}{
		{name: "empty list", action: "list"},
		{name: "missing get", action: "get", id: 99},
		{name: "missing remove", action: "remove", id: 99},
		{name: "zero ID", action: "save", session: "fixture-session", validated: 30},
		{name: "negative ID", action: "save", id: -1, session: "fixture-session", validated: 30},
		{name: "empty session", action: "save", id: 7, validated: 30},
		{name: "zero validated", action: "save", id: 7, session: "fixture-session"},
		{name: "negative validated", action: "save", id: 7, session: "fixture-session", validated: -1},
		{name: "new ignores caller revision and timestamps", action: "save", id: 7, order: 4, revision: 91, validated: 30, nameValue: "first", session: "fixture-session-1"},
		{name: "next uses max order", action: "save", id: 9, validated: 31, nameValue: "second", creator: "fixture-creator", session: "fixture-session-2"},
		{name: "duplicate order rolls back", action: "save", id: 8, order: 4, validated: 32, session: "fixture-conflict"},
		{name: "upsert preserves order and created", action: "save", id: 7, order: 99, revision: 99, validated: 33, nameValue: "renamed", creator: "fixture-renamed", session: "fixture-replacement"},
		{name: "stored account", action: "get", id: 7},
		{name: "rotation zero ID", action: "rotate", revision: 2, validated: 34, session: "fixture-rotate"},
		{name: "rotation negative ID", action: "rotate", id: -1, revision: 2, validated: 34, session: "fixture-rotate"},
		{name: "rotation zero revision", action: "rotate", id: 7, validated: 34, session: "fixture-rotate"},
		{name: "rotation negative revision", action: "rotate", id: 7, revision: -1, validated: 34, session: "fixture-rotate"},
		{name: "rotation empty session", action: "rotate", id: 7, revision: 2, validated: 34},
		{name: "rotation zero validated", action: "rotate", id: 7, revision: 2, session: "fixture-rotate"},
		{name: "rotation negative validated", action: "rotate", id: 7, revision: 2, validated: -1, session: "fixture-rotate"},
		{name: "rotation missing", action: "rotate", id: 99, revision: 2, validated: 34, session: "fixture-rotate"},
		{name: "rotation stale revision", action: "rotate", id: 7, revision: 1, validated: 34, session: "fixture-stale"},
		{name: "rotation success", action: "rotate", id: 7, revision: 2, validated: 34, session: "fixture-rotate"},
		{name: "rotation cannot repeat", action: "rotate", id: 7, revision: 2, validated: 35, session: "fixture-stale"},
		{name: "remove first", action: "remove", id: 7},
		{name: "sort not renumbered", action: "list"},
		{name: "save after removal", action: "save", id: 11, validated: 36, session: "fixture-last"},
		{name: "nonpositive order autoassign", action: "save", id: 12, order: -9, validated: 37, session: "fixture-negative-order"},
		{name: "caller session retained literally", action: "save", id: 13, validated: 38, session: "  fixture-space  "},
		{name: "negative get", action: "get", id: -1},
		{name: "zero remove", action: "remove"},
	} {
		if _, err := db.DB().Exec("UPDATE fanbox_account SET created_at=11, updated_at=22"); err != nil {
			t.Fatal(err)
		}
		row := migrationFanboxRepositoryCase{Name: input.name, Input: map[string]any{"action": input.action, "id": input.id, "sort_order": input.order, "revision": input.revision, "validated_at": input.validated, "display_name": input.nameValue, "creator_id": input.creator, "session": input.session}}
		start := time.Now().Unix()
		var err error
		switch input.action {
		case "save":
			a := account.New(input.id, input.nameValue, input.creator, []byte(input.session))
			a.SortOrder = input.order
			a.CredentialRevision = input.revision
			a.ValidatedAt = input.validated
			a.CreatedAt = 88
			a.UpdatedAt = 99
			err = db.SaveFanboxCredential(ctx, a)
		case "rotate":
			err = db.RotateFanboxSession(ctx, input.id, input.revision, []byte(input.session), input.validated)
		case "remove":
			err = db.RemoveFanbox(ctx, input.id)
		case "get":
			var a account.Account
			a, err = db.GetFanbox(ctx, input.id)
			row.Result = migrationFanboxStoredAccount(t, a, start, time.Now().Unix())
		case "list":
			var accounts []account.Account
			accounts, err = db.ListFanbox(ctx)
			row.Result = migrationFanboxStoredAccounts(t, accounts, start, time.Now().Unix())
		}
		row.Error = migrationFanboxStorageError(err)
		accounts, listErr := db.ListFanbox(ctx)
		if listErr != nil {
			t.Fatal(listErr)
		}
		row.Rows = migrationFanboxStoredAccounts(t, accounts, start, time.Now().Unix())
		cases = append(cases, row)
	}
	for _, action := range []string{"save", "rotate", "list", "get", "remove"} {
		for _, failure := range []string{"canceled", "deadline", "closed", "missing_table"} {
			db := newDB()
			a := account.New(7, "fixture", "", []byte("fixture-session"))
			a.ValidatedAt = 30
			if err := db.SaveFanboxCredential(ctx, a); err != nil {
				t.Fatal(err)
			}
			if _, err := db.DB().Exec("UPDATE fanbox_account SET created_at=11,updated_at=22"); err != nil {
				t.Fatal(err)
			}
			callCtx, cancel := context.WithCancel(ctx)
			switch failure {
			case "canceled":
				cancel()
			case "deadline":
				cancel()
				callCtx, cancel = context.WithDeadline(ctx, time.Unix(1, 0))
			case "closed":
				if err := db.Close(); err != nil {
					t.Fatal(err)
				}
			case "missing_table":
				if _, err := db.DB().Exec("DROP TABLE fanbox_account"); err != nil {
					t.Fatal(err)
				}
			}
			row := migrationFanboxRepositoryCase{Name: action + "/" + failure, Input: map[string]any{"action": action, "failure": failure}}
			var err error
			switch action {
			case "save":
				err = db.SaveFanboxCredential(callCtx, a)
			case "rotate":
				err = db.RotateFanboxSession(callCtx, 7, 1, []byte("fixture-rotated"), 31)
			case "list":
				var accounts []account.Account
				accounts, err = db.ListFanbox(callCtx)
				row.Result = migrationFanboxStoredAccounts(t, accounts, 0, 0)
			case "get":
				var got account.Account
				got, err = db.GetFanbox(callCtx, 7)
				row.Result = migrationFanboxStoredAccount(t, got, 0, 0)
			case "remove":
				err = db.RemoveFanbox(callCtx, 7)
			}
			cancel()
			row.Error = migrationFanboxStorageError(err)
			if failure == "canceled" || failure == "deadline" {
				accounts, e := db.ListFanbox(ctx)
				if e != nil {
					t.Fatal(e)
				}
				row.Rows = migrationFanboxStoredAccounts(t, accounts, 0, time.Now().Unix())
			}
			cases = append(cases, row)
		}
	}
	for _, action := range []string{"save_insert", "save_update", "rotate", "remove"} {
		db := newDB()
		a := account.New(7, "fixture", "", []byte("fixture-session"))
		a.ValidatedAt = 30
		if err := db.SaveFanboxCredential(ctx, a); err != nil {
			t.Fatal(err)
		}
		if _, err := db.DB().Exec("UPDATE fanbox_account SET created_at=11,updated_at=22"); err != nil {
			t.Fatal(err)
		}
		event := "UPDATE"
		if action == "save_insert" {
			event = "INSERT"
		}
		if action == "remove" {
			event = "DELETE"
		}
		if _, err := db.DB().Exec("CREATE TRIGGER reject_fanbox BEFORE " + event + " ON fanbox_account BEGIN SELECT RAISE(ABORT,'owned fixture persist failure'); END"); err != nil {
			t.Fatal(err)
		}
		var err error
		switch action {
		case "save_insert":
			a = account.New(8, "new", "", []byte("fixture-new"))
			a.ValidatedAt = 31
			err = db.SaveFanboxCredential(ctx, a)
		case "save_update":
			err = db.SaveFanboxCredential(ctx, a)
		case "rotate":
			err = db.RotateFanboxSession(ctx, 7, 1, []byte("fixture-rotated"), 31)
		case "remove":
			err = db.RemoveFanbox(ctx, 7)
		}
		accounts, e := db.ListFanbox(ctx)
		if e != nil {
			t.Fatal(e)
		}
		cases = append(cases, migrationFanboxRepositoryCase{Name: action + "/trigger rollback", Input: map[string]any{"action": action, "event": event}, Error: migrationFanboxStorageError(err), Rows: migrationFanboxStoredAccounts(t, accounts, 0, 0)})
	}
	for _, action := range []string{"save_insert", "save_update"} {
		db := newDB()
		a := account.New(7, "fixture", "", []byte("fixture-session"))
		a.ValidatedAt = 30
		if err := db.SaveFanboxCredential(ctx, a); err != nil {
			t.Fatal(err)
		}
		if _, err := db.DB().Exec("UPDATE fanbox_account SET created_at=11,updated_at=22; CREATE TABLE owned_parent(id INTEGER PRIMARY KEY); CREATE TABLE owned_child(id INTEGER REFERENCES owned_parent(id) DEFERRABLE INITIALLY DEFERRED)"); err != nil {
			t.Fatal(err)
		}
		event := "UPDATE"
		if action == "save_insert" {
			event = "INSERT"
			a = account.New(8, "new", "", []byte("fixture-uncommitted"))
			a.ValidatedAt = 31
		}
		if _, err := db.DB().Exec("CREATE TRIGGER deferred_fanbox AFTER " + event + " ON fanbox_account BEGIN INSERT INTO owned_child VALUES(999); END"); err != nil {
			t.Fatal(err)
		}
		start := time.Now().Unix()
		err := db.SaveFanboxCredential(ctx, a)
		visible, e := db.ListFanbox(ctx)
		if e != nil {
			t.Fatal(e)
		}
		next := account.New(9, "next", "", []byte("fixture-next"))
		next.ValidatedAt = 32
		nextErr := db.SaveFanboxCredential(ctx, next)
		if e = db.Close(); e != nil {
			t.Fatal(e)
		}
		reopened, e := Open(filepath.Dir(db.Path()))
		if e != nil {
			t.Fatal(e)
		}
		durable, e := reopened.ListFanbox(ctx)
		if e != nil {
			t.Fatal(e)
		}
		if e = reopened.Close(); e != nil {
			t.Fatal(e)
		}
		cases = append(cases, migrationFanboxRepositoryCase{Name: action + "/deferred commit failure", Input: map[string]any{"action": action, "failure": "owned deferred foreign key"}, Error: migrationFanboxStorageError(err), Rows: migrationFanboxStoredAccounts(t, visible, start, time.Now().Unix()), Result: map[string]any{"next_save_error": migrationFanboxStorageError(nextErr), "durable_after_close": migrationFanboxStoredAccounts(t, durable, 0, 0)}})
	}
	for _, column := range []string{"user_id", "sort_order", "display_name", "creator_id", "session_id", "credential_revision", "validated_at", "created_at", "updated_at"} {
		for _, malformed := range []string{"null", "bad_type"} {
			db := newDB()
			if _, err := db.DB().Exec("DROP TABLE fanbox_account; CREATE TABLE fanbox_account (user_id,sort_order,display_name,creator_id,session_id,credential_revision,validated_at,created_at,updated_at); INSERT INTO fanbox_account VALUES (7,1,'first','',x'6162',1,30,11,22),(8,2,'second','creator',x'6364',2,31,12,23)"); err != nil {
				t.Fatal(err)
			}
			value := "NULL"
			if malformed == "bad_type" {
				value = "'not-an-integer'"
			}
			if column == "user_id" {
				if _, err := db.DB().Exec("UPDATE fanbox_account SET " + column + "=" + value + " WHERE sort_order=2"); err != nil {
					t.Fatal(err)
				}
			} else if _, err := db.DB().Exec("UPDATE fanbox_account SET " + column + "=" + value + " WHERE user_id=8"); err != nil {
				t.Fatal(err)
			}
			start := time.Now().Unix()
			accounts, listErr := db.ListFanbox(ctx)
			got, getErr := db.GetFanbox(ctx, 8)
			cases = append(cases, migrationFanboxRepositoryCase{Name: "scan/" + column + "/" + malformed, Input: map[string]any{"column": column, "value": malformed}, Error: migrationFanboxStorageError(listErr), Rows: migrationFanboxStoredAccounts(t, accounts, start, time.Now().Unix()), Result: map[string]any{"get": migrationFanboxStoredAccount(t, got, start, time.Now().Unix()), "get_error": migrationFanboxStorageError(getErr), "list_nil": accounts == nil}})
		}
	}
	section := map[string]any{"source_commit": "4b4426487ef18bed276706daec385e0d0a6979f9", "source_sha256": sources, "go_version": runtime.Version(), "driver_version": driver, "sqlite_version": sqliteVersion, "cases": cases, "go_only": []string{"typed SQLite extended error code and exact text", "private scanFanboxAccount and ListFanbox partial row discard", "database/sql cancellation/deadline/closed errors", "Deferred foreign-key commit failure leaves a transaction visible in the same Go driver connection and next BeginTx fails until DB.Close rolls it back; owned fixture captures durable reopened rows"}, "limitations": []string{"All SQLite and configuration paths are owned temporary files; no user account or external network is used", "CreatedAt and UpdatedAt values produced by wall clock are range-checked and represented as current-time; all other timestamps retain exact values", "Malformed scan rows use an owned unconstrained SQLite table; released schema still retains NOT NULL and CHECK constraints", "Native Windows ACL and cross-platform SQLite driver diagnostics are not established"}}
	migrationFanboxRepositoryFixture(t, section)
	t.Logf("fresh repository cases: %d", len(cases))
}

func migrationFanboxStorageError(err error) map[string]any {
	text := ""
	code := 0
	if err != nil {
		text = err.Error()
		var typed *sqlite.Error
		if errors.As(err, &typed) {
			code = typed.Code()
		}
	}
	return map[string]any{"text": text, "not_found": errors.Is(err, account.ErrNotFound), "credential_conflict": errors.Is(err, account.ErrCredentialConflict), "canceled": errors.Is(err, context.Canceled), "deadline": errors.Is(err, context.DeadlineExceeded), "sqlite_extended_code": code}
}

func migrationFanboxStoredAccount(t *testing.T, a account.Account, start, end int64) map[string]any {
	t.Helper()
	stamp := func(value int64) any {
		if value > 100 {
			if value < start || value > end {
				t.Fatalf("timestamp outside observed operation: %d not in [%d,%d]", value, start, end)
			}
			return "current-time"
		}
		return value
	}
	return map[string]any{"user_id": a.UserID, "sort_order": a.SortOrder, "display_name": a.DisplayName, "creator_id": a.CreatorID, "session": string(a.SessionIDCopy()), "session_nil": a.SessionIDCopy() == nil, "has_session": a.HasSession(), "credential_revision": a.CredentialRevision, "validated_at": a.ValidatedAt, "created_at": stamp(a.CreatedAt), "updated_at": stamp(a.UpdatedAt)}
}

func migrationFanboxStoredAccounts(t *testing.T, accounts []account.Account, start, end int64) []map[string]any {
	if accounts == nil {
		return nil
	}
	out := []map[string]any{}
	for _, a := range accounts {
		out = append(out, migrationFanboxStoredAccount(t, a, start, end))
	}
	return out
}

func migrationFanboxRepositoryFixture(t *testing.T, section any) {
	t.Helper()
	data, err := json.MarshalIndent(section, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *captureFanboxSavedRepository {
		if err = os.WriteFile(filepath.Join(os.TempDir(), "fanbox-saved-accounts-repository.json"), data, 0600); err != nil {
			t.Fatal(err)
		}
		return
	}
	body, err := os.ReadFile(filepath.Join("..", "..", "..", "crates", "pixiv-app", "tests", "fixtures", "fanbox-saved-accounts.json"))
	if err != nil {
		t.Fatal(err)
	}
	var fixture map[string]json.RawMessage
	if err = json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	var want any
	decoder := json.NewDecoder(bytes.NewReader(fixture["repository"]))
	decoder.UseNumber()
	if err = decoder.Decode(&want); err != nil {
		t.Fatal(err)
	}
	canonical, err := json.MarshalIndent(want, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	canonical = append(canonical, '\n')
	var observed any
	observedDecoder := json.NewDecoder(bytes.NewReader(data))
	observedDecoder.UseNumber()
	if err = observedDecoder.Decode(&observed); err != nil {
		t.Fatal(err)
	}
	actual, err := json.MarshalIndent(observed, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	actual = append(actual, '\n')
	if !bytes.Equal(actual, canonical) {
		t.Fatalf("FANBOX repository contracts changed; fresh observation saved to diagnostic: %s", migrationFanboxRepositoryDiagnostic(t, data))
	}
}

func migrationFanboxRepositoryDiagnostic(t *testing.T, data []byte) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "repository-observed.json")
	if err := os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
	return path
}

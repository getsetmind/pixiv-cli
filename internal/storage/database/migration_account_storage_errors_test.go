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

	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"modernc.org/sqlite"
)

var captureAccountStorageErrors = flag.Bool("migration-capture-account-storage-errors", false, "capture frozen account storage errors")

func TestMigrationAccountStorageErrorsFrozenGo(t *testing.T) {
	type row struct {
		Name         string `json:"name"`
		Trigger      string `json:"trigger"`
		Message      string `json:"message"`
		ExtendedCode int    `json:"extended_code"`
		Token        string `json:"token"`
		Revision     int64  `json:"revision"`
		AccountCount int    `json:"account_count"`
	}
	contract := struct {
		SourceCommit  string            `json:"source_commit"`
		SourceSHA256  map[string]string `json:"source_sha256"`
		GoVersion     string            `json:"go_version"`
		DriverVersion string            `json:"driver_version"`
		SQLiteVersion string            `json:"sqlite_version"`
		Cases         []row             `json:"cases"`
	}{
		SourceCommit: "4b4426487ef18bed276706daec385e0d0a6979f9",
		SourceSHA256: map[string]string{
			"internal/storage/database/repository.go": "75abdfe0d16877d6cff0820efe705a0bb013ceb58088e372ea1a69c95e913477",
		},
		GoVersion: runtime.Version(),
	}
	for path, expected := range contract.SourceSHA256 {
		source, err := os.ReadFile(filepath.Join("..", "..", "..", path))
		if err != nil {
			t.Fatal(err)
		}
		if actual := fmt.Sprintf("%x", sha256.Sum256(source)); actual != expected {
			t.Fatalf("frozen source changed: %s: %s", path, actual)
		}
	}
	build, ok := debug.ReadBuildInfo()
	if !ok {
		t.Fatal("Go build provenance unavailable")
	}
	for _, dependency := range build.Deps {
		if dependency.Path == "modernc.org/sqlite" {
			contract.DriverVersion = dependency.Version
		}
	}
	if contract.DriverVersion != "v1.40.1" {
		t.Fatalf("unexpected SQLite driver: %s", contract.DriverVersion)
	}
	for _, input := range []row{
		{Name: "trigger", Trigger: "CREATE TRIGGER reject_refresh BEFORE UPDATE ON pixiv_account BEGIN SELECT RAISE(ABORT, 'owned fixture persist failure'); END;"},
		{Name: "unique"},
		{Name: "not_null", Trigger: "CREATE TRIGGER reject_refresh BEFORE UPDATE ON pixiv_account BEGIN UPDATE pixiv_account SET username=NULL WHERE user_id=NEW.user_id; END;"},
	} {
		db, err := Open(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		ctx := context.Background()
		if err := db.DB().QueryRow("SELECT sqlite_version()").Scan(&contract.SQLiteVersion); err != nil {
			t.Fatal(err)
		}
		if err := db.SavePixivCredential(ctx, account.New(7, "fixture", []byte("fixture-original"))); err != nil {
			t.Fatal(err)
		}
		if input.Trigger != "" {
			if _, err := db.DB().Exec(input.Trigger); err != nil {
				t.Fatal(err)
			}
			err = db.RotatePixivCredentials(ctx, 7, 1, []byte("fixture-rotated"))
		} else {
			first := account.New(8, "first", []byte("fixture-first"))
			conflicting := account.New(9, "conflicting", []byte("fixture-second"))
			conflicting.SortOrder = 1
			err = db.SavePixivCredentials(ctx, []account.Account{first, conflicting})
		}
		var storage *sqlite.Error
		if err == nil || !errors.As(err, &storage) {
			t.Fatalf("expected typed SQLite failure for %s: %v", input.Name, err)
		}
		input.Message = err.Error()
		input.ExtendedCode = storage.Code()
		stored, err := db.GetPixiv(ctx, 7)
		if err != nil {
			t.Fatal(err)
		}
		input.Token = string(stored.RefreshTokenCopy())
		input.Revision = stored.CredentialRevision
		accounts, err := db.ListPixiv(ctx)
		if err != nil {
			t.Fatal(err)
		}
		input.AccountCount = len(accounts)
		if err := db.Close(); err != nil {
			t.Fatal(err)
		}
		contract.Cases = append(contract.Cases, input)
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "crates", "pixiv-app", "tests", "fixtures", "account_storage_errors.json")
	if *captureAccountStorageErrors {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, expected) {
		t.Fatalf("frozen Go account storage capture differs:\n%s", data)
	}
	t.Logf("frozen Go %s / modernc %s / SQLite %s: %d storage cases", contract.GoVersion, contract.DriverVersion, contract.SQLiteVersion, len(contract.Cases))
}

package database

import (
	"bytes"
	"database/sql"
	"encoding/json"
	"flag"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

var updateDatabaseContract = flag.Bool("migration-update-database", false, "update the database migration contract")
var rustDatabaseContract = flag.Bool("migration-rust-database", false, "run Rust/Go database interoperability")

type migrationContract struct {
	Name  string         `json:"name"`
	Setup string         `json:"setup"`
	Error string         `json:"error"`
	State map[string]any `json:"state"`
}

func TestMigrationDatabaseGoRustFileInteroperability(t *testing.T) {
	if !*rustDatabaseContract {
		t.Skip("requires the Rust workspace toolchain; enable -migration-rust-database")
	}
	golden, err := os.ReadFile(filepath.Join("..", "..", "..", "docs", "migration", "contracts", "database.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []migrationContract
	if err = json.Unmarshal(golden, &cases); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"fresh", "reopen", "legacy-v1"} {
		t.Run(name, func(t *testing.T) {
			dir := t.TempDir()
			var contract migrationContract
			for _, row := range cases {
				if row.Name == name {
					contract = row
				}
			}
			if contract.Setup != "" {
				conn, err := sql.Open("sqlite", dsnWithPragmas(DatabasePath(dir)))
				if err != nil {
					t.Fatal(err)
				}
				if _, err = conn.Exec(contract.Setup); err != nil {
					t.Fatal(err)
				}
				conn.Close()
			}
			command := exec.Command("cargo", "test", "-p", "pixiv-app", "--test", "database", "--locked", "database_opens_shared_go_file", "--", "--ignored", "--exact")
			command.Dir = filepath.Join("..", "..", "..")
			command.Env = append(os.Environ(), "PIXIV_MIGRATION_DATABASE_DIRECTORY="+dir)
			output, err := command.CombinedOutput()
			if err != nil {
				t.Fatalf("Rust open: %v\n%s", err, output)
			}
			if !bytes.Contains(output, []byte("1 passed; 0 failed")) {
				t.Fatalf("Rust interoperability helper did not execute: %s", output)
			}
			db, err := Open(dir)
			if err != nil {
				t.Fatalf("Go reopen after Rust: %v", err)
			}
			defer db.Close()
			got, _ := json.Marshal(migrationState(t, db.DB()))
			want, _ := json.Marshal(contract.State)
			if !bytes.Equal(got, want) {
				t.Fatalf("Go reopened database differs after Rust: %s", got)
			}
		})
	}
}

func TestMigrationDatabasePreservesSchemaAndRejectsDrift(t *testing.T) {
	initial := embeddedMigrations[0].SQL
	old := strings.ReplaceAll(initial, "    schedulable         INTEGER NOT NULL DEFAULT 1,\r\n", "")
	old = strings.ReplaceAll(old, "    schedulable         INTEGER NOT NULL DEFAULT 1,\n", "")
	old = strings.ReplaceAll(old, "    CHECK (pool_last_selected IN (0, 1)),", "    CHECK (pool_last_selected IN (0, 1))")
	old = strings.ReplaceAll(old, "    CHECK (schedulable IN (0, 1))\r\n", "")
	old = strings.ReplaceAll(old, "    CHECK (schedulable IN (0, 1))\n", "")
	old = strings.ReplaceAll(old, "creator_id          TEXT NOT NULL DEFAULT ''", "creator_id          TEXT NULL")
	old += `INSERT INTO schema_migration VALUES(1,'0001_initial','` + legacyInitialMigrationChecksum + `',11);
PRAGMA user_version=1;
INSERT INTO pixiv_account(user_id,sort_order,username,refresh_token,credential_revision,created_at,updated_at) VALUES(7,2,'synthetic',x'010203',4,10,20);
INSERT INTO fanbox_account VALUES(9,3,'synthetic',NULL,x'040506',5,30,10,20);`
	cases := []migrationContract{{Name: "fresh"}, {Name: "legacy-v1", Setup: old}}
	baseDir := t.TempDir()
	base, err := Open(baseDir)
	if err != nil {
		t.Fatal(err)
	}
	base.Close()
	content, err := os.ReadFile(DatabasePath(baseDir))
	if err != nil {
		t.Fatal(err)
	}
	changes := []struct{ name, sql string }{
		{"reopen", ""},
		{"application-unset", "PRAGMA application_id=0"},
		{"application-drift", "PRAGMA application_id=4660"},
		{"future-version", "PRAGMA user_version=4"},
		{"user-version-zero", "PRAGMA user_version=0"},
		{"user-version-one", "PRAGMA user_version=1"},
		{"user-version-two", "PRAGMA user_version=2"},
		{"name-drift", "UPDATE schema_migration SET name='changed' WHERE version=2"},
		{"checksum-drift", "UPDATE schema_migration SET checksum='changed' WHERE version=2"},
		{"first-gap", "DELETE FROM schema_migration WHERE version=1"},
		{"middle-gap", "DELETE FROM schema_migration WHERE version=2"},
		{"future-ledger", "INSERT INTO schema_migration VALUES(4,'future','changed',11)"},
		{"missing-tail", "DELETE FROM schema_migration WHERE version=3"},
		{"empty-ledger", "DELETE FROM schema_migration"},
		{"legacy-checksum", "UPDATE schema_migration SET checksum='" + legacyInitialMigrationChecksum + "' WHERE version=1"},
	}
	for _, ending := range []string{"\n", "\r\n"} {
		setup := ""
		for _, m := range embeddedMigrations {
			source := strings.ReplaceAll(strings.ReplaceAll(m.SQL, "\r\n", "\n"), "\n", ending)
			setup += "UPDATE schema_migration SET checksum='" + checksum([]byte(source)) + "' WHERE name='" + m.Name + "';"
		}
		name := "checksum-lf"
		if ending == "\r\n" {
			name = "checksum-crlf"
		}
		changes = append(changes, struct{ name, sql string }{name, setup})
	}
	for _, change := range changes {
		dir := t.TempDir()
		if err := os.WriteFile(DatabasePath(dir), content, 0600); err != nil {
			t.Fatal(err)
		}
		conn, err := sql.Open("sqlite", dsnWithPragmas(DatabasePath(dir)))
		if err != nil {
			t.Fatal(err)
		}
		if change.sql != "" {
			if _, err = conn.Exec(change.sql); err != nil {
				t.Fatal(err)
			}
		}
		setup := migrationDump(t, conn)
		conn.Close()
		cases = append(cases, migrationContract{Name: change.name, Setup: setup})
	}
	for index := range cases {
		row := &cases[index]
		dir := t.TempDir()
		path := DatabasePath(dir)
		if row.Setup != "" {
			conn, err := sql.Open("sqlite", dsnWithPragmas(path))
			if err != nil {
				t.Fatal(err)
			}
			if _, err = conn.Exec(row.Setup); err != nil {
				t.Fatalf("%s setup: %v", row.Name, err)
			}
			conn.Close()
		}
		db, err := Open(dir)
		if err != nil {
			row.Error = err.Error()
		} else {
			db.Close()
		}
		conn, err := sql.Open("sqlite", dsnWithPragmas(path))
		if err != nil {
			t.Fatal(err)
		}
		row.State = migrationState(t, conn)
		conn.Close()
	}
	encoded, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	encoded = append(encoded, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "database.json")
	if *updateDatabaseContract {
		if err := os.WriteFile(path, encoded, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(encoded, want) {
		t.Fatal("database migration contracts differ")
	}
}

func migrationDump(t *testing.T, db *sql.DB) string {
	t.Helper()
	rows, err := db.Query(`SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type DESC,name`)
	if err != nil {
		t.Fatal(err)
	}
	var statements []string
	for rows.Next() {
		var stmt string
		if err := rows.Scan(&stmt); err != nil {
			t.Fatal(err)
		}
		statements = append(statements, stmt+";")
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	rows.Close()
	rows, err = db.Query(`SELECT version,name,checksum,applied_at FROM schema_migration ORDER BY version`)
	if err != nil {
		t.Fatal(err)
	}
	for rows.Next() {
		var version, at int
		var name, sum string
		if err := rows.Scan(&version, &name, &sum, &at); err != nil {
			t.Fatal(err)
		}
		statements = append(statements, "INSERT INTO schema_migration VALUES("+migrationNumber(version)+",'"+name+"','"+sum+"',11);")
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	rows.Close()
	for _, key := range []string{"application_id", "user_version"} {
		var value int
		if err := db.QueryRow("PRAGMA " + key).Scan(&value); err != nil {
			t.Fatal(err)
		}
		statements = append(statements, "PRAGMA "+key+"="+migrationNumber(value)+";")
	}
	return strings.Join(statements, "\n")
}

func migrationNumber(value int) string { data, _ := json.Marshal(value); return string(data) }

func migrationState(t *testing.T, db *sql.DB) map[string]any {
	t.Helper()
	state := map[string]any{}
	for _, key := range []string{"application_id", "user_version"} {
		var value int
		if err := db.QueryRow("PRAGMA " + key).Scan(&value); err != nil {
			t.Fatal(err)
		}
		state[key] = value
	}
	queries := map[string]string{
		"schema": `SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name`,
		"ledger": `SELECT version,name,checksum FROM schema_migration ORDER BY version`,
		"pixiv":  `SELECT user_id,sort_order,username,hex(refresh_token),credential_revision,schedulable,created_at,updated_at FROM pixiv_account ORDER BY user_id`,
		"fanbox": `SELECT user_id,sort_order,display_name,creator_id,hex(session_id),credential_revision,validated_at,created_at,updated_at FROM fanbox_account ORDER BY user_id`,
	}
	for name, query := range queries {
		rows, err := db.Query(query)
		if err != nil {
			state[name] = nil
			continue
		}
		columns, err := rows.Columns()
		if err != nil {
			t.Fatal(err)
		}
		result := [][]any{}
		for rows.Next() {
			values := make([]any, len(columns))
			pointers := make([]any, len(columns))
			for i := range values {
				pointers[i] = &values[i]
			}
			if err := rows.Scan(pointers...); err != nil {
				t.Fatal(err)
			}
			for i, v := range values {
				if s, ok := v.(string); ok && name == "schema" {
					values[i] = strings.ReplaceAll(s, "\r\n", "\n")
				}
			}
			result = append(result, values)
		}
		if err := rows.Err(); err != nil {
			t.Fatal(err)
		}
		rows.Close()
		state[name] = result
	}
	return state
}

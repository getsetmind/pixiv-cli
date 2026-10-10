//go:build linux && amd64

package sqliteio_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies/sqliteio"
)

var captureMigrationBrowserSQLite = flag.Bool("migration-capture-browser-sqliteio", false, "capture frozen Go sqlite3 process contracts")

type migrationSQLiteCase struct {
	Name      string         `json:"name"`
	Operation string         `json:"operation"`
	Input     map[string]any `json:"input"`
	Output    map[string]any `json:"output"`
}

func TestMigrationBrowserSQLiteProcessHelper(t *testing.T) {
	if os.Getenv("PIXIV_MIGRATION_SQLITE_HELPER") != "1" {
		return
	}
	args := os.Args
	for i, arg := range args {
		if arg == "--" {
			args = args[i+1:]
			break
		}
	}
	body, err := json.Marshal(args)
	if err != nil {
		os.Exit(91)
	}
	if err := os.WriteFile(os.Getenv("PIXIV_MIGRATION_SQLITE_ARGV"), body, 0o600); err != nil {
		os.Exit(92)
	}
	if os.Getenv("PIXIV_MIGRATION_SQLITE_WAIT") == "1" {
		time.Sleep(time.Minute)
		os.Exit(93)
	}
	out, err := hex.DecodeString(os.Getenv("PIXIV_MIGRATION_SQLITE_STDOUT"))
	if err != nil {
		os.Exit(94)
	}
	stderr, err := hex.DecodeString(os.Getenv("PIXIV_MIGRATION_SQLITE_STDERR"))
	if err != nil {
		os.Exit(95)
	}
	_, _ = os.Stdout.Write(out)
	_, _ = os.Stderr.Write(stderr)
	code, _ := strconv.Atoi(os.Getenv("PIXIV_MIGRATION_SQLITE_EXIT"))
	os.Exit(code)
}

func migrationSQLiteHelper(t *testing.T, root string) string {
	t.Helper()
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(root, "process-helper")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	shell := "#!/bin/sh\nexec '" + strings.ReplaceAll(executable, "'", "'\\''") + "' -test.run='^TestMigrationBrowserSQLiteProcessHelper$' -- \"$@\"\n"
	if err := os.WriteFile(filepath.Join(dir, "sqlite3"), []byte(shell), 0o700); err != nil {
		t.Fatal(err)
	}
	return dir
}

func migrationSQLiteError(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}
func migrationSQLiteRows(rows [][]string) any {
	if rows == nil {
		return nil
	}
	encoded := make([][]string, len(rows))
	for i, row := range rows {
		encoded[i] = make([]string, len(row))
		for j, v := range row {
			encoded[i][j] = hex.EncodeToString([]byte(v))
		}
	}
	return encoded
}

func TestMigrationBrowserSQLiteContracts(t *testing.T) {
	root := t.TempDir()
	bin := migrationSQLiteHelper(t, root)
	emptyBin := filepath.Join(root, "empty-path")
	if err := os.Mkdir(emptyBin, 0o700); err != nil {
		t.Fatal(err)
	}
	db := filepath.Join(root, "owned-secret-path.db")
	if err := os.WriteFile(db, []byte("owned fixture marker, not a SQLite database"), 0o600); err != nil {
		t.Fatal(err)
	}
	params := map[string]string{"@h1": ".fanbox.cc", "@h2": "fanbox.cc", "@n": "FANBOXSESSID"}
	type spec struct {
		name, out, stderr          string
		exit                       int
		missing, precancel, cancel bool
		path, sql                  string
		params                     map[string]string
	}
	specs := []spec{
		{name: "readonly-argv-parameter-map", out: ".fanbox.cc,synthetic-session,\n", params: params},
		{name: "empty-output"},
		{name: "blank-lines-skipped", out: "\n\r\nalpha\n\n"},
		{name: "quoted-comma-escaped-quote-and-newline", out: "\"a,b\",\"q\"\"uote\",\"line1\nline2\"\n"},
		{name: "crlf-normalizes-quoted-newlines", out: "\"line1\r\nline2\",tail\r\n"},
		{name: "trailing-empty-columns", out: "alpha,,,\n"},
		{name: "varying-column-counts", out: "one\none,two\none,two,three\n"},
		{name: "empty-quoted-value", out: "\"\"\n"},
		{name: "invalid-utf8-and-nul-preserved", out: string([]byte{0xff, 0x80, 0, ',', 'x', '\n'})},
		{name: "spaces-preserved", out: " space ,\" padded \"\n"},
		{name: "crcrlf-retains-first-cr", out: "alpha,tail\r\r\n"},
		{name: "unterminated-quote", out: "\"never-closed\n"},
		{name: "bare-quote", out: "un\"quoted\n"},
		{name: "post-quote-junk", out: "\"ok\"junk\n"},
		{name: "missing-command", missing: true},
		{name: "permission-stderr-case-insensitive", stderr: "PERMISSION DENIED: synthetic-sensitive-value", exit: 1},
		{name: "not-authorized-before-locked", stderr: "Not Authorized and locked synthetic-sensitive-value", exit: 5},
		{name: "locked-exit-five-without-stderr", exit: 5},
		{name: "locked-substring-case-insensitive", stderr: "database is LOCKED synthetic-sensitive-value", exit: 1},
		{name: "general-exit-redacts-all-output", out: "synthetic-sensitive-value\n", stderr: "private owned path and synthetic-sensitive-value", exit: 3},
		{name: "successful-stderr-does-not-classify", out: "value\n", stderr: "permission denied locked"},
		{name: "pre-cancel-before-process", precancel: true},
		{name: "active-command-cancel", cancel: true},
		{name: "blank-db-before-cancel", path: " \t\n", precancel: true},
		{name: "blank-sql", sql: " \t\n"},
		{name: "invalid-param-before-cancel", params: map[string]string{"@n": "invalid space"}, precancel: true},
		{name: "empty-param-name", params: map[string]string{"": "value"}},
		{name: "empty-param-value", params: map[string]string{"@n": ""}},
		{name: "param-256-byte-boundary", params: map[string]string{"@n": strings.Repeat("x", 256)}},
		{name: "param-257-byte-rejected", params: map[string]string{"@n": strings.Repeat("x", 257)}},
		{name: "param-shell-and-sql-characters-rejected", params: map[string]string{"@n": "value;SELECT"}},
		{name: "param-unicode-rejected", params: map[string]string{"@n": "合成"}},
		{name: "param-extended-ascii-tokens-accepted", params: map[string]string{"$n": "@:$._-012ABCxyz"}},
	}
	cases := make([]migrationSQLiteCase, 0, len(specs))
	for _, s := range specs {
		t.Run(s.name, func(t *testing.T) {
			path, sql := s.path, s.sql
			if path == "" {
				path = db
			}
			if sql == "" {
				sql = "SELECT synthetic_secret FROM owned_table;"
			}
			argvPath := filepath.Join(t.TempDir(), "argv.json")
			t.Setenv("PATH", bin)
			if s.missing {
				t.Setenv("PATH", emptyBin)
			}
			t.Setenv("PIXIV_MIGRATION_SQLITE_HELPER", "1")
			t.Setenv("PIXIV_MIGRATION_SQLITE_ARGV", argvPath)
			t.Setenv("PIXIV_MIGRATION_SQLITE_STDOUT", hex.EncodeToString([]byte(s.out)))
			t.Setenv("PIXIV_MIGRATION_SQLITE_STDERR", hex.EncodeToString([]byte(s.stderr)))
			t.Setenv("PIXIV_MIGRATION_SQLITE_EXIT", strconv.Itoa(s.exit))
			t.Setenv("PIXIV_MIGRATION_SQLITE_WAIT", "")
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			if s.precancel {
				cancel()
			}
			cancelDone := make(chan struct{})
			if s.cancel {
				t.Setenv("PIXIV_MIGRATION_SQLITE_WAIT", "1")
				go func() {
					defer close(cancelDone)
					deadline := time.After(10 * time.Second)
					for {
						if body, err := os.ReadFile(argvPath); err == nil && json.Valid(body) {
							cancel()
							return
						}
						select {
						case <-deadline:
							cancel()
							return
						case <-time.After(time.Millisecond):
						}
					}
				}()
			}
			rows, err := sqliteio.Query(ctx, path, sql, s.params)
			if s.cancel {
				<-cancelDone
			}
			output := map[string]any{"error": migrationSQLiteError(err), "rows_hex": migrationSQLiteRows(rows), "command": nil}
			if body, readErr := os.ReadFile(argvPath); readErr == nil {
				var args []string
				if err := json.Unmarshal(body, &args); err != nil {
					t.Fatal(err)
				}
				if len(args) < 7 {
					t.Fatal("incomplete helper argv")
				}
				commands := map[string]string{}
				for i := 5; i < len(args)-2; i += 2 {
					if args[i] != "-cmd" {
						t.Fatal("parameter flag")
					}
					pieces := strings.SplitN(args[i+1], " ", 4)
					if len(pieces) != 4 {
						t.Fatal("parameter shape")
					}
					commands[pieces[2]] = pieces[3]
				}
				normalizedPath := strings.ReplaceAll(args[len(args)-2], root, "$root")
				output["command"] = map[string]any{"prefix": args[:5], "params": commands, "db_path": normalizedPath, "sql": args[len(args)-1]}
			} else if s.cancel {
				t.Fatal("active cancellation occurred before helper start")
			}
			input := map[string]any{"db_path": strings.ReplaceAll(path, root, "$root"), "sql": sql, "params": s.params, "stdout_hex": hex.EncodeToString([]byte(s.out)), "stderr_hex": hex.EncodeToString([]byte(s.stderr)), "exit": s.exit, "missing_command": s.missing, "pre_cancel": s.precancel, "active_cancel": s.cancel}
			cases = append(cases, migrationSQLiteCase{s.name, "query", input, output})
		})
	}
	cases, shell := migrationSQLiteRealQueries(t, cases)
	sources := map[string]string{}
	for _, name := range []string{"go.mod", "go.sum", "internal/browsercookies/browsercookies.go", "internal/browsercookies/sqliteio/sqliteio.go"} {
		body, err := os.ReadFile(filepath.Join("../../..", name))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(body)
		sources[name] = hex.EncodeToString(sum[:])
	}
	pinnedSources := map[string]string{
		"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/browsercookies/browsercookies.go":    "1b1e373c3a240f54d8b0d255ad5ae5b753b2eae0c8e506df1615e3b9190a3e6b",
		"internal/browsercookies/sqliteio/sqliteio.go": "d922ed106d3a70cd4c0fa79297edb57cbe9adff051816aa3cc8218aaf4b2c902",
	}
	for name, sum := range sources {
		if pinnedSources[name] != sum {
			t.Fatalf("frozen source mismatch: %s", name)
		}
	}
	fixture := map[string]any{"reference": "4b4426487ef18bed276706daec385e0d0a6979f9", "environment": runtime.GOOS + "/" + runtime.GOARCH, "go_version": runtime.Version(), "sources": sources, "scope": "unchanged Go sqliteio.Query; labelled owned process helper for CSV, argv, classifications, cancellation and redaction, plus official SQLite shell queries against genuine owned databases", "cases": cases, "sqlite_shell": shell}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	target := filepath.Join("../../..", "crates/pixiv-cli/tests/fixtures/browser-sqliteio.json")
	if *captureMigrationBrowserSQLite {
		if err := os.WriteFile(target, body, 0o600); err != nil {
			t.Fatal(err)
		}
	} else {
		expected, err := os.ReadFile(target)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(body, expected) {
			t.Fatal("frozen sqliteio fixture changed; inspect source or environment before explicit capture")
		}
	}
	t.Logf("captured/replayed %d distinct process cases; %s", len(cases), fmt.Sprintf("%x", sha256.Sum256(body)))
}

func migrationSQLiteRealQueries(t *testing.T, cases []migrationSQLiteCase) ([]migrationSQLiteCase, map[string]string) {
	t.Helper()
	tool, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Fatal("official sqlite3 shell required for browser contract evidence")
	}
	version, err := exec.Command(tool, "-version").Output()
	if err != nil {
		t.Fatal(err)
	}
	binaryBody, err := os.ReadFile(tool)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(binaryBody)
	metadata := map[string]string{"version": strings.TrimSpace(string(version)), "sha256": hex.EncodeToString(sum[:]), "boundary": "official SQLite shell, genuine owned databases"}
	schema := `CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT);`
	fixedSQL := `SELECT value FROM moz_cookies WHERE (host = @h1 OR host = @h2) AND name = @n;`
	params := map[string]string{"@h1": ".fanbox.cc", "@h2": "fanbox.cc", "@n": "FANBOXSESSID"}
	type spec struct {
		name, state string
		statements  []string
		locked      bool
	}
	specs := []spec{
		{name: "real-shell-exact-host-name-and-path-unfiltered", statements: []string{
			`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID','dotted','/');`,
			`INSERT INTO moz_cookies VALUES ('fanbox.cc','FANBOXSESSID','nodot','/other');`,
			`INSERT INTO moz_cookies VALUES ('..fanbox.cc','FANBOXSESSID','double-dot','/');`,
			`INSERT INTO moz_cookies VALUES ('.other.cc','FANBOXSESSID','other-host','/');`,
			`INSERT INTO moz_cookies VALUES ('.fanbox.cc','fanboxsessid','wrong-name','/');`,
		}},
		{name: "real-shell-csv-quote-newline-unicode", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID',CAST(X'636F6D6D612C2271756F7465220AE59088E68890' AS TEXT),'/');`}},
		{name: "real-shell-invalid-utf8-text", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID',CAST(X'FF80' AS TEXT),'/');`}},
		{name: "real-shell-text-nul-boundary", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID',CAST(X'707265006166746572' AS TEXT),'/');`}},
		{name: "real-shell-empty-value", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID','','/');`}},
		{name: "real-shell-no-matching-rows"},
		{name: "real-shell-invalid-database", state: "invalid"},
		{name: "real-shell-missing-database", state: "missing"},
		{name: "real-shell-exclusive-lock", locked: true},
	}
	for _, s := range specs {
		t.Run(s.name, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, "owned.db")
			if s.state == "invalid" {
				if err := os.WriteFile(path, []byte("owned invalid database"), 0o600); err != nil {
					t.Fatal(err)
				}
			} else if s.state != "missing" {
				db, err := sql.Open("sqlite", "file:"+path)
				if err != nil {
					t.Fatal(err)
				}
				defer db.Close()
				db.SetMaxOpenConns(1)
				for _, statement := range append([]string{schema}, s.statements...) {
					if _, err := db.Exec(statement); err != nil {
						t.Fatal(err)
					}
				}
				if s.locked {
					if _, err := db.Exec(`BEGIN EXCLUSIVE;`); err != nil {
						t.Fatal(err)
					}
					defer db.Exec(`ROLLBACK;`)
				} else if err := db.Close(); err != nil {
					t.Fatal(err)
				}
			}
			rows, err := sqliteio.Query(context.Background(), path, fixedSQL, params)
			input := map[string]any{"schema": schema, "statements": s.statements, "database_state": s.state, "exclusive_lock": s.locked, "db_path": "$root/owned.db", "sql": fixedSQL, "params": params, "boundary": "official_sqlite_shell"}
			cases = append(cases, migrationSQLiteCase{s.name, "query_sqlite", input, map[string]any{"error": migrationSQLiteError(err), "rows_hex": migrationSQLiteRows(rows)}})
		})
	}
	return cases, metadata
}

//go:build linux && amd64

package firefox

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

	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies"
)

var captureMigrationBrowserFirefox = flag.Bool("migration-capture-browser-firefox", false, "capture frozen Go Firefox browser contracts")

type migrationFirefoxCase struct {
	Name      string         `json:"name"`
	Operation string         `json:"operation"`
	Input     map[string]any `json:"input"`
	Output    map[string]any `json:"output"`
}

func migrationFirefoxError(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}
func migrationFirefoxProfiles(profiles []browsercookies.Profile, root string) any {
	if profiles == nil {
		return nil
	}
	result := make([]map[string]string, len(profiles))
	for i, p := range profiles {
		result[i] = map[string]string{"id_hex": hex.EncodeToString([]byte(p.ID)), "name_hex": hex.EncodeToString([]byte(p.Name)), "path_hex": hex.EncodeToString([]byte(strings.ReplaceAll(p.Path, root, "$root")))}
	}
	return result
}
func migrationFirefoxWrite(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
}
func TestMigrationBrowserFirefoxProcessHelper(t *testing.T) {
	if os.Getenv("PIXIV_MIGRATION_FIREFOX_HELPER") != "1" {
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
	if err := os.WriteFile(os.Getenv("PIXIV_MIGRATION_FIREFOX_ARGV"), body, 0o600); err != nil {
		os.Exit(92)
	}

	if metaPath := os.Getenv("PIXIV_MIGRATION_FIREFOX_SNAPSHOT_META"); metaPath != "" {
		path := args[len(args)-2]
		data, readErr := os.ReadFile(path)
		fileInfo, fileErr := os.Stat(path)
		dirInfo, dirErr := os.Stat(filepath.Dir(path))
		if readErr != nil || fileErr != nil || dirErr != nil {
			os.Exit(96)
		}
		sum := sha256.Sum256(data)
		meta := map[string]any{"data_sha256": hex.EncodeToString(sum[:]), "file_mode": fmt.Sprintf("%04o", fileInfo.Mode().Perm()), "dir_mode": fmt.Sprintf("%04o", dirInfo.Mode().Perm())}
		body, err := json.Marshal(meta)
		if err != nil {
			os.Exit(97)
		}
		if err := os.WriteFile(metaPath, body, 0o600); err != nil {
			os.Exit(98)
		}
	}
	stdout, err := hex.DecodeString(os.Getenv("PIXIV_MIGRATION_FIREFOX_STDOUT"))
	if err != nil {
		os.Exit(93)
	}
	_, _ = os.Stdout.Write(stdout)
	stderr, err := hex.DecodeString(os.Getenv("PIXIV_MIGRATION_FIREFOX_STDERR"))
	if err != nil {
		os.Exit(94)
	}
	_, _ = os.Stderr.Write(stderr)
	code, _ := strconv.Atoi(os.Getenv("PIXIV_MIGRATION_FIREFOX_EXIT"))
	os.Exit(code)
}

func TestMigrationBrowserFirefoxContracts(t *testing.T) {
	cases := []migrationFirefoxCase{}
	type node struct {
		Path   string `json:"path"`
		Kind   string `json:"kind"`
		Target string `json:"target,omitempty"`
	}
	type discoverySpec struct {
		name, ini                  string
		nodes                      []node
		missing, directory, cancel bool
	}
	specs := []discoverySpec{
		{name: "missing-ini", missing: true},
		{name: "empty-ini"},
		{name: "non-profile-section-is-accepted", ini: "Path=ignored\n[General]\nName=general\nPath=release\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "order-and-duplicate-ids-preserved", ini: "[Profile9]\nName=first\nPath=one/duplicate\n[Other]\nName=second\nPath=two/duplicate\n[Profile0]\nPath=alpha\n", nodes: []node{{Path: "one/duplicate/cookies.sqlite", Kind: "file"}, {Path: "two/duplicate/cookies.sqlite", Kind: "file"}, {Path: "alpha/cookies.sqlite", Kind: "file"}}},
		{name: "unicode-whitespace-comments-crlf", ini: "\u2003# comment\u2003\r\n ; comment\r\n [ignored name] \r\n Name = 合成 name \r\n IsRelative = 1 \r\n Path = release \r\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "last-recognized-key-wins", ini: "[Profile0]\nName=first\nname=wrong-case-ignored\nName=last\nPath=missing\nPath=release\nignored\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "absolute-declared-path", ini: "[Profile0]\nIsRelative=0\nPath=$root/absolute/release\n", nodes: []node{{Path: "absolute/release/cookies.sqlite", Kind: "file"}}},
		{name: "non-one-relative-is-absolute", ini: "[Profile0]\nIsRelative=true\nPath=$root/absolute/release\n", nodes: []node{{Path: "absolute/release/cookies.sqlite", Kind: "file"}}},
		{name: "relative-default-and-trailing-slash", ini: "[Profile0]\nPath=nested/release/\n", nodes: []node{{Path: "nested/release/cookies.sqlite", Kind: "file"}}},
		{name: "unsafe-empty-missing-profiles-skipped", ini: "[a]\nPath=.hidden\n[b]\nPath=.\n[c]\nPath=..\n[d]\nPath=missing\n[e]\nPath=\n", nodes: []node{{Path: ".hidden/cookies.sqlite", Kind: "file"}}},
		{name: "cookie-directory-is-eligible", ini: "[Profile0]\nPath=release\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "directory"}}},
		{name: "cookie-symlink-followed", ini: "[Profile0]\nPath=release\n", nodes: []node{{Path: "target", Kind: "file"}, {Path: "release/cookies.sqlite", Kind: "symlink", Target: "target"}}},
		{name: "dangling-cookie-symlink-skipped", ini: "[Profile0]\nPath=release\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "symlink", Target: "missing-owned-target"}}},
		{name: "invalid-utf8-name-preserved", ini: "[Profile0]\nName=" + string([]byte{0xff, 0x80}) + "\nPath=release\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "invalid-utf8-profile-id-preserved", ini: "[Profile0]\nPath=" + string([]byte{'r', 0xff}) + "\n", nodes: []node{{Path: string([]byte{'r', 0xff}) + "/cookies.sqlite", Kind: "file"}}},
		{name: "embedded-equals-in-name", ini: "[Profile0]\nName=left=right\nPath=release\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "last-line-without-newline", ini: "[Profile0]\nPath=release", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "long-name-is-not-scanner-limited", ini: "[Profile0]\nName=" + strings.Repeat("n", 65537) + "\nPath=release\n", nodes: []node{{Path: "release/cookies.sqlite", Kind: "file"}}},
		{name: "ini-directory-is-invalid-format", directory: true},
		{name: "pre-cancel-before-missing-ini", missing: true, cancel: true},
	}
	for _, s := range specs {
		t.Run(s.name, func(t *testing.T) {
			root := t.TempDir()
			ini := strings.ReplaceAll(s.ini, "$root", root)
			if s.directory {
				if err := os.Mkdir(filepath.Join(root, profilesIni), 0o700); err != nil {
					t.Fatal(err)
				}
			} else if !s.missing {
				migrationFirefoxWrite(t, filepath.Join(root, profilesIni), []byte(ini))
			}
			encodedNodes := []map[string]string{}
			for _, n := range s.nodes {
				path := filepath.Join(root, n.Path)
				if n.Kind == "directory" {
					if err := os.MkdirAll(path, 0o700); err != nil {
						t.Fatal(err)
					}
				} else if n.Kind == "symlink" {
					if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
						t.Fatal(err)
					}
					if err := os.Symlink(filepath.Join(root, n.Target), path); err != nil {
						t.Fatal(err)
					}
				} else {
					migrationFirefoxWrite(t, path, nil)
				}
				encodedNodes = append(encodedNodes, map[string]string{"path_hex": hex.EncodeToString([]byte(n.Path)), "kind": n.Kind, "target_hex": hex.EncodeToString([]byte(n.Target))})
			}
			ctx, cancel := context.WithCancel(context.Background())
			if s.cancel {
				cancel()
			}
			defer cancel()
			p, err := newProvider(root)
			if err != nil {
				t.Fatal(err)
			}
			profiles, err := p.DiscoverProfiles(ctx)
			input := map[string]any{"ini_hex": hex.EncodeToString([]byte(s.ini)), "nodes": encodedNodes, "ini_missing": s.missing, "ini_directory": s.directory, "pre_cancel": s.cancel}
			output := map[string]any{"profiles": migrationFirefoxProfiles(profiles, root), "error": migrationFirefoxError(err), "name": p.Name(), "close_error": migrationFirefoxError(p.Close())}
			cases = append(cases, migrationFirefoxCase{s.name, "discover", input, output})
		})
	}
	for _, id := range []string{"", ".", "..", ".hidden", "nested/name", "slash\\linux", " profile ", "日本語", "normal", "..suffix", string([]byte{'x', 0})} {
		cases = append(cases, migrationFirefoxCase{"safe-profile-id-" + hex.EncodeToString([]byte(id)), "safe_profile_id", map[string]any{"id_hex": hex.EncodeToString([]byte(id))}, map[string]any{"valid": safeProfileID(id)}})
	}
	for _, xdg := range []bool{false, true} {
		t.Run("default-root", func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("XDG_CONFIG_HOME", "")
			if xdg {
				t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, "owned-xdg"))
			}
			p, err := newProvider("")
			if err != nil {
				t.Fatal(err)
			}
			cases = append(cases, migrationFirefoxCase{map[bool]string{false: "default-home-config-root", true: "xdg-config-root"}[xdg], "default_root", map[string]any{"xdg_set": xdg}, map[string]any{"root": strings.ReplaceAll(p.root, home, "$home")}})
		})
	}
	root := t.TempDir()
	migrationFirefoxWrite(t, filepath.Join(root, profilesIni), []byte("[Profile0]\nName=first\nPath=one/duplicate\n[Profile1]\nName=second\nPath=two/duplicate\n"))
	migrationFirefoxWrite(t, filepath.Join(root, "one/duplicate", cookiesDB), nil)
	migrationFirefoxWrite(t, filepath.Join(root, "two/duplicate", cookiesDB), nil)
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(root, "process-helper")
	if err := os.Mkdir(bin, 0o700); err != nil {
		t.Fatal(err)
	}
	shell := "#!/bin/sh\nexec '" + strings.ReplaceAll(executable, "'", "'\\''") + "' -test.run='^TestMigrationBrowserFirefoxProcessHelper$' -- \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "sqlite3"), []byte(shell), 0o700); err != nil {
		t.Fatal(err)
	}
	type readSpec struct {
		name, out, stderr, id string
		query                 browsercookies.CookieQuery
		exit                  int
		cancel, missing       bool
	}
	reads := []readSpec{
		{name: "read-first-duplicate-profile", out: "synthetic-first\n"},
		{name: "read-zero-rows"},
		{name: "read-multiple-values-order", out: "first\nsecond\n"},
		{name: "read-quoted-comma-newline", out: "\"comma,new\nline\"\n"},
		{name: "read-empty-value", out: "\"\"\n"},
		{name: "read-utf8-and-nul", out: "合成" + string([]byte{0}) + "value\n"},
		{name: "read-invalid-utf8-classified", out: string([]byte{0xff, 0x80, '\n'})},
		{name: "read-prior-good-row-discarded-on-invalid-utf8", out: "good\n" + string([]byte{0xff, '\n'})},
		{name: "read-extra-columns-ignored", out: "first,ignored-column\n"},
		{name: "read-invalid-csv-classified", out: "bad\"quote\n"},
		{name: "read-permission-error-redacted", stderr: "permission denied: synthetic-sensitive-value", exit: 1},
		{name: "read-locked-error", exit: 5},
		{name: "read-missing-command", missing: true},
		{name: "read-pre-cancel", cancel: true},
		{name: "read-invalid-query-before-profile-and-cancel", id: "../private-path", query: browsercookies.CookieQuery{Host: "bad host", Name: "FANBOXSESSID"}, cancel: true},
		{name: "read-invalid-profile-before-cancel", id: "../private-path", cancel: true},
		{name: "read-unknown-profile-redacts-id", id: "unknown-sensitive-id"},
		{name: "read-empty-profile-is-invalid", id: "\x00empty"},
	}
	for _, s := range reads {
		t.Run(s.name, func(t *testing.T) {
			id := s.id
			if id == "" {
				id = "duplicate"
			}
			if id == "\x00empty" {
				id = ""
			}
			query := s.query
			if query.Host == "" && query.Name == "" {
				query = browsercookies.DefaultQuery
			}
			argvPath := filepath.Join(t.TempDir(), "argv.json")
			t.Setenv("PATH", bin)
			if s.missing {
				t.Setenv("PATH", t.TempDir())
			}
			t.Setenv("PIXIV_MIGRATION_FIREFOX_HELPER", "1")
			t.Setenv("PIXIV_MIGRATION_FIREFOX_ARGV", argvPath)
			t.Setenv("PIXIV_MIGRATION_FIREFOX_STDOUT", hex.EncodeToString([]byte(s.out)))
			t.Setenv("PIXIV_MIGRATION_FIREFOX_STDERR", hex.EncodeToString([]byte(s.stderr)))
			t.Setenv("PIXIV_MIGRATION_FIREFOX_EXIT", strconv.Itoa(s.exit))
			ctx, cancel := context.WithCancel(context.Background())
			if s.cancel {
				cancel()
			}
			defer cancel()
			p, _ := newProvider(root)
			secrets, err := p.Read(ctx, query, id)
			var values any
			if secrets != nil {
				v := make([]string, len(secrets))
				for i, secret := range secrets {
					v[i] = hex.EncodeToString([]byte(secret.Value()))
				}
				values = v
			}
			output := map[string]any{"values_hex": values, "error": migrationFirefoxError(err), "command": nil}
			if body, readErr := os.ReadFile(argvPath); readErr == nil {
				var args []string
				if err := json.Unmarshal(body, &args); err != nil {
					t.Fatal(err)
				}
				params := map[string]string{}
				for i := 5; i < len(args)-2; i += 2 {
					pieces := strings.SplitN(args[i+1], " ", 4)
					params[pieces[2]] = pieces[3]
				}
				output["command"] = map[string]any{"prefix": args[:5], "params": params, "db_path": strings.ReplaceAll(args[len(args)-2], root, "$root"), "sql": args[len(args)-1]}
			}
			input := map[string]any{"profile_id_hex": hex.EncodeToString([]byte(id)), "query_host": query.Host, "query_name": query.Name, "stdout_hex": hex.EncodeToString([]byte(s.out)), "stderr_hex": hex.EncodeToString([]byte(s.stderr)), "exit": s.exit, "missing_command": s.missing, "pre_cancel": s.cancel, "layout": "ordered-duplicate-profiles"}
			cases = append(cases, migrationFirefoxCase{s.name, "read", input, output})
		})
	}

	for _, exit := range []int{0, 3} {
		t.Run(fmt.Sprintf("hook-snapshot-cleanup-exit-%d", exit), func(t *testing.T) {
			tempRoot := t.TempDir()
			t.Setenv("TMPDIR", tempRoot)
			t.Setenv("PATH", bin)
			argvPath := filepath.Join(t.TempDir(), "argv.json")
			metaPath := filepath.Join(t.TempDir(), "snapshot-meta.json")
			t.Setenv("PIXIV_MIGRATION_FIREFOX_HELPER", "1")
			t.Setenv("PIXIV_MIGRATION_FIREFOX_ARGV", argvPath)
			t.Setenv("PIXIV_MIGRATION_FIREFOX_STDOUT", hex.EncodeToString([]byte("synthetic-snapshot-value\n")))
			t.Setenv("PIXIV_MIGRATION_FIREFOX_STDERR", "")
			t.Setenv("PIXIV_MIGRATION_FIREFOX_EXIT", strconv.Itoa(exit))
			t.Setenv("PIXIV_MIGRATION_FIREFOX_SNAPSHOT_META", metaPath)
			data := []byte("owned synthetic snapshot bytes, process-boundary evidence only")
			hookCalls := []string{}
			restore := browsercookies.SetProviderFixtureForTest("firefox", func(path string) ([]byte, error) {
				hookCalls = append(hookCalls, strings.ReplaceAll(path, root, "$root"))
				return data, nil
			})
			defer restore()
			p, _ := newProvider(root)
			secrets, err := p.Read(context.Background(), browsercookies.DefaultQuery, "duplicate")
			body, readErr := os.ReadFile(argvPath)
			if readErr != nil {
				t.Fatal(readErr)
			}
			var args []string
			if err := json.Unmarshal(body, &args); err != nil {
				t.Fatal(err)
			}
			snapshotPath := args[len(args)-2]
			metaBody, readErr := os.ReadFile(metaPath)
			if readErr != nil {
				t.Fatal(readErr)
			}
			meta := map[string]any{}
			if err := json.Unmarshal(metaBody, &meta); err != nil {
				t.Fatal(err)
			}
			_, fileErr := os.Stat(snapshotPath)
			_, dirErr := os.Stat(filepath.Dir(snapshotPath))
			entries, readErr := os.ReadDir(tempRoot)
			if readErr != nil {
				t.Fatal(readErr)
			}
			var values any
			if secrets != nil {
				items := make([]string, len(secrets))
				for i, v := range secrets {
					items[i] = hex.EncodeToString([]byte(v.Value()))
				}
				values = items
			}
			output := map[string]any{"values_hex": values, "error": migrationFirefoxError(err), "hook_calls": hookCalls, "snapshot_meta": meta, "snapshot_file_removed": os.IsNotExist(fileErr), "snapshot_dir_removed": os.IsNotExist(dirErr), "owned_temp_root_empty": len(entries) == 0}
			input := map[string]any{"layout": "ordered-duplicate-profiles", "fixture_bytes_hex": hex.EncodeToString(data), "stdout_hex": hex.EncodeToString([]byte("synthetic-snapshot-value\n")), "exit": exit, "profile_id": "duplicate", "boundary": "owned_process_helper"}
			cases = append(cases, migrationFirefoxCase{fmt.Sprintf("hook-snapshot-cleanup-exit-%d", exit), "read_hook_snapshot", input, output})
		})
	}
	cases, shellMetadata := migrationFirefoxRealReads(t, cases)
	sources := map[string]string{}
	for _, name := range []string{"go.mod", "go.sum", "internal/browsercookies/browsercookies.go", "internal/browsercookies/firefox/firefox.go", "internal/browsercookies/firefox/paths_linux.go", "internal/browsercookies/sqliteio/sqliteio.go"} {
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
		"internal/browsercookies/browsercookies.go":      "1b1e373c3a240f54d8b0d255ad5ae5b753b2eae0c8e506df1615e3b9190a3e6b",
		"internal/browsercookies/firefox/firefox.go":     "8ac0f3e3b9ae8998e2617b10eed66b477286b881f56b1db0351404cd205dee5e",
		"internal/browsercookies/firefox/paths_linux.go": "05c9003c1f04eff4abd8043cbc99ae5bd85474d6bb261712a94ae126e9b23fb0",
		"internal/browsercookies/sqliteio/sqliteio.go":   "d922ed106d3a70cd4c0fa79297edb57cbe9adff051816aa3cc8218aaf4b2c902",
	}
	for name, sum := range sources {
		if pinnedSources[name] != sum {
			t.Fatalf("frozen source mismatch: %s", name)
		}
	}
	fixture := map[string]any{"reference": "4b4426487ef18bed276706daec385e0d0a6979f9", "environment": runtime.GOOS + "/" + runtime.GOARCH, "go_version": runtime.Version(), "sources": sources, "scope": "unchanged Go Firefox discovery/parser/profile selection; labelled process-helper reads, private snapshot lifecycle, and official SQLite shell against genuine owned databases; Linux runtime paths only", "cases": cases, "sqlite_shell": shellMetadata}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	target := filepath.Join("../../..", "crates/pixiv-cli/tests/fixtures/browser-firefox.json")
	if *captureMigrationBrowserFirefox {
		if err := os.WriteFile(target, body, 0o600); err != nil {
			t.Fatal(err)
		}
	} else {
		expected, err := os.ReadFile(target)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(body, expected) {
			t.Fatal("frozen Firefox fixture changed; inspect source or environment before explicit capture")
		}
	}
	t.Logf("captured/replayed %d distinct Firefox cases; %x", len(cases), sha256.Sum256(body))
}

func migrationFirefoxRealReads(t *testing.T, cases []migrationFirefoxCase) ([]migrationFirefoxCase, map[string]string) {
	t.Helper()
	tool, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Fatal("official sqlite3 shell required for Firefox contract evidence")
	}
	version, err := exec.Command(tool, "-version").Output()
	if err != nil {
		t.Fatal(err)
	}
	body, err := os.ReadFile(tool)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(body)
	metadata := map[string]string{"version": strings.TrimSpace(string(version)), "sha256": hex.EncodeToString(sum[:]), "boundary": "official SQLite shell, genuine owned Firefox databases"}
	schema := `CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT, path TEXT);`
	type spec struct {
		name, state string
		statements  []string
		locked      bool
	}
	specs := []spec{
		{name: "real-read-exact-host-name-no-path-filter", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID','dotted','/');`, `INSERT INTO moz_cookies VALUES ('fanbox.cc','FANBOXSESSID','nodot','/other');`, `INSERT INTO moz_cookies VALUES ('..fanbox.cc','FANBOXSESSID','double-dot','/');`, `INSERT INTO moz_cookies VALUES ('.other.cc','FANBOXSESSID','other-host','/');`, `INSERT INTO moz_cookies VALUES ('.fanbox.cc','fanboxsessid','wrong-name','/');`}},
		{name: "real-read-csv-quote-newline-unicode", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID',CAST(X'636F6D6D612C2271756F7465220AE59088E68890' AS TEXT),'/');`}},
		{name: "real-read-invalid-utf8-rejected", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID',CAST(X'FF80' AS TEXT),'/');`}},
		{name: "real-read-nul-shell-boundary", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID',CAST(X'707265006166746572' AS TEXT),'/');`}},
		{name: "real-read-empty-value", statements: []string{`INSERT INTO moz_cookies VALUES ('.fanbox.cc','FANBOXSESSID','','/');`}},
		{name: "real-read-invalid-database", state: "invalid"},
		{name: "real-read-exclusive-lock", locked: true},
	}
	for _, s := range specs {
		t.Run(s.name, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, "release", cookiesDB)
			migrationFirefoxWrite(t, filepath.Join(root, profilesIni), []byte("[Profile0]\nPath=release\n"))
			if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
				t.Fatal(err)
			}
			if s.state == "invalid" {
				migrationFirefoxWrite(t, path, []byte("owned invalid database"))
			} else {
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
			p, _ := newProvider(root)
			secrets, err := p.Read(context.Background(), browsercookies.DefaultQuery, "release")
			var values any
			if secrets != nil {
				items := make([]string, len(secrets))
				for i, v := range secrets {
					items[i] = hex.EncodeToString([]byte(v.Value()))
				}
				values = items
			}
			input := map[string]any{"schema": schema, "statements": s.statements, "database_state": s.state, "exclusive_lock": s.locked, "query_host": browsercookies.DefaultQuery.Host, "query_name": browsercookies.DefaultQuery.Name, "profile_id": "release", "layout": "single-release", "boundary": "official_sqlite_shell"}
			cases = append(cases, migrationFirefoxCase{s.name, "read_sqlite", input, map[string]any{"values_hex": values, "error": migrationFirefoxError(err)}})
		})
	}
	return cases, metadata
}

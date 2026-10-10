package fanbox_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var migrationCaptureFanboxSolverAliases = flag.Bool("migration-capture-fanbox-solver-public-aliases", false, "capture frozen Go public solver Unicode field aliases")

func TestMigrationFanboxSolverPublicAliasesFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	protected := map[string]string{
		"sdk/fanbox/migration_solver_public_test.go":                          "a21a5a150c8a644d7874f5e94cbcf78716742279048a41f680d1ede0a5d6fa34",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-public.json":           "a908d47882462e698b67d80f2f48cfab578964e675029afe973a8a024711ba80",
		"internal/services/fanbox/protocol/migration_solver_test.go":          "c9a27c01aafbf271dc7a6b33cab8959a7b8e721b9b3d0edc6d41941d7aae1f56",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver.json":                  "45ed3d463ce1159290dbb6686490e754e30d4540a108754571a926b8bba6f382",
		"internal/services/fanbox/protocol/migration_solver_redirect_test.go": "40a96c8b674c9f5f9185f9b9be49616b6ccd108c9375c76531d54de7f4896da2",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json":         "6a8db01180325dc9074984b4b763250d0029118889ed7e5141a8ae1554709651",
	}
	var original struct {
		Cases []migrationPublicSolverRow `json:"cases"`
	}
	for path, want := range protected {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("published source contract changed: %s", path)
		}
		if strings.HasSuffix(path, "fanbox-solver-public.json") {
			if err := json.Unmarshal(data, &original); err != nil {
				t.Fatal(err)
			}
		}
	}
	if len(original.Cases) != 93 {
		t.Fatal("public93 input inventory changed")
	}
	inputs := map[string]migrationPublicSolverInput{}
	for _, row := range original.Cases {
		if row.Name == "cache/native_future" || row.Name == "expiry/one" {
			inputs[row.Name] = row.Input
		}
	}
	rows := []migrationPublicSolverRow{}
	sources := map[string]string{}
	for _, alias := range []struct {
		name, source, field, key string
		repeated                 bool
	}{
		{"status_long_s", "cache/native_future", "status", "ſtatus", false},
		{"solution_long_s", "cache/native_future", "solution", "ſolution", false},
		{"user_agent_long_s", "cache/native_future", "userAgent", "uſerAgent", false},
		{"cookies_kelvin", "cache/native_future", "cookies", "cooKies", false},
		{"cookies_long_s", "cache/native_future", "cookies", "cookieſ", false},
		{"expires_long_s_past_cache", "expiry/one", "expiry", "expireſ", false},
		{"status_alias_last_wins", "cache/native_future", "status", "ſtatus", true},
	} {
		input, ok := inputs[alias.source]
		if !ok || len(input.Control) != 1 {
			t.Fatal("reused public source input missing")
		}
		input.Native = append([]migrationFanboxStep{}, input.Native...)
		input.Control = append([]migrationFanboxStep{}, input.Control...)
		document := input.Control[0].Body
		field := `"` + alias.field + `":`
		if strings.Count(document, field) != 1 {
			t.Fatal("bounded source field is ambiguous")
		}
		replacement := `"` + alias.key + `":`
		if alias.repeated {
			replacement = `"status":"error","` + alias.key + `":`
		}
		input.Control[0].Body = strings.Replace(document, field, replacement, 1)
		name := "unicode_alias/" + alias.name
		var observation map[string]any
		t.Run(name, func(t *testing.T) { observation = migrationPublicSolverObserve(t, input) })
		rows = append(rows, migrationPublicSolverRow{Name: name, Input: input, Observation: observation})
		sources[name] = alias.source
	}
	if t.Failed() {
		return
	}
	contract := map[string]any{
		"source_commit": reference.SourceCommit, "go_version": reference.GoVersion, "source_sha256": reference.SourceSHA256,
		"protected_published_files_sha256": protected, "source_public_fixture_case_count": 93, "reused_public_case_by_supplement_case": sources,
		"public_operation": "fanbox.Client.CurrentUser", "cases": rows,
		"evidence":           "Actual unchanged frozen public OpenWith/CurrentUser and ordinary owned loopback control through the existing public93 harness. An ASCII-schema JSON member is changed to its Go Unicode SimpleFold alias, or one earlier status field is added to establish repeated alias ordering. The expires alias places the existing expiry/one raw value under the interchangeable expires member, with no competing expiry member. Native body bytes, DTO/error/source, native requests/cache/expiry and diagnostics are actual public observations. Long-s variants cover status, solution, userAgent, cookies and expires; Kelvin-sign covers cookies. The past-expiry alias reuses expiry/one to make recognition observable through clearance removal, rather than checking an unexposed private state.",
		"go_only_boundaries": []string{"Original public93/core124/redirect3 inputs, observations and source bytes remain unchanged", "The same source-only protocol sentinel identities, error types, reader call topology and control default User-Agent/HTTP version projections from public93 are retained", "Unicode field matching is encoding/json SimpleFold behavior, independent of Unicode normalization; cookie name values remain case-sensitive"},
		"limitations":        []string{"This is a bounded seven-row actual public field-alias supplement, not a new Cartesian matrix or complete JSON parser parity", "Only anonymous owned HTTP/1 loopback control and injected native/API responses execute; no native HTTP/2, external authentication, media, account, browser, host trust, HEAD or upload work", "Unpaired-surrogate string replacement, >10000-level JSON depth rejection, RFC3339 leap-second rejection and HTTP-date weekday handling remain source-confirmed follow-on candidates, with no runtime claim in this supplement"},
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-public-aliases.json")
	if *migrationCaptureFanboxSolverAliases {
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public solver alias observations differ from frozen Go")
	}
	t.Logf("public solver aliases: %d rows, bytes=%d sha256=%x", len(rows), len(data), sha256.Sum256(data))
}

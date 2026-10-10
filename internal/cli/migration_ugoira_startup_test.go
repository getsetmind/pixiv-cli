package cli

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/update"
)

var updateUgoiraStartup = flag.Bool("migration-update-ugoira-startup", false, "capture isolated ugoira root startup contracts")

const migrationUgoiraStartupReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

var migrationUgoiraStartupSources = map[string]string{
	"internal/cli/root.go":                            "afbb8c2d7a90ccbe3ce124e9e2ee68ec463b9d020ba161d0d1cb270d1e1e7b1e",
	"internal/cli/execution.go":                       "627d8d8ccd6e7c35508abe0daab3209d3daf34dbf1927ab6f9f70145b623c864",
	"internal/cli/composition.go":                     "61606c01971156607d6e632cbf417a63c8e7f02b4c673172ee3db9392dc0f9fc",
	"internal/cli/commands/lifecycle.go":              "d81bfb1a664832ec707dba371a102da56baa05f3aa079905dad48de309e38080",
	"internal/cli/commands/pixiv/ugoira/ugoira.go":    "c4439b144e472b56d39421e465c4ffdc5d3e4b8cfc976a1cc2e5ae54b69e1320",
	"internal/cli/commands/update/automatic_check.go": "20db84c6bedf70d53a39103f5c69607399da838dbfcf397dc5b57d5798bb350f",
	"internal/cli/diagnostics/diagnostics.go":         "88b5c5f4a4ca3b13bef00bdebf1d9d787e6e32f1fe90f3d6abb3a30d2d9516d9",
	"internal/config/settings/config.go":              "591900e2f5648642a6296eb1306cbf0b2fa25d8f5c06f4d61d6625d5340d73b3",
	"internal/config/settings/defaults.go":            "3dbfbf1f18510bd296c30bba74468732150ee0249f6f3744289cf22f870b0753",
	"internal/config/settings/store.go":               "c5b418cd50e17dce27d4e0f5f499e4d293e0527e07d335e80da9a5fe2711feec",
	"internal/config/settings/schema.go":              "ff41a9f3cda2c29ef1474784440e82600ba56f65883e21d35fcb31483b04ab7e",
	"internal/config/settings/snapshot.go":            "137a245260ccf8be8e26210fa4c91edecf342a8a6d69d72c77dd11fec4843609",
	"internal/config/settings/values.go":              "35738fb8fdfdbc1d2cf27e541547325d3bae633b12fc2a28b58b1785177f1788",
	"internal/config/settings/paths.go":               "f81e702c53e232fa5cbf7f910aec0a60f343547d1f5bcdbdcc21b3efdd3c6b31",
}

type migrationUgoiraStartupRow struct {
	Name         string            `json:"name"`
	Comparison   string            `json:"comparison"`
	Args         []string          `json:"args"`
	Input        string            `json:"input"`
	Before       *string           `json:"before"`
	Env          map[string]string `json:"env"`
	Supported    bool              `json:"supported"`
	CleanupError string            `json:"cleanup_error"`
	EnsureError  string            `json:"ensure_error"`
	Exit         int               `json:"exit"`
	Stdout       string            `json:"stdout"`
	Stderr       string            `json:"stderr"`
	Calls        []string          `json:"calls"`
	StdinReads   int               `json:"stdin_reads"`
	NetworkCalls int32             `json:"network_calls"`
	Config       bool              `json:"config"`
	Database     bool              `json:"database"`
	After        string            `json:"after"`
}

type migrationUgoiraStartupFixture struct {
	Reference string                      `json:"reference"`
	Sources   map[string]string           `json:"sources"`
	Gaps      []string                    `json:"gaps"`
	Rows      []migrationUgoiraStartupRow `json:"rows"`
}

func TestMigrationUgoiraStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_UGOIRA_STARTUP_CHILD")
	if encoded == "" {
		t.Skip("isolated ugoira startup helper")
	}
	var row migrationUgoiraStartupRow
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	row.Calls = []string{}
	cleanupPendingWindowsUpdate = func() error {
		row.Calls = append(row.Calls, "cleanup")
		if row.CleanupError != "" {
			return errors.New(row.CleanupError)
		}
		return nil
	}
	automaticPersistentHandlerSupported = func() bool {
		row.Calls = append(row.Calls, "supported")
		return row.Supported
	}
	ensureURLSchemeRelay = func(context.Context) error {
		row.Calls = append(row.Calls, "ensure")
		if row.EnsureError != "" {
			return errors.New(row.EnsureError)
		}
		return nil
	}
	newCLIAutomaticUpdateChecker = func(string) (*update.AutomaticUpdateChecker, error) {
		row.Calls = append(row.Calls, "automatic-update")
		return nil, errors.New("synthetic automatic update must not execute")
	}
	oldTransport := http.DefaultTransport
	transport := oldTransport.(*http.Transport).Clone()
	var networkCalls atomic.Int32
	denyDial := func(context.Context, string, string) (net.Conn, error) {
		networkCalls.Add(1)
		return nil, errors.New("synthetic network must not execute")
	}
	transport.DialContext, transport.DialTLSContext = denyDial, denyDial
	http.DefaultTransport = transport
	t.Cleanup(func() { http.DefaultTransport = oldTransport; transport.CloseIdleConnections() })
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	if row.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(*row.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	var out, diagnostics bytes.Buffer
	row.Exit = Run(append([]string{"pixiv"}, row.Args...), migrationHiddenReader{input: strings.NewReader(row.Input), reads: &row.StdinReads, fail: true}, &out, &diagnostics)
	row.NetworkCalls = networkCalls.Load()
	if row.NetworkCalls != 0 || row.StdinReads != 0 {
		t.Fatalf("ugoira startup touched network=%d or stdin=%d", row.NetworkCalls, row.StdinReads)
	}
	for _, call := range row.Calls {
		if call == "automatic-update" {
			t.Fatal("failed or help command performed automatic update")
		}
	}
	row.Stdout = strings.ReplaceAll(out.String(), home, "<HOME>")
	row.Stderr = migrationUgoiraStartupDiagnostics(t, strings.ReplaceAll(diagnostics.String(), home, "<HOME>"))
	exists := func(name string) bool {
		_, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return err == nil
	}
	row.Config, row.Database = exists("config.toml"), exists("pixiv-cli.db")
	if row.Config {
		body, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		row.After = string(body)
	}
	body, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), body, 0600); err != nil {
		t.Fatal(err)
	}
}

func migrationUgoiraStartupDiagnostics(t *testing.T, text string) string {
	t.Helper()
	lines := strings.Split(text, "\n")
	for index, line := range lines {
		var record map[string]json.RawMessage
		if json.Unmarshal([]byte(line), &record) != nil || record["time"] == nil {
			continue
		}
		var timestamp string
		if err := json.Unmarshal(record["time"], &timestamp); err != nil {
			t.Fatal(err)
		}
		if _, err := time.Parse(time.RFC3339Nano, timestamp); err != nil {
			t.Fatal(err)
		}
		// Root diagnostics use the wall clock; only their timestamp is non-contractual.
		lines[index] = strings.Replace(line, string(record["time"]), `"<TIME>"`, 1)
	}
	return strings.Join(lines, "\n")
}

func TestMigrationUgoiraStartup(t *testing.T) {
	for path, want := range migrationUgoiraStartupSources {
		body, err := os.ReadFile(filepath.Join("../..", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != want {
			t.Fatalf("%s differs from frozen Go reference", path)
		}
	}
	rows := []migrationUgoiraStartupRow{}
	add := func(name string, args ...string) *migrationUgoiraStartupRow {
		rows = append(rows, migrationUgoiraStartupRow{Name: name, Comparison: "exact", Args: args, Env: map[string]string{}})
		return &rows[len(rows)-1]
	}
	text := func(value string) *string { return &value }
	add("missing", "ugoira")
	add("missing-piped-artwork", "ugoira").Input = "42\n"
	add("missing-piped-record", "ugoira", "--json").Input = "{\"artwork_id\":42}\n"
	add("extra", "ugoira", "42", "43")
	add("unknown-long", "ugoira", "42", "--unknown")
	add("unknown-short", "ugoira", "42", "-x")
	add("ndjson-rejected", "ugoira", "42", "--ndjson")
	add("missing-before-malformed", "ugoira").Before = text("[unfinished\n")
	add("extra-before-malformed", "ugoira", "42", "43").Before = text("[unfinished\n")
	add("unknown-before-malformed", "ugoira", "42", "--unknown").Before = text("[unfinished\n")
	add("help", "ugoira", "--help").Comparison = "startup-state"
	add("help-before-malformed", "ugoira", "--help").Before = text("[unfinished\n")
	rows[len(rows)-1].Comparison = "startup-state"
	add("no-account", "ugoira", "42")
	add("no-account-json", "ugoira", "42", "--json")
	add("no-account-json-false-presence", "ugoira", "42", "--json=false")
	add("no-account-short-json", "ugoira", "42", "-j")
	add("source-before-account", "ugoira", "0")
	add("source-before-json-envelope", "ugoira", "0", "--json=false")
	add("source-before-invalid-proxy", "ugoira", "0", "--proxy=invalid")
	add("proxy-conflict-before-source", "ugoira", "0", "--proxy=", "--no-proxy=false", "--json=false")
	add("proxy-conflict", "ugoira", "42", "--proxy=", "--no-proxy")
	add("proxy-conflict-false-presence", "ugoira", "42", "--proxy=", "--no-proxy=false")
	add("invalid-proxy", "ugoira", "42", "--proxy=invalid", "--json")
	add("empty-proxy", "ugoira", "42", "--proxy=")
	add("no-proxy", "ugoira", "42", "--no-proxy")
	add("no-proxy-false", "ugoira", "42", "--no-proxy=false")
	add("custom-config-preserved", "ugoira", "42").Before = text("# mine\r\n[unknown]\r\nkeep = true\r\n")
	add("malformed-config", "ugoira", "42", "--json").Before = text("[unfinished\n")
	add("config-before-source", "ugoira", "0", "--json").Before = text("[unfinished\n")
	add("config-before-proxy-conflict", "ugoira", "42", "--proxy=", "--no-proxy=false", "--json=false").Before = text("[unfinished\n")
	add("configured-json-plain-stderr", "ugoira", "42").Before = text("[output]\njson = true\n")
	add("configured-json-explicit-false-envelope", "ugoira", "42", "--json=false").Before = text("[output]\njson = true\n")
	add("configured-proxy", "ugoira", "42").Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	add("configured-proxy-clear", "ugoira", "42", "--no-proxy").Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	add("configured-proxy-false", "ugoira", "42", "--no-proxy=false").Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	add("configured-proxy-empty", "ugoira", "42", "--proxy=").Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	add("global-proxy", "ugoira", "42").Before = text("[network]\nhttps_proxy = 'invalid'\n")
	add("global-proxy-clear", "ugoira", "42", "--no-proxy").Before = text("[network]\nhttps_proxy = 'invalid'\n")
	add("env-proxy", "ugoira", "42").Env["HTTPS_PROXY"] = "invalid"
	add("env-proxy-clear", "ugoira", "42", "--no-proxy").Env["HTTPS_PROXY"] = "invalid"
	lower := add("env-proxy-lower-empty-wins", "ugoira", "42")
	lower.Env["HTTPS_PROXY"], lower.Env["https_proxy"] = "invalid", ""
	add("configured-negative-interval", "ugoira", "42").Before = text("[network]\nrequest_interval = '-1s'\n")
	add("env-negative-interval", "ugoira", "0", "--json").Env["PIXIV_REQUEST_INTERVAL"] = "-1s"
	add("env-invalid-interval", "ugoira", "42", "--json=false").Env["PIXIV_REQUEST_INTERVAL"] = "later"
	validEnv := add("env-interval-overrides-config", "ugoira", "42")
	validEnv.Before, validEnv.Env["PIXIV_REQUEST_INTERVAL"] = text("[network]\nrequest_interval = '-1s'\n"), "0s"
	add("undeclared-json-env-does-not-select-machine", "ugoira", "42").Env["PIXIV_OUTPUT_JSON"] = "true"
	add("explicit-source-ignores-piped-input", "ugoira", "42").Input = "bad source\n"
	add("debug-json-root-identity", "ugoira", "42", "--json").Before = text("[logging]\nlevel = 'debug'\nformat = 'json'\n")
	rows[len(rows)-1].Comparison = "go-only"
	debugEnv := add("env-debug-overrides-config", "ugoira", "42")
	debugEnv.Before, debugEnv.Comparison = text("[logging]\nlevel = 'info'\nformat = 'json'\n"), "go-only"
	debugEnv.Env["PIXIV_LOG_LEVEL"] = "debug"
	add("cleanup-failure", "ugoira", "42", "--json").CleanupError = "synthetic cleanup failure"
	rows[len(rows)-1].Comparison = "go-only"
	warning := add("startup-handler-warning", "ugoira", "42", "--json=false")
	warning.Supported, warning.EnsureError, warning.Comparison = true, "private synthetic handler failure /private/path", "go-only"
	add("startup-handler-success", "ugoira", "42").Supported = true
	rows[len(rows)-1].Comparison = "go-only"
	actual := make([]migrationUgoiraStartupRow, len(rows))
	for index, row := range rows {
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			temp := filepath.Join(home, "tmp")
			if err := os.Mkdir(temp, 0700); err != nil {
				t.Fatal(err)
			}
			body, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationUgoiraStartupChild$")
			child.Dir = home
			child.Env = []string{"HOME=" + home, "USERPROFILE=" + home, "TMPDIR=" + temp, "PATH=" + home, "MIGRATION_UGOIRA_STARTUP_CHILD=" + string(body)}
			for key, value := range row.Env {
				child.Env = append(child.Env, key+"="+value)
			}
			if output, err := child.CombinedOutput(); err != nil {
				t.Fatalf("owned startup child: %v\n%s", err, output)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &actual[index]); err != nil {
				t.Fatal(err)
			}
		})
	}
	if t.Failed() {
		return
	}
	fixture := migrationUgoiraStartupFixture{
		Reference: migrationUgoiraStartupReference, Sources: migrationUgoiraStartupSources, Rows: actual,
		Gaps: []string{
			"Cobra help bytes are frozen here; Rust subprocess help comparisons cover startup state, not Clap/Cobra byte parity.",
			"Synthetic native startup hooks and clock-normalized debug diagnostic identity are Go-only in this command fixture; Rust root integration remains separate evidence.",
			"No-account subprocesses do not establish successful HTTPS, live accounts, native registration, browser launch, update checks, or other platform execution.",
		},
	}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	const path = "../../crates/pixiv-cli/tests/fixtures/cli-ugoira-startup.json"
	if *updateUgoiraStartup {
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	expected, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var want migrationUgoiraStartupFixture
	if err := json.Unmarshal(expected, &want); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(want, fixture) {
		t.Fatalf("ugoira startup contract changed\ngot: %s\nwant: %s", body, expected)
	}
}

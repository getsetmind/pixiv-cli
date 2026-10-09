package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
)

var updateDownloadStartup = flag.Bool("migration-update-download-startup", false, "capture isolated download root startup contracts")

type migrationDownloadStartupRow struct {
	Name         string   `json:"name"`
	Args         []string `json:"args"`
	Input        string   `json:"input"`
	Before       *string  `json:"before"`
	ReadError    bool     `json:"read_error"`
	Supported    bool     `json:"supported"`
	CleanupError string   `json:"cleanup_error"`
	EnsureError  string   `json:"ensure_error"`
	Exit         int      `json:"exit"`
	Stdout       string   `json:"stdout"`
	Stderr       string   `json:"stderr"`
	Calls        []string `json:"calls"`
	StdinReads   int      `json:"stdin_reads"`
	NetworkCalls int32    `json:"network_calls"`
	Config       bool     `json:"config"`
	Database     bool     `json:"database"`
	After        string   `json:"after"`
}

func TestMigrationDownloadStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_DOWNLOAD_STARTUP_CHILD")
	if encoded == "" {
		t.Skip("isolated download startup helper")
	}
	var row migrationDownloadStartupRow
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	home := os.Getenv("HOME")
	row.Calls = []string{}
	oldCleanup, oldSupported, oldEnsure := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, ensureURLSchemeRelay
	t.Cleanup(func() {
		cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported, ensureURLSchemeRelay = oldCleanup, oldSupported, oldEnsure
	})
	cleanupPendingWindowsUpdate = func() error {
		row.Calls = append(row.Calls, "cleanup")
		if row.CleanupError != "" {
			return errors.New(row.CleanupError)
		}
		return nil
	}
	automaticPersistentHandlerSupported = func() bool { row.Calls = append(row.Calls, "supported"); return row.Supported }
	ensureURLSchemeRelay = func(context.Context) error {
		row.Calls = append(row.Calls, "ensure")
		if row.EnsureError != "" {
			return errors.New(row.EnsureError)
		}
		return nil
	}
	// SDK constructors clone the concrete default transport, so retain that boundary.
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
	row.StdinReads = 0
	row.Exit = Run(append([]string{"pixiv"}, row.Args...), migrationHiddenReader{input: strings.NewReader(row.Input), reads: &row.StdinReads, fail: row.ReadError}, &out, &diagnostics)
	row.NetworkCalls = networkCalls.Load()
	if row.NetworkCalls != 0 {
		t.Fatalf("startup attempted %d network connections", row.NetworkCalls)
	}
	row.Stdout = strings.ReplaceAll(out.String(), home, "<HOME>")
	row.Stderr = strings.ReplaceAll(diagnostics.String(), home, "<HOME>")
	if row.Name == "root-help-download-leaf" {
		lines := []string{}
		for _, line := range strings.Split(row.Stdout, "\n") {
			if strings.HasPrefix(line, "  download ") {
				lines = append(lines, line)
			}
		}
		if len(lines) != 1 {
			t.Fatalf("root help must expose exactly one download leaf: %q", row.Stdout)
		}
		row.Stdout = lines[0] + "\n"
	}
	exists := func(name string) bool {
		_, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return err == nil
	}
	row.Config, row.Database = exists("config.toml"), exists("pixiv-cli.db")
	row.After = ""
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

func TestMigrationDownloadStartup(t *testing.T) {
	const source = "https://i.pximg.net/direct.png?signature=synthetic"
	rows := []migrationDownloadStartupRow{}
	add := func(name string, args ...string) *migrationDownloadStartupRow {
		rows = append(rows, migrationDownloadStartupRow{Name: name, Args: args})
		return &rows[len(rows)-1]
	}
	add("root-help-download-leaf", "--help").ReadError = true
	add("download-help", "download", "--help").ReadError = true
	add("unknown-flag", "download", source, "--unknown").ReadError = true
	add("empty-piped-input", "download")
	add("stdin-read-error", "download").ReadError = true
	for _, item := range []struct{ name, flag string }{
		{"invalid-pages", "--pages=0"}, {"invalid-quality", "--quality=bad"}, {"invalid-mode", "--ugoira-mode=bad"}, {"invalid-on-error", "--on-error=bad"},
	} {
		add(item.name, "download", source, item.flag).ReadError = true
	}
	add("proxy-flags-presence-conflict", "download", source, "--proxy=http://fixture.invalid:1", "--no-proxy=false").ReadError = true
	add("output-path-conflict", "download", source, "--output=one", "--download-path=two").ReadError = true
	add("output-format-presence-conflict", "download", source, "--json=false", "--ndjson=false").ReadError = true
	add("no-account-direct", "download", source).ReadError = true
	add("no-account-json", "download", source, "--json").ReadError = true
	add("no-account-json-presence-false", "download", source, "--json=false").ReadError = true
	add("no-account-ndjson-false", "download", source, "--ndjson=false").ReadError = true
	add("no-account-no-proxy-false", "download", source, "--no-proxy=false").ReadError = true
	add("stdin-direct-no-account", "download", "--json").Input = source + "\n"
	text := func(value string) *string { return &value }
	add("runtime-malformed", "download", source, "--json").Before = text("[unfinished\n")
	add("runtime-malformed-before-quality", "download", source, "--quality=bad", "--json").Before = text("[unfinished\n")
	add("runtime-negative-interval", "download", source, "--json").Before = text("[network]\nrequest_interval='-1s'\n")
	add("runtime-malformed-interval", "download", source, "--json").Before = text("[network]\nrequest_interval='later'\n")
	add("runtime-json-explicit-false", "download", source, "--json=false").Before = text("[output]\njson=true\n")
	add("runtime-json-ndjson-false", "download", source, "--ndjson=false").Before = text("[output]\njson=true\n")
	add("cleanup-failure", "download", source, "--json").CleanupError = "synthetic cleanup failure"
	warning := add("startup-warning-ignored", "download", source, "--json")
	warning.Supported, warning.EnsureError = true, "private synthetic handler failure /private/path"
	add("supported-ensure", "download", source).Supported = true
	actual := make([]migrationDownloadStartupRow, len(rows))
	for index, row := range rows {
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			data, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			child := exec.Command(os.Args[0], "-test.run=^TestMigrationDownloadStartupChild$")
			child.Dir = home
			for _, entry := range os.Environ() {
				key, _, _ := strings.Cut(entry, "=")
				switch key {
				case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY", "PIXIV_ACCESS_TOKEN", "REQUEST_INTERVAL", "MIGRATION_DOWNLOAD_STARTUP_CHILD":
					continue
				}
				child.Env = append(child.Env, entry)
			}
			child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_DOWNLOAD_STARTUP_CHILD="+string(data))
			if out, err := child.CombinedOutput(); err != nil {
				t.Fatalf("child: %v\n%s", err, out)
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
	const path = "../../crates/pixiv-cli/tests/fixtures/download_startup.json"
	if *updateDownloadStartup {
		body, err := json.MarshalIndent(actual, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(body, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected []migrationDownloadStartupRow
	if err := json.Unmarshal(body, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(expected, actual) {
		got, _ := json.MarshalIndent(actual, "", "  ")
		t.Fatalf("download startup contract changed\ngot: %s\nwant: %s", got, body)
	}
}

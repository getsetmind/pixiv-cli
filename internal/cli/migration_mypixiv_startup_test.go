package cli

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

var updateMyPixivStartup = flag.Bool("migration-update-mypixiv-startup", false, "capture mypixiv actual Run startup contracts")

type mypixivStartupCase struct {
	userWorksStartupCase
	Bytes     int `json:"bytes"`
	ReadCalls int `json:"read_calls"`
}

func mypixivIsolatedStartup(t *testing.T, current mypixivStartupCase) mypixivStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationMyPixivStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_TIMELINE_CHILD="+string(encoded))
	output, err := child.CombinedOutput()
	if err != nil {
		t.Fatalf("isolated mypixiv startup: %v: %s", err, output)
	}
	payload, err := os.ReadFile(filepath.Join(home, "startup.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(payload, &current); err != nil {
		t.Fatal(err)
	}
	return current
}

func TestMigrationMyPixivStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_TIMELINE_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current mypixivStartupCase
	if err := json.Unmarshal([]byte(encoded), &current); err != nil {
		t.Fatal(err)
	}
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	directory := filepath.Join(os.Getenv("HOME"), ".pixiv-cli")
	if current.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(directory, "config.toml"), []byte(*current.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	var out, diagnostics bytes.Buffer
	reader := &migrationTimelineReader{input: strings.NewReader(current.Input)}
	if current.ReadError {
		reader.input = migrationSearchFailedRead{}
	}
	current.Exit = Run(append([]string{"pixiv", "mypixiv"}, current.Args...), reader, &out, &diagnostics)
	current.Stdout, current.Stderr, current.Bytes, current.ReadCalls = out.String(), diagnostics.String(), reader.bytes, reader.calls
	if current.Args[0] == "users" && reader.calls != 0 {
		t.Fatal("mypixiv startup consumed stdin")
	}
	exists := func(name string) bool {
		_, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return err == nil
	}
	current.Config, current.Database = exists("config.toml"), exists("pixiv-cli.db")
	if current.Config {
		after, err := os.ReadFile(filepath.Join(directory, "config.toml"))
		if err != nil {
			t.Fatal(err)
		}
		current.After = string(after)
	}
	data, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(os.Getenv("HOME"), "startup.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
}

func TestMigrationMyPixivStartupPreservesRootDiagnosticsConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	rows := []mypixivStartupCase{}
	add := func(name string, args ...string) {
		rows = append(rows, mypixivStartupCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}})
	}

	add("works-missing-type", "works")
	add("works-missing-type-json", "works", "--json")
	add("works-artwork-auth", "works", "--type=artwork")
	add("works-novel-auth-json", "works", "--type=novel", "--json")
	add("users-auth-ndjson", "users", "--ndjson")
	add("users-auth-auto", "users")
	add("invalid-type-priority", "works", "bad", "--type=invalid", "--page=0")
	add("users-positional", "users", "42")
	add("stdin-id", "works", "--type=manga")
	rows[len(rows)-1].Input = "123\n"
	add("stdin-invalid-before-config", "works", "--type=artwork")
	rows[len(rows)-1].Input = "bad\n"
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("users-stdin-ignored", "users")
	rows[len(rows)-1].Input = "123\n"
	add("users-failed-reader-ignored", "users")
	rows[len(rows)-1].ReadError = true
	add("works-failed-reader", "works", "--type=novel")
	rows[len(rows)-1].ReadError = true
	add("invalid-page", "works", "--type=artwork", "--page=0", "--json")
	add("type-output-conflict", "works", "--type=invalid", "--ndjson", "--json")
	add("proxy-output-conflict", "works", "--type=artwork", "--proxy=", "--no-proxy", "--ndjson", "--json")
	add("unknown-flag-json", "users", "--unknown", "--json")
	add("malformed-limit", "works", "--type=artwork", "--limit=bad")
	add("malformed-bool-json", "users", "--json=bad")
	add("malformed-config-before-type", "works", "--type=invalid")
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("malformed-config-before-page", "users", "--page=0")
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("unknown-config-preserved", "users")
	rows[len(rows)-1].Before = text("# mine\r\n[unknown]\r\nkeep = true\r\n")
	add("configured-json-type-error", "works")
	rows[len(rows)-1].Before = text("[output]\njson = true\n")
	add("invalid-proxy-config", "works", "--type=novel")
	rows[len(rows)-1].Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	add("proxy-config-cleared", "users", "--no-proxy")
	rows[len(rows)-1].Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	for index := range rows {
		rows[index] = mypixivIsolatedStartup(t, rows[index])
	}
	recommendedFixture(t, "cli-mypixiv-startup.json", rows, *updateMyPixivStartup)
}

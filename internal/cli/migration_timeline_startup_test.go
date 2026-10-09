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

var updateTimelineStartup = flag.Bool("migration-update-timeline-startup", false, "capture timeline actual Run startup contracts")

type timelineStartupCase struct {
	userWorksStartupCase
	Bytes     int `json:"bytes"`
	ReadCalls int `json:"read_calls"`
}

func timelineIsolatedStartup(t *testing.T, current timelineStartupCase) timelineStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationTimelineStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_TIMELINE_CHILD="+string(encoded))
	output, err := child.CombinedOutput()
	if err != nil {
		t.Fatalf("isolated timeline startup: %v: %s", err, output)
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

func TestMigrationTimelineStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_TIMELINE_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current timelineStartupCase
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
	current.Exit = Run(append([]string{"pixiv", "timeline"}, current.Args...), reader, &out, &diagnostics)
	current.Stdout, current.Stderr, current.Bytes, current.ReadCalls = out.String(), diagnostics.String(), reader.bytes, reader.calls
	if reader.calls != 0 {
		t.Fatal("timeline startup consumed stdin")
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

func TestMigrationTimelineStartupPreservesRootDiagnosticsConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	rows := []timelineStartupCase{}
	add := func(name string, args ...string) {
		rows = append(rows, timelineStartupCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}})
	}
	add("following-missing-type", "following")
	add("latest-missing-type-json", "latest", "--json")
	add("following-artwork-auth", "following", "--type=artwork")
	add("latest-novel-auth-json", "latest", "--type=novel", "--json")
	add("following-novel-auth-ndjson", "following", "--type=novel", "--ndjson")
	add("invalid-type-priority", "following", "--type=invalid", "--page=0")
	add("positional-input", "latest", "--type=artwork", "42")
	add("stdin-ignored", "following", "--type=artwork")
	rows[len(rows)-1].Input = "{\"id\":42}\n42\n"
	add("failed-reader-ignored", "latest", "--type=novel")
	rows[len(rows)-1].ReadError = true
	add("invalid-page", "following", "--type=artwork", "--page=0", "--json")
	add("restrict-output-conflict", "following", "--type=novel", "--restrict=invalid", "--ndjson", "--json")
	add("proxy-output-conflict", "latest", "--type=artwork", "--proxy=", "--no-proxy", "--ndjson", "--json")
	add("unknown-latest-restrict", "latest", "--type=novel", "--restrict=public")
	add("unknown-flag-json", "following", "--type=artwork", "--unknown", "--json")
	add("malformed-limit", "following", "--type=artwork", "--limit=bad")
	add("malformed-bool-json", "latest", "--type=novel", "--json=bad")
	add("malformed-config-before-type", "following", "--type=invalid")
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("malformed-config-before-page", "latest", "--type=artwork", "--page=0")
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("unknown-config-preserved", "following", "--type=artwork")
	rows[len(rows)-1].Before = text("# mine\r\n[unknown]\r\nkeep = true\r\n")
	add("configured-json-type-error", "latest")
	rows[len(rows)-1].Before = text("[output]\njson = true\n")
	add("invalid-proxy-config", "following", "--type=novel")
	rows[len(rows)-1].Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	add("proxy-config-cleared", "latest", "--type=artwork", "--no-proxy")
	rows[len(rows)-1].Before = text("[pixiv.network]\nproxy_url = 'invalid'\n")
	for index := range rows {
		rows[index] = timelineIsolatedStartup(t, rows[index])
	}
	recommendedFixture(t, "cli-timeline-startup.json", rows, *updateTimelineStartup)
}

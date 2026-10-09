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

var updateRecommendedStartup = flag.Bool("migration-update-recommended-startup", false, "capture recommended startup contracts")

type recommendedStartupCase struct {
	Name      string   `json:"name"`
	Args      []string `json:"args"`
	Input     string   `json:"input"`
	Before    *string  `json:"before"`
	ReadError bool     `json:"read_error"`
	Exit      int      `json:"exit"`
	Stdout    string   `json:"stdout"`
	Stderr    string   `json:"stderr"`
	Config    bool     `json:"config"`
	After     string   `json:"after"`
	Database  bool     `json:"database"`
}

func recommendedIsolatedStartup(t *testing.T, current recommendedStartupCase) recommendedStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationRecommendedStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_RECOMMENDED_CHILD="+string(encoded))
	output, err := child.CombinedOutput()
	if err != nil {
		t.Fatalf("isolated startup: %v: %s", err, output)
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

func TestMigrationRecommendedStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_RECOMMENDED_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current recommendedStartupCase
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
	if current.ReadError {
		current.Exit = Run(append([]string{"pixiv", "recommended"}, current.Args...), migrationSearchFailedRead{}, &out, &diagnostics)
	} else {
		current.Exit = Run(append([]string{"pixiv", "recommended"}, current.Args...), strings.NewReader(current.Input), &out, &diagnostics)
	}
	current.Stdout, current.Stderr = out.String(), diagnostics.String()
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

func TestMigrationRecommendedStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	inputs := []recommendedStartupCase{
		{Name: "missing"},
		{Name: "illust", Args: []string{"illust"}},
		{Name: "manga", Args: []string{"manga"}},
		{Name: "novel", Args: []string{"novel"}},
		{Name: "user", Args: []string{"user"}},
		{Name: "all", Args: []string{"all"}},
		{Name: "artwork", Args: []string{"artwork"}},
		{Name: "invalid_kind", Args: []string{"invalid"}},
		{Name: "empty_kind", Args: []string{""}},
		{Name: "type_artwork", Args: []string{"--type=artwork"}},
		{Name: "type_all", Args: []string{"--type=all"}},
		{Name: "type_illust_rejected", Args: []string{"--type=illust"}},
		{Name: "type_invalid", Args: []string{"--type=invalid"}},
		{Name: "type_empty", Args: []string{"--type="}},
		{Name: "type_and_kind", Args: []string{"novel", "--type=novel"}},
		{Name: "type_and_empty_kind", Args: []string{"", "--type=novel"}},
		{Name: "too_many", Args: []string{"novel", "user"}},
		{Name: "content_manga", Args: []string{"artwork", "--content-type=manga"}},
		{Name: "content_illust", Args: []string{"--type=artwork", "--content-type=illust"}},
		{Name: "content_all", Args: []string{"--type=artwork", "--content-type=all"}},
		{Name: "content_invalid", Args: []string{"artwork", "--content-type=invalid"}},
		{Name: "content_novel", Args: []string{"novel", "--content-type=all"}},
		{Name: "content_positional_illust", Args: []string{"illust", "--content-type=illust"}},
		{Name: "invalid_kind_before_page", Args: []string{"invalid", "--page=0"}},
		{Name: "page_zero", Args: []string{"novel", "--page=0"}},
		{Name: "page_negative", Args: []string{"novel", "--page=-1"}},
		{Name: "limit_zero", Args: []string{"novel", "--limit=0"}},
		{Name: "limit_negative", Args: []string{"novel", "--limit=-1"}},
		{Name: "limit_and_page", Args: []string{"novel", "--limit=1", "--page=1"}},
		{Name: "page_before_proxy", Args: []string{"novel", "--page=0", "--proxy=", "--no-proxy"}},
		{Name: "proxy_both", Args: []string{"novel", "--proxy=", "--no-proxy"}},
		{Name: "proxy_both_false", Args: []string{"novel", "--proxy=", "--no-proxy=false"}},
		{Name: "proxy_empty", Args: []string{"novel", "--proxy="}},
		{Name: "proxy_invalid", Args: []string{"novel", "--proxy=invalid"}},
		{Name: "proxy_repeated", Args: []string{"novel", "--proxy=invalid", "--proxy="}},
		{Name: "no_proxy", Args: []string{"novel", "--no-proxy"}},
		{Name: "no_proxy_false", Args: []string{"novel", "--no-proxy=false"}},
		{Name: "json_ndjson", Args: []string{"novel", "--json", "--ndjson"}},
		{Name: "json_false_ndjson", Args: []string{"novel", "--json=false", "--ndjson"}},
		{Name: "json_ndjson_false", Args: []string{"novel", "--json", "--ndjson=false"}},
		{Name: "custom_config", Args: []string{"novel"}, Before: text("# mine\r\n[unknown]\r\nkeep = true\r\n")},
		{Name: "malformed_config", Args: []string{"novel"}, Before: text("[unfinished\n")},
		{Name: "config_before_kind", Args: []string{"invalid"}, Before: text("[unfinished\n")},
		{Name: "config_before_conflict", Args: []string{"novel", "--type=novel"}, Before: text("[unfinished\n")},
		{Name: "config_before_page", Args: []string{"novel", "--page=0"}, Before: text("[unfinished\n")},
		{Name: "configured_json", Args: []string{"novel"}, Before: text("[output]\njson = true\n")},
		{Name: "configured_proxy", Args: []string{"novel"}, Before: text("[pixiv.network]\nproxy_url = 'invalid'\n")},
		{Name: "configured_proxy_clear", Args: []string{"novel", "--no-proxy"}, Before: text("[pixiv.network]\nproxy_url = 'invalid'\n")},
		{Name: "configured_proxy_false", Args: []string{"novel", "--no-proxy=false"}, Before: text("[pixiv.network]\nproxy_url = 'invalid'\n")},
		{Name: "configured_proxy_empty", Args: []string{"novel", "--proxy="}, Before: text("[pixiv.network]\nproxy_url = 'invalid'\n")},
		{Name: "config_before_missing_kind", Before: text("[unfinished\n")},
		{Name: "stdin_read_failure", ReadError: true},
		{Name: "explicit_skips_read_failure", Args: []string{"novel"}, ReadError: true},
		{Name: "type_still_reads_failure", Args: []string{"--type=novel"}, ReadError: true},
	}
	var rows []recommendedStartupCase
	for _, input := range inputs {
		for _, mode := range []string{"", "--json", "--json=false", "--ndjson"} {
			current := input
			current.Args = append([]string{}, input.Args...)
			if mode != "" {
				current.Args = append(current.Args, mode)
			}
			rows = append(rows, recommendedIsolatedStartup(t, current))
		}
	}
	recommendedFixture(t, "cli-recommended-startup.json", rows, *updateRecommendedStartup)
}

func recommendedFixture(t *testing.T, name string, value any, update bool) {
	t.Helper()
	data, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", name)
	if update {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatalf("%s differs from Go reference", name)
	}
}

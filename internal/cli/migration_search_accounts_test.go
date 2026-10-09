package cli

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var updateSearchAccounts = flag.Bool("migration-update-search-accounts", false, "capture search startup and saved account contracts")

func TestMigrationSearchStartupAndSavedAccountErrors(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Name     string   `json:"name"`
		Args     []string `json:"args"`
		Before   *string  `json:"before"`
		After    string   `json:"after"`
		Database bool     `json:"database"`
		Stdout   string   `json:"stdout"`
		Stderr   string   `json:"stderr"`
		Exit     int      `json:"exit"`
	}
	text := func(value string) *string { return &value }
	inputs := []row{
		{Name: "missing", Args: []string{"cat"}},
		{Name: "custom", Args: []string{"cat"}, Before: text("# mine\r\n[unknown]\r\nkeep = true\r\n")},
		{Name: "malformed", Args: []string{"cat"}, Before: text("[unfinished\n")},
		{Name: "missing_default", Args: []string{"cat"}, Before: text("[pixiv.auth]\ndefault_user_id = 43\n")},
		{Name: "invalid_default", Args: []string{"cat"}, Before: text("[pixiv.auth]\ndefault_user_id = 0\n")},
		{Name: "empty_pool", Args: []string{"cat"}, Before: text("[account_pool]\nenabled = true\nstrategy = 'round_robin'\n")},
		{Name: "invalid_pool", Args: []string{"cat"}, Before: text("[account_pool]\nenabled = 'true'\n")},
		{Name: "invalid_proxy", Args: []string{"cat"}, Before: text("[pixiv.network]\nproxy_url = 'ftp://fixture-user:fixture-secret@proxy.invalid'\n")},
		{Name: "clear_proxy", Args: []string{"cat", "--no-proxy"}, Before: text("[pixiv.network]\nproxy_url = 'ftp://fixture-user:fixture-secret@proxy.invalid'\n")},
		{Name: "proxy_conflict", Args: []string{"cat", "--proxy", "", "--no-proxy=false"}},
		{Name: "invalid_search_by", Args: []string{"cat", "--search-by", "invalid"}},
		{Name: "invalid_sort", Args: []string{"cat", "--sort", "invalid"}},
		{Name: "invalid_limit", Args: []string{"cat", "--limit=-1"}},
		{Name: "invalid_rating", Args: []string{"cat", "--rating", "invalid"}},
		{Name: "config_before_options", Args: []string{"cat", "--search-by", "invalid"}, Before: text("[unfinished\n")},
		{Name: "bookmark_without_range", Args: []string{"cat", "--bookmark-strategy", "local"}},
	}
	var rows []row
	for _, input := range inputs {
		for _, mode := range []string{"default", "json", "ndjson"} {
			t.Run(input.Name+"/"+mode, func(t *testing.T) {
				home := t.TempDir()
				t.Setenv("HOME", home)
				t.Setenv("USERPROFILE", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				path := filepath.Join(home, ".pixiv-cli", "config.toml")
				if input.Before != nil {
					if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
						t.Fatal(err)
					}
					if err := os.WriteFile(path, []byte(*input.Before), 0600); err != nil {
						t.Fatal(err)
					}
				}
				r := input
				r.Name += "/" + mode
				r.Args = append([]string{}, input.Args...)
				if mode != "default" {
					r.Args = append(r.Args, "--"+mode)
				}
				var out, diagnostics bytes.Buffer
				r.Exit = Run(append([]string{"pixiv", "search"}, r.Args...), strings.NewReader(""), &out, &diagnostics)
				after, err := os.ReadFile(path)
				if err != nil {
					t.Fatal(err)
				}
				r.After = string(after)
				_, err = os.Stat(filepath.Join(home, ".pixiv-cli", "pixiv-cli.db"))
				if err != nil && !os.IsNotExist(err) {
					t.Fatal(err)
				}
				r.Database = err == nil
				r.Stdout, r.Stderr = out.String(), diagnostics.String()
				if strings.Contains(r.Stderr, "fixture-secret") {
					t.Fatal("search leaked proxy credentials")
				}
				rows = append(rows, r)
			})
		}
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "search-accounts.json")
	if *updateSearchAccounts {
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
		t.Fatal("search startup and account errors differ from Go reference")
	}
}

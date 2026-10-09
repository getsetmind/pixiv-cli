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

var updateNovelStartup = flag.Bool("migration-update-novel-startup", false, "capture initial detail startup and persistence contracts")

func TestMigrationNovelStartupInitializesConfigurationBeforeInputAndAccountAccess(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Name     string   `json:"name"`
		Flags    []string `json:"flags"`
		Input    string   `json:"input"`
		Before   *string  `json:"before"`
		JSON     bool     `json:"json"`
		After    string   `json:"after"`
		Database bool     `json:"database"`
		Stdout   string   `json:"stdout"`
		Stderr   string   `json:"stderr"`
		Exit     int      `json:"exit"`
	}
	text := func(value string) *string { return &value }
	inputs := []row{
		{Name: "novel_url", Input: "https://www.pixiv.net/novel/show.php?id=42"},
		{Name: "artwork_url", Input: "https://www.pixiv.net/artworks/42"},
		{Name: "content_unavailable", Input: "42", Flags: []string{"--content"}},
		{Name: "content_before_id", Input: "0", Flags: []string{"--content"}},
		{Name: "content_artwork", Input: "42", Flags: []string{"--type=artwork", "--content"}},
		{Name: "invalid_type", Input: "42", Flags: []string{"--type=invalid"}},
		{Name: "content_false", Input: "42", Flags: []string{"--content=false"}},
		{Name: "valid_missing", Input: "42"},
		{Name: "invalid_missing", Input: "0"},
		{Name: "existing_custom", Input: "42", Before: text("# mine\r\n[unknown]\r\nkeep = true\r\n")},
		{Name: "existing_malformed", Input: "42", Before: text("[unfinished\n")},
		{Name: "invalid_config_and_input", Input: "0", Before: text("[unfinished\n")},
	}
	rows := []row{}
	for _, input := range inputs {
		for _, machine := range []bool{false, true} {
			t.Run(input.Name+map[bool]string{false: "_plain", true: "_json"}[machine], func(t *testing.T) {
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
				var out, diagnostics bytes.Buffer
				args := append([]string{"pixiv", "detail", "--type=novel", input.Input}, input.Flags...)
				if machine {
					args = append(args, "--json")
				}
				input.JSON = machine
				input.Exit = Run(args, strings.NewReader(""), &out, &diagnostics)
				after, err := os.ReadFile(path)
				if err != nil {
					t.Fatal(err)
				}
				input.After = string(after)
				_, err = os.Stat(filepath.Join(home, ".pixiv-cli", "pixiv-cli.db"))
				if err != nil && !os.IsNotExist(err) {
					t.Fatal(err)
				}
				input.Database = err == nil
				input.Stdout = out.String()
				input.Stderr = diagnostics.String()
				rows = append(rows, input)
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
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "novel-startup.json")
	if *updateNovelStartup {
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
		t.Fatal("detail startup differs from Go reference")
	}
}

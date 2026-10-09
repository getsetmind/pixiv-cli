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

var updateUserCompatStartup = flag.Bool("migration-update-user-compat-startup", false, "capture actual user detail/search owner startup contracts")

func TestMigrationUserCompatibilityOwnerStartup(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Args     []string `json:"args"`
		Input    string   `json:"input"`
		Before   string   `json:"before,omitempty"`
		Stdout   string   `json:"stdout"`
		Stderr   string   `json:"stderr"`
		Exit     int      `json:"exit"`
		Config   bool     `json:"config"`
		Database bool     `json:"database"`
	}
	cases := []row{}
	for _, args := range [][]string{
		{"detail", "42"}, {"detail", ""}, {"detail", " "}, {"detail", "0"}, {"detail", "-1"}, {"detail", "not-id"}, {"detail", "https://www.pixiv.net/users/42"}, {"detail", "https://www.pixiv.net/artworks/42"}, {"detail"}, {"detail", "42", "43"}, {"detail", "42", "--ndjson"}, {"detail", "42", "--content"}, {"detail", "42", "--proxy=http://localhost", "--no-proxy=false"}, {"detail", "0", "--proxy=http://localhost", "--no-proxy"},
		{"search", "artist"}, {"search", "more", "words"}, {"search", ""}, {"search", " "}, {"search"}, {"search", "artist", "--limit=-1"}, {"search", "artist", "--page=0"}, {"search", "artist", "--limit=0", "--page=2"}, {"search", "artist", "--json=false", "--ndjson"}, {"search", "artist", "--type=user"}, {"search", "artist", "--sort=date_desc"}, {"search", "artist", "--proxy=http://localhost", "--no-proxy=false"}, {"search", "artist", "--limit=-1", "--proxy=http://localhost", "--no-proxy"},
	} {
		for _, machine := range []bool{false, true} {
			current := row{Args: append([]string{}, args...)}
			if machine {
				current.Args = append(current.Args, "--json")
			}
			cases = append(cases, current)
		}
	}
	cases = append(cases, row{Args: []string{"detail"}, Input: "42\n"}, row{Args: []string{"detail"}, Input: "42\n43\n"}, row{Args: []string{"search"}, Input: "more words\n"}, row{Args: []string{"search"}, Input: "first\nsecond\n"}, row{Args: []string{"detail", "0"}, Before: "[unfinished\n"}, row{Args: []string{"search", "artist", "--page=0"}, Before: "[unfinished\n"}, row{Args: []string{"detail", "42", "--json=false"}, Before: "[output]\njson=true\n"})
	for index := range cases {
		current := &cases[index]
		home := t.TempDir()
		t.Setenv("HOME", home)
		t.Setenv("USERPROFILE", home)
		t.Setenv("HTTPS_PROXY", "")
		t.Setenv("REQUEST_INTERVAL", "0")
		if current.Before != "" {
			path := filepath.Join(home, ".pixiv-cli", "config.toml")
			if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(path, []byte(current.Before), 0600); err != nil {
				t.Fatal(err)
			}
		}
		var out, diagnostics bytes.Buffer
		current.Exit = Run(append([]string{"pixiv", "user"}, current.Args...), strings.NewReader(current.Input), &out, &diagnostics)
		current.Stdout, current.Stderr = out.String(), diagnostics.String()
		exists := func(name string) bool {
			_, err := os.Stat(filepath.Join(home, ".pixiv-cli", name))
			if err != nil && !os.IsNotExist(err) {
				t.Fatal(err)
			}
			return err == nil
		}
		current.Config, current.Database = exists("config.toml"), exists("pixiv-cli.db")
	}
	data, err := json.MarshalIndent(cases, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "user-compat-startup.json")
	if *updateUserCompatStartup {
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
		t.Fatal("user owner startup differs from frozen Go")
	}
}

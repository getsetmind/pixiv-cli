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

var updateDetailProxy = flag.Bool("migration-update-detail-proxy", false, "capture detail proxy override and diagnostic order contracts")

func TestMigrationDetailProxyFlagsPreservePresenceAndDiagnosticOrder(t *testing.T) {
	oldCleanup, oldSupported := cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported
	t.Cleanup(func() { cleanupPendingWindowsUpdate, automaticPersistentHandlerSupported = oldCleanup, oldSupported })
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	type row struct {
		Name     string   `json:"name"`
		Input    string   `json:"input"`
		Config   string   `json:"config"`
		Flags    []string `json:"flags"`
		JSON     bool     `json:"json"`
		Stdout   string   `json:"stdout"`
		Stderr   string   `json:"stderr"`
		Exit     int      `json:"exit"`
		Database bool     `json:"database"`
	}
	configured := "[pixiv.network]\nproxy_url = 'ftp://fixture-user:fixture-secret@proxy.invalid'\n"
	inputs := []row{
		{Name: "configured_invalid", Input: "42", Config: configured, Flags: []string{}},
		{Name: "clear_configured", Input: "42", Config: configured, Flags: []string{"--no-proxy"}},
		{Name: "explicit_empty", Input: "42", Config: configured, Flags: []string{"--proxy="}},
		{Name: "explicit_override", Input: "42", Config: configured, Flags: []string{"--proxy=http://override.invalid"}},
		{Name: "no_proxy_false", Input: "42", Config: configured, Flags: []string{"--no-proxy=false"}},
		{Name: "invalid_override", Input: "42", Flags: []string{"--proxy=ftp://fixture-user:fixture-secret@override.invalid"}},
		{Name: "conflict", Input: "42", Flags: []string{"--proxy=", "--no-proxy"}},
		{Name: "conflict_false", Input: "42", Flags: []string{"--proxy=", "--no-proxy=false"}},
		{Name: "invalid_input_before_conflict", Input: "0", Flags: []string{"--proxy=", "--no-proxy"}},
		{Name: "invalid_config_before_conflict", Input: "42", Config: "[unfinished\n", Flags: []string{"--proxy=", "--no-proxy"}},
		{Name: "last_proxy_wins", Input: "42", Config: configured, Flags: []string{"--proxy=ftp://override.invalid", "--proxy="}},
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
				directory := filepath.Join(home, ".pixiv-cli")
				if err := os.MkdirAll(directory, 0700); err != nil {
					t.Fatal(err)
				}
				path := filepath.Join(directory, "config.toml")
				if err := os.WriteFile(path, []byte(input.Config), 0600); err != nil {
					t.Fatal(err)
				}
				args := append([]string{"pixiv", "detail", input.Input}, input.Flags...)
				if machine {
					args = append(args, "--json")
				}
				var out, diagnostics bytes.Buffer
				input.JSON = machine
				input.Exit = Run(args, strings.NewReader(""), &out, &diagnostics)
				input.Stdout, input.Stderr = out.String(), diagnostics.String()
				if strings.Contains(input.Stderr, "fixture-secret") {
					t.Fatal("proxy credential exposed")
				}
				after, err := os.ReadFile(path)
				if err != nil {
					t.Fatal(err)
				}
				if !bytes.Equal(after, []byte(input.Config)) {
					t.Fatal("proxy override changed configuration")
				}
				_, err = os.Stat(filepath.Join(directory, "pixiv-cli.db"))
				if err != nil && !os.IsNotExist(err) {
					t.Fatal(err)
				}
				input.Database = err == nil
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
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "detail-proxy.json")
	if *updateDetailProxy {
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("detail proxy behavior differs from Go reference")
	}
}

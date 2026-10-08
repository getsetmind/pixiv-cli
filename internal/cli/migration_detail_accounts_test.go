package cli

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	detail "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/detail"
	"github.com/spf13/cobra"
)

var updateDetailAccounts = flag.Bool("migration-update-detail-accounts", false, "capture saved account detail failure contracts")

func TestMigrationDetailUsesPersistedAccountAndConfigurationErrors(t *testing.T) {
	type row struct {
		Name   string `json:"name"`
		Config string `json:"config"`
		JSON   bool   `json:"json"`
		Stdout string `json:"stdout"`
		Stderr string `json:"stderr"`
		Exit   int    `json:"exit"`
	}
	inputs := []struct{ name, config string }{
		{"no_account", ""},
		{"missing_default", "[pixiv.auth]\ndefault_user_id = 43\n"},
		{"invalid_default", "[pixiv.auth]\ndefault_user_id = 0\n"},
		{"empty_pool", "[account_pool]\nenabled = true\nstrategy = 'round_robin'\n"},
		{"invalid_pool", "[account_pool]\nenabled = 'true'\n"},
		{"invalid_proxy", "[pixiv.network]\nproxy_url = 'ftp://fixture-user:fixture-secret@proxy.invalid'\n"},
	}
	rows := []row{}
	for _, input := range inputs {
		for _, machine := range []bool{false, true} {
			t.Run(input.name+map[bool]string{false: "_plain", true: "_json"}[machine], func(t *testing.T) {
				home := t.TempDir()
				t.Setenv("USERPROFILE", home)
				t.Setenv("HOME", home)
				t.Setenv("HTTPS_PROXY", "")
				t.Setenv("REQUEST_INTERVAL", "0")
				directory := filepath.Join(home, ".pixiv-cli")
				if err := os.MkdirAll(directory, 0700); err != nil {
					t.Fatal(err)
				}
				configPath := filepath.Join(directory, "config.toml")
				if err := os.WriteFile(configPath, []byte(input.config), 0600); err != nil {
					t.Fatal(err)
				}
				var out, diagnostics bytes.Buffer
				app := app{in: strings.NewReader(""), out: &out, errOut: &diagnostics, closeState: &closeState{}}
				command := detail.New(app.detailDeps())
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(&out)
				root.SetErr(&diagnostics)
				args := []string{"detail", "42"}
				if machine {
					args = append(args, "--json")
				}
				root.SetArgs(args)
				err := root.Execute()
				exit := app.exitWithNDJSONScope(err, false, machine)
				if err := app.closeState.close(); err != nil {
					t.Fatal(err)
				}
				after, err := os.ReadFile(configPath)
				if err != nil {
					t.Fatal(err)
				}
				if !bytes.Equal(after, []byte(input.config)) {
					t.Fatal("detail changed configuration")
				}
				if strings.Contains(diagnostics.String(), "fixture-secret") {
					t.Fatal("detail exposed proxy credentials")
				}
				rows = append(rows, row{Name: input.name, Config: input.config, JSON: machine, Stdout: out.String(), Stderr: diagnostics.String(), Exit: exit})
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
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "detail-accounts.json")
	if *updateDetailAccounts {
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
		t.Fatal("saved account detail differs from Go reference")
	}
}

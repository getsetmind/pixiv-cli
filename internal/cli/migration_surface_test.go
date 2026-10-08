package cli

import (
	"bytes"
	"encoding/json"
	"flag"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"testing"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

var migrationUpdateCLI = flag.Bool("migration-update-cli", false, "regenerate the frozen CLI contract after reviewing reference changes")

type migrationFlag struct {
	Name        string              `json:"name"`
	Shorthand   string              `json:"shorthand"`
	Type        string              `json:"type"`
	Default     string              `json:"default"`
	NoOptValue  string              `json:"no_opt_value"`
	Usage       string              `json:"usage"`
	Hidden      bool                `json:"hidden"`
	Deprecated  string              `json:"deprecated"`
	Annotations map[string][]string `json:"annotations,omitempty"`
}

type migrationCommand struct {
	Path       string          `json:"path"`
	Use        string          `json:"use"`
	Aliases    []string        `json:"aliases"`
	Short      string          `json:"short"`
	Long       string          `json:"long"`
	Example    string          `json:"example"`
	Hidden     bool            `json:"hidden"`
	Deprecated string          `json:"deprecated"`
	Runnable   bool            `json:"runnable"`
	Flags      []migrationFlag `json:"flags"`
	Persistent []migrationFlag `json:"persistent_flags"`
	Inherited  []migrationFlag `json:"inherited_flags"`
}

func migrationFlags(flags *pflag.FlagSet) []migrationFlag {
	result := make([]migrationFlag, 0)
	flags.VisitAll(func(f *pflag.Flag) {
		result = append(result, migrationFlag{
			Name: f.Name, Shorthand: f.Shorthand, Type: f.Value.Type(),
			Default: f.DefValue, NoOptValue: f.NoOptDefVal, Usage: f.Usage,
			Hidden: f.Hidden, Deprecated: f.Deprecated, Annotations: f.Annotations,
		})
	})
	sort.Slice(result, func(i, j int) bool { return result[i].Name < result[j].Name })
	return result
}

func TestMigrationCLIContractKeepsCommandsAliasesAndFlags(t *testing.T) {
	useTempPaths(t)
	root := (app{in: bytes.NewReader(nil), out: io.Discard, errOut: io.Discard, closeState: &closeState{}}).newRootCommand()
	commands := make([]migrationCommand, 0)
	var visit func(*cobra.Command)
	visit = func(cmd *cobra.Command) {
		cmd.InitDefaultHelpFlag()
		cmd.InitDefaultVersionFlag()
		aliases := append([]string{}, cmd.Aliases...)
		sort.Strings(aliases)
		commands = append(commands, migrationCommand{
			Path: cmd.CommandPath(), Use: cmd.Use, Aliases: aliases,
			Short: cmd.Short, Long: cmd.Long, Example: cmd.Example,
			Hidden: cmd.Hidden, Deprecated: cmd.Deprecated, Runnable: cmd.Runnable(),
			Flags:      migrationFlags(cmd.LocalNonPersistentFlags()),
			Persistent: migrationFlags(cmd.PersistentFlags()), Inherited: migrationFlags(cmd.InheritedFlags()),
		})
		for _, child := range cmd.Commands() {
			visit(child)
		}
	}
	visit(root)
	sort.Slice(commands, func(i, j int) bool { return commands[i].Path < commands[j].Path })
	content, err := json.MarshalIndent(commands, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	content = append(content, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "reference", "cli."+runtime.GOOS+"-"+runtime.GOARCH+".json")
	if *migrationUpdateCLI {
		if err := os.WriteFile(path, content, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(content, want) {
		t.Fatal("CLI contract differs from the frozen reference; review the command, alias, or flag change")
	}
}

package cli

import (
	"bytes"
	"context"
	"flag"
	"strings"
	"testing"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	mypixivcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/mypixiv"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateMyPixivFlags = flag.Bool("migration-update-mypixiv-flags", false, "capture mypixiv integer and boolean flag syntax")

type mypixivFlagsCase struct {
	mypixivStartupCase
	ParserOnly  bool    `json:"parser_only"`
	LeafError   *string `json:"leaf_error"`
	LeafStderr  *string `json:"leaf_stderr"`
	LeafExit    *int    `json:"leaf_exit"`
	ParsedValue *string `json:"parsed_value"`
}

func TestMigrationMyPixivFlagsPreserveBaseZeroIntegerAndBooleanSyntax(t *testing.T) {
	rows := []mypixivFlagsCase{}
	add := func(name string, parserOnly bool, args ...string) {
		rows = append(rows, mypixivFlagsCase{mypixivStartupCase: mypixivStartupCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: append([]string{"works", "--type=artwork"}, args...)}}, ParserOnly: parserOnly})
	}
	for _, current := range []struct {
		value      string
		parserOnly bool
	}{
		{"2", false}, {"0x2", false}, {"02", false}, {"0o2", false}, {"0b10", false}, {"08", true}, {"+0x2", false}, {"-0x2", false}, {"1_0", false}, {"0_7", false}, {"0x_2", false}, {"1__0", true}, {"9223372036854775808", true}, {"-9223372036854775809", true},
	} {
		add("limit:"+current.value, current.parserOnly, "--limit="+current.value)
	}
	for _, current := range []struct {
		value      string
		parserOnly bool
	}{
		{"0o2", false}, {"08", true}, {"9223372036854775808", true},
	} {
		add("page:"+current.value, current.parserOnly, "--limit=2", "--page="+current.value)
	}
	for _, value := range []string{"1", "0", "t", "f", "TRUE", "FALSE", "false"} {
		add("json:"+value, false, "--json="+value)
	}
	for _, name := range []string{"json", "ndjson", "no-proxy"} {
		add(name+":bad", true, "--"+name+"=bad")
	}
	add("missing-limit", true, "--limit")
	add("missing-type", true)
	rows[len(rows)-1].Args = []string{"works", "--type"}

	add("limit:18446744073709551616x", true, "--limit=18446744073709551616x")
	add("limit:9223372036854775808x", true, "--limit=9223372036854775808x")
	add("json:bel-control", true, "--json=\a")
	add("page:line-separator", true, "--page=\u2028")

	for index := range rows {
		current := &rows[index]
		current.mypixivStartupCase = mypixivIsolatedStartup(t, current.mypixivStartupCase)
		var output, diagnostics bytes.Buffer
		reader := &migrationTimelineReader{input: strings.NewReader("")}
		command := mypixivcmd.New(deps.Data{Input: reader, Output: &output, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { t.Fatal("invalid flag reached output resolution"); return false, nil }, Pooled: func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
			t.Fatal("invalid flag reached account pool")
			return nil
		}})
		if !current.ParserOnly {
			var leaf *cobra.Command
			for _, candidate := range command.Commands() {
				if candidate.Name() == current.Args[0] {
					leaf = candidate
					break
				}
			}
			if leaf == nil {
				t.Fatal("mypixiv leaf not found")
			}
			if err := leaf.ParseFlags(current.Args[1:]); err != nil {
				t.Fatalf("accepted root flag failed leaf parsing: %v", err)
			}
			flagName := strings.SplitN(current.Name, ":", 2)[0]
			parsed := leaf.Flags().Lookup(flagName).Value.String()
			current.ParsedValue = &parsed
			continue
		}

		root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
		root.AddCommand(command)
		root.SetOut(&output)
		root.SetErr(&diagnostics)
		root.SetArgs(append([]string{"mypixiv"}, current.Args...))
		err := root.Execute()
		if err == nil {
			t.Fatal("invalid flag accepted by leaf parser")
		}
		message := err.Error()
		exit := (app{out: &output, errOut: &diagnostics}).exitWithNDJSONScope(err, false, false)
		diagnostic := diagnostics.String()
		current.LeafError, current.LeafExit, current.LeafStderr = &message, &exit, &diagnostic
		if output.Len() != 0 || reader.calls != 0 {
			t.Fatal("invalid flag consumed input or emitted output")
		}
	}
	recommendedFixture(t, "cli-mypixiv-flags.json", rows, *updateMyPixivFlags)
}

package cli

import (
	"bytes"
	"context"
	"flag"
	"strings"
	"testing"

	recommendedcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/recommended"
	"github.com/FlanChanXwO/pixiv-cli/internal/cli/pipeline"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var updateRecommendedStdin = flag.Bool("migration-update-recommended-stdin", false, "capture recommended text stdin contracts")

type recommendedStdinCase struct {
	Args    []string               `json:"args"`
	Input   string                 `json:"input"`
	Word    string                 `json:"word"`
	Bytes   int                    `json:"bytes"`
	Error   string                 `json:"error"`
	Pooled  bool                   `json:"pooled"`
	Proxy   *string                `json:"proxy"`
	JSON    *bool                  `json:"json"`
	Startup recommendedStartupCase `json:"startup"`
}

func TestMigrationRecommendedStdinPreservesTextAndExplicitArguments(t *testing.T) {
	var rows []recommendedStdinCase
	for _, input := range []string{"", "\n", "\r\n", "novel\n", "novel\r\n", " novel \r\n", "novel\n\n", "novel\r", "novel\nuser\n", "novel user", "{\"type\":\"novel\"}\n"} {
		for _, words := range [][]string{{}, {"novel"}, {"novel", "user"}, {"--type=novel"}, {"--type=artwork", "--content-type=manga"}} {
			for _, mode := range []string{"", "--json=false", "--ndjson"} {
				current := recommendedStdinCase{Args: append([]string{}, words...), Input: input}
				if mode != "" {
					current.Args = append(current.Args, mode)
				}
				reader := &migrationSearchReader{input: strings.NewReader(input)}
				var command *cobra.Command
				var output, diagnostics bytes.Buffer
				command = recommendedcmd.New(recommendedcmd.Dependencies{
					Input: reader, Output: &output, UsageError: newUsageError,
					JSONOut: func(value *bool) (bool, error) { current.JSON = value; return false, nil },
					Pooled: func(_ context.Context, request recommendedcmd.Request, _ func(context.Context, *pixiv.Client) (bool, error)) error {
						current.Word = strings.Join(pipeline.ResolvedArgs(command, nil), " ")
						current.Pooled = true
						current.Proxy = request.HTTPSProxyOverride
						return nil
					},
				})
				root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
				root.AddCommand(command)
				root.SetOut(&output)
				root.SetErr(&diagnostics)
				root.SetArgs(append([]string{"recommended"}, current.Args...))
				if err := root.Execute(); err != nil {
					current.Error = err.Error()
				}
				current.Bytes = reader.bytes
				pipeline.Clear(root)
				current.Startup = recommendedIsolatedStartup(t, recommendedStartupCase{Args: current.Args, Input: input})
				if current.Startup.Stdout != "" {
					t.Fatal("startup error wrote stdout")
				}
				rows = append(rows, current)
			}
		}
	}
	recommendedFixture(t, "cli-recommended-stdin.json", rows, *updateRecommendedStdin)
}

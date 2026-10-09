package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	detailcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/detail"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
)

var migrationUpdateNovelInput = flag.Bool("migration-update-novel-input", false, "capture fixed Go detail input and error contracts")

func TestMigrationNovelInputMatchesFrozenErrorsBeforeAccountAccess(t *testing.T) {
	var references []struct {
		Input *string `json:"input"`
	}
	data, err := os.ReadFile(filepath.Join("..", "..", "docs", "migration", "contracts", "page-references.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, &references); err != nil {
		t.Fatal(err)
	}
	type row struct {
		Input  string `json:"input"`
		JSON   bool   `json:"json"`
		ID     int64  `json:"id"`
		Builds int    `json:"builds"`
		Stdout string `json:"stdout"`
		Stderr string `json:"stderr"`
		Exit   int    `json:"exit"`
	}
	rows := []row{}
	for _, reference := range references {
		if reference.Input == nil {
			continue
		}
		for _, machine := range []bool{false, true} {
			row := row{Input: *reference.Input, JSON: machine}
			var out, diagnostics bytes.Buffer
			command := detailcmd.New(detailcmd.Dependencies{Input: strings.NewReader(""), Output: &out, ErrorOutput: &diagnostics, OutputIsTTY: func() bool { return false }, BuildRequest: func(*cobra.Command, detailcmd.Options) (detailcmd.Request, error) {
				row.Builds++
				return detailcmd.Request{}, nil
			}, JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil }, Pooled: func(ctx context.Context, _ detailcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
				_, err := invoke(ctx, nil)
				return err
			}, FetchNovel: func(_ context.Context, _ *pixiv.Client, id int64) (pixiv.Novel, error) {
				row.ID = id
				return pixiv.Novel{}, sdk.NewError("pixiv", "Novel", sdk.Unauthorized)
			}})
			root := &cobra.Command{Use: "pixiv", SilenceErrors: true, SilenceUsage: true}
			root.AddCommand(command)
			root.SetOut(&out)
			root.SetErr(&diagnostics)
			args := []string{"detail", "--type=novel", "--", row.Input}
			if machine {
				args = []string{"detail", "--type=novel", "--json", "--", row.Input}
			}
			root.SetArgs(args)
			err := root.Execute()
			row.Exit = (app{out: &out, errOut: &diagnostics}).exitWithNDJSONScope(err, false, machine)
			row.Stdout = out.String()
			row.Stderr = diagnostics.String()
			rows = append(rows, row)
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "novel-input.json")
	if *migrationUpdateNovelInput {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("detail input differs from fixed Go reference")
	}
}

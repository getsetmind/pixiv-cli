package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	detailcmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/detail"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/spf13/cobra"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
)

var migrationUpdateWriter = flag.Bool("migration-update-writer", false, "capture fixed Go detail writer and exit contracts")

type migrationLimitedWriter struct {
	out       bytes.Buffer
	remaining int
	err       error
}

func (w *migrationLimitedWriter) Write(data []byte) (int, error) {
	if len(data) > w.remaining {
		data = data[:w.remaining]
		n, _ := w.out.Write(data)
		w.remaining = 0
		return n, w.err
	}
	w.remaining -= len(data)
	return w.out.Write(data)
}
func TestMigrationDetailWriterFailuresMatchFrozenExitAndDiagnostics(t *testing.T) {
	type row struct {
		Mode        string `json:"mode"`
		Cause       string `json:"cause"`
		Limit       int    `json:"limit"`
		Output      string `json:"output"`
		Diagnostics string `json:"diagnostics"`
		Exit        int    `json:"exit"`
	}
	rows := []row{}
	for _, mode := range []string{"human", "json", "ndjson"} {
		for _, cause := range []string{"pipe", "denied", "other"} {
			for _, limit := range []int{0, 8, 100000} {
				row := row{Mode: mode, Cause: cause, Limit: limit}
				var stop error
				switch cause {
				case "pipe":
					stop = syscall.EPIPE
				case "denied":
					stop = errors.New("fixture output denied")
				default:
					stop = errors.New("fixture output stopped")
				}
				writer := &migrationLimitedWriter{remaining: limit, err: stop}
				var diagnostics bytes.Buffer
				command := detailcmd.New(detailcmd.Dependencies{Input: strings.NewReader(""), Output: writer, ErrorOutput: &diagnostics, JSONOut: func(value *bool) (bool, error) { return value != nil && *value, nil }, BuildRequest: func(*cobra.Command, detailcmd.Options) (detailcmd.Request, error) { return detailcmd.Request{}, nil }, Pooled: func(ctx context.Context, _ detailcmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
					_, err := invoke(ctx, nil)
					return err
				}, FetchArtwork: func(context.Context, *pixiv.Client, int64) (pixiv.Artwork, error) {
					return pixiv.Artwork{ID: 42, Kind: pixiv.ArtworkKindIllustration, RawKind: "illust", Title: "writer fixture", User: pixiv.User{Name: "author", ID: 7}}, nil
				}})
				command.SilenceErrors = true
				command.SilenceUsage = true
				args := []string{strconv.Itoa(42)}
				if mode != "human" {
					args = append(args, "--"+mode)
				}
				command.SetArgs(args)
				err := command.Execute()
				row.Exit = (app{errOut: &diagnostics}).exitWithNDJSONScope(err, mode == "ndjson", mode != "human")
				row.Output = writer.out.String()
				row.Diagnostics = diagnostics.String()
				rows = append(rows, row)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "detail-writer.json")
	if *migrationUpdateWriter {
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
		t.Fatal("detail writer failures differ from fixed Go reference")
	}
}

var _ io.Writer = (*migrationLimitedWriter)(nil)

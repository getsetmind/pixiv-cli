package bookmark

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	deps "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateBookmarkRecordWhitespace = flag.Bool("migration-update-bookmark-record-whitespace", false, "capture frozen Go whole-body bookmark record whitespace contracts")

type bookmarkRecordWhitespaceCase struct {
	Command      string `json:"command"`
	Suffix       string `json:"suffix"`
	InputHex     string `json:"input_hex"`
	Error        string `json:"error"`
	PooledCalled bool   `json:"pooled_called"`
	Stdout       string `json:"stdout"`
	Stderr       string `json:"stderr"`
}

func TestMigrationBookmarkRecordWhitespaceMatchesFrozenWholeBodyInput(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..", "..")
	source, err := os.ReadFile(filepath.Join(root, "internal", "cli", "commands", "pixiv", "bookmark", "bookmark.go"))
	if err != nil {
		t.Fatal(err)
	}
	if got := fmt.Sprintf("%x", sha256.Sum256(source)); got != "9b0430c0bca81b59fd28b7308618e6c5b18e3cab64496455a0725f50a074a123" {
		t.Fatalf("frozen Go bookmark source differs: %s", got)
	}
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "bookmark_record_whitespace.json")
	var rows []bookmarkRecordWhitespaceCase
	stop := errors.New("fixture pooled boundary")
	for _, command := range []string{"detail", "list", "tags"} {
		body := `{"id":"42","type":"user","url":"https://www.pixiv.net/users/42"}`
		if command == "detail" {
			body = `{"id":"42","type":"artwork","url":"https://www.pixiv.net/artworks/42"}`
		}
		for _, suffix := range []struct{ name, value string }{{"none", ""}, {"json-whitespace", " \t\r\n"}, {"nbsp", "\u00a0"}} {
			row := bookmarkRecordWhitespaceCase{Command: command, Suffix: suffix.name, InputHex: hex.EncodeToString([]byte(body + suffix.value))}
			var out, errOut bytes.Buffer
			cmd := New(deps.Data{
				Input: bytes.NewReader([]byte(body + suffix.value)), Output: &out, ErrorOutput: &errOut,
				UsageError: func(err error) error { return err }, JSONOut: func(*bool) (bool, error) { return false, nil },
				Pooled: func(context.Context, deps.Request, func(context.Context, *pixiv.Client) (bool, error)) error {
					row.PooledCalled = true
					return stop
				},
			})
			cmd.SilenceErrors, cmd.SilenceUsage = true, true
			cmd.SetOut(&out)
			cmd.SetErr(&errOut)
			cmd.SetArgs([]string{command})
			err := cmd.Execute()
			if err != nil {
				row.Error = err.Error()
			}
			row.Stdout, row.Stderr = out.String(), errOut.String()
			if suffix.name == "nbsp" {
				if row.PooledCalled || row.Error != "invalid record JSON object" {
					t.Fatalf("%s whole-body NBSP: pooled=%v error=%q", command, row.PooledCalled, row.Error)
				}
			} else if !row.PooledCalled || !errors.Is(err, stop) {
				t.Fatalf("%s accepted whole-body input: pooled=%v error=%v", command, row.PooledCalled, err)
			}
			rows = append(rows, row)
		}
	}
	if *migrationUpdateBookmarkRecordWhitespace {
		data, err := json.MarshalIndent(rows, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected []bookmarkRecordWhitespaceCase
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(rows, expected) {
		t.Fatal("frozen whole-body bookmark whitespace contracts differ")
	}
	t.Logf("replayed %d whole-body bookmark whitespace cases; fixture sha256 %x", len(rows), sha256.Sum256(data))
}

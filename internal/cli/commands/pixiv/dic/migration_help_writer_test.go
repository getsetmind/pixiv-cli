package dic

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	dicservice "github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

var migrationUpdateDictionaryHelpWriter = flag.Bool("migration-update-dictionary-help-writer", false, "capture the frozen dictionary default-help writer contract")

const migrationDictionaryHelpWriterReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type migrationDictionaryHelpWriterInput struct {
	Name         string   `json:"name"`
	Args         []string `json:"args"`
	StdoutWriter string   `json:"stdout_writer"`
	StderrWriter string   `json:"stderr_writer"`
}

type migrationDictionaryHelpWriterWrite struct {
	Input string `json:"input"`
	Count int    `json:"count"`
	Error string `json:"error"`
}

type migrationDictionaryHelpWriterResult struct {
	Error        string                               `json:"error"`
	ErrorPipe    bool                                 `json:"error_broken_pipe"`
	Selected     string                               `json:"selected"`
	Stdout       string                               `json:"stdout"`
	Stderr       string                               `json:"stderr"`
	StdoutWrites []migrationDictionaryHelpWriterWrite `json:"stdout_writes"`
	StderrWrites []migrationDictionaryHelpWriterWrite `json:"stderr_writes"`
	Events       []string                             `json:"events"`
	InputReads   int                                  `json:"input_reads"`
	ReaderCalls  int                                  `json:"reader_calls"`
	JSONOutCalls int                                  `json:"json_out_calls"`
	UsageCalls   int                                  `json:"usage_error_calls"`
}

type migrationDictionaryHelpWriterCase struct {
	Input  migrationDictionaryHelpWriterInput  `json:"input"`
	Result migrationDictionaryHelpWriterResult `json:"result"`
}

type migrationDictionaryHelpWriterDependency struct {
	Module  string            `json:"module"`
	Version string            `json:"version"`
	Sum     string            `json:"sum"`
	Origin  json.RawMessage   `json:"origin"`
	Sources map[string]string `json:"sources"`
}

type migrationDictionaryHelpWriterFixture struct {
	Reference string                                  `json:"reference"`
	Sources   map[string]string                       `json:"sources"`
	Cobra     migrationDictionaryHelpWriterDependency `json:"cobra"`
	Gaps      []string                                `json:"gaps"`
	Rows      []migrationDictionaryHelpWriterCase     `json:"rows"`
}

type migrationDictionaryHelpWriter struct {
	mode   string
	stream string
	writes *[]migrationDictionaryHelpWriterWrite
	events *[]string
	output bytes.Buffer
}

func (writer *migrationDictionaryHelpWriter) Write(input []byte) (int, error) {
	count := len(input)
	call := len(*writer.writes) + 1
	var err error
	if writer.mode == "short-nil" {
		count /= 2
	} else if writer.mode == "zero-nil" {
		count = 0
	} else {
		positions := []string{"first", "second", "third"}
		position := ""
		if call <= len(positions) {
			position = positions[call-1]
		}
		if position != "" {
			switch writer.mode {
			case "fail-" + position:
				count, err = 0, errors.New("synthetic dictionary help writer failure")
			case "partial-error-" + position:
				count, err = min(5, count), errors.New("synthetic dictionary help writer failure")
			case "broken-pipe-" + position:
				count, err = 0, syscall.EPIPE
			}
		}
	}
	write := migrationDictionaryHelpWriterWrite{Input: string(input), Count: count}
	if err != nil {
		write.Error = err.Error()
	}
	*writer.writes = append(*writer.writes, write)
	*writer.events = append(*writer.events, fmt.Sprintf("%s:%d", writer.stream, call))
	_, _ = writer.output.Write(input[:count])
	return count, err
}

type migrationDictionaryHelpWriterReader struct{ calls *int }

func (reader migrationDictionaryHelpWriterReader) Article(context.Context, dicservice.ArticleRequest) (dicservice.Article, error) {
	*reader.calls++
	return dicservice.Article{}, errors.New("dictionary help must not fetch an article")
}

func (reader migrationDictionaryHelpWriterReader) Search(context.Context, dicservice.SearchRequest) ([]dicservice.SearchResult, error) {
	*reader.calls++
	return nil, errors.New("dictionary help must not search")
}

type migrationDictionaryHelpWriterInputReader struct{ reads *int }

func (reader migrationDictionaryHelpWriterInputReader) Read([]byte) (int, error) {
	*reader.reads++
	return 0, errors.New("dictionary help must not read stdin")
}

func migrationDictionaryHelpWriterExecute(t *testing.T, input migrationDictionaryHelpWriterInput) migrationDictionaryHelpWriterResult {
	t.Helper()
	result := migrationDictionaryHelpWriterResult{
		StdoutWrites: []migrationDictionaryHelpWriterWrite{},
		StderrWrites: []migrationDictionaryHelpWriterWrite{},
		Events:       []string{},
	}
	stdout := &migrationDictionaryHelpWriter{mode: input.StdoutWriter, stream: "stdout", writes: &result.StdoutWrites, events: &result.Events}
	stderr := &migrationDictionaryHelpWriter{mode: input.StderrWriter, stream: "stderr", writes: &result.StderrWrites, events: &result.Events}
	command := New(Dependencies{
		Input:  migrationDictionaryHelpWriterInputReader{reads: &result.InputReads},
		Output: stdout,
		Reader: migrationDictionaryHelpWriterReader{calls: &result.ReaderCalls},
		JSONOut: func(*bool) (bool, error) {
			result.JSONOutCalls++
			return false, errors.New("dictionary help must not resolve JSON output")
		},
		UsageError: func(err error) error {
			result.UsageCalls++
			return err
		},
	})
	command.SetOut(stdout)
	command.SetErr(stderr)
	command.SetArgs(input.Args)
	selected, err := command.ExecuteC()
	if selected != nil {
		result.Selected = selected.CommandPath()
	}
	if err != nil {
		result.Error = err.Error()
	}
	result.ErrorPipe = errors.Is(err, syscall.EPIPE)
	result.Stdout, result.Stderr = stdout.output.String(), stderr.output.String()
	if err != nil || result.InputReads != 0 || result.ReaderCalls != 0 || result.JSONOutCalls != 0 || result.UsageCalls != 0 {
		t.Fatalf("%s: dictionary help executed content work or returned an error: %+v", input.Name, result)
	}
	if selected == nil || len(result.StdoutWrites) != 3 || len(result.StderrWrites) != 0 {
		t.Fatalf("%s: default help must perform three stdout writes and no stderr writes: %+v", input.Name, result)
	}
	if result.StdoutWrites[0].Input != selected.Short+"\n" || result.StdoutWrites[1].Input != "\n" || !strings.HasPrefix(result.StdoutWrites[2].Input, "Usage:\n") {
		t.Fatalf("%s: default help write boundaries changed: %+v", input.Name, result.StdoutWrites)
	}
	return result
}

func migrationDictionaryHelpWriterSources(t *testing.T, root string) (map[string]string, migrationDictionaryHelpWriterDependency) {
	t.Helper()
	paths := []string{
		"go.mod", "go.sum", "internal/cli/commands/pixiv/dic/dic.go", "internal/cli/commands/pixiv/dic/article.go",
		"internal/cli/commands/pixiv/dic/search.go", "internal/cli/commands/lifecycle.go",
	}
	sources := map[string]string{}
	var frozenSum []byte
	for _, path := range paths {
		working, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		command := exec.Command("git", "show", migrationDictionaryHelpWriterReference+":"+path)
		command.Dir = root
		frozen, err := command.Output()
		if err != nil {
			t.Fatalf("read frozen %s: %v", path, err)
		}
		if !bytes.Equal(working, frozen) {
			t.Fatalf("%s differs from frozen Go reference", path)
		}
		digest := sha256.Sum256(frozen)
		sources[path] = hex.EncodeToString(digest[:])
		if path == "go.sum" {
			frozenSum = frozen
		}
	}
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		resolved, err := exec.Command("go", "env", "GOMODCACHE").Output()
		if err != nil {
			t.Fatalf("resolve Cobra module cache: %v", err)
		}
		cache = strings.TrimSpace(string(resolved))
	}
	dependency := migrationDictionaryHelpWriterDependency{Module: "github.com/spf13/cobra", Version: "v1.10.1", Sources: map[string]string{}}
	artifact := filepath.Join(cache, "cache", "download", "github.com", "spf13", "cobra", "@v", dependency.Version)
	sum, err := os.ReadFile(artifact + ".ziphash")
	if err != nil {
		t.Fatal(err)
	}
	dependency.Sum = strings.TrimSpace(string(sum))
	if !bytes.Contains(frozenSum, []byte(dependency.Module+" "+dependency.Version+" "+dependency.Sum+"\n")) {
		t.Fatal("cached Cobra checksum differs from the frozen go.sum")
	}
	info, err := os.ReadFile(artifact + ".info")
	if err != nil {
		t.Fatal(err)
	}
	var provenance struct {
		Version string          `json:"Version"`
		Origin  json.RawMessage `json:"Origin"`
	}
	if err := json.Unmarshal(info, &provenance); err != nil || provenance.Version != dependency.Version || len(provenance.Origin) == 0 {
		t.Fatalf("invalid cached Cobra provenance: %s (%v)", info, err)
	}
	dependency.Origin = provenance.Origin
	for _, path := range []string{"command.go", "cobra.go", "go.mod"} {
		data, err := os.ReadFile(filepath.Join(cache, "github.com", "spf13", "cobra@"+dependency.Version, path))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(data)
		dependency.Sources[path] = hex.EncodeToString(digest[:])
	}
	return sources, dependency
}

func TestMigrationDictionaryHelpWriter(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..", "..")
	sources, cobra := migrationDictionaryHelpWriterSources(t, root)
	fixture := migrationDictionaryHelpWriterFixture{
		Reference: migrationDictionaryHelpWriterReference,
		Sources:   sources,
		Cobra:     cobra,
		Gaps: []string{
			"Standalone dic.New executes the real default Cobra help path; root-prefixed help bytes and root startup remain in the separate immutable owner/startup fixtures.",
			"Cobra's defaultHelpFunc ignores the results of its three fmt writes and returns nil. Its generic HelpFunc emits stderr only when the help renderer returns an error; that branch is not reached here.",
			"Injected owned writers model finite success, short-nil, zero-nil, one-call failures, partial failures and EPIPE. Native operating-system pipes, process signals and closed console handles are not observed.",
		},
		Rows: []migrationDictionaryHelpWriterCase{},
	}
	invocations := []struct {
		name string
		args []string
	}{
		{"bare-group", []string{}}, {"group-help", []string{"--help"}},
		{"article-help", []string{"article", "--help"}}, {"search-help", []string{"search", "--help"}},
	}
	modes := []string{
		"success", "short-nil", "zero-nil", "fail-first", "fail-second", "fail-third",
		"partial-error-first", "partial-error-second", "partial-error-third",
		"broken-pipe-first", "broken-pipe-second", "broken-pipe-third",
	}
	for _, invocation := range invocations {
		for _, mode := range modes {
			input := migrationDictionaryHelpWriterInput{Name: invocation.name + "/" + mode, Args: invocation.args, StdoutWriter: mode, StderrWriter: "success"}
			fixture.Rows = append(fixture.Rows, migrationDictionaryHelpWriterCase{Input: input, Result: migrationDictionaryHelpWriterExecute(t, input)})
		}
		for _, mode := range []string{"success", "fail-first"} {
			input := migrationDictionaryHelpWriterInput{Name: invocation.name + "/" + mode + "/stderr-fail-first", Args: invocation.args, StdoutWriter: mode, StderrWriter: "fail-first"}
			fixture.Rows = append(fixture.Rows, migrationDictionaryHelpWriterCase{Input: input, Result: migrationDictionaryHelpWriterExecute(t, input)})
		}
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "cli-dictionary-help-writer.json")
	if *migrationUpdateDictionaryHelpWriter {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("dictionary help writer contract differs from frozen Go capture")
	}
}

var _ io.Writer = (*migrationDictionaryHelpWriter)(nil)

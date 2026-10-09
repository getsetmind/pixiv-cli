package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	usercmd "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/user"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var updateUserCompatOutput = flag.Bool("migration-update-user-compat-output", false, "capture dedicated user profile presentation")

func TestMigrationUserCompatibilityOwnerOutput(t *testing.T) {
	type row struct {
		Name   string          `json:"name"`
		Body   json.RawMessage `json:"body"`
		JSON   bool            `json:"json"`
		Output string          `json:"output"`
		Error  string          `json:"error"`
	}
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	source, err := os.ReadFile(filepath.Join(path, "user-output.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Name string          `json:"name"`
		Body json.RawMessage `json:"body"`
		Mode string          `json:"mode"`
	}
	if err := json.Unmarshal(source, &sources); err != nil {
		t.Fatal(err)
	}
	rows := []row{}
	for _, source := range sources {
		if source.Mode != "human" || (source.Name != "rich" && source.Name != "missing:user" && source.Name != "https://user:pass@example.test:8443/a?token=secret#fragment") {
			continue
		}
		for _, machine := range []bool{false, true} {
			rows = append(rows, row{Name: source.Name, Body: source.Body, JSON: machine})
		}
	}
	for _, machine := range []bool{false, true} {
		rows = append(rows, row{Name: "control-fields", JSON: machine, Body: json.RawMessage(`{"user":{"id":42,"name":"name\nnext\t\u001b","account":"account\rnext","comment":"comment\nnext"},"profile":{"webpage":"https://user:secret@example.test/a?private=yes#fragment","region":"region\nnext","country_code":"country\tnext","job":"job\rnext"},"workspace":{"pc":"pc\nnext","monitor":"monitor\tnext","tool":"tool\rnext","scanner":"scanner\nnext","tablet":"tablet\tnext","mouse":"mouse\rnext","printer":"printer\nnext","desktop":"desktop\tnext","music":"music\rnext","desk":"desk\nnext","chair":"chair\tnext","comment":"workspace\rnext"}}`)})
	}
	for index := range rows {
		current := &rows[index]
		var out bytes.Buffer
		command := usercmd.New(usercmd.Dependencies{Input: strings.NewReader(""), Output: &out, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return current.JSON, nil }, Pooled: func(ctx context.Context, _ usercmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
				if req.Method != "GET" || req.URL.Path != "/v1/user/detail" || req.URL.Query().Get("user_id") != "42" {
					t.Fatal("unexpected profile route")
				}
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(current.Body)), Request: req}, nil
			})}})
			if err != nil {
				return err
			}
			_, err = invoke(ctx, client)
			return err
		}})
		args := []string{"detail", "42"}
		if current.JSON {
			args = append(args, "--json")
		}
		command.SetArgs(args)
		command.SilenceUsage = true
		command.SilenceErrors = true
		if err := command.Execute(); err != nil {
			current.Error = err.Error()
		}
		current.Output = out.String()
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "user-compat-output.json")
	if *updateUserCompatOutput {
		if err := os.WriteFile(target, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("dedicated profile presentation differs from frozen Go")
	}
}

func TestMigrationUserCompatibilitySearchReusesFrozenExecution(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "cli-user-search.json"))
	if err != nil {
		t.Fatal(err)
	}
	var rows []struct {
		Args       []string          `json:"args"`
		Bodies     []json.RawMessage `json:"bodies"`
		WireBodies []string          `json:"wire_bodies"`
		Writer     string            `json:"writer"`
		Stdout     string            `json:"stdout"`
		Error      string            `json:"error"`
	}
	if err := json.Unmarshal(data, &rows); err != nil {
		t.Fatal(err)
	}
	tested := 0
	for _, current := range rows {
		args := []string{"search"}
		supported := true
		machine := false
		for _, arg := range current.Args {
			if arg == "--type=user" {
				continue
			}
			if strings.HasPrefix(arg, "--") && !strings.HasPrefix(arg, "--limit=") && !strings.HasPrefix(arg, "--page=") && arg != "--json" && arg != "--ndjson" {
				supported = false
			}
			if arg == "--json" {
				machine = true
			}
			args = append(args, arg)
		}
		if !supported || len(current.WireBodies) > 0 {
			continue
		}
		var output bytes.Buffer
		writer := io.Writer(&output)
		if current.Writer != "" {
			writer = migrationUserSearchWriter{&output, current.Writer}
		}
		command := usercmd.New(usercmd.Dependencies{Input: migrationSearchFailedRead{}, Output: writer, UsageError: newUsageError, JSONOut: func(*bool) (bool, error) { return machine, nil }, Pooled: func(ctx context.Context, _ usercmd.Request, invoke func(context.Context, *pixiv.Client) (bool, error)) error {
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationDateTransport(func(req *http.Request) (*http.Response, error) {
				if req.Method != "GET" || req.URL.Path != "/v1/search/user" {
					t.Fatal("unexpected search route")
				}
				body := current.Bodies[0]
				if len(current.Bodies) > 1 && req.URL.Query().Get("offset") != "" {
					body = current.Bodies[1]
				}
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
			})}})
			if err != nil {
				return err
			}
			_, err = invoke(ctx, client)
			return err
		}})
		command.SilenceUsage = true
		command.SilenceErrors = true
		command.SetArgs(args)
		message := ""
		if err := command.Execute(); err != nil {
			message = err.Error()
		}
		if output.String() != current.Stdout || message != current.Error {
			t.Fatalf("owner search differs for %v: output %q error %q", args, output.String(), message)
		}
		tested++
	}
	if tested != 225 {
		t.Fatalf("only compared %d owner cases", tested)
	}
}

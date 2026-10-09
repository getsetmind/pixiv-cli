package cli

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

var updateCommentReadsStartup = flag.Bool("migration-update-comment-reads-startup", false, "capture bookmark detail and tag startup contracts")

type commentReadsStartupCase struct {
	Operation string   `json:"operation"`
	Name      string   `json:"name"`
	Kind      string   `json:"kind"`
	Args      []string `json:"args"`
	Input     string   `json:"input"`
	Before    *string  `json:"before"`
	ReadError bool     `json:"read_error"`
	Exit      int      `json:"exit"`
	Stdout    string   `json:"stdout"`
	Stderr    string   `json:"stderr"`
	Config    bool     `json:"config"`
	After     string   `json:"after"`
	Database  bool     `json:"database"`
}

func commentReadsIsolatedStartup(t *testing.T, current commentReadsStartupCase) commentReadsStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationCommentReadsStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_COMMENT_READS_CHILD="+string(encoded))
	output, err := child.CombinedOutput()
	if err != nil {
		t.Fatalf("isolated startup: %v: %s", err, output)
	}
	payload, err := os.ReadFile(filepath.Join(home, "startup.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(payload, &current); err != nil {
		t.Fatal(err)
	}
	return current
}

func TestMigrationCommentReadsStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_COMMENT_READS_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current commentReadsStartupCase
	if err := json.Unmarshal([]byte(encoded), &current); err != nil {
		t.Fatal(err)
	}
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	directory := filepath.Join(os.Getenv("HOME"), ".pixiv-cli")
	if current.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(directory, "config.toml"), []byte(*current.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	var out, diagnostics bytes.Buffer
	if current.ReadError {
		current.Exit = Run(commentReadsStartupArgs(current.Operation, current.Kind, current.Args), migrationSearchFailedRead{}, &out, &diagnostics)
	} else {
		current.Exit = Run(commentReadsStartupArgs(current.Operation, current.Kind, current.Args), strings.NewReader(current.Input), &out, &diagnostics)
	}
	current.Stdout, current.Stderr = out.String(), diagnostics.String()
	exists := func(name string) bool {
		_, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return err == nil
	}
	current.Config, current.Database = exists("config.toml"), exists("pixiv-cli.db")
	if current.Config {
		after, err := os.ReadFile(filepath.Join(directory, "config.toml"))
		if err != nil {
			t.Fatal(err)
		}
		current.After = string(after)
	}
	data, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(os.Getenv("HOME"), "startup.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
}

func TestMigrationCommentReadsStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	var rows []commentReadsStartupCase
	for _, operation := range []string{"read", "stamps"} {
		for _, mode := range []string{"", "--json", "--ndjson"} {
			args := []string{"42"}
			if operation == "stamps" {
				args = []string{}
			}
			if mode != "" {
				args = append(args, mode)
			}
			for _, before := range []*string{nil, func() *string { value := "[unfinished\n"; return &value }(), func() *string { value := "[output]\njson = true\n"; return &value }()} {
				rows = append(rows, commentReadsIsolatedStartup(t, commentReadsStartupCase{Operation: operation, Kind: "artwork", Args: args, Before: before}))
			}
		}
	}
	for _, args := range [][]string{{}, {"42", "43"}, {"0"}, {"42", "--type=invalid"}, {"42", "--page=0"}, {"42", "--proxy=invalid"}, {"42", "--ndjson", "--json=false"}, {"42", "--proxy=", "--no-proxy"}} {
		rows = append(rows, commentReadsIsolatedStartup(t, commentReadsStartupCase{Operation: "read", Kind: "artwork", Args: args}))
	}
	for _, input := range []string{"42\n", "42\r\n", "{\"id\":42}\n"} {
		rows = append(rows, commentReadsIsolatedStartup(t, commentReadsStartupCase{Operation: "read", Kind: "novel", Input: input}))
	}
	rows = append(rows, commentReadsIsolatedStartup(t, commentReadsStartupCase{Operation: "stamps", Input: "ignored input"}))
	recommendedFixture(t, "cli-comment-reads-startup.json", rows, *updateCommentReadsStartup)
}
func commentReadsStartupArgs(operation, kind string, args []string) []string {
	prefix := []string{"pixiv", "comment", "--type=" + kind}
	if operation == "stamps" {
		prefix = []string{"pixiv", "comment", "stamps"}
	}
	return append(prefix, args...)
}

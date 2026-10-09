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

var updateCommentMutationsStartup = flag.Bool("migration-update-comment-mutations-startup", false, "capture bookmark detail and tag startup contracts")

type commentMutationsStartupCase struct {
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

func commentMutationsIsolatedStartup(t *testing.T, current commentMutationsStartupCase) commentMutationsStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationCommentMutationsStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_COMMENT_MUTATIONS_CHILD="+string(encoded))
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

func TestMigrationCommentMutationsStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_COMMENT_MUTATIONS_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current commentMutationsStartupCase
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
		current.Exit = Run(commentMutationsStartupArgs(current.Operation, current.Kind, current.Args), migrationSearchFailedRead{}, &out, &diagnostics)
	} else {
		current.Exit = Run(commentMutationsStartupArgs(current.Operation, current.Kind, current.Args), strings.NewReader(current.Input), &out, &diagnostics)
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

func TestMigrationCommentMutationsStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	var rows []commentMutationsStartupCase
	for _, operation := range []string{"create", "delete", "reply", "stamp"} {
		base := []string{"42"}
		if operation == "create" || operation == "reply" {
			base = append(base, "--comment=body")
		}
		if operation == "reply" {
			base = append(base, "--parent-comment-id=9")
		}
		if operation == "stamp" {
			base = append(base, "--stamp-id=7")
		}
		for _, before := range []*string{nil, func() *string { v := "[unfinished\n"; return &v }(), func() *string { v := "[output]\njson = true\n"; return &v }()} {
			rows = append(rows, commentMutationsIsolatedStartup(t, commentMutationsStartupCase{Operation: operation, Kind: "artwork", Args: base, Before: before}))
		}
		rows = append(rows, commentMutationsIsolatedStartup(t, commentMutationsStartupCase{Operation: operation, Kind: "artwork", Args: append(append([]string{}, base...), "--json")}))
		for _, args := range [][]string{{}, {"0"}, {"42", "--type=invalid"}, {"42", "--proxy=", "--no-proxy"}} {
			rows = append(rows, commentMutationsIsolatedStartup(t, commentMutationsStartupCase{Operation: operation, Kind: "artwork", Args: args}))
		}
	}
	for _, input := range []string{"42\n", "42\r\n", `{"id":42}` + "\n"} {
		rows = append(rows, commentMutationsIsolatedStartup(t, commentMutationsStartupCase{Operation: "create", Kind: "novel", Args: []string{"--comment=body"}, Input: input}))
	}

	for _, operation := range []string{"reply", "stamp"} {
		flag := "--parent-comment-id="
		if operation == "stamp" {
			flag = "--stamp-id="
		}
		for _, value := range []string{"bad", "9223372036854775808", "0x9", "010"} {
			rows = append(rows, commentMutationsIsolatedStartup(t, commentMutationsStartupCase{Operation: operation, Kind: "artwork", Args: []string{"42", "--comment=body", flag + value}}))
		}
	}
	for _, args := range [][]string{{"42", "--parent-comment-id=9"}, {"42", "--stamp-id=7"}, {"42", "--ndjson"}} {
		rows = append(rows, commentMutationsIsolatedStartup(t, commentMutationsStartupCase{Operation: "create", Kind: "artwork", Args: args}))
	}
	recommendedFixture(t, "cli-comment-mutations-startup.json", rows, *updateCommentMutationsStartup)
}
func commentMutationsStartupArgs(operation, kind string, args []string) []string {
	prefix := []string{"pixiv", "comment", operation, "--type=" + kind}
	return append(prefix, args...)
}

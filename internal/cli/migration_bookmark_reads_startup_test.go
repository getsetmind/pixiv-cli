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

var updateBookmarkReadsStartup = flag.Bool("migration-update-bookmark-reads-startup", false, "capture bookmark detail and tag startup contracts")

type bookmarkReadsStartupCase struct {
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

func bookmarkReadsIsolatedStartup(t *testing.T, current bookmarkReadsStartupCase) bookmarkReadsStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationBookmarkReadsStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_BOOKMARK_READS_CHILD="+string(encoded))
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

func TestMigrationBookmarkReadsStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_BOOKMARK_READS_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current bookmarkReadsStartupCase
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
		current.Exit = Run(bookmarkReadsStartupArgs(current.Operation, current.Kind, current.Args), migrationSearchFailedRead{}, &out, &diagnostics)
	} else {
		current.Exit = Run(bookmarkReadsStartupArgs(current.Operation, current.Kind, current.Args), strings.NewReader(current.Input), &out, &diagnostics)
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

func TestMigrationBookmarkReadsStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	var rows []bookmarkReadsStartupCase
	for _, operation := range []string{"detail", "tags"} {
		kinds := []string{"artwork", "novel"}
		if operation == "tags" {
			kinds = append(kinds, "all")
		}
		for _, kind := range kinds {
			variants := [][]string{{}, {"42"}, {"0"}, {"-1"}, {"bad"}, {"42", "43"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=invalid"}, {"42", "--no-proxy=false"}, {"0", "--type=invalid"}, {"42", "--type="}, {"42", "--type= artwork "}, {"42", "--type= novel "}, {"42", "--type=all"}, {"https://www.pixiv.net/artworks/42"}, {"https://www.pixiv.net/novel/show.php?id=42"}, {"https://www.pixiv.net/users/42"}, {"https://www.pixiv.net/users/42/bookmarks/artworks"}, {"https://www.pixiv.net/users/42/bookmarks/novels"}}
			if operation == "tags" {
				variants = append(variants, []string{"42", "--page=0"}, []string{"0", "--page=0"}, []string{"42", "--limit=-1"}, []string{"42", "--limit=0", "--page=2"}, []string{"42", "--ndjson", "--json=false"}, []string{"42", "--ndjson=false"}, []string{"42", "--restrict=invalid"}, []string{"0", "--page=0", "--restrict=invalid"})
			}
			for _, args := range variants {
				for _, mode := range []string{"", "--json", "--ndjson", "--json=false"} {
					current := bookmarkReadsStartupCase{Operation: operation, Kind: kind, Args: append([]string{}, args...)}
					if mode != "" {
						current.Args = append(current.Args, mode)
					}
					rows = append(rows, bookmarkReadsIsolatedStartup(t, current))
				}
			}
			for _, before := range []string{"[unfinished\n", "# mine\r\n[unknown]\r\nkeep = true\r\n", "[output]\njson = true\n", "[pixiv.network]\nproxy_url = 'invalid'\n"} {
				for _, args := range [][]string{{}, {"42"}, {"0"}, {"42", "--no-proxy"}, {"42", "--json=false"}} {
					rows = append(rows, bookmarkReadsIsolatedStartup(t, bookmarkReadsStartupCase{Operation: operation, Kind: kind, Args: append([]string{}, args...), Before: text(before)}))
				}
			}
			for _, input := range []string{"", "\n", "\r\n", "42\n", "42\r\n", " 42 \n", "42\n\n", "42\r", "42\n43\n", "https://www.pixiv.net/users/42\n", "{\"id\":42}\n", `{"id":"42","type":"artwork","url":"https://www.pixiv.net/artworks/42"}`, `{"id":"42","type":"novel","url":"https://www.pixiv.net/novel/show.php?id=42"}`, `{"id":"42","type":"user","url":"https://www.pixiv.net/users/42"}`, `{"id":"42","type":"artwork","url":""}`, "{"} {
				for _, args := range [][]string{{}, {"42"}, {"42", "43"}} {
					rows = append(rows, bookmarkReadsIsolatedStartup(t, bookmarkReadsStartupCase{Operation: operation, Kind: kind, Args: append([]string{}, args...), Input: input}))
				}
			}
			for _, args := range [][]string{{}, {"42"}} {
				rows = append(rows, bookmarkReadsIsolatedStartup(t, bookmarkReadsStartupCase{Operation: operation, Kind: kind, Args: append([]string{}, args...), ReadError: true}))
			}
		}
	}
	for _, current := range []bookmarkReadsStartupCase{
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--ndjson", "--json"}},
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--limit=1"}},
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--limit=1", "--json"}},
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--page=1"}},
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--page=1", "--json"}},
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--restrict=private"}},
		{Operation: "detail", Kind: "artwork", Args: []string{"42", "--restrict=private", "--json"}},
		{Operation: "tags", Kind: "artwork", Args: []string{"42", "--tag=cat"}},
		{Operation: "tags", Kind: "artwork", Args: []string{"42", "--tag=cat", "--json"}},
	} {
		captured := bookmarkReadsIsolatedStartup(t, current)
		if captured.Exit != 2 || captured.Stdout != "" || !strings.Contains(captured.Stderr, "unknown option") || captured.Config || captured.Database {
			t.Fatalf("unsupported option reached startup: %#v", captured)
		}
		rows = append(rows, captured)
	}
	recommendedFixture(t, "cli-bookmark-reads-startup.json", rows, *updateBookmarkReadsStartup)
}

func bookmarkReadsStartupArgs(operation, kind string, args []string) []string {
	return append([]string{"pixiv", "bookmark", operation, "--type=" + kind}, args...)
}

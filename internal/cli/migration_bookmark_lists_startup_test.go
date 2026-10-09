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

var updateBookmarkListsStartup = flag.Bool("migration-update-bookmark-lists-startup", false, "capture bookmark lists startup contracts")

type bookmarkListsStartupCase struct {
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

func bookmarkListsIsolatedStartup(t *testing.T, current bookmarkListsStartupCase) bookmarkListsStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationBookmarkListsStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_BOOKMARK_LISTS_CHILD="+string(encoded))
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

func TestMigrationBookmarkListsStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_BOOKMARK_LISTS_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current bookmarkListsStartupCase
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
		current.Exit = Run(bookmarkListsStartupArgs(current.Kind, current.Args), migrationSearchFailedRead{}, &out, &diagnostics)
	} else {
		current.Exit = Run(bookmarkListsStartupArgs(current.Kind, current.Args), strings.NewReader(current.Input), &out, &diagnostics)
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

func TestMigrationBookmarkListsStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	var rows []bookmarkListsStartupCase
	for _, kind := range []string{"artwork", "novel", "all", "user"} {
		variants := [][]string{{}, {"42"}, {"https://www.pixiv.net/users/42"}, {"0"}, {"-1"}, {"bad"}, {"https://www.pixiv.net/artworks/42"}, {"42", "43"}, {"42", "--page=0"}, {"0", "--page=0"}, {"42", "--limit=-1"}, {"42", "--limit=0", "--page=2"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=invalid"}, {"42", "--no-proxy=false"}, {"42", "--ndjson", "--json=false"}, {"42", "--ndjson=false"}, {"42", "--restrict=private"}, {"42", "--restrict="}, {"42", "--restrict=invalid"}, {"42", "--tag=日本語 & tag"}, {"https://www.pixiv.net/users/42/bookmarks/artworks"}, {"https://www.pixiv.net/users/42/bookmarks/novels"}, {"0", "--page=0", "--restrict=invalid"}}
		if kind != "user" {
			variants = append(variants, []string{"0", "--type=invalid"}, []string{"42", "--type=artwork"}, []string{"42", "--type=novel"}, []string{"42", "--type=all"}, []string{"42", "--type=ILLUST"}, []string{"42", "--type="})
		}
		for _, args := range variants {
			for _, mode := range []string{"", "--json", "--ndjson", "--json=false"} {
				current := bookmarkListsStartupCase{Kind: kind, Args: append([]string{}, args...)}
				if mode != "" {
					current.Args = append(current.Args, mode)
				}
				rows = append(rows, bookmarkListsIsolatedStartup(t, current))
			}
		}
		for _, before := range []string{"[unfinished\n", "# mine\r\n[unknown]\r\nkeep = true\r\n", "[output]\njson = true\n", "[pixiv.network]\nproxy_url = 'invalid'\n"} {
			for _, args := range [][]string{{}, {"42"}, {"0"}, {"42", "--no-proxy"}, {"42", "--json=false"}} {
				rows = append(rows, bookmarkListsIsolatedStartup(t, bookmarkListsStartupCase{Kind: kind, Args: append([]string{}, args...), Before: text(before)}))
			}
		}
		for _, input := range []string{"", "\n", "\r\n", "42\n", "42\r\n", " 42 \n", "42\n\n", "42\r", "42\n43\n", "https://www.pixiv.net/users/42\n", "{\"id\":42}\n", `{"id":"42","type":"user","url":"anything"}`, `{"id":"42","type":"novel","url":"anything"}`, `{"id":"42","type":"user","url":""}`, "{"} {
			for _, args := range [][]string{{}, {"42"}, {"42", "43"}} {
				rows = append(rows, bookmarkListsIsolatedStartup(t, bookmarkListsStartupCase{Kind: kind, Args: append([]string{}, args...), Input: input}))
			}
		}
		for _, args := range [][]string{{}, {"42"}} {
			rows = append(rows, bookmarkListsIsolatedStartup(t, bookmarkListsStartupCase{Kind: kind, Args: append([]string{}, args...), ReadError: true}))
		}
	}
	recommendedFixture(t, "cli-bookmark-lists-startup.json", rows, *updateBookmarkListsStartup)
}

func bookmarkListsStartupArgs(kind string, args []string) []string {
	prefix := []string{"pixiv", "bookmark", "list", "--type=" + kind}
	if kind == "user" {
		prefix = []string{"pixiv", "user", "bookmarks"}
	}
	return append(prefix, args...)
}

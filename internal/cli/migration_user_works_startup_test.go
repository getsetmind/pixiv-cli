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

var updateUserWorksStartup = flag.Bool("migration-update-user-works-startup", false, "capture user works startup contracts")

type userWorksStartupCase struct {
	Name      string   `json:"name"`
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

func userWorksIsolatedStartup(t *testing.T, current userWorksStartupCase) userWorksStartupCase {
	t.Helper()
	home := t.TempDir()
	encoded, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationUserWorksStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "HTTP_PROXY=", "http_proxy=", "ALL_PROXY=", "REQUEST_INTERVAL=0", "PIXIV_ACCESS_TOKEN=", "MIGRATION_USER_WORKS_CHILD="+string(encoded))
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

func TestMigrationUserWorksStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_USER_WORKS_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var current userWorksStartupCase
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
		current.Exit = Run(append([]string{"pixiv", "user"}, current.Args...), migrationSearchFailedRead{}, &out, &diagnostics)
	} else {
		current.Exit = Run(append([]string{"pixiv", "user"}, current.Args...), strings.NewReader(current.Input), &out, &diagnostics)
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

func TestMigrationUserWorksStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	var rows []userWorksStartupCase
	for _, kind := range []string{"artworks", "novels"} {
		variants := [][]string{{}, {"42"}, {"https://www.pixiv.net/users/42"}, {"0"}, {"-1"}, {"bad"}, {"https://www.pixiv.net/artworks/42"}, {"42", "43"}, {"42", "--page=0"}, {"0", "--page=0"}, {"42", "--limit=-1"}, {"42", "--limit=0", "--page=2"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=invalid"}, {"42", "--no-proxy=false"}, {"42", "--ndjson", "--json=false"}, {"42", "--ndjson=false"}}
		if kind == "artworks" {
			variants = append(variants, []string{"0", "--type=invalid"}, []string{"42", "--type=illust"}, []string{"42", "--type=illustration"}, []string{"42", "--type=manga"}, []string{"42", "--type=ugoira"}, []string{"42", "--type="})
		}
		for _, args := range variants {
			for _, mode := range []string{"", "--json", "--ndjson", "--json=false"} {
				current := userWorksStartupCase{Args: append([]string{kind}, args...)}
				if mode != "" {
					current.Args = append(current.Args, mode)
				}
				rows = append(rows, userWorksIsolatedStartup(t, current))
			}
		}
		for _, before := range []string{"[unfinished\n", "# mine\r\n[unknown]\r\nkeep = true\r\n", "[output]\njson = true\n", "[pixiv.network]\nproxy_url = 'invalid'\n"} {
			for _, args := range [][]string{{}, {"42"}, {"0"}, {"42", "--no-proxy"}, {"42", "--json=false"}} {
				rows = append(rows, userWorksIsolatedStartup(t, userWorksStartupCase{Args: append([]string{kind}, args...), Before: text(before)}))
			}
		}
		for _, input := range []string{"", "\n", "\r\n", "42\n", "42\r\n", " 42 \n", "42\n\n", "42\r", "42\n43\n", "https://www.pixiv.net/users/42\n", "{\"id\":42}\n"} {
			for _, args := range [][]string{{}, {"42"}, {"42", "43"}} {
				rows = append(rows, userWorksIsolatedStartup(t, userWorksStartupCase{Args: append([]string{kind}, args...), Input: input}))
			}
		}
		for _, args := range [][]string{{}, {"42"}} {
			rows = append(rows, userWorksIsolatedStartup(t, userWorksStartupCase{Args: append([]string{kind}, args...), ReadError: true}))
		}
	}
	recommendedFixture(t, "cli-user-works-startup.json", rows, *updateUserWorksStartup)
}

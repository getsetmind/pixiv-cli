package cli

import (
	"encoding/json"
	"flag"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

var updateAuthTransfer = flag.Bool("migration-update-auth-transfer", false, "capture isolated auth transfer commands")

func TestMigrationAuthTransfer(t *testing.T) {
	var rows []authAccountCase
	add := func(name string, args ...string) {
		rows = append(rows, authAccountCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}, Environment: map[string]string{}})
	}
	text := func(s string) *string { return &s }
	bundle := `{"schema":"pixiv-cli.auth-export","version":1,"default_user_id":1,"accounts":[{"user_id":1,"username":"restored","refresh_token":"synthetic-restored"}]}`
	for _, args := range [][]string{{"import", "--help"}, {"export", "--help"}, {"export", "--json"}, {"export", "--force"}, {"export", "--all", "1"}, {"export", "1", "2"}, {"import", "one", "two"}, {"import", "--proxy=x", "--no-proxy"}} {
		add(strings.Join(args, "-"), args...)
	}
	for _, input := range []string{"", "one\ntwo\n", "{broken", "{}", bundle} {
		add("import-stdin-"+input, "import", "--json")
		rows[len(rows)-1].Input = input
	}
	for _, args := range [][]string{{"import", "--proxy=x"}, {"import", "--no-proxy=false"}, {"import", "--proxy=x", "--no-proxy=false"}} {
		add("bundle-flags-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Input = bundle
	}
	for _, args := range [][]string{{"export"}, {"export", "--all"}, {"export", "1"}, {"export", "99"}, {"export", "--", "private-secret"}, {"export", "--output="}, {"export", "--force", "--output="}, {"export", "--all", "--output="}} {
		add("export-seed-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].Before = text("[pixiv.auth]\ndefault_user_id=1\n")
	}
	for _, args := range [][]string{{"export"}, {"export", "--all"}, {"export", "bad"}, {"export", "--force"}, {"export", "--output="}} {
		add("export-empty-"+strings.Join(args, "-"), args...)
	}
	for _, args := range [][]string{{"export", "1"}, {"export", "--force"}, {"import", "synthetic"}, {"import"}} {
		add("bad-config-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].Before = text("[unfinished\n")
		if len(args) == 1 {
			rows[len(rows)-1].Input = bundle
		}
	}
	for _, args := range [][]string{{"export", "--all"}, {"export", "1"}, {"import", "--json"}, {"import"}} {
		for _, short := range []bool{false, true} {
			add("writer-"+strings.Join(args, "-")+map[bool]string{false: "error", true: "short"}[short], args...)
			r := &rows[len(rows)-1]
			r.Seed = true
			r.Input = bundle
			r.ShortWrite = short
			r.WriteError = !short
		}
	}

	for _, args := range [][]string{{"import", "--json=false", "--json=bad"}, {"import", "--proxy"}, {"export", "--output"}, {"import", "--no-proxy=bad"}, {"export", "--force=bad"}, {"--json", "import"}, {"--help", "export"}, {"import", "--json", "--unknown"}, {"import", "--unknown", "--json"}} {
		add("parse-"+strings.Join(args, "-"), args...)
	}
	for _, input := range []string{"42\r\n", "42\n43\n", "private-secret", ""} {
		add("export-stdin-"+input, "export")
		rows[len(rows)-1].Input = input
		rows[len(rows)-1].Seed = true
	}
	for _, op := range []string{"import", "export"} {
		add(op+"-read-error", op)
		rows[len(rows)-1].ReadError = true
	}
	add("bundle-json-huge-number", "import", "--json")
	rows[len(rows)-1].Input = `{"version":1e999}`
	for _, args := range [][]string{{"import", "", "--json"}, {"import", "  ", "--json"}, {"import", "synthetic", "--proxy=x"}, {"import", "synthetic", "--proxy=x", "--no-proxy"}} {
		add("token-error-"+strings.Join(args, "-"), args...)
	}
	for index, row := range rows {
		home := t.TempDir()
		data, err := json.Marshal(row)
		if err != nil {
			t.Fatal(err)
		}
		child := exec.Command(os.Args[0], "-test.run=^TestMigrationAuthAccountsChild$")
		for _, entry := range os.Environ() {
			key, _, _ := strings.Cut(entry, "=")
			switch key {
			case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY":
				continue
			}
			child.Env = append(child.Env, entry)
		}
		child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_AUTH_ACCOUNT_CHILD="+string(data))
		for key, value := range row.Environment {
			child.Env = append(child.Env, key+"="+value)
		}
		if out, err := child.CombinedOutput(); err != nil {
			t.Fatalf("%s: %v %s", row.Name, err, out)
		}
		data, err = os.ReadFile(filepath.Join(home, "result.json"))
		if err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(data, &rows[index]); err != nil {
			t.Fatal(err)
		}
	}
	recommendedFixture(t, "cli-auth-transfer.json", rows, *updateAuthTransfer)
}

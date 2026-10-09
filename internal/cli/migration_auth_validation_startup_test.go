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

var updateAuthValidation = flag.Bool("migration-update-auth-validation", false, "capture isolated auth validation commands")

func TestMigrationAuthValidationStartup(t *testing.T) {
	var rows []authAccountCase
	add := func(name string, args ...string) {
		rows = append(rows, authAccountCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}, Environment: map[string]string{}})
	}
	text := func(s string) *string { return &s }
	for _, op := range []string{"check", "refresh"} {
		for _, args := range [][]string{{}, {"--help"}, {"--json"}, {"--json=false"}, {"--json=bad"}, {"1", "2"}, {"--unknown"}, {"0"}, {"bad"}, {"--", "-1"}, {"--proxy=x", "--no-proxy=false"}, {"--proxy"}, {"--all"}, {"bad", "--all"}} {
			add(op+"-"+strings.Join(args, "-"), append([]string{op}, args...)...)
		}
		for _, input := range []string{"", "bad\n", "42\n43\n"} {
			add(op+"-stdin-"+input, op, "--json")
			rows[len(rows)-1].Input = input
		}
		for _, before := range []string{"[unfinished\n", "[network]\nhttps_proxy='invalid'\n", "[pixiv.auth]\ndefault_user_id=99\n"} {
			add(op+"-config-"+before, op, "--json")
			rows[len(rows)-1].Before = text(before)
		}
		add(op+"-bad-config-invalid-uid", op, "bad", "--json")
		rows[len(rows)-1].Before = text("[unfinished\n")
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
	recommendedFixture(t, "cli-auth-validation.json", rows, *updateAuthValidation)
}

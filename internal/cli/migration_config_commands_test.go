package cli

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

var updateConfigCommands = flag.Bool("migration-update-config-commands", false, "capture isolated config commands")

type configCommandCase struct {
	userWorksStartupCase
	Environment      map[string]string `json:"environment"`
	WriteError       bool              `json:"write_error"`
	DiagnosticsError bool              `json:"diagnostics_error"`
}

type migrationConfigWriter struct{}

func (migrationConfigWriter) Write([]byte) (int, error) { return 0, errors.New("fixture write failed") }
func TestMigrationConfigCommandsChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_CONFIG_COMMAND_CHILD")
	if encoded == "" {
		t.Skip("isolated config helper")
	}
	var row configCommandCase
	if err := json.Unmarshal([]byte(encoded), &row); err != nil {
		t.Fatal(err)
	}
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	home := os.Getenv("HOME")
	directory := filepath.Join(home, ".pixiv-cli")
	path := filepath.Join(directory, "config.toml")
	if row.Before != nil {
		if err := os.MkdirAll(directory, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(*row.Before), 0600); err != nil {
			t.Fatal(err)
		}
	}
	var out, diagnostics bytes.Buffer
	var outputWriter io.Writer = &out
	var diagnosticWriter io.Writer = &diagnostics
	if row.WriteError {
		outputWriter = migrationConfigWriter{}
	}
	if row.DiagnosticsError {
		diagnosticWriter = migrationConfigWriter{}
	}
	if row.ReadError {
		row.Exit = Run(append([]string{"pixiv", "config"}, row.Args...), migrationSearchFailedRead{}, outputWriter, diagnosticWriter)
	} else {
		row.Exit = Run(append([]string{"pixiv", "config"}, row.Args...), strings.NewReader(row.Input), outputWriter, diagnosticWriter)
	}
	row.Stdout = strings.ReplaceAll(out.String(), home, "<HOME>")
	row.Stderr = strings.ReplaceAll(diagnostics.String(), home, "<HOME>")
	_, err := os.Stat(path)
	row.Config = err == nil
	_, err = os.Stat(filepath.Join(directory, "pixiv-cli.db"))
	row.Database = err == nil
	if row.Config {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		row.After = string(data)
	}
	data, err := json.Marshal(row)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(home, "result.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
}
func TestMigrationConfigCommands(t *testing.T) {
	var rows []configCommandCase
	add := func(name string, args ...string) {
		rows = append(rows, configCommandCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}, Environment: map[string]string{}})
	}
	text := func(s string) *string { return &s }
	add("group")
	add("group-extra", "extra")
	add("help", "--help")
	for _, operation := range []string{"path", "get", "set", "unset"} {
		add(operation+"-help", operation, "--help")
	}
	add("path", "path")
	add("path-extra", "path", "extra")
	for _, alias := range []string{"account_pool_enabled", "account_pool_strategy", "directory_template", "download_path", "filename_template", "https_proxy", "log_format", "log_level", "request_interval", "reverse_search_pixiv_only", "reverse_search_provider", "saucenao_api_key"} {
		add("get-"+alias, "get", alias)
	}
	for _, alias := range []string{"unknown", "output_json", "login_open_browser", "web_fallback_enabled", "account_pool_accounts"} {
		add("get-"+alias, "get", alias)
		add("set-"+alias, "set", alias, "true")
		add("unset-"+alias, "unset", alias)
	}
	add("get-stdin", "get")
	rows[len(rows)-1].Input = "download_path\r\n"
	add("get-empty", "get")
	add("get-extra", "get", "log_level", "x")
	add("get-read-error", "get")
	rows[len(rows)-1].ReadError = true
	add("set-missing-key", "set")
	add("set-missing-value", "set", "log_level")
	add("set-stdin", "set", "download_path")
	rows[len(rows)-1].Input = "  a\n\n"
	add("set-sensitive-argv", "set", "saucenao_api_key", "synthetic-key")
	add("set-sensitive-stdin", "set", "saucenao_api_key")
	rows[len(rows)-1].Input = "synthetic-key\r\n"
	add("set-sensitive-empty", "set", "saucenao_api_key")
	add("set-sensitive-read-error", "set", "saucenao_api_key")
	rows[len(rows)-1].ReadError = true
	for _, pair := range [][2]string{{"account_pool_enabled", "TRUE"}, {"account_pool_strategy", "random"}, {"request_interval", "1.5s"}, {"reverse_search_pixiv_only", "bad"}, {"log_level", "warn"}, {"request_interval", "-1s"}} {
		add("set-"+pair[0]+"-"+pair[1], "set", pair[0], pair[1])
	}
	add("set-override", "set", "download_path", "file-path")
	rows[len(rows)-1].Environment["DOWNLOAD_PATH"] = "env-path"
	add("unset-override", "unset", "https_proxy")
	rows[len(rows)-1].Environment["https_proxy"] = "https://user:synthetic@proxy.test"
	add("get-proxy-redacted", "get", "https_proxy")
	rows[len(rows)-1].Environment["https_proxy"] = "https://user:synthetic@proxy.test"
	add("path-malformed", "path")
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("get-malformed", "get", "log_level")
	rows[len(rows)-1].Before = text("[unfinished\n")
	add("get-other-invalid", "get", "download_path")
	rows[len(rows)-1].Before = text("[logging]\nlevel='invalid'\n")
	add("unset-tombstone", "unset", "web_fallback_enabled")
	rows[len(rows)-1].Before = text("# keep\n[web]\nfallback_enabled=true\n[unknown]\nkeep=1\n")
	for _, pair := range [][2]string{{"download_path", "new-downloads"}, {"filename_template", "{id}"}, {"directory_template", "{author}"}, {"https_proxy", "http://proxy.test:8080"}, {"request_interval", "2s"}, {"log_level", "debug"}, {"log_format", "json"}, {"reverse_search_provider", "all"}, {"reverse_search_pixiv_only", "false"}, {"saucenao_api_key", "synthetic-key"}, {"account_pool_enabled", "true"}, {"account_pool_strategy", "random"}} {
		add("set-managed-"+pair[0], "set", pair[0])
		if pair[0] == "saucenao_api_key" {
			rows[len(rows)-1].Input = pair[1] + "\n"
		} else {
			rows[len(rows)-1].Args = append(rows[len(rows)-1].Args, pair[1])
		}
		add("unset-managed-"+pair[0], "unset", pair[0])
	}
	add("get-proxy-empty-lower-env", "get", "https_proxy")
	rows[len(rows)-1].Environment = map[string]string{"https_proxy": "", "HTTPS_PROXY": "http://upper.test"}
	add("set-sensitive-override", "set", "saucenao_api_key")
	rows[len(rows)-1].Input = "synthetic-file\n"
	rows[len(rows)-1].Environment["SAUCENAO_API_KEY"] = "synthetic-env"
	add("set-ordinary-read-error", "set", "log_level")
	rows[len(rows)-1].ReadError = true
	add("set-negative-duration-delimited", "set", "request_interval", "--", "-1s")
	for _, args := range [][]string{{"get", "log_level", "--help=false"}, {"get", "--help=TRUE"}, {"set", "--help=bad"}, {"get", "-h=false", "log_level"}, {"path", "--help=0"}, {"--help=false"}, {"path", "-x"}, {"get", "--help", "--unknown"}} {
		add("flags-"+strings.Join(args, "-"), args...)
	}
	for _, args := range [][]string{{"get", "-hh"}, {"get", "-hx"}, {"set", "download_path", "-"}, {"get", "--help="}, {"get", "-hTRUE"}} {
		add("short-flags-"+strings.Join(args, "-"), args...)
	}
	add("unknown-flag", "get", "log_level", "--json")
	add("unset-stdin", "unset")
	rows[len(rows)-1].Input = "directory_template\n"
	for _, args := range [][]string{{}, {"--help"}, {"get", "--help"}, {"path"}, {"get", "download_path"}, {"get", "directory_template"}, {"get", "saucenao_api_key"}, {"set", "log_level", "debug"}, {"unset", "log_level"}} {
		add("output-error-"+strings.Join(args, "-"), args...)
		rows[len(rows)-1].WriteError = true
	}
	add("override-note-error", "set", "download_path", "file-path")
	rows[len(rows)-1].DiagnosticsError = true
	rows[len(rows)-1].Environment["DOWNLOAD_PATH"] = "env-path"
	for index, row := range rows {
		home := t.TempDir()
		data, err := json.Marshal(row)
		if err != nil {
			t.Fatal(err)
		}
		child := exec.Command(os.Args[0], "-test.run=^TestMigrationConfigCommandsChild$")
		for _, entry := range os.Environ() {
			key, _, _ := strings.Cut(entry, "=")
			switch key {
			case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY":
				continue
			}
			child.Env = append(child.Env, entry)
		}
		child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_CONFIG_COMMAND_CHILD="+string(data))
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
	recommendedFixture(t, "cli-config-commands.json", rows, *updateConfigCommands)
}

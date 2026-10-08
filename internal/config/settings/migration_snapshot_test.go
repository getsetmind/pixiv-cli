package settings_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
)

var updateConfigSnapshot = flag.Bool("migration-update-config-snapshot", false, "capture configuration snapshot contracts")

type migrationConfigInput struct {
	Name    string            `json:"name"`
	Body    string            `json:"body"`
	Missing bool              `json:"missing"`
	Env     map[string]string `json:"env"`
	Aliases []string          `json:"aliases"`
}

type migrationConfigValue struct {
	Alias   string              `json:"alias"`
	Value   config.SettingValue `json:"value"`
	Message string              `json:"message"`
	Removed bool                `json:"removed"`
}

type migrationDefaultID struct {
	ID      int64  `json:"id"`
	Present bool   `json:"present"`
	Message string `json:"message"`
}

type migrationConfigRow struct {
	Input          migrationConfigInput   `json:"input"`
	LoadMessage    string                 `json:"load_message"`
	Runtime        *config.RuntimeConfig  `json:"runtime"`
	RuntimeMessage string                 `json:"runtime_message"`
	RuntimeRemoved bool                   `json:"runtime_removed"`
	Values         []migrationConfigValue `json:"values"`
	Pixiv          migrationDefaultID     `json:"pixiv"`
	Fanbox         migrationDefaultID     `json:"fanbox"`
}

func migrationConfigInputs() []migrationConfigInput {
	inputs := []migrationConfigInput{
		{Name: "missing", Missing: true, Aliases: allResolvableAliases},
		{Name: "empty"},
		{Name: "whitespace", Body: " \t\r\n"},
		{Name: "unknown_and_comments", Body: "# preserve this comment\n[unknown]\nvalue = [1, 2]\n[pixiv.network]\nuser_agent = false\n[login]\nrelay_secret = 'synthetic-old-secret'\nrelay_target = 'unused'\n"},
		{Name: "all_file", Body: `[download]
path = './art'
filename_template = '{id}'
directory_template = '{author}'
[network]
https_proxy = 'http://global.invalid'
request_interval = '1h2m3.004005006s'
[logging]
level = ' debug '
format = ' json '
[output]
json = true
[update]
check_enabled = false
[login]
open_browser = false
use_after_login = true
relay_public_url = 'https://relay.invalid'
relay_listen_addr = '127.0.0.1:1234'
relay_tls_cert_file = 'cert.pem'
relay_tls_key_file = 'key.pem'
[reverse_search]
provider = ' all '
pixiv_only = false
saucenao_api_key = 'synthetic-file-key'
[account_pool]
enabled = true
strategy = ' random '
[pixiv.auth]
default_user_id = 42
[fanbox.auth]
default_user_id = 43.0
[pixiv.network]
proxy_url = ''
user_agent = 'ignored'
[fanbox.network]
proxy_url = 'http://fanbox.invalid'
user_agent = ''
[reverse_search.network]
proxy_url = ''
user_agent = 'synthetic-agent'
[fanbox.flaresolverr]
url = ' http://solver.invalid '
proxy_url = ''
[reverse_search.flaresolverr]
url = 'http://reverse-solver.invalid'
proxy_url = 'http://solver-proxy.invalid'
`, Aliases: allResolvableAliases},
		{Name: "all_env", Body: "[download]\npath = 'file'\n[network]\nhttps_proxy = 'file'\nrequest_interval = '-1s'\n[logging]\nlevel = 'invalid'\nformat = 'invalid'\n", Env: map[string]string{"DOWNLOAD_PATH": "env-path", "FILENAME_TEMPLATE": "env-name", "DIRECTORY_TEMPLATE": "env-directory", "https_proxy": "http://env.invalid", "HTTPS_PROXY": "http://env.invalid", "PIXIV_REQUEST_INTERVAL": "0s", "PIXIV_LOG_LEVEL": "debug", "PIXIV_LOG_FORMAT": "json", "SAUCENAO_API_KEY": "synthetic-env-key"}, Aliases: allResolvableAliases},
		{Name: "empty_env", Body: "[download]\npath = 'file'\nfilename_template = 'file'\ndirectory_template = 'file'\n[network]\nhttps_proxy = 'file'\n", Env: map[string]string{"DOWNLOAD_PATH": "", "FILENAME_TEMPLATE": "", "DIRECTORY_TEMPLATE": "", "https_proxy": "", "HTTPS_PROXY": "", "SAUCENAO_API_KEY": ""}, Aliases: []string{"download_path", "filename_template", "directory_template", "https_proxy", "saucenao_api_key"}},
		{Name: "uppercase_proxy", Env: map[string]string{"HTTPS_PROXY": "http://upper.invalid"}, Aliases: []string{"https_proxy"}},
		{Name: "undeclared_env", Env: map[string]string{"PIXIV_ACCOUNT_POOL_ENABLED": "true", "PIXIV_DEFAULT_USER_ID": "99", "LOG_LEVEL": "debug", "PIXIV_OUTPUT_JSON": "true"}},
		{Name: "scalar_string_coercion", Body: "[download]\npath = 42\nfilename_template = false\ndirectory_template = [1, 2]\n[network]\nhttps_proxy = 1.5\n[login]\nrelay_listen_addr = 1979-05-27T07:32:00Z\n", Aliases: []string{"download_path", "filename_template", "directory_template", "https_proxy", "login_relay_listen_addr"}},
		{Name: "unknown_alias", Aliases: []string{"unknown"}},
		{Name: "web_removed", Body: "[web]\nfallback_enabled = false\n", Aliases: []string{"web_fallback_enabled"}},
		{Name: "pool_removed", Body: "[account_pool]\naccounts = []\nenabled = 'true'\n", Aliases: []string{"account_pool_accounts", "account_pool_enabled"}},
		{Name: "pool_string_bool", Body: "[account_pool]\nenabled = 'true'\n", Aliases: []string{"account_pool_enabled"}},
		{Name: "pool_invalid_strategy", Body: "[account_pool]\nstrategy = 'weighted'\n", Aliases: []string{"account_pool_strategy"}},
		{Name: "pool_numeric_strategy", Body: "[account_pool]\nstrategy = 42\n", Aliases: []string{"account_pool_strategy"}},
		{Name: "ordinary_before_web", Body: "[network]\nrequest_interval = '-1s'\n[web]\nfallback_enabled = true\n"},
		{Name: "web_before_later_scalar", Body: "[web]\nfallback_enabled = false\n[reverse_search]\nprovider = 'invalid'\n"},
		{Name: "scalar_before_pool", Body: "[logging]\nlevel = 'invalid'\n[account_pool]\naccounts = []\n"},
		{Name: "pool_before_network", Body: "[account_pool]\nenabled = 1\n[pixiv.network]\nproxy_url = false\n"},
		{Name: "pixiv_before_fanbox", Body: "[pixiv.network]\nproxy_url = false\n[fanbox.network]\nuser_agent = false\n"},
		{Name: "fanbox_before_reverse", Body: "[fanbox.network]\nuser_agent = false\n[reverse_search.network]\nproxy_url = false\n"},
		{Name: "solver_proxy_without_url", Body: "[fanbox.flaresolverr]\nproxy_url = ''\n"},
		{Name: "solver_blank_url", Body: "[fanbox.flaresolverr]\nurl = '  '\n"},
		{Name: "solver_bad_proxy_before_blank_url", Body: "[fanbox.flaresolverr]\nurl = ''\nproxy_url = false\n"},
		{Name: "reverse_solver_proxy_without_url", Body: "[reverse_search.flaresolverr]\nproxy_url = 'http://proxy.invalid'\n"},
		{Name: "solver_empty_tables", Body: "[fanbox.flaresolverr]\n[reverse_search.flaresolverr]\n"},
		{Name: "empty_scalar", Body: "[download]\npath = ''\ndirectory_template = ''\n[network]\nhttps_proxy = ''\n", Aliases: []string{"download_path", "directory_template", "https_proxy"}},
	}
	for _, value := range []string{"true", "false", "'1'", "'0'", "'TRUE'", "' false '", "'yes'", "1", "[]"} {
		inputs = append(inputs, migrationConfigInput{Name: "bool_" + value, Body: "[output]\njson = " + value + "\n", Aliases: []string{"output_json"}})
	}
	for _, value := range []string{"'0'", "'1.5ms'", "'1us'", "'1µs'", "'1μs'", "' 2s '", "'-1ns'", "'1d'", "''", "'9223372036854775808ns'", "1"} {
		inputs = append(inputs, migrationConfigInput{Name: "duration_" + value, Body: "[network]\nrequest_interval = " + value + "\n", Aliases: []string{"request_interval"}})
	}
	for _, value := range []string{"'9223372036854775807ns'", "'-9223372036854775808ns'", "'+0'", "'-0'", "'.5s'", "'1.s'", "'1'", "'.s'", "'1h0m0.000000001s'", "'0.123456789012345678901234567890h'", "'0.0000000001s'"} {
		inputs = append(inputs, migrationConfigInput{Name: "duration_boundary_" + value, Body: "[network]\nrequest_interval = " + value + "\n", Aliases: []string{"request_interval"}})
	}
	for _, value := range []string{"1e6", "1e20", "1e-4", "1e-5", "-0.0", "inf", "-inf", "nan", "1979-05-27", "07:32:00", "1979-05-27T07:32:00", "1979-05-27T07:32:00+00:00", "1979-05-27T07:32:00.120000000Z", "1979-05-27T07:32:00+09:00", "1979-05-27T07:32:00-03:30", "{a = 1, b = true}"} {
		inputs = append(inputs, migrationConfigInput{Name: "string_boundary_" + value, Body: "[download]\ndirectory_template = " + value + "\n", Aliases: []string{"directory_template"}})
	}
	for _, value := range []string{"42", "42.0", "9223372036854775807", "0", "-1", "42.5", "'42'", "true", "[]", "9.223372036854776e18"} {
		inputs = append(inputs, migrationConfigInput{Name: "uid_" + value, Body: "[pixiv.auth]\ndefault_user_id = " + value + "\n[fanbox.auth]\ndefault_user_id = " + value + "\n"})
	}
	for _, path := range []string{"pixiv.network.proxy_url", "fanbox.network.proxy_url", "fanbox.network.user_agent", "reverse_search.network.proxy_url", "reverse_search.network.user_agent", "fanbox.flaresolverr.url", "reverse_search.flaresolverr.url"} {
		inputs = append(inputs, migrationConfigInput{Name: "strict_" + path, Body: path + " = 42\n"})
	}
	for _, body := range []string{"[logging]\nlevel = 'INFO'\n", "[logging]\nformat = ''\n", "[reverse_search]\nprovider = 'SAUCENAO'\n", "[download]\npath = 'first'\npath = 'duplicate'\n", "[download\n", "download.path = {name = 'value'}\n"} {
		inputs = append(inputs, migrationConfigInput{Name: fmt.Sprintf("invalid_%d", len(inputs)), Body: body})
	}
	return inputs
}

func TestMigrationConfigSnapshotPreservesRuntimePrecedenceAndDefaultAccounts(t *testing.T) {
	var rows []migrationConfigRow
	for _, input := range migrationConfigInputs() {
		t.Run(input.Name, func(t *testing.T) {
			clearSettingEnvironment(t)
			for _, name := range []string{"PIXIV_ACCOUNT_POOL_ENABLED", "PIXIV_DEFAULT_USER_ID", "LOG_LEVEL", "PIXIV_OUTPUT_JSON"} {
				t.Setenv(name, "")
				if err := os.Unsetenv(name); err != nil {
					t.Fatal(err)
				}
			}
			for name, value := range input.Env {
				t.Setenv(name, value)
			}
			path := filepath.Join(t.TempDir(), "config.toml")
			if !input.Missing {
				if err := os.WriteFile(path, []byte(input.Body), 0o600); err != nil {
					t.Fatal(err)
				}
			}
			store := config.Store{Files: diskFileStore{path}}
			row := migrationConfigRow{Input: input}
			snapshot, err := store.Current()
			if err != nil {
				row.LoadMessage = err.Error()
			} else {
				runtime, runtimeErr := snapshot.Runtime()
				if runtimeErr != nil {
					row.RuntimeMessage = runtimeErr.Error()
					row.RuntimeRemoved = errors.Is(runtimeErr, config.ErrRemovedSetting)
				} else {
					row.Runtime = &runtime
				}
				for _, alias := range input.Aliases {
					value, valueErr := snapshot.Effective(alias)
					entry := migrationConfigValue{Alias: alias, Value: value}
					if valueErr != nil {
						entry.Message = valueErr.Error()
						entry.Removed = errors.Is(valueErr, config.ErrRemovedSetting)
					}
					row.Values = append(row.Values, entry)
				}
			}
			id, present, err := store.ReadPixivDefaultUserID()
			row.Pixiv = migrationDefaultID{ID: id, Present: present}
			if err != nil {
				row.Pixiv.Message = err.Error()
			}
			id, present, err = store.ReadFanboxDefaultUserID()
			row.Fanbox = migrationDefaultID{ID: id, Present: present}
			if err != nil {
				row.Fanbox.Message = err.Error()
			}
			body, err := os.ReadFile(path)
			if input.Missing {
				if !errors.Is(err, os.ErrNotExist) {
					t.Fatalf("reading settings created a missing file: %v", err)
				}
			} else if err != nil || !bytes.Equal(body, []byte(input.Body)) {
				t.Fatalf("reading settings changed the file: %v", err)
			}
			rows = append(rows, row)
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "config-snapshot.json")
	if *updateConfigSnapshot {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("configuration snapshot differs from fixed Go reference")
	}
}

func TestMigrationConfigSnapshotRetainsCapturedFileAndEnvironment(t *testing.T) {
	clearSettingEnvironment(t)
	t.Setenv("DOWNLOAD_PATH", "first-env")
	files := &injectedFileStore{path: "synthetic/config.toml", files: map[string][]byte{"synthetic/config.toml": []byte("[logging]\nlevel = 'debug'\n")}}
	store := config.Store{Files: files}
	snapshot, err := store.Current()
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("DOWNLOAD_PATH", "second-env")
	files.files[files.path] = []byte("[logging]\nlevel = 'info'\n")
	old, err := snapshot.Runtime()
	if err != nil {
		t.Fatal(err)
	}
	if old.DownloadPath != "first-env" || old.LogLevel != "debug" {
		t.Fatalf("captured snapshot changed: %+v", old)
	}
	fresh, err := store.Current()
	if err != nil {
		t.Fatal(err)
	}
	current, err := fresh.Runtime()
	if err != nil {
		t.Fatal(err)
	}
	if current.DownloadPath != "second-env" || current.LogLevel != "info" {
		t.Fatalf("fresh snapshot did not observe changes: %+v", current)
	}
}

type migrationConfigFilePort struct {
	pathError error
	readError error
	reads     int
}

func (p *migrationConfigFilePort) Path() (string, error) {
	return "synthetic/config.toml", p.pathError
}

func (p *migrationConfigFilePort) ReadFile(string) ([]byte, error) {
	p.reads++
	return []byte("[pixiv.auth]\ndefault_user_id = 42\n"), p.readError
}

func (*migrationConfigFilePort) WritePrivateFile(string, []byte) error {
	panic("configuration read attempted a write")
}

func (*migrationConfigFilePort) EnsurePrivateFile(string, []byte) error {
	panic("configuration read attempted initialization")
}

func TestMigrationConfigSnapshotUsesOneReadAndPropagatesFileFailures(t *testing.T) {
	clearSettingEnvironment(t)
	port := &migrationConfigFilePort{}
	store := config.Store{Files: port}
	snapshot, err := store.Current()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := snapshot.Runtime(); err != nil {
		t.Fatal(err)
	}
	if _, err := snapshot.Effective("download_path"); err != nil {
		t.Fatal(err)
	}
	if port.reads != 1 {
		t.Fatalf("snapshot reread its file: %d reads", port.reads)
	}
	for range 2 {
		id, present, err := store.ReadPixivDefaultUserID()
		if err != nil || !present || id != 42 {
			t.Fatalf("default account: %d, %v, %v", id, present, err)
		}
	}
	if port.reads != 3 {
		t.Fatalf("default account reads reused stale data: %d reads", port.reads)
	}
	readFailure := errors.New("synthetic read failure")
	port.readError = readFailure
	if _, err := store.Current(); err != readFailure {
		t.Fatalf("read cause was replaced: %v", err)
	}
	if _, _, err := store.ReadPixivDefaultUserID(); err != readFailure {
		t.Fatalf("default account read cause was replaced: %v", err)
	}
	port.readError = os.ErrNotExist
	missing, err := store.Current()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := missing.Runtime(); err != nil {
		t.Fatal(err)
	}
	if id, present, err := store.ReadPixivDefaultUserID(); err != nil || present || id != 0 {
		t.Fatalf("missing file account: %d, %v, %v", id, present, err)
	}
	pathFailure := errors.New("synthetic path failure")
	port.pathError = pathFailure
	reads := port.reads
	if _, err := store.Current(); err != pathFailure {
		t.Fatalf("path cause was replaced: %v", err)
	}
	if _, _, err := store.ReadFanboxDefaultUserID(); err != pathFailure {
		t.Fatalf("default account path cause was replaced: %v", err)
	}
	if port.reads != reads {
		t.Fatal("path failure still read a file")
	}
	if _, err := (config.Store{}).Current(); err == nil || err.Error() != "config file store is not configured" {
		t.Fatalf("missing file port: %v", err)
	}
	if _, err := config.LoadSnapshotAtWithFileStore("synthetic/config.toml", nil); err == nil || err.Error() != "config file store is not configured" {
		t.Fatalf("missing snapshot file port: %v", err)
	}
}

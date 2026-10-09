package settings_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"github.com/FlanChanXwO/pixiv-cli/internal/config/paths"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
)

var updateMutations = flag.Bool("migration-update-config-mutations", false, "capture configuration Store mutation contracts")

func TestMigrationConfigurationStoreMutationsPreserveSparseDocuments(t *testing.T) {
	type row struct {
		Name        string            `json:"name"`
		Before      *string           `json:"before"`
		Operation   string            `json:"operation"`
		Alias       string            `json:"alias"`
		Raw         string            `json:"raw"`
		Environment map[string]string `json:"environment"`
		After       *string           `json:"after"`
		Error       string            `json:"error"`
		Removed     bool              `json:"removed"`
		Text        string            `json:"text"`
		Source      string            `json:"source"`
		HasValue    bool              `json:"has_value"`
		EnvOverride string            `json:"env_override"`
		HasOverride bool              `json:"has_override"`
	}
	ptr := func(s string) *string { return &s }
	custom := "# keep header\r\n[download] # table comment\r\n# path comment\r\npath='./old' # keep trailer\r\ncustom = [1,2,3]\r\n\r\n[unknown]\r\nkeep = {a=1,b='two'}\r\n"
	rows := []row{
		{Name: "set-missing", Operation: "set", Alias: "download_path", Raw: " ./new "},
		{Name: "set-comments-unknown-crlf", Before: ptr(custom), Operation: "set", Alias: "download_path", Raw: "./new"},
		{Name: "append-key", Before: ptr("[download]\npath = './saved'\n"), Operation: "set", Alias: "directory_template", Raw: " {author} "},
		{Name: "append-section", Before: ptr("# custom\n[unknown]\nkeep=true\n"), Operation: "set", Alias: "output_json", Raw: " TRUE "},
		{Name: "duration-normalized", Before: ptr(""), Operation: "set", Alias: "request_interval", Raw: " 1500ms "},
		{Name: "string-escaping", Before: ptr(""), Operation: "set", Alias: "filename_template", Raw: " a\n\"b\"\\c "},
		{Name: "unset-key-comments", Before: ptr(custom), Operation: "unset", Alias: "download_path"},
		{Name: "unset-empty-section", Before: ptr("# first\n[output] # gone\n# owned\njson=true # gone\n\n[unknown]\nkeep=true\n"), Operation: "unset", Alias: "output_json"},
		{Name: "unset-absent", Before: ptr("[unknown]\nkeep=true # retained\n"), Operation: "unset", Alias: "download_path"},
		{Name: "unset-missing", Operation: "unset", Alias: "download_path"},
		{Name: "unset-removed", Before: ptr("[web]\nfallback_enabled=true\n"), Operation: "unset", Alias: "web_fallback_enabled"},
		{Name: "set-removed", Before: ptr(""), Operation: "set", Alias: "web_fallback_enabled", Raw: "true"},
		{Name: "get-removed", Before: ptr("[web]\nfallback_enabled=true\n"), Operation: "get", Alias: "web_fallback_enabled"},
		{Name: "get-absent-removed", Before: ptr(""), Operation: "get", Alias: "web_fallback_enabled"},
		{Name: "get-unknown", Before: ptr(""), Operation: "get", Alias: "unknown"},
		{Name: "set-unknown", Before: ptr(""), Operation: "set", Alias: "unknown", Raw: "true"},
		{Name: "unset-unknown", Before: ptr(""), Operation: "unset", Alias: "unknown"},
		{Name: "invalid-bool", Before: ptr(""), Operation: "set", Alias: "output_json", Raw: "maybe"},
		{Name: "invalid-duration", Before: ptr(""), Operation: "set", Alias: "request_interval", Raw: "later"},
		{Name: "negative-duration", Before: ptr(""), Operation: "set", Alias: "request_interval", Raw: "-1s"},
		{Name: "invalid-choice", Before: ptr(""), Operation: "set", Alias: "log_level", Raw: "warn"},
		{Name: "get-invalid-value", Before: ptr("[output]\njson='maybe'\n"), Operation: "get", Alias: "output_json"},
		{Name: "malformed-set", Before: ptr("[unfinished\n"), Operation: "set", Alias: "output_json", Raw: "true"},
		{Name: "environment-empty-precedence", Before: ptr(""), Operation: "set", Alias: "https_proxy", Raw: "file-proxy", Environment: map[string]string{"https_proxy": "", "HTTPS_PROXY": "upper-proxy"}},
		{Name: "environment-note", Before: ptr(""), Operation: "unset", Alias: "download_path", Environment: map[string]string{"DOWNLOAD_PATH": "./env"}},
		{Name: "sensitive-environment-note", Before: ptr(""), Operation: "set", Alias: "saucenao_api_key", Raw: "synthetic-file-key", Environment: map[string]string{"SAUCENAO_API_KEY": "synthetic-env-key"}},
		{Name: "sensitive-get", Before: ptr("[reverse_search]\nsaucenao_api_key='synthetic-file-key'\n"), Operation: "get", Alias: "saucenao_api_key"},
		{Name: "set-log-level", Before: ptr(""), Operation: "set", Alias: "log_level", Raw: " debug "},
		{Name: "set-log-format", Before: ptr(""), Operation: "set", Alias: "log_format", Raw: " json "},
		{Name: "set-provider", Before: ptr(""), Operation: "set", Alias: "reverse_search_provider", Raw: " ascii2d-bovw "},
		{Name: "set-pixiv-only", Before: ptr(""), Operation: "set", Alias: "reverse_search_pixiv_only", Raw: " 0 "},
		{Name: "set-pool-enabled", Before: ptr(""), Operation: "set", Alias: "account_pool_enabled", Raw: " T "},
		{Name: "unset-removed-pool-accounts", Before: ptr("[account_pool]\naccounts=[1,2]\nenabled=true\n"), Operation: "unset", Alias: "account_pool_accounts"},
		{Name: "private-store-alias", Before: ptr(""), Operation: "set", Alias: "login_relay_public_url", Raw: " https://example.invalid/relay "},
		{Name: "set-pool-strategy", Before: ptr(""), Operation: "set", Alias: "account_pool_strategy", Raw: "random"},
	}
	for i := range rows {
		t.Run(rows[i].Name, func(t *testing.T) {
			for _, key := range []string{"DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "https_proxy", "HTTPS_PROXY", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY"} {
				old, present := os.LookupEnv(key)
				os.Unsetenv(key)
				t.Cleanup(func() {
					if present {
						os.Setenv(key, old)
					} else {
						os.Unsetenv(key)
					}
				})
			}
			for key, value := range rows[i].Environment {
				t.Setenv(key, value)
			}
			path := "synthetic/config.toml"
			files := &injectedFileStore{path: path, files: map[string][]byte{}}
			if rows[i].Before != nil {
				files.files[path] = []byte(*rows[i].Before)
			}
			store := config.Store{Files: files}
			var err error
			if rows[i].Operation == "get" {
				var value config.SettingValue
				value, err = store.Get(rows[i].Alias)
				if err == nil {
					rows[i].Text = config.PublicSettingText(rows[i].Alias, value.Text)
					rows[i].Source = value.Source
					rows[i].HasValue = value.HasValue
				}
			} else {
				var result config.ConfigMutationResult
				if rows[i].Operation == "set" {
					result, err = store.Set(rows[i].Alias, rows[i].Raw)
				} else {
					result, err = store.Unset(rows[i].Alias)
				}
				rows[i].EnvOverride = result.EnvOverride
				rows[i].HasOverride = result.HasOverride
			}
			if err != nil {
				rows[i].Error = err.Error()
				rows[i].Removed = errors.Is(err, config.ErrRemovedSetting)
			}
			if body, ok := files.files[path]; ok {
				rows[i].After = ptr(string(body))
			}
		})
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "config-mutations.json")
	if *updateMutations {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("configuration Store mutations differ from frozen Go reference")
	}
}

type mutationFailureStore struct {
	*injectedFileStore
	readError  error
	writeError error
	writes     int
}

func (s *mutationFailureStore) ReadFile(path string) ([]byte, error) {
	if s.readError != nil {
		return nil, s.readError
	}
	return s.injectedFileStore.ReadFile(path)
}
func (s *mutationFailureStore) WritePrivateFile(path string, body []byte) error {
	s.writes++
	if s.writeError != nil {
		return s.writeError
	}
	return s.injectedFileStore.WritePrivateFile(path, body)
}

func TestMigrationConfigurationStorePreservesFileFailuresAndInvalidWrites(t *testing.T) {
	path := "synthetic/config.toml"
	before := []byte("[download]\npath='retained'\n")
	sentinel := errors.New("synthetic file failure")
	for _, stage := range []string{"read", "write", "validation"} {
		t.Run(stage, func(t *testing.T) {
			files := &mutationFailureStore{injectedFileStore: &injectedFileStore{path: path, files: map[string][]byte{path: append([]byte(nil), before...)}}}
			raw := "true"
			switch stage {
			case "read":
				files.readError = sentinel
			case "write":
				files.writeError = sentinel
			case "validation":
				raw = "invalid"
			}
			_, err := (config.Store{Files: files}).Set("output_json", raw)
			if err == nil {
				t.Fatal("expected failure")
			}
			if stage != "validation" && !errors.Is(err, sentinel) {
				t.Fatal("file failure identity was lost")
			}
			if !bytes.Equal(files.files[path], before) {
				t.Fatal("failed mutation changed original bytes")
			}
			if stage != "write" && files.writes != 0 {
				t.Fatal("read or validation failure reached write boundary")
			}
		})
	}
}

func TestMigrationConfigurationPrivateMutationsCreateAndTightenPermissions(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("Unix mode contract")
	}
	home := t.TempDir()
	path := filepath.Join(home, "nested", "settings", "config.toml")
	restore := paths.SetConfigFilePathForTest(path)
	defer restore()
	store := config.DefaultStore()
	if _, err := store.Set("download_path", "./saved"); err != nil {
		t.Fatal(err)
	}
	assertMode := func(path string, want os.FileMode) {
		t.Helper()
		info, err := os.Stat(path)
		if err != nil {
			t.Fatal(err)
		}
		if info.Mode().Perm() != want {
			t.Fatalf("mode %o; want %o", info.Mode().Perm(), want)
		}
	}
	assertMode(filepath.Dir(path), 0700)
	assertMode(path, 0600)
	if err := os.Chmod(filepath.Dir(path), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(path, 0644); err != nil {
		t.Fatal(err)
	}
	if _, err := store.Unset("download_path"); err != nil {
		t.Fatal(err)
	}
	assertMode(filepath.Dir(path), 0700)
	assertMode(path, 0600)
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 {
		t.Fatal("mutation left staging artifacts")
	}
}

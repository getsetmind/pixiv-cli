package cli

import (
	"context"
	"encoding/json"
	"flag"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

var updateStaticDownloadStartup = flag.Bool("migration-update-static-download-startup", false, "capture isolated static artwork root startup contracts")

func TestMigrationStaticDownloadStartup(t *testing.T) {
	rows := []migrationDownloadStartupRow{
		{Name: "pid-no-account", Args: []string{"download", "42"}, ReadError: true},
		{Name: "artwork-url-json-no-account", Args: []string{"download", "https://www.pixiv.net/artworks/42", "--json"}, ReadError: true},
		{Name: "pid-ndjson-no-account", Args: []string{"download", "42", "--ndjson"}, ReadError: true},
		{Name: "stdin-artwork-url-no-account", Args: []string{"download", "--json"}, Input: "https://www.pixiv.net/artworks/42\n"},
		{Name: "pid-options-no-account", Args: []string{"download", "42", "--quality=small", "--pages=3,1", "--filename-template={author_id}-{id}-{num}", "--download-path=static-output"}, ReadError: true},
		{Name: "pid-invalid-quality-before-account", Args: []string{"download", "42", "--quality=bad", "--json"}, ReadError: true},
		{Name: "pid-invalid-pages-before-account", Args: []string{"download", "42", "--pages=0", "--json"}, ReadError: true},
	}
	malformed := "[unfinished\n"
	rows = append(rows, migrationDownloadStartupRow{Name: "static-runtime-before-quality", Args: []string{"download", "42", "--quality=bad", "--json"}, Before: &malformed, ReadError: true})
	actual := make([]migrationDownloadStartupRow, len(rows))
	for index, row := range rows {
		t.Run(row.Name, func(t *testing.T) {
			home := t.TempDir()
			body, err := json.Marshal(row)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			child := exec.CommandContext(ctx, os.Args[0], "-test.run=^TestMigrationDownloadStartupChild$")
			child.Dir = home
			for _, entry := range os.Environ() {
				key, _, _ := strings.Cut(entry, "=")
				switch key {
				case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY", "PIXIV_ACCESS_TOKEN", "PIXIV_REFRESH_TOKEN", "REQUEST_INTERVAL", "MIGRATION_DOWNLOAD_STARTUP_CHILD":
					continue
				}
				child.Env = append(child.Env, entry)
			}
			child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_DOWNLOAD_STARTUP_CHILD="+string(body))
			if out, err := child.CombinedOutput(); err != nil {
				t.Fatalf("isolated root child: %v\n%s", err, out)
			}
			result, err := os.ReadFile(filepath.Join(home, "result.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(result, &actual[index]); err != nil {
				t.Fatal(err)
			}
		})
	}
	if t.Failed() {
		return
	}
	const path = "../../crates/pixiv-cli/tests/fixtures/download_static_startup.json"
	if *updateStaticDownloadStartup {
		body, err := json.MarshalIndent(actual, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(body, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected []migrationDownloadStartupRow
	if err := json.Unmarshal(body, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(expected, actual) {
		got, _ := json.MarshalIndent(actual, "", "  ")
		t.Fatalf("static root contract changed\ngot: %s\nwant: %s", got, body)
	}
}

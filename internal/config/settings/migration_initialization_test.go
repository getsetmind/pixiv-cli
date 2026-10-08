package settings_test

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/config/paths"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
)

var updateInitialization = flag.Bool("migration-update-config-initialization", false, "capture initial configuration file contracts")

func TestMigrationInitialConfigurationPreservesExistingDocumentsAndDefaultBytes(t *testing.T) {
	type row struct {
		Name   string  `json:"name"`
		Before *string `json:"before"`
		After  string  `json:"after"`
	}
	text := func(value string) *string { return &value }
	inputs := []row{
		{Name: "missing"},
		{Name: "empty", Before: text("")},
		{Name: "custom", Before: text("# custom\r\n[download]\r\npath = './saved'\r\n[unknown]\r\nkeep = true\r\n")},
		{Name: "malformed", Before: text("[unfinished\n")},
	}
	for index := range inputs {
		t.Run(inputs[index].Name, func(t *testing.T) {
			home := t.TempDir()
			path := filepath.Join(home, ".pixiv-cli", "config.toml")
			restore := paths.SetConfigFilePathForTest(path)
			defer restore()
			if inputs[index].Before != nil {
				if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(path, []byte(*inputs[index].Before), 0600); err != nil {
					t.Fatal(err)
				}
			}
			store := config.DefaultStore()
			if err := store.EnsureDefaultConfigFile(); err != nil {
				t.Fatal(err)
			}
			first, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if inputs[index].Before != nil && !bytes.Equal(first, []byte(*inputs[index].Before)) {
				t.Fatal("initialization changed an existing document")
			}
			if err := store.EnsureDefaultConfigFile(); err != nil {
				t.Fatal(err)
			}
			second, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(first, second) {
				t.Fatal("repeated initialization changed the file")
			}
			inputs[index].After = string(first)
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(inputs, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "..", "docs", "migration", "contracts", "config-initialization.json")
	if *updateInitialization {
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
		t.Fatal("configuration initialization differs from Go reference")
	}
}

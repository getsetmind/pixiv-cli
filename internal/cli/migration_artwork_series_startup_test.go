package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

type artworkSeriesStartup struct {
	Exit     int
	Stdout   string
	Stderr   string
	Config   bool
	Database bool
}

func artworkSeriesIsolatedStartup(t *testing.T, args []string, input string) artworkSeriesStartup {
	t.Helper()
	home := t.TempDir()
	arguments, err := json.Marshal(args)
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(os.Args[0], "-test.run=^TestMigrationArtworkSeriesStartupChild$")
	child.Env = append(os.Environ(), "HOME="+home, "USERPROFILE="+home, "HTTPS_PROXY=", "https_proxy=", "REQUEST_INTERVAL=0", "MIGRATION_ARTWORK_SERIES_CHILD="+string(arguments), "MIGRATION_ARTWORK_SERIES_INPUT="+input)
	output, err := child.CombinedOutput()
	if err != nil {
		t.Fatalf("isolated startup: %v: %s", err, output)
	}
	var result artworkSeriesStartup
	payload, err := os.ReadFile(filepath.Join(home, "startup.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(payload, &result); err != nil {
		t.Fatal(err)
	}
	return result
}

func TestMigrationArtworkSeriesStartupChild(t *testing.T) {
	encoded := os.Getenv("MIGRATION_ARTWORK_SERIES_CHILD")
	if encoded == "" {
		t.Skip("isolated startup helper")
	}
	var args []string
	if err := json.Unmarshal([]byte(encoded), &args); err != nil {
		t.Fatal(err)
	}
	cleanupPendingWindowsUpdate = func() error { return nil }
	automaticPersistentHandlerSupported = func() bool { return false }
	var out, diagnostics bytes.Buffer
	result := artworkSeriesStartup{}
	result.Exit = Run(append([]string{"pixiv", "series"}, args...), strings.NewReader(os.Getenv("MIGRATION_ARTWORK_SERIES_INPUT")), &out, &diagnostics)
	result.Stdout, result.Stderr = out.String(), diagnostics.String()
	directory := filepath.Join(os.Getenv("HOME"), ".pixiv-cli")
	exists := func(name string) bool {
		_, err := os.Stat(filepath.Join(directory, name))
		if err != nil && !os.IsNotExist(err) {
			t.Fatal(err)
		}
		return err == nil
	}
	result.Config, result.Database = exists("config.toml"), exists("pixiv-cli.db")
	data, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(os.Getenv("HOME"), "startup.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
}

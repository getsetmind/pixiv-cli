package loginhelper_test

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
)

func TestMigrationPersistentStartupTestExecutableIsInert(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	if loginhelper.AutomaticPersistentHandlerSupported() {
		t.Fatal("test binary may not initialize a native association")
	}
}

func TestMigrationPersistentStartupFrozenSourcePolicy(t *testing.T) {
	root, err := filepath.Abs("../../../../../..")
	if err != nil {
		t.Fatal(err)
	}
	fixturePath := filepath.Join(root, "crates/pixiv-cli/tests/fixtures/cli_login_hidden_startup.json")
	fixture, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var anchors struct {
		Reference string            `json:"reference"`
		Sources   map[string]string `json:"sources"`
	}
	if err := json.Unmarshal(fixture, &anchors); err != nil {
		t.Fatal(err)
	}
	if anchors.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" {
		t.Fatal("frozen reference changed")
	}
	for path, want := range anchors.Sources {
		source, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(source)) != want {
			t.Fatalf("%s differs from frozen Go reference", path)
		}
	}
	source, err := os.ReadFile("persistent_state.go")
	if err != nil {
		t.Fatal(err)
	}
	temp, err := os.MkdirTemp(filepath.Join(root, "internal"), "persistent-startup-contract-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(temp)
	transformed := string(source)
	for _, change := range []struct {
		old, replacement string
		count            int
	}{
		{"\t\"runtime\"\n", "", 1},
		{"os.Args[0]", "policyArgs0", 1},
		{"runtime.GOOS", "policyOS", 2},
		{"os.Executable()", "policyExecutable()", 1},
	} {
		if strings.Count(transformed, change.old) != change.count {
			t.Fatalf("unexpected transformation boundary for %q", change.old)
		}
		transformed = strings.ReplaceAll(transformed, change.old, change.replacement)
	}
	if err := os.WriteFile(filepath.Join(temp, "persistent_state.go"), []byte(transformed), 0600); err != nil {
		t.Fatal(err)
	}
	payload, err := os.ReadFile("migration_persistent_startup_payload_test.go")
	if err != nil {
		t.Fatal(err)
	}
	const tag = "//go:build persistent_startup_contract_payload\n\n"
	if !strings.HasPrefix(string(payload), tag) {
		t.Fatal("missing payload isolation tag")
	}
	if err := os.WriteFile(filepath.Join(temp, "contract_test.go"), payload[len(tag):], 0600); err != nil {
		t.Fatal(err)
	}
	for _, check := range [][]string{{"test", "-count=1", "-v", "./internal/" + filepath.Base(temp)}, {"vet", "./internal/" + filepath.Base(temp)}} {
		cmd := exec.Command("go", check...)
		cmd.Dir = root
		cmd.Env = append(os.Environ(), "PERSISTENT_STARTUP_CONTRACT_FIXTURE="+fixturePath)
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("source-driven persistent startup %v: %v\n%s", check, err, out)
		}
		t.Logf("source-driven persistent startup %v passed\n%s", check, out)
	}
}

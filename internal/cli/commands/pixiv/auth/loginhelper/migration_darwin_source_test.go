package loginhelper_test

import (
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// Native Darwin execution needs a macOS runner; this test freezes Go control flow without system registration.
func TestMigrationDarwinFrozenSourceContracts(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("private Unix source permissions require a Unix runner")
	}
	root, err := filepath.Abs("../../../../../..")
	if err != nil {
		t.Fatal(err)
	}
	source, err := os.ReadFile("install_darwin.go")
	if err != nil {
		t.Fatal(err)
	}
	const frozen = "7cc57f3f1791038a2edbd4f0ff3432955218f1397934d83bcc6105dfbeec18f8"
	if fmt.Sprintf("%x", sha256.Sum256(source)) != frozen {
		t.Fatal("Darwin source changed from reference 4b4426487ef18bed276706daec385e0d0a6979f9")
	}
	temp, err := os.MkdirTemp(filepath.Join(root, "internal"), "darwin-contract-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(temp)
	mockImport := "github.com/FlanChanXwO/pixiv-cli/internal/" + filepath.Base(temp) + "/mockexec"
	if strings.Count(string(source), "\"os/exec\"") != 1 || !strings.HasPrefix(string(source), "//go:build darwin\n\n") {
		t.Fatal("unexpected transformation boundary")
	}
	transformed := strings.TrimPrefix(string(source), "//go:build darwin\n\n")
	transformed = strings.Replace(transformed, "\"os/exec\"", "exec \""+mockImport+"\"", 1)
	write := func(path string, body []byte) {
		t.Helper()
		if e := os.WriteFile(path, body, 0600); e != nil {
			t.Fatal(e)
		}
	}
	write(filepath.Join(temp, "install.go"), []byte(transformed))
	for _, file := range []string{"persistent_state.go", "endpoint.go"} {
		body, e := os.ReadFile(file)
		if e != nil {
			t.Fatal(e)
		}
		hashes := map[string]string{
			"persistent_state.go": "db753b648f1fba7f722e8a41856126210a99639ccda174a98cada1b85c8a06e4",
			"endpoint.go":         "91be2a6cd6ffb996559e9fe06097383a54527551cebf1513273890ba15671beb",
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != hashes[file] {
			t.Fatalf("%s changed from frozen reference", file)
		}
		write(filepath.Join(temp, file), body)
	}
	if err := os.Mkdir(filepath.Join(temp, "mockexec"), 0700); err != nil {
		t.Fatal(err)
	}
	payload := func(name string) string {
		body, e := os.ReadFile(name)
		if e != nil {
			t.Fatal(e)
		}
		return strings.TrimPrefix(string(body), "//go:build darwin_contract_payload\n\n")
	}
	write(filepath.Join(temp, "mockexec", "exec.go"), []byte(payload("migration_darwin_mock_payload_test.go")))
	write(filepath.Join(temp, "contract_test.go"), []byte(strings.ReplaceAll(payload("migration_darwin_contract_payload_test.go"), "MOCK_IMPORT", mockImport)))
	fixture := filepath.Join(root, "crates", "pixiv-app", "tests", "fixtures", "darwin_handler.json")
	cmd := exec.Command("go", "test", "-count=1", "-v", "./internal/"+filepath.Base(temp))
	cmd.Dir = root
	cmd.Env = append(os.Environ(), "DARWIN_CONTRACT_FIXTURE="+fixture)
	if out, e := cmd.CombinedOutput(); e != nil {
		t.Fatalf("source-driven Darwin contracts: %v\n%s", e, out)
	} else {
		t.Log(string(out))
	}
	vet := exec.Command("go", "vet", "./internal/"+filepath.Base(temp))
	vet.Dir = cmd.Dir
	vet.Env = cmd.Env
	if out, e := vet.CombinedOutput(); e != nil {
		t.Fatalf("source-driven Darwin contract vet: %v\n%s", e, out)
	} else {
		t.Logf("source-driven Darwin contract vet passed\n%s", out)
	}
}

package loginhelper_test

import (
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestMigrationWindowsShellFrozenSourceContracts(t *testing.T) {
	root, err := filepath.Abs("../../../../../..")
	if err != nil {
		t.Fatal(err)
	}
	source, err := os.ReadFile("delegate_windows.go")
	if err != nil {
		t.Fatal(err)
	}
	const frozen = "17a4b6801773cbbb5a790f0f51e7dc3d85d02ef36f911394ec1aa0ce66b72e94"
	if fmt.Sprintf("%x", sha256.Sum256(source)) != frozen {
		t.Fatal("Windows shell source changed from reference 4b4426487ef18bed276706daec385e0d0a6979f9")
	}
	temp, err := os.MkdirTemp(filepath.Join(root, "internal"), "windows-shell-contract-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(temp)
	mockImport := "github.com/FlanChanXwO/pixiv-cli/internal/" + filepath.Base(temp) + "/mockwindows"
	if strings.Count(string(source), "\"golang.org/x/sys/windows\"") != 1 || !strings.HasPrefix(string(source), "//go:build windows\n\n") {
		t.Fatal("unexpected shell transformation boundary")
	}
	transformed := strings.TrimPrefix(string(source), "//go:build windows\n\n")
	transformed = strings.Replace(transformed, "\"golang.org/x/sys/windows\"", "windows \""+mockImport+"\"", 1)
	write := func(path, body string) {
		t.Helper()
		if err := os.WriteFile(path, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
	}
	payload := func(name string) string {
		t.Helper()
		body, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		const tag = "//go:build windows_shell_contract_payload\n\n"
		if !strings.HasPrefix(string(body), tag) {
			t.Fatalf("unexpected shell payload tag in %s", name)
		}
		return strings.TrimPrefix(string(body), tag)
	}
	write(filepath.Join(temp, "delegate.go"), transformed)
	if err := os.Mkdir(filepath.Join(temp, "mockwindows"), 0700); err != nil {
		t.Fatal(err)
	}
	write(filepath.Join(temp, "mockwindows", "windows.go"), payload("migration_windows_shell_mock_payload_test.go"))
	write(filepath.Join(temp, "contract_test.go"), strings.ReplaceAll(payload("migration_windows_shell_contract_payload_test.go"), "MOCK_IMPORT", mockImport))
	for _, arguments := range [][]string{
		{"test", "-count=1", "-v", "./internal/" + filepath.Base(temp)},
		{"vet", "./internal/" + filepath.Base(temp) + "/..."},
	} {
		cmd := exec.Command("go", arguments...)
		cmd.Dir = root
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("source-driven Windows shell %s: %v\n%s", arguments[0], err, out)
		} else {
			t.Log(string(out))
		}
	}
}

package loginhelper_test

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestMigrationWindowsFrozenSourceContracts(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("private Unix source permissions require a Unix runner")
	}
	root, err := filepath.Abs("../../../../../..")
	if err != nil {
		t.Fatal(err)
	}
	fixture := filepath.Join(root, "crates", "pixiv-app", "tests", "fixtures", "windows_handler.json")
	body, err := os.ReadFile(fixture)
	if err != nil {
		t.Fatal(err)
	}
	var anchors struct {
		Reference      string `json:"reference"`
		Source         string `json:"source_sha256"`
		DelegateSource string `json:"delegate_source_sha256"`
	}
	if err := json.Unmarshal(body, &anchors); err != nil {
		t.Fatal(err)
	}
	const reference = "4b4426487ef18bed276706daec385e0d0a6979f9"
	if anchors.Reference != reference || anchors.Source != "c7586a3889e5616c8ede815911726886531cc7b5ba755673695f01c4151c2383" || anchors.DelegateSource != "17a4b6801773cbbb5a790f0f51e7dc3d85d02ef36f911394ec1aa0ce66b72e94" {
		t.Fatal("fixture reference anchors differ")
	}
	temp, err := os.MkdirTemp(filepath.Join(root, "internal"), "windows-contract-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(temp)
	baseImport := "github.com/FlanChanXwO/pixiv-cli/internal/" + filepath.Base(temp)
	write := func(path, body string) {
		t.Helper()
		if err := os.WriteFile(path, []byte(body), 0600); err != nil {
			t.Fatal(err)
		}
	}
	for file, hash := range map[string]string{
		"install_windows.go":  anchors.Source,
		"delegate_windows.go": anchors.DelegateSource,
		"persistent_state.go": "db753b648f1fba7f722e8a41856126210a99639ccda174a98cada1b85c8a06e4",
		"endpoint.go":         "91be2a6cd6ffb996559e9fe06097383a54527551cebf1513273890ba15671beb",
	} {
		source, err := os.ReadFile(file)
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(source)) != hash {
			t.Fatalf("%s changed from frozen reference %s", file, reference)
		}
		if file == "delegate_windows.go" {
			continue
		}
		transformed := string(source)
		target := file
		if file == "install_windows.go" {
			if !strings.HasPrefix(transformed, "//go:build windows\n\n") {
				t.Fatal("unexpected build-tag transformation boundary")
			}
			transformed = strings.TrimPrefix(transformed, "//go:build windows\n\n")
			old, replacement := "\"os/exec\"", "exec \""+baseImport+"/mockexec\""
			if strings.Count(transformed, old) != 1 {
				t.Fatal("unexpected import transformation boundary")
			}
			transformed = strings.Replace(transformed, old, replacement, 1)
			target = strings.TrimSuffix(file, "_windows.go") + ".go"
		}
		write(filepath.Join(temp, target), transformed)
	}
	for dir, payload := range map[string]string{
		"mockexec": "migration_windows_mock_payload_test.go",
	} {
		if err := os.Mkdir(filepath.Join(temp, dir), 0700); err != nil {
			t.Fatal(err)
		}
		body, err := os.ReadFile(payload)
		if err != nil {
			t.Fatal(err)
		}
		write(filepath.Join(temp, dir, "mock.go"), strings.TrimPrefix(string(body), "//go:build windows_contract_payload\n\n"))
	}
	body, err = os.ReadFile("migration_windows_contract_payload_test.go")
	if err != nil {
		t.Fatal(err)
	}
	payload := strings.TrimPrefix(string(body), "//go:build windows_contract_payload\n\n")
	payload = strings.ReplaceAll(payload, "MOCK_EXEC_IMPORT", baseImport+"/mockexec")
	write(filepath.Join(temp, "contract_test.go"), payload)
	write(filepath.Join(temp, "native.go"), "package loginhelper\nimport \"context\"\nfunc windowsShellOpenClass(context.Context, string, string) error { panic(\"native Windows shell must not execute\") }\n")
	for _, check := range [][]string{{"test", "-count=1", "-v", "./internal/" + filepath.Base(temp)}, {"vet", "./internal/" + filepath.Base(temp) + "/..."}} {
		cmd := exec.Command("go", check...)
		cmd.Dir = root
		cmd.Env = append(os.Environ(), "WINDOWS_CONTRACT_FIXTURE="+fixture)
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("source-driven Windows contracts %v: %v\n%s", check, err, out)
		}
		t.Logf("source-driven Windows contracts %v passed\n%s", check, out)
	}
}

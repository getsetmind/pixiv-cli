package migration_test

import (
	"bytes"
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

//go:embed browser-native-secret/*.txt
var browserNativeSupport embed.FS

const browserNativeReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type browserNativeSource struct {
	Platform          string   `json:"platform"`
	Path              string   `json:"path"`
	TemporaryPath     string   `json:"temporary_path"`
	OriginalHash      string   `json:"original_sha256"`
	TemporaryHash     string   `json:"temporary_sha256"`
	Substitutions     []string `json:"substitutions"`
	SubstitutionCount int      `json:"substitution_count"`
}

type browserNativeSupportSource struct {
	Path string `json:"path"`
	Hash string `json:"sha256"`
}

type browserNativeFixture struct {
	SchemaVersion          int                          `json:"schema_version"`
	ReferenceCommit        string                       `json:"reference_commit"`
	Verification           string                       `json:"verification"`
	NativeVerification     string                       `json:"native_verification"`
	Sources                []browserNativeSource        `json:"sources"`
	Support                []browserNativeSupportSource `json:"support_sources"`
	DarwinSecretCases      json.RawMessage              `json:"darwin_secret_cases"`
	DarwinMappingCases     json.RawMessage              `json:"darwin_mapping_cases"`
	WindowsDPAPICases      json.RawMessage              `json:"windows_dpapi_cases"`
	WindowsRoutingCases    json.RawMessage              `json:"windows_routing_cases"`
	UnexecutedSizeBoundary string                       `json:"unexecuted_size_boundary"`
}

var browserNativeHashes = map[string]string{
	"internal/browsercookies/browsercookies.go":               "1b1e373c3a240f54d8b0d255ad5ae5b753b2eae0c8e506df1615e3b9190a3e6b",
	"internal/browsercookies/secret/secret.go":                "a273cc81348c6e66e9bbc92d6152a8a657f59474e7624322b68fa0263c723798",
	"internal/browsercookies/secret/darwin.go":                "cbe58d241f11be2e7a0312ee6ddc2857338050ee3cdbc278607f78ba261a3e47",
	"internal/browsercookies/secret/windows.go":               "00cdc3350afabdc27921d7aca15c55dba0330a02bd82b6dc2c204721231ca654",
	"internal/browsercookies/chromium/chromium.go":            "bca8e41df3d25be82fa0fa0a1f85e42c5fa354d4602ccb943bb69845c57bb877",
	"internal/browsercookies/chromium/chromecrypto.go":        "24c0323d3d5bef2e2f0ab6d120391ea622494995374b71515069daab25ea0cf7",
	"internal/browsercookies/chromium/decrypt.go":             "dde205f1b323b991ee0eef6ef73ce72e427981edcc46da3d64dc59224cf52da4",
	"internal/browsercookies/chromium/darwin.go":              "7a4f1b58488b12d362734e4a6c23a3f79daa6644fa138cae14920a155cc71bd3",
	"internal/browsercookies/chromium/windows.go":             "d3d470187901a5f3f60017ab11ef9badf823d687b494873fc170992f59b442f9",
	"internal/browsercookies/chromium/paths_darwin.go":        "ee213f68ad18c12aca424c86b6394fbc17df77d56ac579dec6252c5859ec7690",
	"internal/browsercookies/chromium/paths_windows.go":       "af935dff4b56243f03bcb7a44ab1a8c744dfda8a8d5f38bff37e046de5dc43ea",
	"internal/browsercookies/chromium/legacy_keys_unix.go":    "d37eaa8ee973929ef43f45fbd041e2fd37a4829ee26027c5e6b798100a5a69f7",
	"internal/browsercookies/chromium/legacy_blob_other.go":   "5b7a4339a3ff0a001e63e1b6b4383c788ba84ab30527794c2c6db462cd143275",
	"internal/browsercookies/chromium/legacy_blob_windows.go": "95bb94121cad6da1451e6bb0b39e86de984debc9f8725f0dce7cbe39e32c8257",
	"internal/browsercookies/sqliteio/sqliteio.go":            "d922ed106d3a70cd4c0fa79297edb57cbe9adff051816aa3cc8218aaf4b2c902",
}

func browserNativeDigest(body []byte) string {
	hash := sha256.Sum256(body)
	return hex.EncodeToString(hash[:])
}

func browserNativeWrite(t *testing.T, root, name string, body []byte) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(name))
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, body, 0o600); err != nil {
		t.Fatal(err)
	}
}

func browserNativeGo(t *testing.T, dir string, extra []string, args ...string) {
	t.Helper()
	goTool := filepath.Join(runtime.GOROOT(), "bin", "go")
	cmd := exec.Command(goTool, args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GOWORK=off", "GOTOOLCHAIN=local", "GOPROXY=off", "GOSUMDB=off", "GOFLAGS=", "GOOS=linux", "GOARCH=amd64")
	cmd.Env = append(cmd.Env, extra...)
	body, err := cmd.CombinedOutput()
	t.Logf("source-driven Linux boundary command %s: %s", strings.Join(args, " "), body)
	if err != nil {
		t.Fatalf("owned source-driven Go command failed: %v", err)
	}
}

func browserNativePackage(t *testing.T, repository, temporary, platform string) []browserNativeSource {
	t.Helper()
	paths := []string{
		"internal/browsercookies/browsercookies.go",
		"internal/browsercookies/secret/secret.go",
		"internal/browsercookies/secret/" + platform + ".go",
		"internal/browsercookies/chromium/chromium.go",
		"internal/browsercookies/chromium/chromecrypto.go",
		"internal/browsercookies/chromium/decrypt.go",
		"internal/browsercookies/chromium/" + platform + ".go",
		"internal/browsercookies/chromium/paths_" + platform + ".go",
		"internal/browsercookies/sqliteio/sqliteio.go",
	}
	if platform == "darwin" {
		paths = append(paths, "internal/browsercookies/chromium/legacy_keys_unix.go", "internal/browsercookies/chromium/legacy_blob_other.go")
	} else {
		paths = append(paths, "internal/browsercookies/chromium/legacy_blob_windows.go")
	}
	var sources []browserNativeSource
	for _, name := range paths {
		body, err := os.ReadFile(filepath.Join(repository, filepath.FromSlash(name)))
		if err != nil {
			t.Fatal(err)
		}
		if browserNativeDigest(body) != browserNativeHashes[name] {
			t.Fatalf("frozen production source changed: %s", name)
		}
		cmd := exec.Command("git", "show", browserNativeReference+":"+name)
		cmd.Dir = repository
		frozen, err := cmd.Output()
		if err != nil || !bytes.Equal(body, frozen) {
			t.Fatalf("source does not match the explicit frozen commit: %s", name)
		}
		entry := browserNativeSource{Platform: platform, Path: name, TemporaryPath: name, OriginalHash: browserNativeDigest(body), Substitutions: []string{}}
		if strings.HasSuffix(name, "_"+platform+".go") {
			entry.TemporaryPath = strings.TrimSuffix(name, ".go") + "_source.go"
			entry.Substitutions = append(entry.Substitutions, "remove-filename-build-constraint:"+name+"=>"+entry.TemporaryPath)
		}
		if bytes.HasPrefix(body, []byte("//go:build ")) {
			end := bytes.IndexByte(body, '\n')
			entry.Substitutions = append(entry.Substitutions, "remove-build-tag:"+string(body[:end]))
			body = body[end+1:]
		}
		if name == "internal/browsercookies/secret/windows.go" {
			old := []byte(`"golang.org/x/sys/windows"`)
			if bytes.Count(body, old) != 1 {
				t.Fatal("Windows native import is not the expected unique boundary")
			}
			body = bytes.Replace(body, old, []byte(`windows "github.com/FlanChanXwO/pixiv-cli/internal/browsercookies/nativemock"`), 1)
			entry.Substitutions = append(entry.Substitutions, "replace-native-import:golang.org/x/sys/windows=>internal/browsercookies/nativemock")
		}
		entry.TemporaryHash = browserNativeDigest(body)
		entry.SubstitutionCount = len(entry.Substitutions)
		browserNativeWrite(t, temporary, entry.TemporaryPath, body)
		sources = append(sources, entry)
	}
	return sources
}

func TestMigrationBrowserNativeSecretContract(t *testing.T) {
	if runtime.GOOS != "linux" || runtime.GOARCH != "amd64" {
		t.Fatal("this fixture is explicitly source-driven Linux/amd64 boundary evidence")
	}
	repository, err := filepath.Abs(filepath.Join("..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	fixture := browserNativeFixture{
		SchemaVersion: 1, ReferenceCommit: browserNativeReference,
		Verification:           "actual frozen Go functions executed in owned Linux/amd64 temporary packages; only build-tag/native-import boundary substitutions",
		NativeVerification:     "Darwin/Windows native compile and native runtime unverified; no real Keychain/DPAPI/browser/account/credential/network accessed",
		UnexecutedSizeBoundary: "DPAPI input length above uint32::MAX is source-anchored but not executed; no oversized or forged allocation was made",
	}
	owned := t.TempDir()
	helper := filepath.Join(owned, "helper")
	securitySource, err := browserNativeSupport.ReadFile("browser-native-secret/security_helper.go.txt")
	if err != nil {
		t.Fatal(err)
	}
	browserNativeWrite(t, helper, "go.mod", []byte("module owned-security-helper\n\ngo 1.27.1\n"))
	browserNativeWrite(t, helper, "main.go", securitySource)
	browserNativeGo(t, helper, nil, "build", "-buildvcs=false", "-o", filepath.Join(helper, "security"), ".")
	supportNames := []string{"common_test.go.txt", "darwin_test.go.txt", "windows_test.go.txt", "native_mock.go.txt", "security_helper.go.txt"}
	for _, name := range supportNames {
		body, err := browserNativeSupport.ReadFile("browser-native-secret/" + name)
		if err != nil {
			t.Fatal(err)
		}
		fixture.Support = append(fixture.Support, browserNativeSupportSource{Path: "scripts/tests/migration/browser-native-secret/" + name, Hash: browserNativeDigest(body)})
	}
	for _, platform := range []string{"darwin", "windows"} {
		module := filepath.Join(owned, platform)
		fixture.Sources = append(fixture.Sources, browserNativePackage(t, repository, module, platform)...)
		browserNativeWrite(t, module, "go.mod", []byte("module github.com/FlanChanXwO/pixiv-cli\n\ngo 1.27.1\n"))
		for _, name := range []string{"common", platform} {
			body, err := browserNativeSupport.ReadFile("browser-native-secret/" + name + "_test.go.txt")
			if err != nil {
				t.Fatal(err)
			}
			browserNativeWrite(t, module, "internal/browsercookies/chromium/"+name+"_test.go", body)
		}
		if platform == "windows" {
			body, err := browserNativeSupport.ReadFile("browser-native-secret/native_mock.go.txt")
			if err != nil {
				t.Fatal(err)
			}
			browserNativeWrite(t, module, "internal/browsercookies/nativemock/windows.go", body)
		}
		result := filepath.Join(module, "result.json")
		env := []string{"PIXIV_NATIVE_RESULT=" + result, "PIXIV_OWNED_SECURITY=" + filepath.Join(helper, "security")}
		if os.Getenv("PIXIV_BROWSER_NATIVE_VET") == "1" {
			browserNativeGo(t, module, env, "vet", "./...")
		}
		args := []string{"test", "-buildvcs=false", "-v", "-count=1"}
		if os.Getenv("PIXIV_BROWSER_NATIVE_RACE") == "1" {
			args = append(args, "-race")
		}
		args = append(args, "./internal/browsercookies/chromium")
		browserNativeGo(t, module, env, args...)
		body, err := os.ReadFile(result)
		if err != nil {
			t.Fatal(err)
		}
		var sections map[string]json.RawMessage
		if err := json.Unmarshal(body, &sections); err != nil {
			t.Fatal(err)
		}
		if platform == "darwin" {
			fixture.DarwinSecretCases, fixture.DarwinMappingCases = sections["secret"], sections["mapping"]
		} else {
			fixture.WindowsDPAPICases, fixture.WindowsRoutingCases = sections["dpapi"], sections["routing"]
		}
	}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join(repository, "crates/pixiv-cli/tests/fixtures/browser-native-secret.json")
	if os.Getenv("PIXIV_BROWSER_NATIVE_CAPTURE") == "1" {
		if err := os.WriteFile(path, body, 0o644); err != nil {
			t.Fatal(err)
		}
	} else {
		frozen, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(body, frozen) {
			t.Fatal("source-driven native secret result differs from the captured frozen Go fixture")
		}
	}
}

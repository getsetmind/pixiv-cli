//go:build persistent_startup_contract_payload

package loginhelper

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

var (
	policyOS              string
	policyArgs0           string
	policyExecutablePath  string
	policyExecutableError string
	policyEnsureError     string
	policyCalls           []string
	policyContext         context.Context
)

func policyExecutable() (string, error) {
	policyCalls = append(policyCalls, "executable")
	if policyExecutableError != "" {
		return "", errors.New(policyExecutableError)
	}
	return policyExecutablePath, nil
}
func EnsurePersistent(ctx context.Context) error {
	if ctx != policyContext {
		panic("persistent startup changed context")
	}
	policyCalls = append(policyCalls, "ensure")
	if policyEnsureError != "" {
		return errors.New(policyEnsureError)
	}
	manifest, exists, err := LoadHandlerManifest()
	if err != nil {
		return err
	}
	if !exists {
		manifest = HandlerManifest{Version: 1}
	}
	manifest.ExecutablePath = policyExecutablePath
	return SaveHandlerManifest(manifest)
}

type migrationPersistentPolicyFixture struct {
	StartupPolicy []struct {
		Name      string `json:"name"`
		OS        string `json:"os"`
		Argv0     string `json:"argv0"`
		Supported bool   `json:"supported"`
	} `json:"startup_policy"`
	PersistentPolicy []struct {
		Name            string   `json:"name"`
		OS              string   `json:"os"`
		Argv0           string   `json:"argv0"`
		Executable      string   `json:"executable"`
		ExecutableError string   `json:"executable_error"`
		Manifest        *string  `json:"manifest"`
		EnsureError     string   `json:"ensure_error"`
		Repeats         int      `json:"repeats"`
		Calls           []string `json:"calls"`
		Error           string   `json:"error"`
	} `json:"persistent_policy"`
}

func migrationPersistentFixture(t *testing.T) migrationPersistentPolicyFixture {
	t.Helper()
	body, err := os.ReadFile(os.Getenv("PERSISTENT_STARTUP_CONTRACT_FIXTURE"))
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationPersistentPolicyFixture
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	return fixture
}

func TestPersistentStartupPlatformAndTestBinaryGate(t *testing.T) {
	for _, c := range migrationPersistentFixture(t).StartupPolicy {
		t.Run(c.Name, func(t *testing.T) {
			policyOS, policyArgs0 = c.OS, c.Argv0
			if got := AutomaticPersistentHandlerSupported(); got != c.Supported {
				t.Fatalf("supported=%v want=%v", got, c.Supported)
			}
		})
	}
}

func TestPersistentStartupManifestSkipAndReregisterTransitions(t *testing.T) {
	for _, c := range migrationPersistentFixture(t).PersistentPolicy {
		t.Run(c.Name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			policyOS, policyArgs0, policyExecutablePath, policyExecutableError, policyEnsureError = c.OS, c.Argv0, c.Executable, c.ExecutableError, c.EnsureError
			policyCalls = []string{}
			policyContext = context.WithValue(context.Background(), struct{}{}, "synthetic context")
			manifestPath, err := HandlerManifestPath()
			if err != nil {
				t.Fatal(err)
			}
			if c.Manifest != nil {
				if err := os.MkdirAll(filepath.Dir(manifestPath), 0700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(manifestPath, []byte(*c.Manifest), 0600); err != nil {
					t.Fatal(err)
				}
			}
			for n := 0; n < c.Repeats; n++ {
				message := ""
				if err := EnsurePersistentIfNeeded(policyContext); err != nil {
					message = err.Error()
				}
				if message != c.Error {
					t.Fatalf("invocation %d error=%q want=%q", n, message, c.Error)
				}
			}
			if !reflect.DeepEqual(policyCalls, c.Calls) {
				t.Fatalf("calls=%v want=%v", policyCalls, c.Calls)
			}
			for _, filename := range []string{"config.toml", "pixiv-cli.db"} {
				if _, err := os.Stat(filepath.Join(home, ".pixiv-cli", filename)); !os.IsNotExist(err) {
					t.Fatalf("startup policy unexpectedly created %s: %v", filename, err)
				}
			}
		})
	}
}

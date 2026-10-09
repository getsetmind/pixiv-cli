package auth

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"reflect"
	"testing"
)

type migrationInstallerReader struct{ reads *int }

func (r migrationInstallerReader) Read([]byte) (int, error) {
	*r.reads++
	return 0, errors.New("hidden installer must not read stdin")
}

type migrationInstallerErrorWriter struct{}

func (migrationInstallerErrorWriter) Write([]byte) (int, error) {
	return 0, errors.New("synthetic diagnostic write failure")
}

func TestMigrationHiddenInstallerWarningSuccess(t *testing.T) {
	body, err := os.ReadFile("../../../../../crates/pixiv-cli/tests/fixtures/cli_login_hidden_startup.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Installer []struct {
			Name               string   `json:"name"`
			Args               []string `json:"args"`
			EnsureError        string   `json:"ensure_error"`
			ErrorOutputFailure bool     `json:"error_output_failure"`
			Stdout             string   `json:"stdout"`
			Stderr             string   `json:"stderr"`
			Error              string   `json:"error"`
			Calls              []string `json:"calls"`
			StdinReads         int      `json:"stdin_reads"`
		} `json:"installer"`
	}
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	for _, c := range fixture.Installer {
		t.Run(c.Name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			calls := []string{}
			reads := 0
			ctx := context.WithValue(context.Background(), struct{}{}, "synthetic context")
			loginHooksMu.Lock()
			old := ensureURLSchemeRelay
			ensureURLSchemeRelay = func(got context.Context) error {
				if got != ctx {
					t.Fatal("installer changed context")
				}
				calls = append(calls, "ensure")
				if c.EnsureError != "" {
					return errors.New(c.EnsureError)
				}
				return nil
			}
			loginHooksMu.Unlock()
			t.Cleanup(func() { loginHooksMu.Lock(); ensureURLSchemeRelay = old; loginHooksMu.Unlock() })
			var out, diagnostics bytes.Buffer
			var errorOutput io.Writer = &diagnostics
			if c.ErrorOutputFailure {
				errorOutput = migrationInstallerErrorWriter{}
			}
			a := controller{in: migrationInstallerReader{reads: &reads}, out: &out, errOut: errorOutput}
			cmd := a.newAccountURLHandlerInstallCommand()
			cmd.SetOut(&out)
			cmd.SetErr(&diagnostics)
			cmd.SilenceErrors = true
			cmd.SilenceUsage = true
			cmd.SetArgs(c.Args)
			err := cmd.ExecuteContext(ctx)
			message := ""
			if err != nil {
				message = err.Error()
			}
			if out.String() != c.Stdout || diagnostics.String() != c.Stderr || message != c.Error || reads != c.StdinReads || !reflect.DeepEqual(calls, c.Calls) {
				t.Fatalf("output=%q stderr=%q error=%q reads=%d calls=%v", out.String(), diagnostics.String(), message, reads, calls)
			}
		})
	}
}

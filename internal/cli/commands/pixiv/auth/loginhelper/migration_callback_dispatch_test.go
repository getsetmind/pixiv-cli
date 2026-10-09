package loginhelper

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"
)

type migrationCallbackDispatchFixture struct {
	Paths     map[string]string                             `json:"paths"`
	Endpoints []struct{ Name, Input, Output, Error string } `json:"endpoints"`
	Dispatch  []struct {
		Name, Input, Local, Remote, Result, Error string
		DelegateError                             string   `json:"delegate_error"`
		Calls                                     []string `json:"calls"`
	} `json:"dispatch"`
	PathInputs []struct {
		Name, Home, Output, Error string
		Unset                     bool
	} `json:"path_inputs"`
	Relay []struct{ Name, Endpoint, Callback, Output string } `json:"relay"`
}

func migrationCallbackFixture(t *testing.T) migrationCallbackDispatchFixture {
	t.Helper()
	body, err := os.ReadFile("../../../../../../crates/pixiv-app/tests/fixtures/callback_dispatch.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationCallbackDispatchFixture
	if err := json.Unmarshal(body, &fixture); err != nil {
		t.Fatal(err)
	}
	return fixture
}

func TestMigrationCallbackEndpointValidation(t *testing.T) {
	for _, c := range migrationCallbackFixture(t).Endpoints {
		t.Run(c.Name, func(t *testing.T) {
			got, err := validatedCallbackEndpoint(c.Input)
			message := ""
			if err != nil {
				message = err.Error()
			}
			if got != c.Output || message != c.Error {
				t.Fatalf("endpoint=%q error=%q, want %q %q", got, message, c.Output, c.Error)
			}
		})
	}
}

func TestMigrationCallbackDefaultPathsAndRelay(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("APPDATA", t.TempDir())
	paths := map[string]func() (string, error){"endpoint": CallbackEndpointPath, "manifest": HandlerManifestPath, "active": ActiveRemoteLoginPath}
	for name, relative := range migrationCallbackFixture(t).Paths {
		got, err := paths[name]()
		if err != nil || got != filepath.Join(home, filepath.FromSlash(relative)) {
			t.Fatalf("%s path=%q err=%v", name, got, err)
		}
	}
	path, err := CallbackEndpointPath()
	if err != nil {
		t.Fatal(err)
	}
	const raw = " \tpixiv://account/login?code=synthetic&state=two#original \n"
	if _, err := CallbackRelayURL(raw); !errors.Is(err, ErrNoActiveLocalCallback) {
		t.Fatal(err)
	}
	if _, err := CallbackRelayURL("https://invalid.example"); err == nil || err.Error() != "invalid Pixiv callback URL" {
		t.Fatal(err)
	}
	written, err := WriteCallbackEndpoint(" \thttp://127.0.0.1:41871/callback\n")
	if err != nil || written != path {
		t.Fatalf("%q %v", written, err)
	}
	body, err := os.ReadFile(path)
	if err != nil || string(body) != "http://127.0.0.1:41871/callback\n" {
		t.Fatalf("%q %v", body, err)
	}
	if runtime.GOOS != "windows" {
		for file, mode := range map[string]os.FileMode{path: 0600, filepath.Dir(path): 0700} {
			info, err := os.Stat(file)
			if err != nil || info.Mode().Perm() != mode {
				t.Fatalf("mode for %s: %v %v", file, info, err)
			}
		}
	}
	relay, err := CallbackRelayURL(raw)
	if err != nil {
		t.Fatal(err)
	}
	parsed, err := url.Parse(relay)
	if err != nil || parsed.Fragment != raw || parsed.RawQuery != "" {
		t.Fatalf("relay=%q err=%v", relay, err)
	}
	if _, err := WriteCallbackEndpoint("https://invalid.example/callback"); err == nil {
		t.Fatal("invalid endpoint accepted")
	}
	unchanged, err := os.ReadFile(path)
	if err != nil || string(unchanged) != string(body) {
		t.Fatalf("invalid write replaced endpoint: %q %v", unchanged, err)
	}
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(path, 0700); err != nil {
		t.Fatal(err)
	}
	if _, err := CallbackRelayURL(raw); err == nil || err.Error() != "could not read Pixiv login callback endpoint" {
		t.Fatal(err)
	}
}

func TestMigrationCallbackDispatchStopsAndSentinels(t *testing.T) {
	for _, c := range migrationCallbackFixture(t).Dispatch {
		t.Run(c.Name, func(t *testing.T) {
			ctx := context.WithValue(context.Background(), struct{}{}, "synthetic context")
			calls := []string{}
			check := func(gotCtx context.Context, raw string, name string) {
				if gotCtx != ctx || raw != c.Input {
					t.Fatalf("%s changed context or raw URL", name)
				}
				calls = append(calls, name)
			}
			t.Cleanup(SetCallbackRelayURLForHandler(func(raw string) (string, error) {
				check(ctx, raw, "local")
				switch c.Local {
				case "inactive":
					return "", ErrNoActiveLocalCallback
				case "wrapped_inactive":
					return "", fmt.Errorf("synthetic wrapped: %w", ErrNoActiveLocalCallback)
				case "error":
					return "", errors.New("synthetic local failure")
				default:
					return "http://127.0.0.1:9/callback", nil
				}
			}))
			remote := &RemoteCallbackSession{ResultURL: "https://relay.example/result/synthetic"}
			t.Cleanup(SetForwardActiveRemoteCallbackForHandler(func(gotCtx context.Context, raw string) (*RemoteCallbackSession, error) {
				check(gotCtx, raw, "remote")
				switch c.Remote {
				case "inactive":
					return nil, ErrNoActiveRemoteLogin
				case "wrapped_inactive":
					return nil, fmt.Errorf("synthetic wrapped: %w", ErrNoActiveRemoteLogin)
				case "error":
					return nil, errors.New("synthetic remote failure")
				default:
					return remote, nil
				}
			}))
			t.Cleanup(SetDelegateToPreviousForHandler(func(gotCtx context.Context, raw string) error {
				check(gotCtx, raw, "delegate")
				if c.DelegateError != "" {
					return errors.New(c.DelegateError)
				}
				return nil
			}))
			got, err := HandleCallback(ctx, c.Input)
			message := ""
			if err != nil {
				message = err.Error()
			}
			if message != c.Error || !reflect.DeepEqual(calls, c.Calls) {
				t.Fatalf("calls=%v error=%q, want %v %q", calls, message, c.Calls, c.Error)
			}
			if c.Result == "remote" {
				if got.RemoteCallback != remote || got.LocalRelayURL != "" || got.RemoteLoginStart != nil {
					t.Fatalf("result=%+v", got)
				}
			} else if c.Result == "local" {
				if got.LocalRelayURL != "http://127.0.0.1:9/callback" || got.RemoteCallback != nil || got.RemoteLoginStart != nil {
					t.Fatalf("result=%+v", got)
				}
			} else if c.Result == "start" {
				want := &RemoteLoginStart{Origin: "https://relay.example", SessionID: "synthetic-session", Proof: "synthetic-proof"}
				if !reflect.DeepEqual(got.RemoteLoginStart, want) || got.LocalRelayURL != "" || got.RemoteCallback != nil {
					t.Fatalf("result=%+v", got)
				}
			} else if got.LocalRelayURL != "" || got.RemoteCallback != nil || got.RemoteLoginStart != nil {
				t.Fatalf("unexpected result=%+v", got)
			}
		})
	}
}

func TestMigrationCallbackHomePathInputs(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("HOME diagnostics are Unix-specific; Windows uses USERPROFILE")
	}
	for _, c := range migrationCallbackFixture(t).PathInputs {
		t.Run(c.Name, func(t *testing.T) {
			t.Setenv("HOME", c.Home)
			if c.Unset {
				if err := os.Unsetenv("HOME"); err != nil {
					t.Fatal(err)
				}
			}
			got, err := CallbackEndpointPath()
			message := ""
			if err != nil {
				message = err.Error()
			}
			if got != c.Output || message != c.Error {
				t.Fatalf("path=%q error=%q want %q %q", got, message, c.Output, c.Error)
			}
		})
	}
}

func TestMigrationCallbackOriginalFragmentSerialization(t *testing.T) {
	for _, c := range migrationCallbackFixture(t).Relay {
		t.Run(c.Name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			if _, err := WriteCallbackEndpoint(c.Endpoint); err != nil {
				t.Fatal(err)
			}
			got, err := CallbackRelayURL(c.Callback)
			if err != nil || got != c.Output {
				t.Fatalf("relay=%q error=%v want %q", got, err, c.Output)
			}
		})
	}
}

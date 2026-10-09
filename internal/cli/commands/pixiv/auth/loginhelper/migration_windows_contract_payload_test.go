//go:build windows_contract_payload

package loginhelper

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	exec "MOCK_EXEC_IMPORT"
)

type windowsFlowCase struct {
	Name, Operation, Current, Failure, Error string
	ErrorKind                                string `json:"error_kind"`
	Previous                                 bool
	InitialManifest                          bool `json:"initial_manifest"`
	Trace                                    []string
	ManifestExists                           bool   `json:"manifest_exists"`
	EndpointExists                           bool   `json:"endpoint_exists"`
	CleanupCount                             int    `json:"cleanup_count"`
	EndpointReplacement                      string `json:"endpoint_replacement"`
	CurrentTree                              string `json:"current_tree"`
	PreviousTree                             bool   `json:"previous_tree"`
}
type windowsFixture struct {
	RegistryKey         string `json:"registry_key"`
	PreviousRegistryKey string `json:"previous_registry_key"`
	PreviousProgID      string `json:"previous_prog_id"`
	Executable          string
	CallbackURL         string                                       `json:"callback_url"`
	EndpointURL         string                                       `json:"endpoint_url"`
	CommandCases        []struct{ Name, Executable, Command string } `json:"command_cases"`
	KeyCases            []struct {
		Name, Failure, Error string
		Exists               bool
	} `json:"key_cases"`
	OwnerCases []struct {
		Name, Output, Failure, Error string
		Ours                         bool
	} `json:"owner_cases"`
	Cases []windowsFlowCase
}

func readWindowsFixture(t *testing.T) windowsFixture {
	t.Helper()
	body, err := os.ReadFile(os.Getenv("WINDOWS_CONTRACT_FIXTURE"))
	if err != nil {
		t.Fatal(err)
	}
	var f windowsFixture
	if err := json.Unmarshal(body, &f); err != nil {
		t.Fatal(err)
	}
	return f
}
func mustWindows(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}
func windowsExists(path string) bool { _, err := os.Stat(path); return err == nil }
func windowsError(t *testing.T, err error, expected, kind string) {
	t.Helper()
	if kind == "filesystem" {
		var pathError *os.PathError
		var linkError *os.LinkError
		if !errors.As(err, &pathError) && !errors.As(err, &linkError) {
			t.Fatalf("expected filesystem PathError or LinkError, got %v", err)
		}
		return
	}
	actual := ""
	if err != nil {
		actual = err.Error()
	}
	if actual != expected {
		t.Fatalf("error %q, expected %q", actual, expected)
	}
}
func frozenExit(failure string) error {
	switch failure {
	case "":
		return nil
	case "exit":
		return &exec.ExitError{Message: "synthetic exit"}
	case "wrapped-exit":
		return fmt.Errorf("wrapped: %w", &exec.ExitError{Message: "synthetic exit"})
	default:
		return errors.New("synthetic native")
	}
}
func TestFrozenWindowsCommands(t *testing.T) {
	f := readWindowsFixture(t)
	if f.RegistryKey != defaultWindowsURLHandlerRegistryKey || f.PreviousRegistryKey != previousWindowsURLHandlerRegistryKey || f.PreviousProgID != PreviousWindowsURLHandlerProgID {
		t.Fatal("registry constants differ")
	}
	for _, c := range f.CommandCases {
		t.Run(c.Name, func(t *testing.T) {
			if got := WindowsURLHandlerCommand(c.Executable); got != c.Command {
				t.Fatalf("command %q, expected %q", got, c.Command)
			}
		})
	}
}
func TestFrozenWindowsRegistryKeyAbsence(t *testing.T) {
	f := readWindowsFixture(t)
	for _, c := range f.KeyCases {
		t.Run(c.Name, func(t *testing.T) {
			exec.RunCommand = func(_ context.Context, name string, args []string, mode string) ([]byte, error) {
				if name != "reg.exe" || mode != "run" || !reflect.DeepEqual(args, []string{"query", f.RegistryKey}) {
					t.Fatalf("key command %s %s %v", name, mode, args)
				}
				return nil, frozenExit(c.Failure)
			}
			present, err := windowsRegistryKeyExists(context.Background(), f.RegistryKey)
			windowsError(t, err, c.Error, "")
			if present != c.Exists {
				t.Fatal("key existence differs")
			}
		})
	}
}
func TestFrozenWindowsOwnerQuery(t *testing.T) {
	f := readWindowsFixture(t)
	for _, c := range f.OwnerCases {
		t.Run(c.Name, func(t *testing.T) {
			exec.RunCommand = func(_ context.Context, name string, args []string, mode string) ([]byte, error) {
				if name != "reg.exe" || mode != "output" || !reflect.DeepEqual(args, []string{"query", f.RegistryKey + "\\shell\\open\\command", "/ve"}) {
					t.Fatalf("owner command %s %s %v", name, mode, args)
				}
				return []byte(c.Output), frozenExit(c.Failure)
			}
			ours, err := windowsDefaultHandlerIsOurs(context.Background())
			windowsError(t, err, c.Error, "")
			if ours != c.Ours {
				t.Fatal("owner detection differs")
			}
		})
	}
}

type frozenWindowsTree struct {
	Kind   string
	Values map[string]string
}

func originalWindowsTree() *frozenWindowsTree {
	return &frozenWindowsTree{Kind: "previous", Values: map[string]string{
		"": "previous protocol", "previous\\marker": "keep",
		"shell\\open\\command": "\"old.exe\" \"%1\"",
	}}
}
func cloneWindowsTree(tree *frozenWindowsTree) *frozenWindowsTree {
	if tree == nil {
		return nil
	}
	copy := &frozenWindowsTree{Kind: tree.Kind, Values: map[string]string{}}
	for key, value := range tree.Values {
		copy.Values[key] = value
	}
	return copy
}

type windowsRequestContextKey struct{}

func TestFrozenWindowsFlows(t *testing.T) {
	f := readWindowsFixture(t)
	for _, c := range f.Cases {
		t.Run(c.Name, func(t *testing.T) {
			home, temporary := t.TempDir(), t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("TMPDIR", temporary)
			manifestPath, err := HandlerManifestPath()
			mustWindows(t, err)
			endpoint, err := CallbackEndpointPath()
			mustWindows(t, err)
			trace := []string{}
			var current, previous, exported *frozenWindowsTree
			if c.Previous {
				current = originalWindowsTree()
			}
			if c.InitialManifest {
				m := HandlerManifest{Version: 1, ExecutablePath: "stale executable"}
				if c.Previous {
					m.PreviousHandler = f.PreviousProgID
					previous = originalWindowsTree()
				}
				mustWindows(t, SaveHandlerManifest(m))
				if c.Operation == "disable" {
					switch c.Current {
					case "ours":
						current = &frozenWindowsTree{Kind: "ours", Values: map[string]string{"shell\\open\\command": WindowsURLHandlerCommand(f.Executable)}}
					case "other":
						current = &frozenWindowsTree{Kind: "other", Values: map[string]string{"shell\\open\\command": "\"other.exe\" \"%1\""}}
					case "absent":
						current = nil
					}
				}
			}
			if c.Failure == "invalid-manifest" {
				mustWindows(t, os.MkdirAll(filepath.Dir(manifestPath), 0700))
				mustWindows(t, os.WriteFile(manifestPath, []byte("{invalid"), 0600))
			}
			if c.Failure == "tempdir" {
				blocked := filepath.Join(temporary, "blocked")
				mustWindows(t, os.WriteFile(blocked, []byte("untouched"), 0600))
				t.Setenv("TMPDIR", blocked)
			}
			t.Cleanup(SetWindowsExecutablePath(func() (string, error) {
				if c.Failure == "resolve" {
					return "", errors.New("synthetic executable")
				}
				return f.Executable, nil
			}))
			t.Cleanup(SetOpenWindowsPreviousClass(func(_ context.Context, class, rawURL string) error {
				trace = append(trace, "open")
				if class != f.PreviousProgID || rawURL != f.CallbackURL {
					t.Fatalf("delegate changed class %q or raw URL %q", class, rawURL)
				}
				if c.Failure == "open" {
					return errors.New("private-code shell diagnostic")
				}
				return nil
			}))
			backupPath := ""
			exec.RunCommand = func(ctx context.Context, name string, args []string, mode string) ([]byte, error) {
				if name != "reg.exe" || len(args) == 0 {
					t.Fatalf("unexpected process %s %s %v", name, mode, args)
				}
				label := ""
				var expected []string
				switch args[0] {
				case "query":
					if mode == "output" {
						label, expected = "query-owner", []string{"query", f.RegistryKey + "\\shell\\open\\command", "/ve"}
					} else {
						label, expected = "query", []string{"query", f.RegistryKey}
					}
				case "copy":
					if args[1] == f.RegistryKey {
						label, expected = "copy-previous", []string{"copy", f.RegistryKey, f.PreviousRegistryKey, "/s", "/f"}
					} else {
						label, expected = "restore-previous", []string{"copy", f.PreviousRegistryKey, f.RegistryKey, "/s", "/f"}
					}
				case "add":
					if args[1] != f.RegistryKey {
						label, expected = "add-command", []string{"add", f.RegistryKey + "\\shell\\open\\command", "/ve", "/t", "REG_SZ", "/d", WindowsURLHandlerCommand(f.Executable), "/f"}
					} else if args[2] == "/ve" {
						label, expected = "add-protocol", []string{"add", f.RegistryKey, "/ve", "/t", "REG_SZ", "/d", "URL:Pixiv Protocol", "/f"}
					} else {
						label, expected = "add-url", []string{"add", f.RegistryKey, "/v", "URL Protocol", "/t", "REG_SZ", "/d", "", "/f"}
					}
				case "delete":
					label = "delete-current"
					if args[1] == f.PreviousRegistryKey {
						label = "delete-previous"
					}
					expected = []string{"delete", args[1], "/f"}
					if args[1] != f.RegistryKey && args[1] != f.PreviousRegistryKey {
						t.Fatal("unexpected delete key")
					}
				case "export":
					label, backupPath = "export", args[2]
					if filepath.Dir(filepath.Dir(backupPath)) != temporary || !strings.HasPrefix(filepath.Base(filepath.Dir(backupPath)), "pixiv-cli-url-handler-registry-") || filepath.Base(backupPath) != "pixiv-url-handler.reg" {
						t.Fatal("backup is not a private random path")
					}
					expected = []string{"export", f.RegistryKey, backupPath, "/y"}
				case "import":
					label, expected = "import", []string{"import", backupPath}
				default:
					t.Fatalf("unexpected registry verb %v", args)
				}
				expectedMode := "run"
				if label == "query-owner" {
					expectedMode = "output"
				}
				if mode != expectedMode || !reflect.DeepEqual(args, expected) {
					t.Fatalf("%s command %s %v, expected %s %v", label, mode, args, expectedMode, expected)
				}
				trace = append(trace, label)
				if c.Operation == "install" && (label == "delete-current" || label == "import") {
					if ctx.Err() != nil || ctx.Value(windowsRequestContextKey{}) != nil {
						t.Fatal("restoration must use fresh background context")
					}
				} else if ctx.Value(windowsRequestContextKey{}) != true {
					t.Fatal("request context was not propagated")
				}
				if backupPath != "" && label != "export" && windowsExists(backupPath) {
					for path, mode := range map[string]os.FileMode{backupPath: 0600, filepath.Dir(backupPath): 0700} {
						info, err := os.Stat(path)
						mustWindows(t, err)
						if info.Mode().Perm() != mode {
							t.Fatalf("backup permissions %s %o", path, info.Mode().Perm())
						}
					}
				}
				if label == c.Failure {
					return nil, errors.New("synthetic " + label)
				}
				switch label {
				case "query", "query-owner":
					if current == nil {
						return nil, &exec.ExitError{Message: "synthetic absent key"}
					}
					if label == "query-owner" {
						return []byte(current.Values["shell\\open\\command"]), nil
					}
				case "copy-previous":
					previous = cloneWindowsTree(current)
				case "restore-previous":
					current = cloneWindowsTree(previous)
				case "add-protocol", "add-url", "add-command":
					if current == nil {
						current = &frozenWindowsTree{Values: map[string]string{}}
					}
					current.Kind = "ours"
					key, value := "", "URL:Pixiv Protocol"
					if label == "add-url" {
						key, value = "URL Protocol", ""
					} else if label == "add-command" {
						key, value = "shell\\open\\command", WindowsURLHandlerCommand(f.Executable)
					}
					current.Values[key] = value
					if c.Failure == "save" && label == "add-command" {
						mustWindows(t, os.MkdirAll(manifestPath, 0700))
						mustWindows(t, os.WriteFile(filepath.Join(manifestPath, "blocked"), nil, 0600))
					}
				case "delete-current":
					current = nil
				case "delete-previous":
					previous = nil
				case "export":
					info, err := os.Stat(filepath.Dir(backupPath))
					mustWindows(t, err)
					if info.Mode().Perm() != 0700 {
						t.Fatal("backup directory is not private at export")
					}
					exported = cloneWindowsTree(current)
					if c.Failure != "export-no-file" {
						body, err := json.Marshal(exported)
						mustWindows(t, err)
						mustWindows(t, os.WriteFile(backupPath, body, 0644))
					}
				case "import":
					body, err := os.ReadFile(backupPath)
					if err != nil {
						return nil, &exec.ExitError{Message: "synthetic missing backup"}
					}
					var tree frozenWindowsTree
					mustWindows(t, json.Unmarshal(body, &tree))
					if !reflect.DeepEqual(&tree, exported) {
						t.Fatal("registry backup lost nested tree")
					}
					current = cloneWindowsTree(&tree)
				}
				return nil, nil
			}
			ctx, cancel := context.WithCancel(context.WithValue(context.Background(), windowsRequestContextKey{}, true))
			defer cancel()
			if c.Operation == "repeat" {
				mustWindows(t, EnsurePersistent(ctx))
				manifest, present, err := LoadHandlerManifest()
				mustWindows(t, err)
				if !present {
					t.Fatal("first ensure did not save manifest")
				}
				manifest.ExecutablePath = "stale executable"
				mustWindows(t, SaveHandlerManifest(manifest))
				trace = []string{}
			}
			switch c.Operation {
			case "ensure", "repeat":
				err = EnsurePersistent(ctx)
			case "disable":
				err = DisablePersistent(ctx)
			case "delegate":
				err = DelegateToPrevious(ctx, f.CallbackURL)
			case "install":
				var cleanup func()
				cleanup, err = Install(ctx, f.EndpointURL)
				if err == nil {
					if cleanup == nil {
						t.Fatal("cleanup missing")
					}
					body, e := os.ReadFile(endpoint)
					mustWindows(t, e)
					if string(body) != f.EndpointURL+"\n" {
						t.Fatal("endpoint changed")
					}
					info, e := os.Stat(endpoint)
					mustWindows(t, e)
					if info.Mode().Perm() != 0600 {
						t.Fatal("endpoint is not private")
					}
					if c.EndpointReplacement != "" {
						mustWindows(t, os.Remove(endpoint))
						mustWindows(t, os.Mkdir(endpoint, 0700))
						if c.EndpointReplacement == "nonempty-directory" {
							mustWindows(t, os.WriteFile(filepath.Join(endpoint, "kept"), []byte("untouched"), 0600))
						}
					}
					cancel()
					count := c.CleanupCount
					if count == 0 {
						count = 1
					}
					for range count {
						cleanup()
					}
				} else if cleanup != nil {
					t.Fatal("failure returned cleanup")
				}
			default:
				t.Fatal("unknown fixture operation")
			}
			windowsError(t, err, c.Error, c.ErrorKind)
			if !reflect.DeepEqual(trace, c.Trace) {
				t.Fatalf("trace %v, expected %v", trace, c.Trace)
			}
			if windowsExists(manifestPath) != c.ManifestExists || windowsExists(endpoint) != c.EndpointExists {
				t.Fatalf("manifest/endpoint lifetime %v/%v, expected %v/%v", windowsExists(manifestPath), windowsExists(endpoint), c.ManifestExists, c.EndpointExists)
			}
			kind := "absent"
			if current != nil {
				kind = current.Kind
			}
			if kind != c.CurrentTree || (previous != nil) != c.PreviousTree {
				t.Fatalf("tree residues current=%s previous=%v, expected %s/%v", kind, previous != nil, c.CurrentTree, c.PreviousTree)
			}
			if current != nil && kind == "previous" && !reflect.DeepEqual(current, originalWindowsTree()) {
				t.Fatal("original nested current registry tree changed")
			}
			if previous != nil && !reflect.DeepEqual(previous, originalWindowsTree()) {
				t.Fatal("original nested private registry tree changed")
			}
			if c.ManifestExists && c.Failure != "save" && c.Failure != "invalid-manifest" {
				manifest, present, e := LoadHandlerManifest()
				mustWindows(t, e)
				if !present || (c.Previous && manifest.PreviousHandler != f.PreviousProgID) || (!c.Previous && manifest.PreviousHandler != "") {
					t.Fatalf("previous manifest class %+v", manifest)
				}
				if err == nil && (c.Operation == "ensure" || c.Operation == "repeat") && manifest.ExecutablePath != f.Executable {
					t.Fatal("manifest executable was not refreshed")
				}
				body, e := os.ReadFile(manifestPath)
				mustWindows(t, e)
				if strings.Contains(string(body), "private-code") || strings.Contains(string(body), f.EndpointURL) {
					t.Fatal("manifest contains authentication state")
				}
				info, e := os.Stat(manifestPath)
				mustWindows(t, e)
				if info.Mode().Perm() != 0600 {
					t.Fatal("manifest is not private")
				}
			}
			if c.EndpointExists {
				body, e := os.ReadFile(filepath.Join(endpoint, "kept"))
				mustWindows(t, e)
				if string(body) != "untouched" {
					t.Fatal("replacement endpoint contents changed")
				}
			}
			residue, e := filepath.Glob(filepath.Join(temporary, "pixiv-cli-url-handler-registry-*"))
			mustWindows(t, e)
			if len(residue) != 0 {
				t.Fatalf("registry backup residue %v", residue)
			}
		})
	}
}

func TestFrozenWindowsNativeExecutableBytesAndManifestJSON(t *testing.T) {
	f := readWindowsFixture(t)
	invalid := string([]byte{0xed, 0xa0, 0x80, 0xff})
	executable := "C:\\path\\" + invalid + "\"quoted\"\\pixiv.exe"
	expectedCommand := "\"C:\\path\\" + invalid + "\\\"quoted\\\"\\pixiv.exe\" auth _callback \"%1\""
	expectedManifestExecutable := "C:\\path\\" + "\ufffd\ufffd\ufffd\ufffd" + "\"quoted\"\\pixiv.exe"
	for _, operation := range []string{"ensure", "install"} {
		t.Run(operation, func(t *testing.T) {
			home, temporary := t.TempDir(), t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("TMPDIR", temporary)
			t.Cleanup(SetWindowsExecutablePath(func() (string, error) { return executable, nil }))
			var commands [][]string
			exec.RunCommand = func(_ context.Context, name string, args []string, mode string) ([]byte, error) {
				if name != "reg.exe" || mode != "run" {
					t.Fatalf("native executable process %s %s", name, mode)
				}
				commands = append(commands, append([]string(nil), args...))
				if reflect.DeepEqual(args, []string{"query", f.RegistryKey}) {
					return nil, &exec.ExitError{Message: "synthetic absent key"}
				}
				return nil, nil
			}
			switch operation {
			case "ensure":
				mustWindows(t, EnsurePersistent(context.Background()))
				manifest, present, err := LoadHandlerManifest()
				mustWindows(t, err)
				if !present || manifest.ExecutablePath != expectedManifestExecutable {
					t.Fatalf("manifest executable bytes %x, expected %x", manifest.ExecutablePath, expectedManifestExecutable)
				}
				path, err := HandlerManifestPath()
				mustWindows(t, err)
				body, err := os.ReadFile(path)
				mustWindows(t, err)
				if strings.Count(string(body), "\ufffd") != 4 || strings.Contains(string(body), invalid) {
					t.Fatalf("manifest JSON must replace each invalid byte: %s", body)
				}
			case "install":
				cleanup, err := Install(context.Background(), f.EndpointURL)
				mustWindows(t, err)
				if cleanup == nil {
					t.Fatal("cleanup missing")
				}
				cleanup()
				path, err := HandlerManifestPath()
				mustWindows(t, err)
				if windowsExists(path) {
					t.Fatal("temporary installation wrote persistent manifest")
				}
				endpoint, err := CallbackEndpointPath()
				mustWindows(t, err)
				if windowsExists(endpoint) {
					t.Fatal("temporary cleanup retained endpoint")
				}
				residue, err := filepath.Glob(filepath.Join(temporary, "pixiv-cli-url-handler-registry-*"))
				mustWindows(t, err)
				if len(residue) != 0 {
					t.Fatal("temporary cleanup retained private registry backup")
				}
			}
			expected := [][]string{
				{"query", f.RegistryKey},
				{"add", f.RegistryKey, "/ve", "/t", "REG_SZ", "/d", "URL:Pixiv Protocol", "/f"},
				{"add", f.RegistryKey, "/v", "URL Protocol", "/t", "REG_SZ", "/d", "", "/f"},
				{"add", f.RegistryKey + "\\shell\\open\\command", "/ve", "/t", "REG_SZ", "/d", expectedCommand, "/f"},
			}
			if operation == "install" {
				expected = append(expected, []string{"delete", f.RegistryKey, "/f"})
			}
			if !reflect.DeepEqual(commands, expected) {
				t.Fatalf("registry native executable argv %q, expected %q", commands, expected)
			}
		})
	}
}

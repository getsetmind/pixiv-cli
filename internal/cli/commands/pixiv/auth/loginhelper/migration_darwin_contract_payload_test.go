//go:build darwin_contract_payload

package loginhelper

import (
	exec "MOCK_IMPORT"
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strconv"
	"strings"
	"testing"
)

type flowCase struct {
	Name, Operation, Previous, Current, Failure, Error string
	Trace                                              []string
	ManifestExists                                     bool `json:"manifest_exists"`
	EndpointExists                                     bool `json:"endpoint_exists"`
}
type cacheCase struct {
	Name, Kind string
	Compile    bool
}
type scriptCase struct {
	Scheme      string
	BundleID    string `json:"bundle_id"`
	QueryScript string `json:"query_script"`
	SetScript   string `json:"set_script"`
	Error       string
}
type fixture struct {
	CompilerCacheCases        []cacheCase     `json:"compiler_cache_cases"`
	NativeScriptCases         []scriptCase    `json:"native_script_cases"`
	BundleManifestZeroVersion json.RawMessage `json:"bundle_manifest_zero_version"`
	BundleID                  string          `json:"bundle_id"`
	SourceVersion             string          `json:"source_version"`
	AppRelativePath           string          `json:"app_relative_path"`
	CallbackURL               string          `json:"callback_url"`
	EndpointURL               string          `json:"endpoint_url"`
	Plist                     string
	SwiftSource               string `json:"swift_source"`
	Cases                     []flowCase
}

func readFixture(t *testing.T) fixture {
	t.Helper()
	b, e := os.ReadFile(os.Getenv("DARWIN_CONTRACT_FIXTURE"))
	if e != nil {
		t.Fatal(e)
	}
	var f fixture
	if e = json.Unmarshal(b, &f); e != nil {
		t.Fatal(e)
	}
	return f
}
func must(t *testing.T, e error) {
	t.Helper()
	if e != nil {
		t.Fatal(e)
	}
}
func exists(path string) bool { _, e := os.Stat(path); return e == nil }
func TestFrozenAssets(t *testing.T) {
	f := readFixture(t)
	if f.BundleID != PixivURLHandlerBundleID || f.SourceVersion != PixivURLHandlerSourceVersion || f.Plist != pixivURLHandlerInfoPlist || f.SwiftSource != PixivURLHandlerSwiftSource {
		t.Fatal("frozen assets differ")
	}
}
func TestFrozenFlows(t *testing.T) {
	f := readFixture(t)
	for _, c := range f.Cases {
		t.Run(c.Name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			t.Setenv("USERPROFILE", home)
			t.Setenv("TMPDIR", t.TempDir())
			trace := []string{}
			current := c.Current
			app, e := PixivURLHandlerAppPath()
			must(t, e)
			if app != filepath.Join(home, f.AppRelativePath) {
				t.Fatalf("app path %q", app)
			}
			endpoint, e := CallbackEndpointPath()
			must(t, e)
			manifestPath, e := HandlerManifestPath()
			must(t, e)
			exec.Find = func(name string) (string, error) {
				trace = append(trace, "find:"+name)
				if c.Failure == name {
					return "", errors.New("synthetic missing " + name)
				}
				return name, nil
			}
			exec.RunCommand = func(name string, args []string, mode string) ([]byte, error) {
				switch {
				case name == "swiftc":
					trace = append(trace, "compile")
					if mode != "combined" || len(args) != 3 || args[1] != "-o" {
						t.Fatalf("compiler command %s %v", mode, args)
					}
					b, e := os.ReadFile(args[0])
					must(t, e)
					if string(b) != f.SwiftSource {
						t.Fatal("source content")
					}
					must(t, os.WriteFile(args[2], []byte("mock executable"), 0700))
					return nil, nil
				case strings.HasSuffix(name, "/lsregister"):
					trace = append(trace, "register")
					if mode != "combined" || !reflect.DeepEqual(args, []string{"-f", app}) {
						t.Fatalf("register command %s %v", mode, args)
					}
					if strings.HasPrefix(c.Operation, "ensure") || c.Operation == "repeat" {
						b, e := os.ReadFile(handlerBundleManifestPath(app))
						must(t, e)
						var m HandlerManifest
						must(t, json.Unmarshal(b, &m))
						real, e := os.Executable()
						must(t, e)
						if m.ExecutablePath != real || m.HomeDirectory != home || m.PreviousHandler != c.Previous {
							t.Fatalf("bundle manifest %+v", m)
						}
					}
					if c.Failure == "register" {
						return []byte(" stdout\nstderr \n"), errors.New("synthetic exit")
					}
					return nil, nil
				case name == "open":
					trace = append(trace, "open")
					if mode != "run" || !reflect.DeepEqual(args, []string{"-b", c.Previous, f.CallbackURL}) {
						t.Fatalf("delegate argv %s %v", mode, args)
					}
					if c.Failure == "open" {
						return nil, errors.New("private-code secret output")
					}
					return nil, nil
				case name == "swift":
					if len(args) != 2 || args[0] != "-e" {
						t.Fatalf("swift argv %v", args)
					}
					if strings.Contains(args[1], "LSCopyDefaultHandlerForURLScheme") {
						if mode != "output" {
							t.Fatal(mode)
						}
						trace = append(trace, "query")
						if c.Failure == "query" {
							return nil, errors.New("synthetic query")
						}
						if c.Operation == "ensure" || c.Operation == "install" || c.Operation == "repeat" {
							return []byte(" " + c.Previous + "\n"), nil
						}
						return []byte(current), nil
					}
					if mode != "combined" {
						t.Fatal(mode)
					}
					match := regexp.MustCompile("let handler=(.+) as NSString; let status").FindStringSubmatch(args[1])
					if len(match) != 2 {
						t.Fatal(args[1])
					}
					bundle, e := strconv.Unquote(match[1])
					must(t, e)
					trace = append(trace, "set:"+bundle)
					if c.Failure == "set" && (bundle == f.BundleID || c.Operation == "disable") {
						return []byte(" stdout\nstderr \n"), errors.New("synthetic exit")
					}
					current = bundle
					if c.Failure == "save" && bundle == f.BundleID {
						must(t, os.MkdirAll(manifestPath, 0700))
						must(t, os.WriteFile(filepath.Join(manifestPath, "blocked"), nil, 0600))
					}
					return nil, nil
				default:
					t.Fatalf("unexpected command %s %v", name, args)
					return nil, nil
				}
			}
			if c.Operation == "repeat" {
				must(t, EnsurePersistent(context.Background()))
				m, _, e := LoadHandlerManifest()
				must(t, e)
				m.ExecutablePath = "stale executable"
				must(t, SaveHandlerManifest(m))
				trace = []string{}
			}
			if c.Operation == "disable" || c.Operation == "delegate" {
				must(t, SaveHandlerManifest(HandlerManifest{Version: 1, ExecutablePath: "old", PreviousHandler: c.Previous}))
			}
			var err error
			switch c.Operation {
			case "ensure", "repeat":
				err = EnsurePersistent(context.Background())
			case "disable", "disable-missing":
				err = DisablePersistent(context.Background())
			case "delegate", "delegate-missing":
				err = DelegateToPrevious(context.Background(), f.CallbackURL)
			case "install":
				raw := f.EndpointURL
				if c.Failure == "endpoint" {
					raw = "invalid-relay"
				}
				cleanup, e := Install(context.Background(), raw)
				err = e
				if err == nil {
					if cleanup == nil || !exists(endpoint) {
						t.Fatal("successful install requires endpoint and cleanup")
					}
					b, e := os.ReadFile(endpoint)
					must(t, e)
					if string(b) != f.EndpointURL+"\n" {
						t.Fatal("endpoint content")
					}
					cleanup()
					cleanup()
				} else if cleanup != nil {
					t.Fatal("failure returned cleanup")
				}
			}
			if c.Error == "filesystem" {
				if err == nil {
					t.Fatal("expected manifest filesystem failure")
				}
			} else {
				actual := ""
				if err != nil {
					actual = err.Error()
				}
				if actual != c.Error {
					t.Fatalf("error %q != %q", actual, c.Error)
				}
			}
			if !reflect.DeepEqual(trace, c.Trace) {
				t.Fatalf("trace %v != %v", trace, c.Trace)
			}
			m, present, loadErr := LoadHandlerManifest()
			if c.Failure == "save" {
				if loadErr == nil {
					t.Fatal("save obstacle lost")
				}
				present = false
			} else {
				must(t, loadErr)
			}
			if present != c.ManifestExists {
				t.Fatalf("manifest %v != %v", present, c.ManifestExists)
			}
			if present && (c.Operation == "ensure" || c.Operation == "repeat") {
				real, e := os.Executable()
				must(t, e)
				if m.PreviousHandler != c.Previous || m.ExecutablePath != real || m.HomeDirectory != home {
					t.Fatalf("manifest %+v", m)
				}
				info, e := os.Stat(manifestPath)
				must(t, e)
				if info.Mode().Perm() != 0600 {
					t.Fatal("manifest permissions")
				}
			}
			if exists(endpoint) != c.EndpointExists {
				t.Fatal("endpoint lifetime")
			}
		})
	}
}
func TestFrozenCompilerLifecycle(t *testing.T) {
	f := readFixture(t)
	home := t.TempDir()
	t.Setenv("TMPDIR", home)
	app := filepath.Join(t.TempDir(), "handler.app")
	var sources []string
	calls := 0
	legacy := filepath.Join(home, "pixiv-cli-url-handler.swift")
	canary := filepath.Join(home, "canary")
	must(t, os.WriteFile(canary, []byte("untouched"), 0600))
	must(t, os.Symlink(canary, legacy))
	compile := func(_ context.Context, source, target string) ([]byte, error) {
		calls++
		sources = append(sources, source)
		for path, mode := range map[string]os.FileMode{source: 0600, filepath.Dir(source): 0700} {
			info, e := os.Stat(path)
			must(t, e)
			if info.Mode().Perm() != mode {
				t.Fatalf("permission %s %o", path, info.Mode().Perm())
			}
		}
		if filepath.Dir(filepath.Dir(source)) != home || !strings.HasPrefix(filepath.Base(filepath.Dir(source)), "pixiv-cli-url-handler-") || !strings.HasPrefix(filepath.Base(source), "url-handler-") {
			t.Fatal("source is not private random file")
		}
		body, e := os.ReadFile(source)
		must(t, e)
		if string(body) != f.SwiftSource {
			t.Fatal("source asset")
		}
		must(t, os.WriteFile(target, []byte("compiled"), 0700))
		return nil, nil
	}
	must(t, EnsurePixivURLHandlerAppWithCompiler(context.Background(), app, compile))
	must(t, EnsurePixivURLHandlerAppWithCompiler(context.Background(), app, compile))
	if calls != 1 {
		t.Fatal("matching assets must skip compiler")
	}
	version := filepath.Join(app, "Contents", "Resources", "source-version")
	must(t, os.WriteFile(version, []byte("5\n"), 0600))
	must(t, EnsurePixivURLHandlerAppWithCompiler(context.Background(), app, compile))
	if calls != 2 || sources[0] == sources[1] {
		t.Fatal("version rebuild needs fresh random source")
	}
	info := filepath.Join(app, "Contents", "Info.plist")
	body, e := os.ReadFile(info)
	must(t, e)
	if string(body) != f.Plist {
		t.Fatal("plist asset")
	}
	body, e = os.ReadFile(version)
	must(t, e)
	if string(body) != f.SourceVersion+"\n" {
		t.Fatal("version asset")
	}
	for _, path := range []string{info, version} {
		i, e := os.Stat(path)
		must(t, e)
		if i.Mode().Perm() != 0600 {
			t.Fatal("asset private permissions")
		}
	}
	for _, source := range sources {
		if exists(filepath.Dir(source)) {
			t.Fatal("source cleanup")
		}
	}
	body, e = os.ReadFile(canary)
	must(t, e)
	if string(body) != "untouched" {
		t.Fatal("legacy symlink target overwritten")
	}
	failed := filepath.Join(t.TempDir(), "failure.app")
	var failedSource string
	err := EnsurePixivURLHandlerAppWithCompiler(context.Background(), failed, func(_ context.Context, source, target string) ([]byte, error) {
		failedSource = source
		return []byte(" stdout\nstderr \n"), errors.New("synthetic exit")
	})
	if err == nil || err.Error() != "compile pixiv:// callback helper: synthetic exit: stdout\nstderr" {
		t.Fatalf("compiler error %v", err)
	}
	if exists(filepath.Dir(failedSource)) || exists(filepath.Join(failed, "Contents", "Info.plist")) || exists(filepath.Join(failed, "Contents", "Resources", "source-version")) {
		t.Fatal("failed compile left source or assets")
	}
}
func TestFrozenNativeCommandPorts(t *testing.T) {
	f := readFixture(t)
	for _, c := range f.NativeScriptCases {
		t.Run(c.Scheme, func(t *testing.T) {
			var trace []string
			exec.RunCommand = func(name string, args []string, mode string) ([]byte, error) {
				trace = append(trace, name+":"+mode)
				script := c.QueryScript
				if mode == "combined" {
					script = c.SetScript
				}
				if name != "swift" || !reflect.DeepEqual(args, []string{"-e", script}) {
					t.Fatalf("native port %s %v expected %s", name, args, script)
				}
				if mode == "output" {
					return []byte(" old.bundle \n"), nil
				}
				return []byte(" stdout\nstderr \n"), errors.New("synthetic exit")
			}
			value, e := defaultURLSchemeHandler(context.Background(), c.Scheme)
			must(t, e)
			if value != "old.bundle" {
				t.Fatal(value)
			}
			e = setDefaultURLSchemeHandler(context.Background(), c.Scheme, c.BundleID)
			if e == nil || e.Error() != c.Error {
				t.Fatalf("set port error %v", e)
			}
			if !reflect.DeepEqual(trace, []string{"swift:output", "swift:combined"}) {
				t.Fatal(trace)
			}
		})
	}
}
func TestFrozenBundleManifestVersionIsUnpromoted(t *testing.T) {
	f := readFixture(t)
	t.Setenv("HOME", t.TempDir())
	app := filepath.Join(t.TempDir(), "handler.app")
	m := HandlerManifest{Version: 0, ExecutablePath: "old", PreviousHandler: "old.bundle"}
	must(t, SaveHandlerBundleManifest(app, m))
	body, e := os.ReadFile(handlerBundleManifestPath(app))
	must(t, e)
	var got, want interface{}
	must(t, json.Unmarshal(body, &got))
	must(t, json.Unmarshal(f.BundleManifestZeroVersion, &want))
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("bundle manifest %s", body)
	}
	must(t, SaveHandlerManifest(m))
	saved, present, e := LoadHandlerManifest()
	must(t, e)
	if !present || saved.Version != 1 {
		t.Fatal("shared manifest must promote zero version")
	}
}
func TestFrozenCompilerCacheFileKinds(t *testing.T) {
	f := readFixture(t)
	for _, c := range f.CompilerCacheCases {
		t.Run(c.Name, func(t *testing.T) {
			app := filepath.Join(t.TempDir(), "handler.app")
			exe := filepath.Join(app, "Contents", "MacOS", "PixivCLIURLHandler")
			plist := filepath.Join(app, "Contents", "Info.plist")
			version := filepath.Join(app, "Contents", "Resources", "source-version")
			for _, path := range []string{exe, plist, version} {
				must(t, os.MkdirAll(filepath.Dir(path), 0700))
				must(t, os.WriteFile(path, []byte("6"), 0600))
			}
			switch c.Kind {
			case "trim":
				must(t, os.WriteFile(version, []byte(" \t6\n"), 0600))
			case "symlink":
				for _, path := range []string{exe, plist} {
					target := path + ".target"
					must(t, os.Rename(path, target))
					must(t, os.Symlink(target, path))
				}
			case "exec-directory":
				must(t, os.Remove(exe))
				must(t, os.Mkdir(exe, 0700))
			case "plist-directory":
				must(t, os.Remove(plist))
				must(t, os.Mkdir(plist, 0700))
			case "missing":
				must(t, os.Remove(exe))
			}
			calls := 0
			must(t, EnsurePixivURLHandlerAppWithCompiler(context.Background(), app, func(_ context.Context, source, target string) ([]byte, error) {
				calls++
				must(t, os.RemoveAll(target))
				must(t, os.WriteFile(target, []byte("compiled"), 0700))
				if c.Kind == "plist-directory" {
					must(t, os.Remove(plist))
				}
				return nil, nil
			}))
			if (calls == 1) != c.Compile {
				t.Fatalf("compile calls %d expected %v", calls, c.Compile)
			}
		})
	}
}

func TestFrozenInstallCleanupEndpointReplacementDirectories(t *testing.T) {
	f := readFixture(t)
	for _, nonempty := range []bool{false, true} {
		name := "empty-directory-removed"
		if nonempty {
			name = "nonempty-directory-retained"
		}
		t.Run(name, func(t *testing.T) {
			t.Setenv("HOME", t.TempDir())
			app, e := PixivURLHandlerAppPath()
			must(t, e)
			must(t, EnsurePixivURLHandlerAppWithCompiler(context.Background(), app, func(_ context.Context, source, target string) ([]byte, error) {
				must(t, os.WriteFile(target, []byte("mock executable"), 0700))
				return nil, nil
			}))
			var sets []string
			exec.Find = func(name string) (string, error) { return name, nil }
			exec.RunCommand = func(name string, args []string, mode string) ([]byte, error) {
				if strings.HasSuffix(name, "/lsregister") {
					if mode != "combined" || !reflect.DeepEqual(args, []string{"-f", app}) {
						t.Fatal("registration command")
					}
					return nil, nil
				}
				if name != "swift" || len(args) != 2 || args[0] != "-e" {
					t.Fatalf("unexpected command %s %v", name, args)
				}
				if mode == "output" {
					return []byte("old.bundle"), nil
				}
				match := regexp.MustCompile("let handler=(.+) as NSString; let status").FindStringSubmatch(args[1])
				if mode != "combined" || len(match) != 2 {
					t.Fatal("set command")
				}
				bundle, e := strconv.Unquote(match[1])
				must(t, e)
				sets = append(sets, bundle)
				return nil, nil
			}
			cleanup, e := Install(context.Background(), f.EndpointURL)
			must(t, e)
			if cleanup == nil {
				t.Fatal("cleanup missing")
			}
			endpoint, e := CallbackEndpointPath()
			must(t, e)
			must(t, os.Remove(endpoint))
			must(t, os.Mkdir(endpoint, 0700))
			if nonempty {
				must(t, os.WriteFile(filepath.Join(endpoint, "kept"), []byte("preserved"), 0600))
			}
			cleanup()
			cleanup()
			if exists(endpoint) != nonempty {
				t.Fatalf("replacement directory exists=%v, expected %v", exists(endpoint), nonempty)
			}
			if nonempty {
				body, e := os.ReadFile(filepath.Join(endpoint, "kept"))
				must(t, e)
				if string(body) != "preserved" {
					t.Fatal("nonempty directory content changed")
				}
			}
			if !reflect.DeepEqual(sets, []string{f.BundleID, "old.bundle", "old.bundle"}) {
				t.Fatalf("cleanup must attempt restoration despite endpoint removal result: %v", sets)
			}
		})
	}
}

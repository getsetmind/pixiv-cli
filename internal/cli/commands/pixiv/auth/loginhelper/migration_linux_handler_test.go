//go:build linux

package loginhelper_test

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"

	lh "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth/loginhelper"
	"github.com/stretchr/testify/require"
)

type linuxNativePathCase struct {
	Name      string `json:"name"`
	LeafHex   string `json:"leaf_hex"`
	LossyLeaf string `json:"lossy_leaf"`
}
type linuxHandlerFixture struct {
	NativePathCases []linuxNativePathCase `json:"native_path_cases"`
	DesktopID       string                `json:"desktop_id"`
	Scheme          string                `json:"scheme"`
	Executable      string                `json:"executable"`
	Entry           string                `json:"entry"`
	InvalidIDs      []string              `json:"invalid_ids"`
	Errors          map[string]string     `json:"errors"`
	SnapshotOrder   []string              `json:"snapshot_order"`
	SetDefaultArgs  []string              `json:"set_default_args"`
}

func linuxFixture(t *testing.T) linuxHandlerFixture {
	t.Helper()
	b, err := os.ReadFile("../../../../../../crates/pixiv-app/tests/fixtures/linux_handler.json")
	require.NoError(t, err)
	var f linuxHandlerFixture
	require.NoError(t, json.Unmarshal(b, &f))
	return f
}
func linuxHome(t *testing.T) string {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, "config"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "data"))
	t.Setenv("XDG_DATA_DIRS", filepath.Join(home, "system"))
	return home
}
func linuxWrite(t *testing.T, path, body string, mode os.FileMode) {
	t.Helper()
	require.NoError(t, os.MkdirAll(filepath.Dir(path), 0700))
	require.NoError(t, os.WriteFile(path, []byte(body), mode))
	require.NoError(t, os.Chmod(path, mode))
}
func linuxHooks(t *testing.T, f linuxHandlerFixture, trace *[]string, current *string) {
	t.Helper()
	t.Cleanup(lh.SetFindLinuxCommand(func(name string) (string, error) { *trace = append(*trace, "find:"+name); return "/mock/" + name, nil }))
	t.Cleanup(lh.SetLinuxExecutablePath(func() (string, error) { *trace = append(*trace, "executable"); return f.Executable, nil }))
	t.Cleanup(lh.SetQueryLinuxDefaultHandler(func(context.Context) (string, error) { *trace = append(*trace, "query"); return *current, nil }))
	t.Cleanup(lh.SetRunLinuxXDGMIme(func(_ context.Context, args ...string) error {
		*trace = append(*trace, "default")
		require.Equal(t, f.SetDefaultArgs, args)
		*current = f.DesktopID
		return nil
	}))
}
func TestMigrationLinuxFixtureAndLookup(t *testing.T) {
	f := linuxFixture(t)
	require.Equal(t, f.DesktopID, lh.LinuxURLHandlerDesktopFile)
	require.Equal(t, f.Scheme, lh.LinuxPixivURLScheme)
	require.Equal(t, f.Entry, lh.LinuxDesktopEntry(f.Executable))
	home := linuxHome(t)
	user := filepath.Join(home, "data", "applications")
	system := filepath.Join(home, "system", "applications")
	t.Setenv("XDG_DATA_DIRS", " "+filepath.Join(home, "system")+":"+filepath.Join(home, "system", ".")+": ")
	for _, id := range f.InvalidIDs {
		_, err := lh.LinuxDesktopFilePath(id)
		require.EqualError(t, err, "invalid desktop handler ID")
	}
	linuxWrite(t, filepath.Join(system, "mixed.DESKTOP"), "system", 0644)
	got, err := lh.LinuxDesktopFilePath(" mixed.DESKTOP ")
	require.NoError(t, err)
	require.Equal(t, filepath.Join(system, "mixed.DESKTOP"), got)
	require.NoError(t, os.MkdirAll(filepath.Join(user, "mixed.DESKTOP"), 0700))
	got, err = lh.LinuxDesktopFilePath("mixed.DESKTOP")
	require.NoError(t, err)
	require.Equal(t, filepath.Join(system, "mixed.DESKTOP"), got)
	require.NoError(t, os.Remove(filepath.Join(user, "mixed.DESKTOP")))
	linuxWrite(t, filepath.Join(user, "mixed.DESKTOP"), "user", 0600)
	got, err = lh.LinuxDesktopFilePath("mixed.DESKTOP")
	require.NoError(t, err)
	require.Equal(t, filepath.Join(user, "mixed.DESKTOP"), got)
	t.Setenv("XDG_DATA_HOME", " \t")
	got, err = lh.LinuxApplicationsDir()
	require.NoError(t, err)
	require.Equal(t, filepath.Join(home, ".local", "share", "applications"), got)
}
func TestMigrationLinuxPersistentFirstRepeatAndExternalDefault(t *testing.T) {
	f := linuxFixture(t)
	home := linuxHome(t)
	var trace []string
	current := "previous.desktop"
	linuxHooks(t, f, &trace, &current)
	paths := []string{}
	for _, p := range f.SnapshotOrder {
		paths = append(paths, filepath.Join(home, p))
	}
	linuxWrite(t, paths[0], "original\x00config", 0640)
	linuxWrite(t, paths[1], "original data", 0604)
	linuxWrite(t, paths[2], "original desktop", 0644)
	require.NoError(t, lh.EnsurePersistent(context.Background()))
	require.Equal(t, []string{"find:xdg-mime", "find:gio", "executable", "query", "default"}, trace)
	m, exists, err := lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.True(t, exists)
	require.Equal(t, "previous.desktop", m.PreviousHandler)
	require.Len(t, m.LinuxMIMESnapshots, 3)
	for i, s := range m.LinuxMIMESnapshots {
		require.Equal(t, paths[i], s.Path)
	}
	original := m.LinuxMIMESnapshots
	trace = nil
	current = "external.desktop"
	linuxWrite(t, paths[0], "external", 0600)
	require.NoError(t, lh.EnsurePersistent(context.Background()))
	require.Equal(t, []string{"find:xdg-mime", "find:gio", "executable", "default"}, trace)
	m, _, err = lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.Equal(t, original, m.LinuxMIMESnapshots)
	require.Equal(t, "previous.desktop", m.PreviousHandler)
	current = "external.desktop"
	require.NoError(t, lh.DisablePersistent(context.Background()))
	_, exists, err = lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.False(t, exists)
	b, err := os.ReadFile(paths[0])
	require.NoError(t, err)
	require.Equal(t, "external", string(b))
	b, err = os.ReadFile(paths[2])
	require.NoError(t, err)
	require.Equal(t, f.Entry, string(b))
	require.NoError(t, lh.SaveHandlerManifest(m))
	current = f.DesktopID
	require.NoError(t, lh.DisablePersistent(context.Background()))
	for _, s := range original {
		b, err := os.ReadFile(s.Path)
		require.NoError(t, err)
		require.Equal(t, s.Content, b)
		info, err := os.Stat(s.Path)
		require.NoError(t, err)
		require.Equal(t, s.Mode, info.Mode().Perm())
	}
}
func TestMigrationLinuxLegacyDisable(t *testing.T) {
	f := linuxFixture(t)
	for _, previous := range []string{"previous.desktop", "", f.DesktopID} {
		t.Run(previous, func(t *testing.T) {
			home := linuxHome(t)
			path := filepath.Join(home, "data", "applications", f.DesktopID)
			linuxWrite(t, path, "installed", 0600)
			require.NoError(t, lh.SaveHandlerManifest(lh.HandlerManifest{Version: 1, ExecutablePath: "old", PreviousHandler: previous}))
			t.Cleanup(lh.SetQueryLinuxDefaultHandler(func(context.Context) (string, error) { return f.DesktopID, nil }))
			var calls [][]string
			t.Cleanup(lh.SetRunLinuxXDGMIme(func(_ context.Context, args ...string) error { calls = append(calls, args); return nil }))
			err := lh.DisablePersistent(context.Background())
			_, exists, loadErr := lh.LoadHandlerManifest()
			require.NoError(t, loadErr)
			switch previous {
			case "":
				require.EqualError(t, err, f.Errors["unsafe_restore"])
				require.True(t, exists)
				require.FileExists(t, path)
			case f.DesktopID:
				require.NoError(t, err)
				require.False(t, exists)
				require.Empty(t, calls)
				require.FileExists(t, path)
			default:
				require.NoError(t, err)
				require.False(t, exists)
				require.Equal(t, [][]string{{"default", previous, f.Scheme}}, calls)
				require.NoFileExists(t, path)
			}
		})
	}
}
func TestMigrationLinuxEnsureFailureOrdering(t *testing.T) {
	f := linuxFixture(t)
	for _, stage := range []string{"xdg-mime", "gio", "executable", "query", "snapshot", "default", "manifest"} {
		t.Run(stage, func(t *testing.T) {
			home := linuxHome(t)
			var trace []string
			current := "old.desktop"
			linuxHooks(t, f, &trace, &current)
			injected := errors.New("injected")
			config := filepath.Join(home, "config", "mimeapps.list")
			desktop := filepath.Join(home, "data", "applications", f.DesktopID)
			linuxWrite(t, config, "before", 0640)
			switch stage {
			case "xdg-mime", "gio":
				t.Cleanup(lh.SetFindLinuxCommand(func(name string) (string, error) {
					trace = append(trace, "find:"+name)
					if name == stage {
						return "", injected
					}
					return "/mock/" + name, nil
				}))
			case "executable":
				t.Cleanup(lh.SetLinuxExecutablePath(func() (string, error) { trace = append(trace, "executable"); return "", injected }))
			case "query":
				t.Cleanup(lh.SetQueryLinuxDefaultHandler(func(context.Context) (string, error) { trace = append(trace, "query"); return "", injected }))
			case "snapshot":
				require.NoError(t, os.Remove(config))
				require.NoError(t, os.Mkdir(config, 0700))
			case "default", "manifest":
				t.Cleanup(lh.SetRunLinuxXDGMIme(func(_ context.Context, _ ...string) error {
					trace = append(trace, "default")
					linuxWrite(t, config, "modified", 0600)
					if stage == "default" {
						return injected
					}
					p, err := lh.HandlerManifestPath()
					require.NoError(t, err)
					require.NoError(t, os.MkdirAll(p, 0700))
					return nil
				}))
			}
			err := lh.EnsurePersistent(context.Background())
			require.Error(t, err)
			if stage == "xdg-mime" {
				require.EqualError(t, err, f.Errors["xdg"])
				require.Equal(t, []string{"find:xdg-mime"}, trace)
			}
			if stage == "gio" {
				require.EqualError(t, err, f.Errors["gio"])
				require.Equal(t, []string{"find:xdg-mime", "find:gio"}, trace)
			}
			if stage == "snapshot" {
				require.EqualError(t, err, f.Errors["nonregular"])
			} else {
				b, e := os.ReadFile(config)
				require.NoError(t, e)
				require.Equal(t, "before", string(b))
			}
			require.NoFileExists(t, desktop)
		})
	}
}
func TestMigrationLinuxDelegateFailures(t *testing.T) {
	f := linuxFixture(t)
	for _, stage := range []string{"missing", "empty", "self", "gio", "locate", "launch", "success"} {
		t.Run(stage, func(t *testing.T) {
			home := linuxHome(t)
			previous := "previous.desktop"
			if stage == "empty" {
				previous = ""
			}
			if stage == "self" {
				previous = f.DesktopID
			}
			if stage != "missing" {
				require.NoError(t, lh.SaveHandlerManifest(lh.HandlerManifest{Version: 1, ExecutablePath: "old", PreviousHandler: previous}))
			}
			var trace []string
			t.Cleanup(lh.SetFindLinuxCommand(func(name string) (string, error) {
				trace = append(trace, "find:"+name)
				if stage == "gio" {
					return "", errors.New("private diagnostic")
				}
				return "/mock/gio", nil
			}))
			desktop := filepath.Join(home, "system", "applications", previous)
			if stage == "launch" || stage == "success" {
				linuxWrite(t, desktop, "entry", 0600)
			}
			raw := "pixiv://path?code=synthetic&arg=$(value)"
			t.Cleanup(lh.SetRunLinuxGioLaunch(func(_ context.Context, p, u string) error {
				trace = append(trace, "launch")
				require.Equal(t, desktop, p)
				require.Equal(t, raw, u)
				if stage == "launch" {
					return errors.New("private diagnostic")
				}
				return nil
			}))
			err := lh.DelegateToPrevious(context.Background(), raw)
			switch stage {
			case "missing", "empty", "self":
				require.EqualError(t, err, f.Errors["no_previous"])
				require.Empty(t, trace)
			case "gio":
				require.EqualError(t, err, f.Errors["delegate_gio"])
			case "locate":
				require.EqualError(t, err, f.Errors["delegate_locate"])
			case "launch":
				require.EqualError(t, err, f.Errors["delegate_launch"])
			case "success":
				require.NoError(t, err)
				require.Equal(t, []string{"find:gio", "launch"}, trace)
			}
		})
	}
}
func TestMigrationLinuxInstallFailureAndCleanup(t *testing.T) {
	f := linuxFixture(t)
	for _, stage := range []string{"executable", "snapshot", "desktop", "default", "success"} {
		t.Run(stage, func(t *testing.T) {
			home := linuxHome(t)
			config := filepath.Join(home, "config", "mimeapps.list")
			desktop := filepath.Join(home, "data", "applications", f.DesktopID)
			linuxWrite(t, config, "original", 0642)
			var trace []string
			current := ""
			linuxHooks(t, f, &trace, &current)
			endpoint, e := lh.CallbackEndpointPath()
			require.NoError(t, e)
			t.Cleanup(lh.SetLinuxExecutablePath(func() (string, error) {
				trace = append(trace, "executable")
				require.FileExists(t, endpoint)
				if stage == "executable" {
					return "", errors.New("resolve failure")
				}
				return f.Executable, nil
			}))
			if stage == "snapshot" {
				require.NoError(t, os.MkdirAll(desktop, 0700))
			}
			if stage == "desktop" {
				require.NoError(t, os.MkdirAll(filepath.Join(home, "data"), 0700))
				require.NoError(t, os.WriteFile(filepath.Join(home, "data", "applications"), nil, 0600))
			}
			t.Cleanup(lh.SetRunLinuxXDGMIme(func(_ context.Context, args ...string) error {
				trace = append(trace, "default")
				require.Equal(t, f.SetDefaultArgs, args)
				require.FileExists(t, endpoint)
				b, e := os.ReadFile(desktop)
				require.NoError(t, e)
				require.Equal(t, f.Entry, string(b))
				linuxWrite(t, config, "modified", 0600)
				if stage == "default" {
					return errors.New("run xdg-mime: synthetic exit")
				}
				return nil
			}))
			cleanup, err := lh.Install(context.Background(), "http://127.0.0.1:41871/callback")
			if stage == "success" {
				require.NoError(t, err)
				require.NotNil(t, cleanup)
				cleanup()
				cleanup()
			} else {
				require.Error(t, err)
				require.Nil(t, cleanup)
			}
			require.NoFileExists(t, endpoint)
			b, e := os.ReadFile(config)
			require.NoError(t, e)
			require.Equal(t, "original", string(b))
			info, e := os.Stat(config)
			require.NoError(t, e)
			require.Equal(t, os.FileMode(0642), info.Mode().Perm())
			if stage == "success" || stage == "default" || stage == "executable" {
				require.NoFileExists(t, desktop)
			}
		})
	}
}
func TestMigrationLinuxRepeatFailureRestoresOriginalAndRetainsManifest(t *testing.T) {
	f := linuxFixture(t)
	home := linuxHome(t)
	config := filepath.Join(home, "config", "mimeapps.list")
	desktop := filepath.Join(home, "data", "applications", f.DesktopID)
	linuxWrite(t, config, "original", 0640)
	var trace []string
	current := "old.desktop"
	linuxHooks(t, f, &trace, &current)
	require.NoError(t, lh.EnsurePersistent(context.Background()))
	before, exists, err := lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.True(t, exists)
	t.Cleanup(lh.SetLinuxExecutablePath(func() (string, error) { return "replacement", nil }))
	t.Cleanup(lh.SetRunLinuxXDGMIme(func(context.Context, ...string) error {
		linuxWrite(t, config, "modified", 0600)
		return errors.New("run xdg-mime: synthetic exit")
	}))
	require.EqualError(t, lh.EnsurePersistent(context.Background()), "run xdg-mime: synthetic exit")
	after, exists, err := lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.True(t, exists)
	require.Equal(t, before, after)
	b, err := os.ReadFile(config)
	require.NoError(t, err)
	require.Equal(t, "original", string(b))
	require.NoFileExists(t, desktop)
}
func TestMigrationLinuxDisableRestoreFailureRetainsManifestAndRestoresLaterFiles(t *testing.T) {
	f := linuxFixture(t)
	home := linuxHome(t)
	blocked := filepath.Join(home, "blocked")
	linuxWrite(t, blocked, "file", 0600)
	later := filepath.Join(home, "later")
	require.NoError(t, lh.SaveHandlerManifest(lh.HandlerManifest{Version: 1, ExecutablePath: "old", LinuxMIMESnapshots: []lh.HandlerFileSnapshot{{Path: filepath.Join(blocked, "child"), Exists: true, Mode: 0600}, {Path: later, Exists: true, Mode: 0640, Content: []byte("restored")}}}))
	t.Cleanup(lh.SetQueryLinuxDefaultHandler(func(context.Context) (string, error) { return f.DesktopID, nil }))
	require.Error(t, lh.DisablePersistent(context.Background()))
	_, exists, err := lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.True(t, exists)
	b, err := os.ReadFile(later)
	require.NoError(t, err)
	require.Equal(t, "restored", string(b))
}
func TestMigrationLinuxDisableStopsBeforeChangingState(t *testing.T) {
	f := linuxFixture(t)
	for _, stage := range []string{"missing", "query", "legacy-default", "legacy-remove"} {
		t.Run(stage, func(t *testing.T) {
			home := linuxHome(t)
			desktop := filepath.Join(home, "data", "applications", f.DesktopID)
			var trace []string
			t.Cleanup(lh.SetQueryLinuxDefaultHandler(func(context.Context) (string, error) {
				trace = append(trace, "query")
				if stage == "query" {
					return "", errors.New("query xdg-mime: synthetic exit")
				}
				return f.DesktopID, nil
			}))
			t.Cleanup(lh.SetRunLinuxXDGMIme(func(_ context.Context, args ...string) error {
				trace = append(trace, "default")
				require.Equal(t, []string{"default", "old.desktop", f.Scheme}, args)
				if stage == "legacy-default" {
					return errors.New("run xdg-mime: synthetic exit")
				}
				return nil
			}))
			if stage != "missing" {
				require.NoError(t, lh.SaveHandlerManifest(lh.HandlerManifest{Version: 1, ExecutablePath: "old", PreviousHandler: "old.desktop"}))
				if stage == "legacy-remove" {
					require.NoError(t, os.MkdirAll(desktop, 0700))
					linuxWrite(t, filepath.Join(desktop, "child"), "kept", 0600)
				} else {
					linuxWrite(t, desktop, "kept", 0600)
				}
			}
			err := lh.DisablePersistent(context.Background())
			_, exists, loadErr := lh.LoadHandlerManifest()
			require.NoError(t, loadErr)
			if stage == "missing" {
				require.NoError(t, err)
				require.False(t, exists)
				require.Empty(t, trace)
			} else {
				require.Error(t, err)
				require.True(t, exists)
				if stage == "query" {
					require.Equal(t, []string{"query"}, trace)
				} else {
					require.Equal(t, []string{"query", "default"}, trace)
				}
			}
		})
	}
}
func TestMigrationLinuxExistingLegacyEnsureFailureHasNoSnapshotRollback(t *testing.T) {
	f := linuxFixture(t)
	home := linuxHome(t)
	desktop := filepath.Join(home, "data", "applications", f.DesktopID)
	legacy := lh.HandlerManifest{Version: 1, ExecutablePath: "old", PreviousHandler: "old.desktop"}
	require.NoError(t, lh.SaveHandlerManifest(legacy))
	var trace []string
	current := ""
	linuxHooks(t, f, &trace, &current)
	t.Cleanup(lh.SetRunLinuxXDGMIme(func(context.Context, ...string) error { return errors.New("run xdg-mime: synthetic exit") }))
	require.EqualError(t, lh.EnsurePersistent(context.Background()), "run xdg-mime: synthetic exit")
	b, err := os.ReadFile(desktop)
	require.NoError(t, err)
	require.Equal(t, f.Entry, string(b))
	after, exists, err := lh.LoadHandlerManifest()
	require.NoError(t, err)
	require.True(t, exists)
	require.Equal(t, legacy, after)
	require.Equal(t, []string{"find:xdg-mime", "find:gio", "executable"}, trace)
}
func TestMigrationLinuxTemporaryCleanupPreservesNativeNonUTF8Paths(t *testing.T) {
	f := linuxFixture(t)
	for _, c := range f.NativePathCases {
		t.Run(c.Name, func(t *testing.T) {
			leaf, decodeErr := hex.DecodeString(c.LeafHex)
			require.NoError(t, decodeErr)
			home := linuxHome(t)
			native := filepath.Join(home, string(leaf))
			t.Setenv("XDG_CONFIG_HOME", filepath.Join(native, "config"))
			t.Setenv("XDG_DATA_HOME", filepath.Join(native, "data"))
			config := filepath.Join(native, "config", "mimeapps.list")
			data := filepath.Join(native, "data", "applications", "mimeapps.list")
			desktop := filepath.Join(native, "data", "applications", f.DesktopID)
			original := map[string]string{config: "original config", data: "original data", desktop: "original desktop"}
			for path, body := range original {
				linuxWrite(t, path, body, 0640)
			}
			t.Cleanup(lh.SetLinuxExecutablePath(func() (string, error) { return f.Executable, nil }))
			t.Cleanup(lh.SetRunLinuxXDGMIme(func(_ context.Context, args ...string) error {
				require.Equal(t, f.SetDefaultArgs, args)
				for path := range original {
					linuxWrite(t, path, "registered", 0600)
				}
				return nil
			}))
			cleanup, err := lh.Install(context.Background(), "http://127.0.0.1:41871/callback")
			require.NoError(t, err)
			cleanup()
			for path, body := range original {
				actual, err := os.ReadFile(path)
				require.NoError(t, err)
				require.Equal(t, body, string(actual))
				info, err := os.Stat(path)
				require.NoError(t, err)
				require.Equal(t, os.FileMode(0640), info.Mode().Perm())
			}
			require.NoDirExists(t, filepath.Join(home, c.LossyLeaf))
		})
	}
}
func TestMigrationLinuxPersistentManifestMakesNonUTF8SnapshotPathsLossy(t *testing.T) {
	f := linuxFixture(t)
	for _, c := range f.NativePathCases {
		t.Run(c.Name, func(t *testing.T) {
			leaf, decodeErr := hex.DecodeString(c.LeafHex)
			require.NoError(t, decodeErr)
			home := linuxHome(t)
			native := filepath.Join(home, string(leaf))
			t.Setenv("XDG_CONFIG_HOME", filepath.Join(native, "config"))
			t.Setenv("XDG_DATA_HOME", filepath.Join(native, "data"))
			config := filepath.Join(native, "config", "mimeapps.list")
			linuxWrite(t, config, "original", 0640)
			var trace []string
			current := "old.desktop"
			linuxHooks(t, f, &trace, &current)
			t.Cleanup(lh.SetRunLinuxXDGMIme(func(context.Context, ...string) error {
				current = f.DesktopID
				linuxWrite(t, config, "registered", 0600)
				return nil
			}))
			require.NoError(t, lh.EnsurePersistent(context.Background()))
			manifest, exists, err := lh.LoadHandlerManifest()
			require.NoError(t, err)
			require.True(t, exists)
			lossy := filepath.Join(home, c.LossyLeaf, "config", "mimeapps.list")
			require.Equal(t, lossy, manifest.LinuxMIMESnapshots[0].Path)
			require.NoError(t, lh.DisablePersistent(context.Background()))
			b, err := os.ReadFile(config)
			require.NoError(t, err)
			require.Equal(t, "registered", string(b))
			b, err = os.ReadFile(lossy)
			require.NoError(t, err)
			require.Equal(t, "original", string(b))
			require.FileExists(t, filepath.Join(native, "data", "applications", f.DesktopID))
		})
	}
}
func TestMigrationLinuxInvalidEndpointPrecedesMissingHomeAndHostHooks(t *testing.T) {
	t.Setenv("HOME", "")
	t.Setenv("XDG_CONFIG_HOME", "")
	t.Setenv("XDG_DATA_HOME", "")
	t.Cleanup(lh.SetLinuxExecutablePath(func() (string, error) { t.Fatal("executable resolved before endpoint validation"); return "", nil }))
	t.Cleanup(lh.SetFindLinuxCommand(func(string) (string, error) { t.Fatal("command looked up before endpoint validation"); return "", nil }))
	t.Cleanup(lh.SetRunLinuxXDGMIme(func(context.Context, ...string) error {
		t.Fatal("command executed before endpoint validation")
		return nil
	}))
	_, pathErr := lh.CallbackEndpointPath()
	require.Error(t, pathErr)
	cleanup, err := lh.Install(context.Background(), "invalid-relay")
	require.Nil(t, cleanup)
	require.EqualError(t, err, "invalid callback endpoint")
}

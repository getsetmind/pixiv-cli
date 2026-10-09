//go:build linux

package loginhelper

import (
	"errors"
	"github.com/stretchr/testify/require"
	"os"
	"path/filepath"
	"testing"
)

func TestMigrationLinuxSnapshotDedupAndHomeResolution(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "data"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, "data", "applications", "."))
	app := filepath.Join(home, "data", "applications")
	desktop := filepath.Join(app, LinuxURLHandlerDesktopFile)
	require.NoError(t, os.MkdirAll(app, 0700))
	mime := filepath.Join(app, "mimeapps.list")
	require.NoError(t, os.WriteFile(mime, []byte{'a', 0, 255}, 0600))
	require.NoError(t, os.Chmod(mime, 0641))
	s, err := snapshotLinuxMimeState(app, desktop)
	require.NoError(t, err)
	require.Len(t, s, 2)
	require.Equal(t, mime, s[0].Path)
	require.True(t, s[0].Exists)
	require.Equal(t, os.FileMode(0641), s[0].Mode)
	require.Equal(t, []byte{'a', 0, 255}, s[0].Content)
	require.Equal(t, HandlerFileSnapshot{Path: desktop}, s[1])
	require.NoError(t, os.Mkdir(desktop, 0700))
	_, err = snapshotLinuxMimeState(app, desktop)
	require.EqualError(t, err, "desktop integration state is not a regular file")
	t.Setenv("HOME", "")
	require.Nil(t, linuxMimeAppsPaths())
	got, err := LinuxApplicationsDir()
	require.NoError(t, err)
	require.Equal(t, app, got)
	require.NoError(t, os.Remove(desktop))
	s, err = snapshotLinuxMimeState(app, desktop)
	require.NoError(t, err)
	require.Equal(t, []HandlerFileSnapshot{{Path: desktop}}, s)
}
func TestMigrationLinuxRestoreContinuesAfterErrorsAndKeepsOrder(t *testing.T) {
	home := t.TempDir()
	blocked := filepath.Join(home, "blocked")
	require.NoError(t, os.WriteFile(blocked, []byte("block"), 0600))
	removeBlocked := filepath.Join(home, "nonempty")
	require.NoError(t, os.Mkdir(removeBlocked, 0700))
	require.NoError(t, os.WriteFile(filepath.Join(removeBlocked, "child"), nil, 0600))
	restored := filepath.Join(home, "created", "restored")
	duplicate := filepath.Join(home, "duplicate")
	err := restoreLinuxMimeState([]HandlerFileSnapshot{{Path: filepath.Join(blocked, "child"), Exists: true, Mode: 0600, Content: []byte("no")}, {Path: restored, Exists: true, Mode: 0642, Content: []byte{'a', 0, 255}}, {Path: removeBlocked}, {Path: duplicate, Exists: true, Mode: 0644, Content: []byte("first")}, {Path: duplicate, Exists: true, Mode: 0601, Content: []byte("last")}})
	require.Error(t, err)
	joined, ok := err.(interface{ Unwrap() []error })
	require.True(t, ok)
	require.Len(t, joined.Unwrap(), 2)
	var first *os.PathError
	require.True(t, errors.As(joined.Unwrap()[0], &first))
	require.Equal(t, blocked, first.Path)
	var second *os.PathError
	require.True(t, errors.As(joined.Unwrap()[1], &second))
	require.Equal(t, removeBlocked, second.Path)
	b, e := os.ReadFile(restored)
	require.NoError(t, e)
	require.Equal(t, []byte{'a', 0, 255}, b)
	info, e := os.Stat(restored)
	require.NoError(t, e)
	require.Equal(t, os.FileMode(0642), info.Mode().Perm())
	b, e = os.ReadFile(duplicate)
	require.NoError(t, e)
	require.Equal(t, "last", string(b))
	info, e = os.Stat(duplicate)
	require.NoError(t, e)
	require.Equal(t, os.FileMode(0601), info.Mode().Perm())
	require.NoError(t, restoreLinuxMimeState([]HandlerFileSnapshot{{Path: filepath.Join(home, "missing")}}))
}
func TestMigrationLinuxRestorePreservesExistingParentDirectoryModes(t *testing.T) {
	home := t.TempDir()
	parent := filepath.Join(home, "existing")
	require.NoError(t, os.Mkdir(parent, 0700))
	require.NoError(t, os.Chmod(parent, 0751))
	existing := filepath.Join(parent, "state")
	require.NoError(t, os.WriteFile(existing, []byte("changed"), 0600))
	require.NoError(t, restoreLinuxMimeState([]HandlerFileSnapshot{{Path: existing, Exists: true, Mode: 0640, Content: []byte("restored")}, {Path: filepath.Join(parent, "new"), Exists: true, Mode: 0604, Content: []byte("created")}}))
	info, err := os.Stat(parent)
	require.NoError(t, err)
	require.Equal(t, os.FileMode(0751), info.Mode().Perm())
	info, err = os.Stat(existing)
	require.NoError(t, err)
	require.Equal(t, os.FileMode(0640), info.Mode().Perm())
	info, err = os.Stat(filepath.Join(parent, "new"))
	require.NoError(t, err)
	require.Equal(t, os.FileMode(0604), info.Mode().Perm())
}

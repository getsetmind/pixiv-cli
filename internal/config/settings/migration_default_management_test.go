package settings_test

import (
	"errors"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"github.com/stretchr/testify/require"
	"testing"
)

func TestMigrationDefaultManagementFreezesDocumentMutations(t *testing.T) {
	for _, tc := range []struct {
		name, before   string
		missing, clear bool
		id             int64
		after, want    string
	}{
		{name: "set-missing", missing: true, id: 42, after: "[pixiv.auth]\ndefault_user_id = 42\n"},
		{name: "clear-missing", missing: true, clear: true, after: ""},
		{name: "replace-comments", before: "# root\n[pixiv.auth]  # table\n# owned\ndefault_user_id=7 # owned\nkeep='value' # retained\n\n[unknown]\nkeep=true\n", id: 42, after: "# root\n[pixiv.auth]  # table\ndefault_user_id = 42\nkeep = 'value'  # retained\n\n[unknown]\nkeep = true\n"},
		{name: "clear-empty-table", before: "[pixiv.auth]\ndefault_user_id=7\n", clear: true, after: ""},
		{name: "clear-keeps-unknown", before: "[pixiv.auth]\ndefault_user_id=7\nkeep=true\n", clear: true, after: "[pixiv.auth]\nkeep = true\n"},
		{name: "invalid-before-read", before: "[broken\n", id: 0, after: "[broken\n", want: "config: default_user_id must be positive"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			path := "synthetic/config.toml"
			files := &injectedFileStore{path: path, files: map[string][]byte{}}
			if !tc.missing {
				files.files[path] = []byte(tc.before)
			}
			store := config.Store{Files: files}
			var err error
			if tc.clear {
				err = store.ClearPixivDefaultUserID()
			} else {
				err = store.SetPixivDefaultUserID(tc.id)
			}
			if tc.want == "" {
				require.NoError(t, err)
			} else {
				require.EqualError(t, err, tc.want)
			}
			require.Equal(t, tc.after, string(files.files[path]))
		})
	}
}

func TestMigrationDefaultManagementPreservesReadAndPrivateWriteFailures(t *testing.T) {
	for _, clear := range []bool{false, true} {
		for _, stage := range []string{"read", "write"} {
			path := "synthetic/config.toml"
			before := []byte("[pixiv.auth]\ndefault_user_id=42\n")
			files := &mutationFailureStore{injectedFileStore: &injectedFileStore{path: path, files: map[string][]byte{path: append([]byte(nil), before...)}}}
			sentinel := errors.New("synthetic file failure")
			if stage == "read" {
				files.readError = sentinel
			} else {
				files.writeError = sentinel
			}
			store := config.Store{Files: files}
			var err error
			if clear {
				err = store.ClearPixivDefaultUserID()
			} else {
				err = store.SetPixivDefaultUserID(7)
			}
			require.ErrorIs(t, err, sentinel)
			require.Equal(t, before, files.files[path])
			if stage == "read" {
				require.Zero(t, files.writes)
			} else {
				require.Equal(t, 1, files.writes)
			}
		}
	}
}

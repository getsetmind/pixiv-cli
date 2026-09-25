package downloader

import (
	"archive/zip"
	"bytes"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// zipEntries 按给定顺序写入 entry，允许重复名以覆盖 duplicate 分支。
func zipEntries(names ...string) []byte {
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	for _, name := range names {
		w, err := zw.Create(name)
		if err != nil {
			panic(err)
		}
		if _, err := w.Write([]byte("frame")); err != nil {
			panic(err)
		}
	}
	if err := zw.Close(); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

func declaredFrames(names ...string) []pixiv.UgoiraFrame {
	frames := make([]pixiv.UgoiraFrame, 0, len(names))
	for _, name := range names {
		frames = append(frames, pixiv.UgoiraFrame{Filename: name})
	}
	return frames
}

func TestInspectUgoiraArchive(t *testing.T) {
	cases := []struct {
		name           string
		body           []byte
		declared       []pixiv.UgoiraFrame
		wantCode       string
		wantActual     int
		wantUndeclared []string
	}{
		{"undeclared", zipEntries("000000.jpg", "000002.jpg"), declaredFrames("000000.jpg"), "", 2, []string{"000002.jpg"}},
		{"unsafe", zipEntries("../evil.jpg"), declaredFrames("../evil.jpg"), CodeInsecureFrameName, 0, nil},
		{"duplicate", zipEntries("000000.jpg", "000000.jpg"), declaredFrames("000000.jpg"), string(sdk.UgoiraFrameMismatch), 1, nil},
		{"notzip", []byte("not a zip"), declaredFrames("000000.jpg"), string(sdk.UgoiraArchiveMissing), 0, nil},
		{"empty", zipEntries(), declaredFrames(), string(sdk.UgoiraArchiveMissing), 0, nil},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "a.zip")
			require.NoError(t, os.WriteFile(path, tc.body, 0o644))
			report, code, _ := inspectUgoiraArchive(path, tc.declared)
			assert.Equal(t, tc.wantCode, code)
			assert.Equal(t, tc.wantActual, report.Actual)
			if tc.wantUndeclared != nil {
				assert.Equal(t, tc.wantUndeclared, report.Undeclared)
			}
		})
	}
}

func TestValidateUgoiraFormatAcceptsZipAndRaw(t *testing.T) {
	assert.NoError(t, ValidateUgoiraFormat(UgoiraFormatZip))
	assert.NoError(t, ValidateUgoiraFormat(UgoiraFormatRaw))
	assert.Error(t, ValidateUgoiraFormat(UgoiraFormat("frames")))
}

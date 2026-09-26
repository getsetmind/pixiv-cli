package download

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"

	downloader "github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestWriteDownloadReportNDJSONMixesSuccessAndFailure(t *testing.T) {
	report := downloader.DownloadReport{
		Items: []downloader.DownloadedArtwork{
			{IllustID: 1, Type: "illustration", Files: []downloader.DownloadedFile{{Path: "a.jpg", Page: 1, Bytes: 10}}},
			{
				IllustID:    2,
				Type:        "ugoira",
				Quality:     "original",
				Frames:      []pixiv.UgoiraFrame{{Filename: "000000.jpg", DelayMilliseconds: 100}},
				FrameReport: &downloader.UgoiraFrameReport{Declared: 1, Actual: 1},
				Files:       []downloader.DownloadedFile{{Path: "b.zip", Page: 1, Bytes: 20}},
			},
		},
		Failures: []downloader.DownloadFailure{{IllustID: 3, Code: "ugoira_frame_mismatch", Path: "q.zip", Missing: []string{"000001.jpg"}}},
	}
	var out bytes.Buffer
	require.NoError(t, writeDownloadReport(&out, report, true))
	lines := strings.Split(strings.TrimSpace(out.String()), "\n")
	require.Len(t, lines, 3)

	var static map[string]any
	require.NoError(t, json.Unmarshal([]byte(lines[0]), &static))
	assert.Equal(t, "illustration", static["kind"])
	assert.Equal(t, float64(1), static["page"])
	assert.Equal(t, float64(10), static["bytes"])

	var ugoira map[string]any
	require.NoError(t, json.Unmarshal([]byte(lines[1]), &ugoira))
	assert.Equal(t, "original", ugoira["quality"])
	assert.NotNil(t, ugoira["frames"])
	assert.NotNil(t, ugoira["frame_report"])
	_, hasPage := ugoira["page"]
	assert.False(t, hasPage)

	var failure map[string]any
	require.NoError(t, json.Unmarshal([]byte(lines[2]), &failure))
	errObject := failure["error"].(map[string]any)
	assert.Equal(t, "ugoira_frame_mismatch", errObject["code"])
	assert.Equal(t, "q.zip", errObject["path"])
}

func TestWriteDownloadReportJSONKeepsResourceOutsideArtworkContract(t *testing.T) {
	report := downloader.DownloadReport{Items: []downloader.DownloadedArtwork{{
		Type:  downloader.DownloadedResourceType,
		Files: []downloader.DownloadedFile{{Path: "c.bin", Page: 1, Bytes: 3}},
	}}}
	var out bytes.Buffer
	require.NoError(t, writeDownloadReport(&out, report, false))
	var records []map[string]any
	require.NoError(t, json.Unmarshal(out.Bytes(), &records))
	require.Len(t, records, 1)
	assert.Equal(t, "resource", records[0]["kind"])
	_, hasID := records[0]["artwork_id"]
	assert.False(t, hasID)
	assert.Equal(t, float64(3), records[0]["bytes"])
}

package downloader_test

import (
	"archive/zip"
	"bytes"
	"context"
	"os"
	"path/filepath"
	"testing"

	downloader "github.com/FlanChanXwO/pixiv-cli/internal/media/downloader"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func ugoiraZipBody(names ...string) []byte {
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	for _, name := range names {
		w, err := zw.Create(name)
		if err != nil {
			t := err
			panic(t)
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

func ugoiraZipMetadata(zipURL string, frames ...string) pixiv.UgoiraMetadata {
	metadata := pixiv.UgoiraMetadata{
		ArtworkID: 9,
		Archives:  []pixiv.UgoiraArchive{{Quality: pixiv.UgoiraQualityOriginal, Resource: testResource(zipURL)}},
	}
	for index, name := range frames {
		metadata.Frames = append(metadata.Frames, pixiv.UgoiraFrame{Filename: name, DelayMilliseconds: 100 + index})
	}
	return metadata
}

func ugoiraZipClient(zipURL string, body []byte, metadata pixiv.UgoiraMetadata) *fakePixivClient {
	return &fakePixivClient{
		details: map[int64]pixiv.Artwork{
			9: {ID: 9, Title: "ugo", PageCount: 1, Kind: pixiv.ArtworkKindUgoira, User: pixiv.User{Name: "author"}},
		},
		ugoira:    map[int64]pixiv.UgoiraMetadata{9: metadata},
		downloads: map[string][]byte{zipURL: body},
	}
}

func TestDownloadUgoiraZipSavesOriginalByteForByte(t *testing.T) {
	dir := t.TempDir()
	zipURL := "https://i.example/ugoira.zip"
	body := ugoiraZipBody("000000.jpg", "000001.jpg")
	client := ugoiraZipClient(zipURL, body, ugoiraZipMetadata(zipURL, "000000.jpg", "000001.jpg"))
	manager := downloader.NewManager(client, dir, "")

	result, err := manager.Download(context.Background(), downloader.DownloadRequest{IllustIDs: []int64{9}, UgoiraFormat: downloader.UgoiraFormatZip})
	require.NoError(t, err)
	require.Len(t, result.Items, 1)
	item := result.Items[0]
	require.Len(t, item.Files, 1)
	assert.Equal(t, "original", item.Quality)
	assert.Equal(t, int64(len(body)), item.Files[0].Bytes)
	require.NotNil(t, item.FrameReport)
	assert.Equal(t, 2, item.FrameReport.Declared)
	assert.Equal(t, 2, item.FrameReport.Actual)
	assert.Empty(t, item.FrameReport.Missing)
	saved, err := os.ReadFile(item.Files[0].Path)
	require.NoError(t, err)
	assert.Equal(t, body, saved, "zip mode must not recompress or rename entries")
	assert.Equal(t, ".zip", filepath.Ext(item.Files[0].Path))
}

func TestDownloadUgoiraZipMissingFrameQuarantines(t *testing.T) {
	dir := t.TempDir()
	zipURL := "https://i.example/ugoira.zip"
	client := ugoiraZipClient(zipURL, ugoiraZipBody("000000.jpg"), ugoiraZipMetadata(zipURL, "000000.jpg", "000001.jpg"))
	manager := downloader.NewManager(client, dir, "")

	result, err := manager.Download(context.Background(), downloader.DownloadRequest{IllustIDs: []int64{9}, UgoiraFormat: downloader.UgoiraFormatRaw})
	require.NoError(t, err)
	assert.Empty(t, result.Items)
	require.Len(t, result.Failures, 1)
	failure := result.Failures[0]
	assert.Equal(t, string(sdk.UgoiraFrameMismatch), failure.Code)
	assert.Equal(t, []string{"000001.jpg"}, failure.Missing)
	assert.Equal(t, filepath.Join(dir, ".quarantine", "9.zip"), failure.Path)
	_, statErr := os.Stat(failure.Path)
	assert.NoError(t, statErr)
}

package ugoira

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func testMetadata() pixiv.UgoiraMetadata {
	return pixiv.UgoiraMetadata{
		ArtworkID: 42,
		Archives: []pixiv.UgoiraArchive{
			{Quality: pixiv.UgoiraQualityMedium},
			{Quality: pixiv.UgoiraQualityOriginal},
		},
		Frames: []pixiv.UgoiraFrame{
			{Filename: "000000.jpg", DelayMilliseconds: 100},
			{Filename: "000001.jpg", DelayMilliseconds: 80},
		},
	}
}

func pooled(deps Dependencies) Dependencies {
	deps.Pooled = func(ctx context.Context, _ Request, attempt func(context.Context, *pixiv.Client) (bool, error)) error {
		_, err := attempt(ctx, nil)
		return err
	}
	if deps.UsageError == nil {
		deps.UsageError = func(err error) error { return err }
	}
	return deps
}

func run(t *testing.T, deps Dependencies, args ...string) (string, error) {
	t.Helper()
	var out bytes.Buffer
	deps.Output = &out
	cmd := New(pooled(deps))
	cmd.SetArgs(args)
	cmd.SetOut(io.Discard)
	cmd.SetErr(io.Discard)
	err := cmd.Execute()
	return out.String(), err
}

func TestUgoiraJSONPutsOriginalFirst(t *testing.T) {
	deps := Dependencies{
		JSONOut: func(*bool) (bool, error) { return true, nil },
		FetchArtwork: func(context.Context, *pixiv.Client, int64) (pixiv.Artwork, error) {
			return pixiv.Artwork{ID: 42, Kind: pixiv.ArtworkKindUgoira}, nil
		},
		FetchUgoiraMetadata: func(context.Context, *pixiv.Client, int64) (pixiv.UgoiraMetadata, error) { return testMetadata(), nil },
	}
	out, err := run(t, deps, "42", "--json")
	require.NoError(t, err)
	var dto pixiv.UgoiraMetadataDTO
	require.NoError(t, json.Unmarshal([]byte(out), &dto))
	assert.Equal(t, int64(42), dto.ArtworkID)
	require.Len(t, dto.Archives, 2)
	assert.Equal(t, pixiv.UgoiraQualityOriginal, dto.Archives[0].Quality)
	assert.Equal(t, pixiv.UgoiraQualityMedium, dto.Archives[1].Quality)
	require.Len(t, dto.Frames, 2)
	assert.Equal(t, "000000.jpg", dto.Frames[0].Filename)
	assert.Equal(t, 100, dto.Frames[0].DelayMilliseconds)
}

func TestUgoiraRejectsNonUgoiraBeforeMetadata(t *testing.T) {
	deps := Dependencies{
		JSONOut: func(*bool) (bool, error) { return false, nil },
		FetchArtwork: func(context.Context, *pixiv.Client, int64) (pixiv.Artwork, error) {
			return pixiv.Artwork{ID: 7, Kind: pixiv.ArtworkKindManga}, nil
		},
		FetchUgoiraMetadata: func(context.Context, *pixiv.Client, int64) (pixiv.UgoiraMetadata, error) {
			t.Fatal("metadata must not be fetched for a non-ugoira artwork")
			return pixiv.UgoiraMetadata{}, nil
		},
	}
	_, err := run(t, deps, "7")
	require.Error(t, err)
	assert.Equal(t, sdk.NotUgoira, sdk.ReasonOf(err))
}

func TestUgoiraHumanOutput(t *testing.T) {
	deps := Dependencies{
		JSONOut: func(*bool) (bool, error) { return false, nil },
		FetchArtwork: func(context.Context, *pixiv.Client, int64) (pixiv.Artwork, error) {
			return pixiv.Artwork{ID: 42, Kind: pixiv.ArtworkKindUgoira}, nil
		},
		FetchUgoiraMetadata: func(context.Context, *pixiv.Client, int64) (pixiv.UgoiraMetadata, error) { return testMetadata(), nil },
	}
	out, err := run(t, deps, "42")
	require.NoError(t, err)
	assert.True(t, strings.Contains(out, "frames: 2"), out)
	assert.True(t, strings.Contains(out, "000000.jpg 100ms"), out)
}

func TestUgoiraRequiresOneArgument(t *testing.T) {
	deps := Dependencies{JSONOut: func(*bool) (bool, error) { return false, nil }}
	_, err := run(t, deps)
	require.Error(t, err)
}

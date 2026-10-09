package cli

import (
	"flag"
	"testing"
)

var updateBookmarkListsRecords = flag.Bool("migration-update-bookmark-lists-records", false, "capture bookmark target record and raw stdin boundaries")

func TestMigrationBookmarkListsRecordsPreservesJSONAndRawStdinBoundaries(t *testing.T) {
	var rows []bookmarkListsRow
	inputs := []string{
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":" +43 ","type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":"00043","type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":4.3e1,"type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43.0,"type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/44"}`,
		`{"id":43,"type":"artwork","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"novel","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"User","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43/bookmarks/artworks"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43/bookmarks/novels"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/artworks/43"}`,
		`{"id":0,"id":43,"type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"id":0,"type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":null,"id":43,"type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"artwork","type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","type":"artwork","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","url":"bad","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43","url":null}`,
		`{"ID":43,"TYPE":"user","URL":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43","extra":"\ud800","version":1}`,
		`{"id":"\ud800","type":"user","url":"https://www.pixiv.net/users/43"}`,
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43"}` + "\n\n",
		`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43"}` + "\n" + `{"id":44,"type":"user","url":"https://www.pixiv.net/users/44"}`,
	}
	rawInputs := [][]byte{{0xff}, {'4', '3', 0xff, '\n'}, append([]byte(`{"id":43,"type":"user","url":"https://www.pixiv.net/users/43","extra":"`), append([]byte{0xff}, []byte(`"}`)...)...), append([]byte(`{"id":"`), append([]byte{0xff}, []byte(`","type":"user","url":"https://www.pixiv.net/users/43"}`)...)...), append([]byte(`{"id":43,"type":"user","url":"https://www.pixiv.net/users/`), append([]byte{0xff}, []byte(`"}`)...)...)}
	for _, kind := range []string{"artwork", "novel", "all", "user"} {
		for _, input := range inputs {
			for _, args := range [][]string{{"--limit=1"}, {"42", "--limit=1"}, {"--page=0"}, {"--proxy=", "--no-proxy"}} {
				for _, mode := range []string{"human", "json", "ndjson"} {
					current := bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), Input: input, Identity: 42, Mode: mode, Bodies: bookmarkListsBodies(kind)}
					if mode != "human" {
						current.Args = append(current.Args, "--"+mode)
					}
					rows = append(rows, bookmarkListsCapture(t, current))
				}
			}
		}
		for _, input := range rawInputs {
			for _, args := range [][]string{{}, {"42"}} {
				for _, mode := range []string{"human", "json", "ndjson"} {
					current := bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), InputBytes: input, Identity: 42, Mode: mode, Bodies: bookmarkListsBodies(kind)}
					if mode != "human" {
						current.Args = append(current.Args, "--"+mode)
					}
					rows = append(rows, bookmarkListsCapture(t, current))
				}
			}
		}
		for _, args := range [][]string{{}, {"42"}} {
			for _, mode := range []string{"human", "json", "ndjson"} {
				current := bookmarkListsRow{Kind: kind, Args: append([]string{}, args...), ReadError: true, Identity: 42, Mode: mode, Bodies: bookmarkListsBodies(kind)}
				if mode != "human" {
					current.Args = append(current.Args, "--"+mode)
				}
				rows = append(rows, bookmarkListsCapture(t, current))
			}
		}
	}
	recommendedFixture(t, "cli-bookmark-lists-records.json", rows, *updateBookmarkListsRecords)
}

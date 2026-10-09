package cli

import (
	"flag"
	"strings"
	"testing"
)

var updateBookmarkReadsRecords = flag.Bool("migration-update-bookmark-reads-records", false, "capture bookmark detail and tag record and raw stdin boundaries")

func TestMigrationBookmarkReadsRecordsPreservesTextRecordsAndRawStdin(t *testing.T) {
	var rows []bookmarkReadsRow
	inputs := []string{"", "\n", "\r\n", "42\n", "42\r\n", " 42 \n", "42\n\n", "42\r", "42\n43\n", "\n42\n", "https://www.pixiv.net/artworks/43\r\n", "https://www.pixiv.net/novel/show.php?id=43\n", "https://www.pixiv.net/users/43\n", "https://www.pixiv.net/users/43/bookmarks/artworks\n", "https://www.pixiv.net/users/43/bookmarks/novels\n", "{", "[]", "null", `{"id":"43"}`, `{"id":"43","type":"user","url":"anything"}`, `{"id":43,"type":"artwork","url":"anything"}`, `{"id":43,"type":"novel","url":"anything"}`, `{"id":43,"type":null,"url":"anything"}`, `{"id":43,"type":"artwork","url":""}`, `{"id":0,"type":"artwork","url":"anything"}`, `{"id":1.5,"type":"artwork","url":"anything"}`, `{"id":9223372036854775808,"type":"artwork","url":"anything"}`, `{"id":43,"type":"artwork","url":"anything"} trailing`}
	for _, typ := range []string{"artwork", "illust", "manga", "ugoira", "novel", "user", "Artwork", "User", ""} {
		inputs = append(inputs, `{"id":"43","type":"`+typ+`","url":"https://www.pixiv.net/artworks/43"}`)
		inputs = append(inputs, `{"id":"43","type":"`+typ+`","url":"https://www.pixiv.net/novel/show.php?id=43"}`)
		inputs = append(inputs, `{"id":"43","type":"`+typ+`","url":"https://www.pixiv.net/users/43"}`)
	}
	for _, id := range []string{`43`, `" +43 "`, `"00043"`, `4.3e1`, `43.0`, `null`, `true`, `[]`} {
		inputs = append(inputs, `{"id":`+id+`,"type":"artwork","url":"https://www.pixiv.net/artworks/43"}`, `{"id":`+id+`,"type":"novel","url":"https://www.pixiv.net/novel/show.php?id=43"}`, `{"id":`+id+`,"type":"user","url":"https://www.pixiv.net/users/43"}`)
	}
	inputs = append(inputs, `{"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/44"}`, `{"id":43,"type":"user","url":"https://www.pixiv.net/users/44"}`, `{"id":0,"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/43"}`, `{"id":43,"id":0,"type":"artwork","url":"https://www.pixiv.net/artworks/43"}`, `{"ID":43,"TYPE":"artwork","URL":"https://www.pixiv.net/artworks/43"}`, `{"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/43","extra":"\ud800","version":1}`, `{"id":"\ud800","type":"artwork","url":"https://www.pixiv.net/artworks/43"}`, `{"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/43"}`+"\n\n", `{"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/43"}`+"\n"+`{"id":44,"type":"artwork","url":"https://www.pixiv.net/artworks/44"}`, "\n "+`{"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/43"}`+"\r\n")
	rawInputs := [][]byte{{0xff}, {'4', '3', 0xff, '\n'}, append([]byte(`{"id":43,"type":"artwork","url":"https://www.pixiv.net/artworks/43","extra":"`), append([]byte{0xff}, []byte(`"}`)...)...), append([]byte(`{"id":"`), append([]byte{0xff}, []byte(`","type":"artwork","url":"https://www.pixiv.net/artworks/43"}`)...)...), append([]byte(`{"id":43,"type":"user","url":"https://www.pixiv.net/users/`), append([]byte{0xff}, []byte(`"}`)...)...)}
	for _, operation := range []string{"detail", "tags"} {
		kinds := []string{"artwork", "novel"}
		if operation == "tags" {
			kinds = append(kinds, "all")
		}
		for _, kind := range kinds {
			for _, input := range inputs {
				for _, args := range [][]string{{}, {"42"}, {"--type=invalid"}, {"--proxy=", "--no-proxy"}} {
					for _, mode := range []string{"human", "json", "ndjson", "auto"} {
						current := bookmarkReadsCase(operation, kind, mode, args)
						current.Input = input
						captured := bookmarkReadsCapture(t, current)
						if input == "42\n" && len(args) == 0 && mode == "human" && (captured.Exit != 0 || captured.Error != "" || captured.Stdout == "" || captured.Bytes != len(input) || len(captured.Paths) != 1) {
							t.Fatalf("text input did not reach %s/%s read: %#v", operation, kind, captured)
						}
						rows = append(rows, captured)
					}
				}
			}
			for _, input := range rawInputs {
				for _, args := range [][]string{{}, {"42"}} {
					for _, mode := range []string{"human", "json", "ndjson"} {
						current := bookmarkReadsCase(operation, kind, mode, args)
						current.InputBytes = input
						rows = append(rows, bookmarkReadsCapture(t, current))
					}
				}
			}
			for _, args := range [][]string{{}, {"42"}} {
				for _, mode := range []string{"human", "json", "ndjson"} {
					current := bookmarkReadsCase(operation, kind, mode, args)
					current.ReadError = true
					rows = append(rows, bookmarkReadsCapture(t, current))
				}
			}
			for _, size := range []int{65535, 65536, 65537} {
				current := bookmarkReadsCase(operation, kind, "json", nil)
				current.Input = strings.Repeat(" ", size) + "42\n"
				rows = append(rows, bookmarkReadsCapture(t, current))
			}
		}
	}
	recommendedFixture(t, "cli-bookmark-reads-records.json", rows, *updateBookmarkReadsRecords)
}

package cli

import (
	"encoding/json"
	"flag"
	"fmt"
	"testing"
)

var updateBookmarkReadsBodies = flag.Bool("migration-update-bookmark-reads-bodies", false, "capture bookmark detail and tag body shape, status, and continuation boundaries")

func TestMigrationBookmarkReadsBodiesPreservesEmptyShapesStatusesAndContinuation(t *testing.T) {
	var rows []bookmarkReadsRow
	for _, kind := range []string{"artwork", "novel"} {
		for _, body := range []string{`{}`, `null`, `{"bookmark_detail":null}`, `{"bookmark_detail":{}}`, `{"bookmark_detail":{"is_bookmarked":false,"restrict":"public","tags":[{"name":"ignored","is_registered":true}]}}`, `{"bookmark_detail":{"restrict":"private","tags":[{"name":"cat","is_registered":true},{"name":"skip"},{"name":"cat","is_registered":true},{"name":"","is_registered":true},null]}}`, `{"bookmark_detail":{"is_bookmarked":null,"restrict":"friends","tags":null}}`, `{"bookmark_detail":{"is_bookmarked":true,"restrict":null,"tags":[{"name":null,"is_registered":true},{"name":"ignored","is_registered":null}]}}`, `{"bookmark_detail":false}`, `{"bookmark_detail":[]}`, `{"bookmark_detail":{"is_bookmarked":"true"}}`, `{"bookmark_detail":{"restrict":1}}`, `{"bookmark_detail":{"tags":{}}}`, `{"bookmark_detail":{"tags":[1]}}`, `{"bookmark_detail":{"tags":[{"name":1}]}}`, `{"bookmark_detail":{"tags":[{"is_registered":"true"}]}}`, `{"bookmark_detail":{"is_bookmarked":false,"tags":[{"name":1}]}}`, `[]`, `not JSON`, ``, `{"bookmark_detail":{"is_bookmarked":true,"restrict":"public","tags":[]}}`} {
			for _, mode := range []string{"human", "json", "auto"} {
				current := bookmarkReadsCase("detail", kind, mode, []string{"42"})
				current.WireBody = &body
				rows = append(rows, bookmarkReadsCapture(t, current))
			}
		}
	}
	for _, kind := range []string{"artwork", "novel", "all"} {
		for _, body := range []string{`{}`, `null`, `[]`, `{"bookmark_tags":null}`, `{"bookmark_tags":[]}`, `{"bookmark_tags":{}}`, `{"bookmark_tags":[null]}`, `{"bookmark_tags":[{}]}`, `{"bookmark_tags":[{"name":null}]}`, `{"bookmark_tags":[{"name":1}]}`, `{"bookmark_tags":[{"name":"cat","count":"3"}]}`, `{"bookmark_tags":[{"name":"cat","count":1.5}]}`, `{"bookmark_tags":[{"name":"cat","count":9223372036854775808}]}`, `{"bookmark_tags":[{"name":"cat","count":null}]}`, `{"bookmark_tags":[{"name":"cat"}]}`, `{"bookmark_tags":[{"name":"cat","count":1},{"name":"cat","count":1}]}`, `{"bookmark_tags":[],"next_url":1}`, `{"bookmark_tags":[],"next_url":""}`, `{"bookmark_tags":[],"next_url":"https://app-api.pixiv.net/next"}`, `not JSON`, ``} {
			for _, mode := range []string{"human", "json", "ndjson", "auto"} {
				current := bookmarkReadsCase("tags", kind, mode, []string{"42"})
				current.WireBody = &body
				captured := bookmarkReadsCapture(t, current)
				expectedRequests := 1
				if kind == "all" {
					expectedRequests = 2
				}
				if body == `{"bookmark_tags":[]}` && (captured.Exit != 0 || captured.Error != "" || len(captured.Paths) != expectedRequests || mode != "json" && captured.Stdout != "") {
					t.Fatalf("empty tag success failed: %#v", captured)
				}
				rows = append(rows, captured)
			}
		}
		for scenario := 0; scenario < 7; scenario++ {
			for _, mode := range []string{"human", "json", "ndjson", "auto"} {
				current := bookmarkReadsCase("tags", kind, mode, []string{"42", "--limit=0"})
				for index := 0; index < len(current.Bodies); index += 2 {
					endpoint := "/v1/user/bookmark-tags/illust"
					if kind == "novel" || kind == "all" && index == 2 {
						endpoint = "/v1/user/bookmark-tags/novel"
					}
					switch scenario {
					case 0:
						current.Bodies[index] = json.RawMessage(fmt.Sprintf(`{"bookmark_tags":[],"next_url":"https://app-api.pixiv.net%s?offset=30"}`, endpoint))
					case 1:
						current.Bodies[index+1] = json.RawMessage(`{"bookmark_tags":null}`)
					case 2:
						current.Bodies[index+1] = current.Bodies[index]
					case 3:
						current.Bodies[index], current.Bodies[index+1] = json.RawMessage(`{"bookmark_tags":[],"next_url":null}`), json.RawMessage(`{"bookmark_tags":[],"next_url":null}`)
					case 4:
						current.Bodies[index+1] = json.RawMessage(`{"bookmark_tags":{}}`)
					case 5:
						current.Bodies[index] = json.RawMessage(`{"bookmark_tags":[{"name":"x","count":1}],"next_url":"https://app-api.pixiv.net/next"}`)
					case 6:
						current.Bodies[index] = json.RawMessage(fmt.Sprintf(`{"bookmark_tags":[{"name":"x","count":1}],"next_url":"https://app-api.pixiv.net%s?offset=30"}`, endpoint))
					}
				}
				rows = append(rows, bookmarkReadsCapture(t, current))
			}
		}
	}
	for _, operation := range []string{"detail", "tags"} {
		kinds := []string{"artwork", "novel"}
		if operation == "tags" {
			kinds = append(kinds, "all")
		}
		for _, kind := range kinds {
			for _, status := range []int{204, 400, 401, 403, 404, 410, 429, 500} {
				for _, mode := range []string{"human", "json", "auto"} {
					current := bookmarkReadsCase(operation, kind, mode, []string{"42"})
					current.Status = status
					body := "not JSON"
					current.WireBody = &body
					rows = append(rows, bookmarkReadsCapture(t, current))
				}
			}
		}
	}
	recommendedFixture(t, "cli-bookmark-reads-bodies.json", rows, *updateBookmarkReadsBodies)
}

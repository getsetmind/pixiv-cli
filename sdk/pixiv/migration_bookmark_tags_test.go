package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateBookmarkTags = flag.Bool("migration-update-bookmark-tags", false, "capture fixed Go novel bookmark tag contracts")

func TestMigrationNovelBookmarkTagsMatchFrozenValidationAndDTOs(t *testing.T) {
	type row struct {
		ID       int64                  `json:"id"`
		Restrict string                 `json:"restrict"`
		Cursor   bool                   `json:"cursor"`
		Status   int                    `json:"status"`
		Body     string                 `json:"body"`
		Items    []pixiv.BookmarkTagDTO `json:"items"`
		Next     string                 `json:"next"`
		Error    json.RawMessage        `json:"error"`
		Requests []string               `json:"requests"`
	}
	rows := []row{}
	for _, body := range []string{
		`{"bookmark_tags":[]}`, `{"bookmark_tags":[{"name":"日本語","count":3},{"name":"日本語","count":-1},{"name":" ","count":9223372036854775807}]}`,
		`{"bookmark_tags":[{"name":"story","count":null}],"next_url":null}`, `{"bookmark_tags":[{"name":"story"}]}`,
		`{}`, `null`, `[]`, `{"bookmark_tags":null}`, `{"bookmark_tags":{}}`, `{"bookmark_tags":[null]}`, `{"bookmark_tags":[{}]}`,
		`{"bookmark_tags":[{"name":null}]}`, `{"bookmark_tags":[{"name":1}]}`, `{"bookmark_tags":[{"name":"story","count":"3"}]}`,
		`{"bookmark_tags":[{"name":"story","count":1.5}]}`, `{"bookmark_tags":[{"name":"story","count":9223372036854775808}]}`,
		`{"bookmark_tags":[],"next_url":""}`, `{"bookmark_tags":[],"next_url":"https://app-api.pixiv.net/next"}`, `{"bookmark_tags":[],"next_url":1}`, `not JSON`,
	} {
		rows = append(rows, row{ID: 8, Status: 200, Body: body})
	}
	for _, restrict := range []string{"", "public", "private", "friends", " public", "PUBLIC"} {
		rows = append(rows, row{ID: 8, Restrict: restrict, Status: 200, Body: `{"bookmark_tags":[]}`})
	}
	for _, status := range []int{204, 400, 401, 403, 404, 410, 429, 500} {
		rows = append(rows, row{ID: 8, Status: status, Body: "not JSON"})
	}
	rows = append(rows, row{ID: 0, Restrict: "friends", Cursor: true}, row{ID: -1}, row{ID: 8, Restrict: "friends", Cursor: true}, row{ID: 8, Cursor: true})
	for index := range rows {
		row := &rows[index]
		row.Requests = []string{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			row.Requests = append(row.Requests, req.Method+" "+req.URL.RequestURI())
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		cursor := sdk.Cursor{}
		if row.Cursor {
			cursor, err = sdk.NewCursor("pixiv", "UserNovelBookmarkTags", 1, "digest", []byte("payload"))
			if err != nil {
				t.Fatal(err)
			}
		}
		page, err := client.UserNovelBookmarkTags(context.Background(), pixiv.UserNovelBookmarkTagsRequest{UserID: row.ID, Restrict: pixiv.Restrict(row.Restrict), Cursor: cursor})
		if err != nil {
			row.Error, err = json.Marshal(err)
			if err != nil {
				t.Fatal(err)
			}
		} else {
			row.Items = make([]pixiv.BookmarkTagDTO, 0, len(page.Items))
			for _, item := range page.Items {
				row.Items = append(row.Items, pixiv.ToBookmarkTagDTO(item))
			}
			row.Next = page.Next.String()
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "novel-bookmark-tags.json")
	if *migrationUpdateBookmarkTags {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("novel bookmark tags differ from fixed Go reference")
	}
}

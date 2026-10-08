package pixiv_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
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

var migrationUpdateArtworkTags = flag.Bool("migration-update-artwork-tags", false, "capture fixed Go artwork bookmark tag pagination")

func TestMigrationArtworkBookmarkTagsMatchFrozenPagesAndCursors(t *testing.T) {
	type row struct {
		ID       int64                    `json:"id"`
		Restrict string                   `json:"restrict"`
		Cursor   string                   `json:"cursor"`
		Bodies   []string                 `json:"bodies"`
		Items    [][]pixiv.BookmarkTagDTO `json:"items"`
		Next     []string                 `json:"next"`
		Error    json.RawMessage          `json:"error"`
		Requests []string                 `json:"requests"`
	}
	rows := []row{}
	for _, body := range []string{`{"bookmark_tags":[]}`, `{"bookmark_tags":[{"name":"cat","count":3},{"name":"cat","count":-1},{"name":"日本語","count":9223372036854775807}]}`, `{"bookmark_tags":[{"name":" ","count":null}],"next_url":null}`, `{}`, `null`, `{"bookmark_tags":null}`, `{"bookmark_tags":[null]}`, `{"bookmark_tags":[{}]}`, `{"bookmark_tags":[{"name":1}]}`, `{"bookmark_tags":[{"name":"cat","count":1.5}]}`, `{"bookmark_tags":[{"name":"cat","count":9223372036854775808}]}`, `not JSON`} {
		rows = append(rows, row{ID: 8, Bodies: []string{body}})
	}
	base := "https://app-api.pixiv.net/v1/user/bookmark-tags/illust"
	for _, next := range []string{base + "?offset=2", base + "?offset=0002", base + "?offset=%2B2", base + "?offset=9223372036854775807", base + "?offset=2&user_id=99&restrict=private", base + "?offset=2&user_id=&restrict=", base + "?offset=2#", base + "?offset=2&&", base + "?offset=0", base + "?offset=-1", base + "?offset=9223372036854775808", base + "?offset=1.0", base + "?offset=+2", base + "?offset=", base, base + "?offset=2&offset=3", base + "?offset=2&user_id=8&user_id=8", base + "?offset=2&extra=1", base + "?offset=2;user_id=8", base + "?offset=%xx", base + "?offset=2#x", "", strings.Replace(base, "https:", "http:", 1) + "?offset=2", strings.Replace(base, "app-api.pixiv.net", "APP-API.PIXIV.NET", 1) + "?offset=2", strings.Replace(base, "app-api.pixiv.net", "app-api.pixiv.net:443", 1) + "?offset=2", strings.Replace(base, "app-api.pixiv.net", "x@app-api.pixiv.net", 1) + "?offset=2", strings.Replace(base, "/illust", "/%69llust", 1) + "?offset=2", "/v1/user/bookmark-tags/illust?offset=2"} {
		data, err := json.Marshal(map[string]any{"bookmark_tags": []any{}, "next_url": next})
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{ID: 8, Bodies: []string{string(data)}})
	}
	sum := sha256.Sum256([]byte("restrict=&user_id=8&"))
	digest := hex.EncodeToString(sum[:])
	for _, payload := range []string{`{"k":"offset","v":2}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"other","v":2}`, `{"k":"offset","v":2,"s":-1}`, `{"k":"offset","v":2,"p":{"x":["y"]}}`, `{"p":{"offset":["2"]}}`, `{"k":"offset","v":null}`, `{"k":"offset","v":2,"s":3}`, `{"k":"offset","v":"2"}`, `null`, `not JSON`} {
		cursor, err := sdk.NewCursor("pixiv", "UserArtworkBookmarkTags", 1, digest, []byte(payload))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{ID: 8, Cursor: cursor.String(), Bodies: []string{`{"bookmark_tags":[]}`}})
	}
	for _, binding := range []struct {
		product, operation, digest string
		version                    int
	}{{"pixiv", "UserNovelBookmarkTags", digest, 1}, {"fanbox", "UserArtworkBookmarkTags", digest, 1}, {"pixiv", "UserArtworkBookmarkTags", digest, 2}, {"pixiv", "UserArtworkBookmarkTags", "other", 1}} {
		cursor, err := sdk.NewCursor(binding.product, binding.operation, binding.version, binding.digest, []byte(`{"k":"offset","v":2}`))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{ID: 8, Cursor: cursor.String(), Bodies: []string{`{"bookmark_tags":[]}`}})
	}
	rows = append(rows, row{ID: 8, Cursor: rows[len(rows)-1].Cursor, Bodies: []string{`{"bookmark_tags":[]}`}}, row{ID: 0, Restrict: "friends", Cursor: rows[len(rows)-1].Cursor}, row{ID: 8, Restrict: "friends", Cursor: rows[len(rows)-1].Cursor}, row{ID: 8, Restrict: "private", Bodies: []string{`{"bookmark_tags":[]}`}}, row{ID: 8, Bodies: []string{`{"bookmark_tags":[{"name":"first","count":1}],"next_url":"` + base + `?offset=2"}`, `{"bookmark_tags":[],"next_url":"` + base + `?offset=4"}`, `{"bookmark_tags":[{"name":"last","count":2}]}`}})
	for index := range rows {
		row := &rows[index]
		row.Items = [][]pixiv.BookmarkTagDTO{}
		row.Next = []string{}
		row.Requests = []string{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			position := len(row.Requests)
			row.Requests = append(row.Requests, req.Method+" "+req.URL.RequestURI())
			if position >= len(row.Bodies) {
				t.Fatal("unexpected request")
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(row.Bodies[position])), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		cursor := sdk.Cursor{}
		if row.Cursor != "" {
			cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for range row.Bodies {
			page, callErr := client.UserArtworkBookmarkTags(context.Background(), pixiv.UserArtworkBookmarkTagsRequest{UserID: row.ID, Restrict: pixiv.Restrict(row.Restrict), Cursor: cursor})
			if callErr != nil {
				row.Error, err = json.Marshal(callErr)
				if err != nil {
					t.Fatal(err)
				}
				break
			}
			dtos := make([]pixiv.BookmarkTagDTO, 0, len(page.Items))
			for _, item := range page.Items {
				dtos = append(dtos, pixiv.ToBookmarkTagDTO(item))
			}
			row.Items = append(row.Items, dtos)
			row.Next = append(row.Next, page.Next.String())
			cursor = page.Next
		}
		if len(row.Bodies) == 0 {
			_, err = client.UserArtworkBookmarkTags(context.Background(), pixiv.UserArtworkBookmarkTagsRequest{UserID: row.ID, Restrict: pixiv.Restrict(row.Restrict), Cursor: cursor})
			if err == nil {
				t.Fatal("validation unexpectedly succeeded")
			}
			row.Error, err = json.Marshal(err)
			if err != nil {
				t.Fatal(err)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "artwork-bookmark-tags.json")
	if *migrationUpdateArtworkTags {
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
		t.Fatal("artwork bookmark tag pages differ from fixed Go reference")
	}
}

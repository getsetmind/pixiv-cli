package pixiv_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type migrationBookmarkListsResult struct {
	Items   []any      `json:"items"`
	Cursor  any        `json:"cursor"`
	Reason  sdk.Reason `json:"reason"`
	Message string     `json:"message"`
}

var migrationUpdateBookmarkLists = flag.Bool("migration-update-bookmark-lists", false, "capture user artwork and novel bookmark list contracts")

type migrationBookmarkListsRow struct {
	Restrict   string                         `json:"restrict"`
	Tag        string                         `json:"tag"`
	Name       string                         `json:"name"`
	Operation  string                         `json:"operation"`
	ID         int64                          `json:"id"`
	UserID     int64                          `json:"user_id"`
	Cursor     string                         `json:"cursor"`
	NextMode   string                         `json:"next_mode"`
	Bodies     []json.RawMessage              `json:"bodies"`
	Results    []migrationBookmarkListsResult `json:"results"`
	Queries    []url.Values                   `json:"queries"`
	RawQueries []string                       `json:"raw_queries"`
}

func TestMigrationBookmarkListsPreserveContract(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	var rows []migrationBookmarkListsRow
	for _, op := range []string{"UserArtworkBookmarks", "UserNovelBookmarks"} {
		key, endpoint := "novels", "/v1/user/bookmarks/novel"
		item := json.RawMessage(`{"id":9301,"title":"story","caption":"caption","user":{"id":31,"name":"writer","account":"author","comment":"hello","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"tags":[{"name":"tag","translated_name":"翻訳"}],"create_date":"2026-09-01T03:04:05.123456789+09:00","x_restrict":1,"text_length":1234,"is_original":true,"total_bookmarks":12,"total_view":34,"image_urls":{"original":"https://i.pximg.net/novel.jpg"}}`)
		base := "restrict=&user_id=123&"
		if op == "UserArtworkBookmarks" {
			key, endpoint = "illusts", "/v1/user/bookmarks/illust"
			base = "restrict=&user_id=123&"
			item = json.RawMessage(`{"id":123,"title":"sample","type":"manga","user":{"id":31},"create_date":"2026-09-01T00:00:00Z","page_count":2,"width":100,"height":200,"illust_ai_type":2,"ai_type":1,"tools":["pen"],"image_urls":{"medium":"https://i.pximg.net/art.jpg"},"meta_pages":[{"image_urls":{"original":"https://i.pximg.net/page1.jpg"}},{"image_urls":{"original":"https://i.pximg.net/page2.jpg"}}]}`)
		}
		batch := func(next any) json.RawMessage {
			data, err := json.Marshal(map[string]any{key: []json.RawMessage{item}, "next_url": next})
			if err != nil {
				t.Fatal(err)
			}
			return data
		}
		final := batch(nil)
		prefix := "https://app-api.pixiv.net" + endpoint + "?"
		for _, next := range []any{nil, "", prefix + "max_bookmark_id=30", prefix + "max_bookmark_id=0", prefix + "max_bookmark_id=-1", prefix + "max_bookmark_id=%2B30", prefix + "max_bookmark_id=0030", prefix + "max_bookmark_id=9223372036854775807", prefix + "max_bookmark_id=9223372036854775808", prefix + "max_bookmark_id=30&max_bookmark_id=40", prefix + "max_bookmark_id=30&unknown=1", prefix + "max_bookmark_id=30#fragment", prefix + "max_bookmark_id=%zz", prefix + "max_bookmark_id=30;x=1", "http://app-api.pixiv.net" + endpoint + "?max_bookmark_id=30", "HTTPS://app-api.pixiv.net" + endpoint + "?max_bookmark_id=30", "https://APP-API.PIXIV.NET" + endpoint + "?max_bookmark_id=30", "https://app-api.pixiv.net:443" + endpoint + "?max_bookmark_id=30", "https://app-api.pixiv.net:" + endpoint + "?max_bookmark_id=30", "https://user@app-api.pixiv.net" + endpoint + "?max_bookmark_id=30", prefix + "max_bookmark_id=30#", prefix + "max_bookmark_id=30&user_id=456", prefix + "max_bookmark_id=30&filter=changed", prefix + "max_bookmark_id=30&restrict=", prefix + "max_bookmark_id=30&tag=", prefix + "max_bookmark_id=30&user_id=", prefix + "max_bookmark_id=30&restrict=changed", prefix + "max_bookmark_id=30&tag=changed", 1} {
			rows = append(rows, migrationBookmarkListsRow{Name: op + fmt.Sprintf(":next:%v", next), Operation: op, ID: 123, Bodies: []json.RawMessage{batch(next), final}})
		}
		for _, id := range []int64{0, -1, 9223372036854775807} {
			rows = append(rows, migrationBookmarkListsRow{Name: fmt.Sprintf("%s:id:%d", op, id), Operation: op, ID: id, Restrict: "bad", Bodies: []json.RawMessage{final}})
		}
		for _, restrict := range []string{"", "public", "private", "unknown", "Public", " public", "private "} {
			for _, tag := range []string{"", "cat", "空 白+&/=~!*"} {
				rows = append(rows, migrationBookmarkListsRow{Name: op + ":restrict:" + restrict + ":tag:" + tag, Operation: op, ID: 123, Restrict: restrict, Tag: tag, Bodies: []json.RawMessage{batch(prefix + "max_bookmark_id=30"), final}})
			}
		}

		for _, body := range []string{`{}`, fmt.Sprintf(`{"%s":null}`, key), fmt.Sprintf(`{"%s":[]}`, key), fmt.Sprintf(`{"%s":{}}`, key), fmt.Sprintf(`{"%s":[null]}`, key), fmt.Sprintf(`{"%s":[{}]}`, key), fmt.Sprintf(`{"%s":[{"id":-1}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"title":1}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"create_date":"invalid"}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":0},"create_date":"2026-09-01T00:00:00Z"}]}`, key)} {
			rows = append(rows, migrationBookmarkListsRow{Name: op + ":body:" + body, Operation: op, ID: 123, Bodies: []json.RawMessage{[]byte(body)}})
		}
		rows = append(rows, migrationBookmarkListsRow{Name: op + ":id:max-valid", Operation: op, ID: 9223372036854775807, Bodies: []json.RawMessage{final}})
		for _, field := range []string{"title", "caption", "create_date", "user", "tags", "image_urls", "total_bookmarks"} {
			for _, value := range []json.RawMessage{json.RawMessage(`null`), json.RawMessage(`42`)} {
				var altered map[string]json.RawMessage
				if err := json.Unmarshal(item, &altered); err != nil {
					t.Fatal(err)
				}
				altered[field] = value
				modified, err := json.Marshal(map[string]any{key: []any{altered}})
				if err != nil {
					t.Fatal(err)
				}
				rows = append(rows, migrationBookmarkListsRow{Name: op + ":field:" + field + ":" + string(value), Operation: op, ID: 123, Bodies: []json.RawMessage{modified}})
			}
		}
		for _, body := range []string{`null`, `[]`, `42`, `"text"`, fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"create_date":"invalid"}],"next_url":"bad"}`, key), fmt.Sprintf(`{"%s":[{"id":0,"user":{"id":0}}],"next_url":42}`, key)} {
			rows = append(rows, migrationBookmarkListsRow{Name: op + ":envelope:" + body, Operation: op, ID: 123, Bodies: []json.RawMessage{[]byte(body)}})
		}

		digest := fmt.Sprintf("%x", sha256.Sum256([]byte(base)))
		for _, payload := range []string{`{"k":"max_bookmark_id","v":30}`, `{"k":"max_bookmark_id","v":0}`, `{"k":"other","v":30}`, `{"k":"other","v":0}`, `{"k":"max_bookmark_id","v":9223372036854775807}`, `{"k":"max_bookmark_id","v":30,"s":1}`, `{"k":"max_bookmark_id","v":-1}`, `{"k":"max_bookmark_id","v":30,"s":-1}`, `{"k":"max_bookmark_id","v":30,"p":{"x":["1"]}}`, `{"p":{"max_bookmark_id":["30"]}}`, `{}`, `null`, `{"k":"max_bookmark_id","v":"30"}`} {
			for _, identity := range []string{"0", "foreign"} {
				cur, err := sdk.NewCursor("pixiv", op, 1, digest, []byte(payload), sdk.WithCursorIdentity(identity))
				if err != nil {
					t.Fatal(err)
				}
				rows = append(rows, migrationBookmarkListsRow{Name: op + ":cursor:" + identity + ":" + payload, Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
			}
		}
		for _, version := range []int{2, 3} {
			cur, err := sdk.NewCursor("pixiv", op, version, digest, []byte(`{"k":"max_bookmark_id","v":30}`))
			if err != nil {
				t.Fatal(err)
			}
			rows = append(rows, migrationBookmarkListsRow{Name: fmt.Sprintf("%s:version:%d", op, version), Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
		}
		for _, mismatch := range []string{"product", "operation", "digest", "invalid_id_first", "invalid_restrict_first"} {
			product, cursorOp, cursorDigest := "pixiv", op, digest
			id, restrict := int64(123), ""
			switch mismatch {
			case "product":
				product = "fanbox"
			case "operation":
				cursorOp = "UserArtworks"
			case "digest":
				cursorDigest = "foreign"
			case "invalid_id_first":
				id, restrict, cursorDigest = 0, "bad", "foreign"
			case "invalid_restrict_first":
				restrict, cursorDigest = "bad", "foreign"
			}
			cur, err := sdk.NewCursor(product, cursorOp, 1, cursorDigest, []byte(`{"k":"max_bookmark_id","v":30}`))
			if err != nil {
				t.Fatal(err)
			}
			rows = append(rows, migrationBookmarkListsRow{Name: op + ":mismatch:" + mismatch, Operation: op, ID: id, Restrict: restrict, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
		}

		for _, mode := range []string{"same_account", "other_account", "anonymous_account", "other_client", "changed_id", "changed_restrict", "changed_tag"} {
			rows = append(rows, migrationBookmarkListsRow{Name: op + ":binding:" + mode, Operation: op, ID: 123, UserID: 7, NextMode: mode, Bodies: []json.RawMessage{batch(prefix + "max_bookmark_id=30"), final}})
		}
	}

	for index := range rows {
		row := &rows[index]
		row.Results = []migrationBookmarkListsResult{}
		row.Queries = []url.Values{}
		row.RawQueries = []string{}
		userID := row.UserID
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				payload := fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d,"name":"fixture"}}`, userID)
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(payload)), Request: req}, nil
			}
			endpoint := "/v1/user/bookmarks/novel"
			if row.Operation == "UserArtworkBookmarks" {
				endpoint = "/v1/user/bookmarks/illust"
			}
			if req.Method != "GET" || req.URL.Path != endpoint {
				t.Fatal("unexpected feed request")
			}
			step := len(row.Queries)
			row.Queries = append(row.Queries, req.URL.Query())
			row.RawQueries = append(row.RawQueries, req.URL.RawQuery)
			if step >= len(row.Bodies) {
				step = len(row.Bodies) - 1
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[step])), Request: req}, nil
		})
		makeClient := func() *pixiv.Client {
			var client *pixiv.Client
			var err error
			if userID > 0 {
				client, _, err = pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
			} else {
				client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
			}
			if err != nil {
				t.Fatal(err)
			}
			if client.UserID() != userID {
				t.Fatalf("client identity = %d, want %d", client.UserID(), userID)
			}
			return client
		}
		client := makeClient()
		originalID, originalRestrict, originalTag := row.ID, row.Restrict, row.Tag
		cur := sdk.Cursor{}
		if row.Cursor != "" {
			var err error
			cur, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			var next sdk.Cursor
			var callErr error
			result := migrationBookmarkListsResult{Items: []any{}}
			if row.Operation == "UserArtworkBookmarks" {
				var page sdk.Page[pixiv.Artwork]
				page, callErr = client.UserArtworkBookmarks(context.Background(), pixiv.UserArtworkBookmarksRequest{UserID: row.ID, Restrict: pixiv.Restrict(row.Restrict), Tag: row.Tag, Cursor: cur})
				if callErr == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
					}
					next = page.Next
				}
			} else {
				var page sdk.Page[pixiv.Novel]
				page, callErr = client.UserNovelBookmarks(context.Background(), pixiv.UserNovelBookmarksRequest{UserID: row.ID, Restrict: pixiv.Restrict(row.Restrict), Tag: row.Tag, Cursor: cur})
				if callErr == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToNovelDTO(item))
					}
					next = page.Next
				}
			}
			if callErr != nil {
				result.Reason = sdk.ReasonOf(callErr)
				result.Message = callErr.Error()
			} else {
				result.Cursor = migrationSearchCursor(t, next)
			}
			row.Results = append(row.Results, result)
			if callErr != nil || next.IsZero() {
				break
			}
			cur = next
			if row.NextMode == "changed_id" {
				row.ID = 124
			}
			if row.NextMode == "changed_restrict" {
				row.Restrict = "public"
			}
			if row.NextMode == "changed_tag" {
				row.Tag = "different"
			}

			if row.NextMode == "same_account" {
				client = makeClient()
			}
			if row.NextMode == "other_account" {
				userID = 8
				client = makeClient()
			}
			if row.NextMode == "anonymous_account" {
				userID = 0
				client = makeClient()
			}
			if row.NextMode == "other_client" {
				client = makeClient()
			}
		}
		row.ID, row.Restrict, row.Tag = originalID, originalRestrict, originalTag
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "bookmark-lists.json")
	if *migrationUpdateBookmarkLists {
		if err = os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("bookmark lists differ from frozen Go reference")
	}
}

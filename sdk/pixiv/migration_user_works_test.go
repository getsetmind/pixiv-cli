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

type migrationUserWorksResult struct {
	Items   []any      `json:"items"`
	Cursor  any        `json:"cursor"`
	Reason  sdk.Reason `json:"reason"`
	Message string     `json:"message"`
}

var migrationUpdateUserWorks = flag.Bool("migration-update-user-works", false, "capture related and recommended artwork contracts")

type migrationUserWorksRow struct {
	Kind       string                     `json:"kind"`
	NextKind   string                     `json:"next_kind"`
	Name       string                     `json:"name"`
	Operation  string                     `json:"operation"`
	ID         int64                      `json:"id"`
	UserID     int64                      `json:"user_id"`
	Cursor     string                     `json:"cursor"`
	NextMode   string                     `json:"next_mode"`
	Bodies     []json.RawMessage          `json:"bodies"`
	Results    []migrationUserWorksResult `json:"results"`
	Queries    []url.Values               `json:"queries"`
	RawQueries []string                   `json:"raw_queries"`
}

func TestMigrationUserWorksPreserveContract(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	var rows []migrationUserWorksRow
	for _, op := range []string{"UserArtworks", "UserNovels"} {
		key, endpoint := "novels", "/v1/user/novels"
		item := json.RawMessage(`{"id":9301,"title":"story","caption":"caption","user":{"id":31,"name":"writer","account":"author","comment":"hello","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"tags":[{"name":"tag","translated_name":"翻訳"}],"create_date":"2026-09-01T03:04:05.123456789+09:00","x_restrict":1,"text_length":1234,"is_original":true,"total_bookmarks":12,"total_view":34,"image_urls":{"original":"https://i.pximg.net/novel.jpg"}}`)
		base := "user_id=123&"
		if op == "UserArtworks" {
			key, endpoint = "illusts", "/v1/user/illusts"
			base = "type=illust&user_id=123&"
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
		for _, next := range []any{nil, "", prefix + "offset=30", prefix + "offset=0", prefix + "offset=-1", prefix + "offset=%2B30", prefix + "offset=0030", prefix + "offset=9223372036854775807", prefix + "offset=9223372036854775808", prefix + "offset=30&offset=40", prefix + "offset=30&unknown=1", prefix + "offset=30#fragment", prefix + "offset=%zz", prefix + "offset=30;x=1", "http://app-api.pixiv.net" + endpoint + "?offset=30", "HTTPS://app-api.pixiv.net" + endpoint + "?offset=30", "https://APP-API.PIXIV.NET" + endpoint + "?offset=30", "https://app-api.pixiv.net:443" + endpoint + "?offset=30", "https://app-api.pixiv.net:" + endpoint + "?offset=30", "https://user@app-api.pixiv.net" + endpoint + "?offset=30", prefix + "offset=30#", prefix + "offset=30&user_id=456", prefix + "offset=30&filter=changed", prefix + "offset=30&type=manga", 1} {
			rows = append(rows, migrationUserWorksRow{Name: op + fmt.Sprintf(":next:%v", next), Operation: op, ID: 123, Bodies: []json.RawMessage{batch(next), final}})
		}
		for _, id := range []int64{0, -1, 9223372036854775807} {
			rows = append(rows, migrationUserWorksRow{Name: fmt.Sprintf("%s:id:%d", op, id), Operation: op, ID: id, Kind: "bad", Bodies: []json.RawMessage{final}})
		}
		if op == "UserArtworks" {
			for _, kind := range []string{"", "illustration", "illust", "manga", "ugoira", "all", "unknown", "Illust", " illust", "illust "} {
				rows = append(rows, migrationUserWorksRow{Name: op + ":kind:" + kind, Operation: op, ID: 123, Kind: kind, Bodies: []json.RawMessage{batch(prefix + "offset=30"), final}})
			}
			rows = append(rows, migrationUserWorksRow{Name: op + ":alias", Operation: op, ID: 123, Kind: "illustration", NextKind: "illust", Bodies: []json.RawMessage{batch(prefix + "offset=30"), final}})
		}
		for _, body := range []string{`{}`, fmt.Sprintf(`{"%s":null}`, key), fmt.Sprintf(`{"%s":[]}`, key), fmt.Sprintf(`{"%s":{}}`, key), fmt.Sprintf(`{"%s":[null]}`, key), fmt.Sprintf(`{"%s":[{}]}`, key), fmt.Sprintf(`{"%s":[{"id":-1}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"title":1}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"create_date":"invalid"}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":0},"create_date":"2026-09-01T00:00:00Z"}]}`, key)} {
			rows = append(rows, migrationUserWorksRow{Name: op + ":body:" + body, Operation: op, ID: 123, Bodies: []json.RawMessage{[]byte(body)}})
		}
		digest := fmt.Sprintf("%x", sha256.Sum256([]byte(base)))
		for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"other","v":30}`, `{"k":"offset","v":30,"s":1}`, `{"k":"offset","v":-1}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"offset","v":30,"p":{"x":["1"]}}`, `{"p":{"offset":["30"]}}`, `{}`, `null`, `{"k":"offset","v":"30"}`} {
			for _, identity := range []string{"0", "foreign"} {
				cur, err := sdk.NewCursor("pixiv", op, 1, digest, []byte(payload), sdk.WithCursorIdentity(identity))
				if err != nil {
					t.Fatal(err)
				}
				rows = append(rows, migrationUserWorksRow{Name: op + ":cursor:" + identity + ":" + payload, Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
			}
		}
		for _, version := range []int{2, 3} {
			cur, err := sdk.NewCursor("pixiv", op, version, digest, []byte(`{"k":"offset","v":30}`))
			if err != nil {
				t.Fatal(err)
			}
			rows = append(rows, migrationUserWorksRow{Name: fmt.Sprintf("%s:version:%d", op, version), Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
		}
		for _, mode := range []string{"same_account", "other_account", "anonymous_account", "other_client", "changed_id", "changed_kind"} {
			rows = append(rows, migrationUserWorksRow{Name: op + ":binding:" + mode, Operation: op, ID: 123, UserID: 7, NextMode: mode, Bodies: []json.RawMessage{batch(prefix + "offset=30"), final}})
		}
	}

	for index := range rows {
		row := &rows[index]
		row.Results = []migrationUserWorksResult{}
		row.Queries = []url.Values{}
		row.RawQueries = []string{}
		userID := row.UserID
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				payload := fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d,"name":"fixture"}}`, userID)
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(payload)), Request: req}, nil
			}
			endpoint := "/v1/user/novels"
			if row.Operation == "UserArtworks" {
				endpoint = "/v1/user/illusts"
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
		originalID, originalKind := row.ID, row.Kind
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
			result := migrationUserWorksResult{Items: []any{}}
			if row.Operation == "UserArtworks" {
				var page sdk.Page[pixiv.Artwork]
				page, callErr = client.UserArtworks(context.Background(), pixiv.UserArtworksRequest{UserID: row.ID, Kind: pixiv.ArtworkKind(row.Kind), Cursor: cur})
				if callErr == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
					}
					next = page.Next
				}
			} else {
				var page sdk.Page[pixiv.Novel]
				page, callErr = client.UserNovels(context.Background(), pixiv.UserNovelsRequest{UserID: row.ID, Cursor: cur})
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
			if row.NextKind != "" {
				row.Kind = row.NextKind
			}
			if row.NextMode == "changed_id" {
				row.ID = 124
			}
			if row.NextMode == "changed_kind" {
				row.Kind = "manga"
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
		row.ID, row.Kind = originalID, originalKind
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "user-works.json")
	if *migrationUpdateUserWorks {
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
		t.Fatal("artwork feeds differ from frozen Go reference")
	}
}

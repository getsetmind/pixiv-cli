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

type migrationRecommendationResult struct {
	Items   []any      `json:"items"`
	Cursor  any        `json:"cursor"`
	Reason  sdk.Reason `json:"reason"`
	Message string     `json:"message"`
}

var migrationUpdateNovelUserRecommendations = flag.Bool("migration-update-novel-user-recommendations", false, "capture related and recommended artwork contracts")

type migrationRecommendationRow struct {
	Name       string                          `json:"name"`
	Operation  string                          `json:"operation"`
	ID         int64                           `json:"id"`
	UserID     int64                           `json:"user_id"`
	Cursor     string                          `json:"cursor"`
	NextMode   string                          `json:"next_mode"`
	Bodies     []json.RawMessage               `json:"bodies"`
	Results    []migrationRecommendationResult `json:"results"`
	Queries    []url.Values                    `json:"queries"`
	RawQueries []string                        `json:"raw_queries"`
}

func TestMigrationNovelUserRecommendationsPreserveContract(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	novel := json.RawMessage(`{"id":9301,"title":"story","caption":"caption","user":{"id":31,"name":"writer","account":"author","comment":"hello","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"tags":[{"name":"tag","translated_name":"翻訳"}],"create_date":"2026-09-01T03:04:05.123456789+09:00","x_restrict":1,"text_length":1234,"is_original":true,"total_bookmarks":12,"total_view":34,"image_urls":{"original":"https://i.pximg.net/novel.jpg"}}`)
	artwork := json.RawMessage(`{"id":123,"title":"sample","type":"manga","user":{"id":31},"create_date":"2026-09-01T00:00:00Z","page_count":2,"width":100,"height":200,"illust_ai_type":2,"ai_type":1,"tools":["pen"],"image_urls":{"medium":"https://i.pximg.net/art.jpg"},"meta_pages":[{"image_urls":{"original":"https://i.pximg.net/page1.jpg"}},{"image_urls":{"original":"https://i.pximg.net/page2.jpg"}}]}`)
	operation := ""
	batch := func(next any) json.RawMessage {
		body := map[string]any{"novels": []json.RawMessage{novel}}
		if operation == "RecommendedUsers" {
			body = map[string]any{"user_previews": []any{map[string]any{"user": map[string]any{"id": 31, "name": "writer", "account": "author", "comment": "hello", "is_followed": true, "profile_image_urls": map[string]any{"medium": "https://i.pximg.net/profile.jpg"}}, "illusts": []json.RawMessage{artwork}, "novels": []json.RawMessage{novel}}}}
		}
		body["next_url"] = next
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	final := batch(nil)
	rows := []migrationRecommendationRow{}
	for _, op := range []string{"RecommendedUsers", "RecommendedNovels"} {
		operation = op
		final = batch(nil)
		endpoint := "/v1/novel/recommended"
		base := ""
		if op == "RecommendedUsers" {
			endpoint = "/v1/user/recommended"
			base = ""
		}
		prefix := "https://app-api.pixiv.net" + endpoint + "?"
		nexts := []any{nil, "", prefix + base + "offset=30", prefix + base + "offset=0", prefix + base + "offset=-1", prefix + base + "offset=%2B30", prefix + base + "offset=0030", prefix + base + "offset=9223372036854775807", prefix + base + "offset=9223372036854775808", prefix + base + "offset=30&offset=40", prefix + base + "offset=30&unknown=1", prefix + base + "offset=30#fragment", prefix + base + "offset=%zz", prefix + base + "offset=30;x=1", "http://app-api.pixiv.net" + endpoint + "?" + base + "offset=30", "https://APP-API.PIXIV.NET" + endpoint + "?" + base + "offset=30", "https://app-api.pixiv.net:443" + endpoint + "?" + base + "offset=30", "https://app-api.pixiv.net:" + endpoint + "?" + base + "offset=30", "https://user@app-api.pixiv.net" + endpoint + "?" + base + "offset=30", prefix + base + "offset=30#", 1}
		if op == "RecommendedNovels" {
			nexts = append(nexts, prefix+"offset=0&already_recommended=2,1,2&max_bookmark_id_for_recommend=%2B34&include_ranking_novels=true&include_privacy_policy=false", prefix+"already_recommended=2,1,2", prefix+"include_privacy_policy=true", prefix+"max_bookmark_id_for_recommend=-1", prefix+"include_ranking_novels=TRUE", prefix+"already_recommended=", prefix+"already_recommended=x&already_recommended=y")
		}
		for _, next := range nexts {
			rows = append(rows, migrationRecommendationRow{Name: op + fmt.Sprintf(":next:%v", next), Operation: op, ID: 123, Bodies: []json.RawMessage{batch(next), final}})
		}
		bodies := []string{`{}`, `{"novels":null}`, `{"novels":[]}`, `{"novels":{}}`, `{"novels":[null]}`, `{"novels":[{}]}`, `{"novels":[{"id":-1}]}`, `{"novels":[{"id":1,"user":{"id":2},"title":1}]}`, `{"novels":[{"id":1,"user":{"id":2},"create_date":"invalid"}]}`}
		if op == "RecommendedUsers" {
			bodies = []string{`{}`, `{"user_previews":null}`, `{"user_previews":[]}`, `{"user_previews":{}}`, `{"user_previews":[null]}`, `{"user_previews":[{}]}`, `{"user_previews":[{"user":{"id":1},"illusts":[null]}]}`, `{"user_previews":[{"user":{"id":1},"novels":[null]}]}`, `{"user_previews":[{"user":{"id":1},"illusts":null,"novels":null}]}`, `{"user_previews":[{"user":{"id":1},"illusts":[{"id":2,"user":{"id":0}}]}]}`, `{"user_previews":[{"user":{"id":1},"novels":[{"id":2,"user":{"id":3},"create_date":"invalid"}]}]}`, `{"user_previews":[{"user":{"id":1},"name":4}]}`}
		}
		for _, body := range bodies {
			rows = append(rows, migrationRecommendationRow{Name: op + ":body:" + body, Operation: op, Bodies: []json.RawMessage{[]byte(body)}})
		}
		digest := fmt.Sprintf("%x", sha256.Sum256([]byte(base)))
		payloads := []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"other","v":30}`, `{"k":"offset","v":30,"s":1}`, `{"p":{"already_recommended":["2,1,2"]}}`, `{"p":{"include_ranking_novels":["false"]}}`, `{}`, `null`, `{"k":"offset","v":-1}`, `{"p":{"offset":["30"]},"s":-1}`, `{"p":{"offset":["30"]},"s":1}`, `{"p":{"offset":["30"]},"v":-1}`, `{"k":"offset","p":{"offset":["30"]}}`, `{"p":{"offset":["0"]}}`, `{"p":{"offset":["30","40"]}}`, `{"p":{"offset":null}}`, `{"p":{"unknown":["30"]}}`, `{"p":{"include_privacy_policy":["true"]}}`, `{"p":{"illust_id":["123"],"offset":["30"]}}`, `{"p":{"illust_id":["124"],"offset":["30"]}}`, `{"p":{"illust_id":["123"],"seed_illust_ids[]":["789","456","789"],"viewed[]":["123","456"]}}`}
		for _, payload := range payloads {
			for _, identity := range []string{"0", "foreign"} {
				version := 2
				if op == "RecommendedUsers" {
					version = 1
				}
				cur, err := sdk.NewCursor("pixiv", op, version, digest, []byte(payload), sdk.WithCursorIdentity(identity))
				if err != nil {
					t.Fatal(err)
				}
				rows = append(rows, migrationRecommendationRow{Name: op + ":cursor:" + identity + ":" + payload, Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
			}
		}
		for _, version := range []int{2, 3} {
			cur, err := sdk.NewCursor("pixiv", op, version, digest, []byte(`{"p":{"offset":["30"]}}`), sdk.WithCursorIdentity("0"))
			if err != nil {
				t.Fatal(err)
			}
			rows = append(rows, migrationRecommendationRow{Name: fmt.Sprintf("%s:version:%d", op, version), Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
		}
		for _, mode := range []string{"same_account", "other_account", "anonymous_account"} {
			rows = append(rows, migrationRecommendationRow{Name: op + ":identity:" + mode, Operation: op, ID: 123, UserID: 7, NextMode: mode, Bodies: []json.RawMessage{batch(prefix + base + "offset=30"), final}})
		}
		for _, mode := range []string{"other_client"} {
			rows = append(rows, migrationRecommendationRow{Name: op + ":binding:" + mode, Operation: op, ID: 123, NextMode: mode, Bodies: []json.RawMessage{batch(prefix + base + "offset=30"), final}})
		}
	}

	for index := range rows {
		row := &rows[index]
		row.Results = []migrationRecommendationResult{}
		row.Queries = []url.Values{}
		row.RawQueries = []string{}
		userID := row.UserID
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				payload := fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d,"name":"fixture"}}`, userID)
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(payload)), Request: req}, nil
			}
			endpoint := "/v1/novel/recommended"
			if row.Operation == "RecommendedUsers" {
				endpoint = "/v1/user/recommended"
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
			return client
		}
		client := makeClient()
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
			result := migrationRecommendationResult{Items: []any{}}
			if row.Operation == "RecommendedUsers" {
				var page sdk.Page[pixiv.UserPreview]
				page, callErr = client.RecommendedUsers(context.Background(), pixiv.RecommendedUsersRequest{Cursor: cur})
				if callErr == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToUserPreviewDTO(item))
					}
					next = page.Next
				}
			} else {
				var page sdk.Page[pixiv.Novel]
				page, callErr = client.RecommendedNovels(context.Background(), pixiv.RecommendedNovelsRequest{Cursor: cur})
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
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "novel-user-recommendations.json")
	if *migrationUpdateNovelUserRecommendations {
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

func TestMigrationRecommendationSampleWireGaps(t *testing.T) {
	type gap struct {
		Name   string                        `json:"name"`
		Body   json.RawMessage               `json:"body"`
		Result migrationRecommendationResult `json:"result"`
	}
	rows := []gap{
		{Name: "null-tools", Body: json.RawMessage(`{"user_previews":[{"user":{"id":1},"illusts":[{"id":2,"user":{"id":1},"tools":[null],"create_date":"2026-09-01T00:00:00Z"}]}]}`)},
		{Name: "null-meta-page", Body: json.RawMessage(`{"user_previews":[{"user":{"id":1},"illusts":[{"id":2,"user":{"id":1},"meta_pages":[null],"create_date":"2026-09-01T00:00:00Z"}]}]}`)},
		{Name: "invalid-meta-page-index", Body: json.RawMessage(`{"user_previews":[{"user":{"id":1},"illusts":[{"id":2,"user":{"id":1},"meta_pages":[{"page_index":"bad"}],"create_date":"2026-09-01T00:00:00Z"}]}]}`)},
		{Name: "invalid-meta-page-extension", Body: json.RawMessage(`{"user_previews":[{"user":{"id":1},"illusts":[{"id":2,"user":{"id":1},"meta_pages":[{"extension":1}],"create_date":"2026-09-01T00:00:00Z"}]}]}`)},
		{Name: "uppercase-preview", Body: json.RawMessage(`{"user_previews":[{"USER":{"ID":1,"NAME":"writer"}}]}`)},
	}
	rows = append(rows, gap{Name: "escaped-novel-cursor", Body: json.RawMessage(`{"novels":[],"next_url":"https://app-api.pixiv.net/v1/novel/recommended?already_recommended=%3C%3E%26%E2%80%A8%E2%80%A9"}`)})
	for index := range rows {
		row := &rows[index]
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(request *http.Request) (*http.Response, error) {
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Body)), Request: request}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		var page sdk.Page[pixiv.UserPreview]
		if row.Name == "escaped-novel-cursor" {
			novels, callErr := client.RecommendedNovels(context.Background(), pixiv.RecommendedNovelsRequest{})
			err = callErr
			page.Next = novels.Next
		} else {
			page, err = client.RecommendedUsers(context.Background(), pixiv.RecommendedUsersRequest{})
		}
		row.Result.Items = []any{}
		row.Result.Cursor = migrationSearchCursor(t, page.Next)
		if err != nil {
			row.Result.Reason = sdk.ReasonOf(err)
			row.Result.Message = err.Error()
		} else {
			for _, item := range page.Items {
				row.Result.Items = append(row.Result.Items, pixiv.ToUserPreviewDTO(item))
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "recommendation-sample-wire-gaps.json")
	if *migrationUpdateNovelUserRecommendations {
		if err = os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("sample wire gap reference changed")
	}
}

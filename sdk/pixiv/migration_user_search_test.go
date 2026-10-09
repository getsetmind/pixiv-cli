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

var migrationUpdateUserSearch = flag.Bool("migration-update-user-search", false, "capture user search pages and validation")

type migrationUserSearchRow struct {
	Name     string                      `json:"name"`
	Word     string                      `json:"word"`
	Cursor   string                      `json:"cursor"`
	NextWord string                      `json:"next_word"`
	Bodies   []json.RawMessage           `json:"bodies"`
	Results  []migrationUserSearchResult `json:"results"`
	Queries  []url.Values                `json:"queries"`
}

type migrationUserSearchResult struct {
	Items   []pixiv.UserPreviewDTO `json:"items"`
	Cursor  any                    `json:"cursor"`
	Reason  sdk.Reason             `json:"reason,omitempty"`
	Message string                 `json:"message,omitempty"`
}

func TestMigrationUserSearchPreservesConditionsDTOsPagesAndCursorBinding(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	batch := func(next any) json.RawMessage {
		var body map[string]any
		if err := json.Unmarshal([]byte(`{"user_previews":[{"user":{"id":31,"name":"name\nfirst","account":"account","comment":"hello\tworld","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"illusts":[{"id":7}],"novels":[{"id":9}]},{"user":{"id":32}}]}`), &body); err != nil {
			t.Fatal(err)
		}
		body["next_url"] = next
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	final := batch(nil)
	var rows []migrationUserSearchRow
	for _, word := range []string{"miku", "", " ", "\u3000", "日本語 + &\n", "\x00"} {
		rows = append(rows, migrationUserSearchRow{Name: "word:" + word, Word: word, Bodies: []json.RawMessage{final}})
	}
	for _, next := range []any{nil, "", "https://app-api.pixiv.net/v1/search/user?offset=30", "https://app-api.pixiv.net:/v1/search/user?offset=%2B30", "https://app-api.pixiv.net/v1/search/user?offset=0", "https://app-api.pixiv.net/v1/search/user?offset=-1", "https://app-api.pixiv.net/v1/search/user?offset=9223372036854775808", "http://app-api.pixiv.net/v1/search/user?offset=30", "https://APP-API.PIXIV.NET/v1/search/user?offset=30", "https://app-api.pixiv.net:443/v1/search/user?offset=30", "https://user@app-api.pixiv.net/v1/search/user?offset=30", "https://app-api.pixiv.net/v1/search/illust?offset=30", "https://app-api.pixiv.net/v1/search/user?offset=30&offset=60", "https://app-api.pixiv.net/v1/search/user?offset=30&unknown=1", "https://app-api.pixiv.net/v1/search/user?offset=30&word=changed", "https://app-api.pixiv.net/v1/search/user?offset=30&sort=date_desc", "https://app-api.pixiv.net/v1/search/user?offset=30#fragment", "https://app-api.pixiv.net/v1/search/user?offset=%zz", "https://app-api.pixiv.net/v1/search/user?offset=30;word=miku", 1} {
		rows = append(rows, migrationUserSearchRow{Name: fmt.Sprintf("next:%v", next), Word: "miku", Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, body := range []string{`{}`, `{"user_previews":null}`, `{"user_previews":{}}`, `{"user_previews":[]}`, `{"user_previews":[null]}`, `{"user_previews":[{}]}`, `{"user_previews":[{"user":null}]}`, `{"user_previews":[{"user":{"id":-1}}]}`} {
		rows = append(rows, migrationUserSearchRow{Name: "body:" + body, Word: "miku", Bodies: []json.RawMessage{[]byte(body)}})
	}
	for _, mutation := range []struct {
		name  string
		value any
	}{
		{"id", 0}, {"id", nil}, {"id", "31"}, {"id", 1.5}, {"name", nil}, {"name", 7}, {"account", false}, {"comment", 7}, {"is_followed", nil}, {"is_followed", "true"}, {"profile_image_urls", nil}, {"profile_image_urls", map[string]any{"medium": nil}}, {"profile_image_urls", map[string]any{"medium": 7}}, {"profile_image_urls", map[string]any{"medium": "https://evil.example/profile.jpg"}},
	} {
		var body map[string]any
		if err := json.Unmarshal(final, &body); err != nil {
			t.Fatal(err)
		}
		body["user_previews"].([]any)[0].(map[string]any)["user"].(map[string]any)[mutation.name] = mutation.value
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationUserSearchRow{Name: fmt.Sprintf("field:%s:%v", mutation.name, mutation.value), Word: "miku", Bodies: []json.RawMessage{encoded}})
	}
	for _, samples := range []any{nil, 7, "invalid", []any{nil, map[string]any{"id": -1}}} {
		var body map[string]any
		if err := json.Unmarshal(final, &body); err != nil {
			t.Fatal(err)
		}
		preview := body["user_previews"].([]any)[0].(map[string]any)
		preview["illusts"], preview["novels"] = samples, samples
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationUserSearchRow{Name: fmt.Sprintf("samples:%v", samples), Word: "miku", Bodies: []json.RawMessage{encoded}})
	}
	digest := fmt.Sprintf("%x", sha256.Sum256([]byte("word=miku&")))
	for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"offset","v":30,"s":1}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"last_order","v":30}`, `{}`, `null`, `{"k":"offset","v":30,"p":{"offset":["30"]}}`, `{"p":{"offset":["30"]}}`, `{"k":"offset","v":"30"}`} {
		cursor, err := sdk.NewCursor("pixiv", "SearchUsers", 1, digest, []byte(payload), sdk.WithCursorIdentity("foreign"))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationUserSearchRow{Name: "cursor:" + payload, Word: "miku", Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	rows = append(rows, migrationUserSearchRow{Name: "binding:word", Word: "miku", NextWord: "changed", Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/search/user?offset=30"), final}})

	for index := range rows {
		row := &rows[index]
		row.Results = []migrationUserSearchResult{}
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "GET" || req.URL.Path != "/v1/search/user" {
				t.Fatal("unexpected user search request")
			}
			row.Queries = append(row.Queries, req.URL.Query())
			body := row.Bodies[0]
			if len(row.Bodies) > 1 && req.URL.Query().Get("offset") != "" {
				body = row.Bodies[1]
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		request := pixiv.SearchUsersRequest{Word: row.Word}
		if row.Cursor != "" {
			request.Cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			page, err := client.SearchUsers(context.Background(), request)
			result := migrationUserSearchResult{Items: []pixiv.UserPreviewDTO{}}
			if err != nil {
				result.Reason = sdk.ReasonOf(err)
				result.Message = err.Error()
			} else {
				for _, item := range page.Items {
					result.Items = append(result.Items, pixiv.ToUserPreviewDTO(item))
				}
				result.Cursor = migrationSearchCursor(t, page.Next)
			}
			row.Results = append(row.Results, result)
			if err != nil || page.Next.IsZero() {
				break
			}
			request.Cursor = page.Next
			if row.NextWord != "" {
				request.Word = row.NextWord
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "user-search.json")
	if *migrationUpdateUserSearch {
		if err := os.WriteFile(target, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("user search differs from fixed Go reference")
	}
}

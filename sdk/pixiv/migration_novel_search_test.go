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

var migrationUpdateNovelSearch = flag.Bool("migration-update-novel-search", false, "capture novel search pages and validation")

type migrationNovelSearchRow struct {
	Name     string                        `json:"name"`
	Word     string                        `json:"word"`
	Target   string                        `json:"target"`
	Sort     string                        `json:"sort"`
	Duration string                        `json:"duration"`
	Cursor   string                        `json:"cursor"`
	NextWord string                        `json:"next_word"`
	Bodies   []json.RawMessage             `json:"bodies"`
	Results  []migrationNovelRankingResult `json:"results"`
	Queries  []url.Values                  `json:"queries"`
}

func TestMigrationNovelSearchPreservesConditionsDTOsPagesAndCursorBinding(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	batch := func(next any) json.RawMessage {
		var body map[string]any
		if err := json.Unmarshal([]byte(`{"novels":[{"id":9301,"title":"小説\nfirst","caption":"<p>caption</p>","user":{"id":31,"name":"writer","account":"author","comment":"hello","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"tags":[{"name":"novel","translated_name":"小説"}],"create_date":"2026-09-01T03:04:05.123456789+09:00","x_restrict":1,"text_length":1234,"is_original":true,"total_bookmarks":12,"total_view":34,"image_urls":{"original":"https://i.pximg.net/novel.jpg","large":"https://i.pximg.net/large.jpg"}},{"id":9302,"title":"second","user":{"id":32},"create_date":"2026-09-02T00:00:00Z"}]}`), &body); err != nil {
			t.Fatal(err)
		}
		for _, value := range body["novels"].([]any) {
			novel := value.(map[string]any)
			for _, key := range []string{"x_restrict", "text_length"} {
				if _, ok := novel[key]; !ok {
					novel[key] = 0
				}
			}
			if _, ok := novel["is_original"]; !ok {
				novel["is_original"] = false
			}
		}
		body["next_url"] = next
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	final := batch(nil)
	var rows []migrationNovelSearchRow
	for _, target := range []string{"", "partial_match_for_tags", "exact_match_for_tags", "title_and_caption", "keyword", "invalid", " title_and_caption"} {
		rows = append(rows, migrationNovelSearchRow{Name: "target:" + target, Word: "miku", Target: target, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/search/novel?offset=30"), final}})
	}
	for _, sort := range []string{"date_asc", "popular_desc", "invalid", "DATE_DESC"} {
		rows = append(rows, migrationNovelSearchRow{Name: "sort:" + sort, Word: "miku", Sort: sort, Bodies: []json.RawMessage{final}})
	}
	for _, duration := range []string{"within_last_day", "within_last_week", "within_last_month", "invalid"} {
		rows = append(rows, migrationNovelSearchRow{Name: "duration:" + duration, Word: "miku", Duration: duration, Bodies: []json.RawMessage{final}})
	}
	for _, word := range []string{"", " ", "\u3000", "日本語 + &\n", "\x00"} {
		rows = append(rows, migrationNovelSearchRow{Name: "word:" + word, Word: word, Bodies: []json.RawMessage{final}})
	}
	for _, next := range []any{nil, "", "https://app-api.pixiv.net/v1/search/novel?offset=30", "https://app-api.pixiv.net:/v1/search/novel?offset=%2B30", "https://app-api.pixiv.net/v1/search/novel?offset=0", "https://app-api.pixiv.net/v1/search/novel?offset=-1", "https://app-api.pixiv.net/v1/search/novel?offset=9223372036854775808", "http://app-api.pixiv.net/v1/search/novel?offset=30", "https://APP-API.PIXIV.NET/v1/search/novel?offset=30", "https://app-api.pixiv.net:443/v1/search/novel?offset=30", "https://user@app-api.pixiv.net/v1/search/novel?offset=30", "https://app-api.pixiv.net/v1/search/illust?offset=30", "https://app-api.pixiv.net/v1/search/novel?offset=30&offset=60", "https://app-api.pixiv.net/v1/search/novel?offset=30&unknown=1", "https://app-api.pixiv.net/v1/search/novel?offset=30&sort=changed&duration=invalid", "https://app-api.pixiv.net/v1/search/novel?offset=30#fragment", "https://app-api.pixiv.net/v1/search/novel?offset=%zz", "https://app-api.pixiv.net/v1/search/novel?offset=30;mode=day", 1} {
		rows = append(rows, migrationNovelSearchRow{Name: fmt.Sprintf("next:%v", next), Word: "miku", Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, body := range []string{`{}`, `{"novels":null}`, `{"novels":{}}`, `{"novels":[]}`, `{"novels":[null]}`, `{"novels":[{}]}`, `{"novels":[{"id":-1}]}`, `{"novels":[{"id":1,"title":1}]}`, `{"novels":[{"id":1,"create_date":"invalid"}]}`} {
		rows = append(rows, migrationNovelSearchRow{Name: "body:" + body, Word: "miku", Bodies: []json.RawMessage{[]byte(body)}})
	}
	for _, mutation := range []struct {
		name  string
		value any
	}{
		{"id", 0}, {"user", nil}, {"create_date", "invalid"}, {"tags", []any{nil}}, {"tags", nil}, {"title", 7}, {"x_restrict", "1"}, {"is_original", "true"}, {"image_urls", map[string]any{"large": "https://i.pximg.net/large.jpg"}}, {"image_urls", map[string]any{"medium": "https://i.pximg.net/medium.jpg"}}, {"image_urls", map[string]any{"square_medium": "https://i.pximg.net/square.jpg"}}, {"image_urls", map[string]any{"original": "https://evil.example/cover.jpg"}}, {"user", map[string]any{"id": 31, "profile_image_urls": map[string]any{"medium": "https://evil.example/profile.jpg"}}},
	} {
		var body map[string]any
		if err := json.Unmarshal(final, &body); err != nil {
			t.Fatal(err)
		}
		body["novels"].([]any)[0].(map[string]any)[mutation.name] = mutation.value
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationNovelSearchRow{Name: fmt.Sprintf("field:%s:%v", mutation.name, mutation.value), Word: "miku", Bodies: []json.RawMessage{encoded}})
	}
	digest := fmt.Sprintf("%x", sha256.Sum256([]byte("search_target=partial_match_for_tags&sort=date_desc&word=miku&")))
	for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"offset","v":30,"s":1}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"last_order","v":30}`, `{}`, `null`, `{"k":"offset","v":30,"p":{"offset":["30"]}}`, `{"p":{"offset":["30"]}}`, `{"k":"offset","v":"30"}`} {
		cursor, err := sdk.NewCursor("pixiv", "SearchNovels", 1, digest, []byte(payload), sdk.WithCursorIdentity("foreign"))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationNovelSearchRow{Name: "cursor:" + payload, Word: "miku", Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	rows = append(rows, migrationNovelSearchRow{Name: "binding:word", Word: "miku", NextWord: "changed", Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/search/novel?offset=30"), final}})
	for _, key := range []string{"x_restrict", "text_length", "is_original"} {
		var body map[string]any
		if err := json.Unmarshal(final, &body); err != nil {
			t.Fatal(err)
		}
		delete(body["novels"].([]any)[0].(map[string]any), key)
		encoded, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationNovelSearchRow{Name: "missing:" + key, Word: "miku", Bodies: []json.RawMessage{encoded}})
	}

	for index := range rows {
		row := &rows[index]
		row.Results = []migrationNovelRankingResult{}
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "GET" || req.URL.Path != "/v1/search/novel" {
				t.Fatal("unexpected novel search request")
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
		request := pixiv.SearchNovelsRequest{Word: row.Word, Target: pixiv.SearchTarget(row.Target), Sort: pixiv.SortMode(row.Sort), Duration: pixiv.DurationFilter(row.Duration)}
		if row.Cursor != "" {
			request.Cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			page, err := client.SearchNovels(context.Background(), request)
			result := migrationNovelRankingResult{Items: []pixiv.NovelDTO{}}
			if err != nil {
				result.Reason = sdk.ReasonOf(err)
				result.Message = err.Error()
			} else {
				for _, item := range page.Items {
					result.Items = append(result.Items, pixiv.ToNovelDTO(item))
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
	target := filepath.Join(path, "novel-search.json")
	if *migrationUpdateNovelSearch {
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
		t.Fatal("novel ranking differs from fixed Go reference")
	}
}

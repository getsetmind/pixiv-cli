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

type migrationNovelRankingResult struct {
	Items   []pixiv.NovelDTO `json:"items"`
	Cursor  any              `json:"cursor"`
	Reason  sdk.Reason       `json:"reason"`
	Message string           `json:"message"`
}

var migrationUpdateNovelRanking = flag.Bool("migration-update-novel-ranking", false, "capture novel ranking pages and validation")

type migrationNovelRankingRow struct {
	Name     string                        `json:"name"`
	Mode     string                        `json:"mode"`
	Date     string                        `json:"date"`
	Cursor   string                        `json:"cursor"`
	NextMode string                        `json:"next_mode"`
	NextDate string                        `json:"next_date"`
	Bodies   []json.RawMessage             `json:"bodies"`
	Results  []migrationNovelRankingResult `json:"results"`
	Queries  []url.Values                  `json:"queries"`
}

func TestMigrationNovelRankingPreservesModesDTOsPagesAndCursorBinding(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	batch := func(next any) json.RawMessage {
		var body map[string]any
		if err := json.Unmarshal([]byte(`{"novels":[{"id":9301,"title":"小説\nfirst","caption":"<p>caption</p>","user":{"id":31,"name":"writer","account":"author","comment":"hello","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"tags":[{"name":"novel","translated_name":"小説"}],"create_date":"2026-09-01T03:04:05.123456789+09:00","x_restrict":1,"text_length":1234,"is_original":true,"total_bookmarks":12,"total_view":34,"image_urls":{"original":"https://i.pximg.net/novel.jpg","large":"https://i.pximg.net/large.jpg"}},{"id":9302,"title":"second","user":{"id":32},"create_date":"2026-09-02T00:00:00Z"}]}`), &body); err != nil {
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
	var rows []migrationNovelRankingRow
	for _, mode := range []string{"", "day", "day_male", "day_female", "week", "week_original", "week_rookie", "month", "day_manga", "week_manga", "month_manga", "week_rookie_manga", "day_r18", "day_male_r18", "day_female_r18", "week_r18", "week_r18g", "invalid", "DAY", " day", "day "} {
		rows = append(rows, migrationNovelRankingRow{Name: "mode:" + mode, Mode: mode, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/novel/ranking?offset=30&mode=day"), final}})
	}
	for _, next := range []any{nil, "", "https://app-api.pixiv.net/v1/novel/ranking?offset=30", "https://app-api.pixiv.net:/v1/novel/ranking?offset=%2B30", "https://app-api.pixiv.net/v1/novel/ranking?offset=0", "https://app-api.pixiv.net/v1/novel/ranking?offset=-1", "https://app-api.pixiv.net/v1/novel/ranking?offset=9223372036854775808", "http://app-api.pixiv.net/v1/novel/ranking?offset=30", "https://APP-API.PIXIV.NET/v1/novel/ranking?offset=30", "https://app-api.pixiv.net:443/v1/novel/ranking?offset=30", "https://user@app-api.pixiv.net/v1/novel/ranking?offset=30", "https://app-api.pixiv.net/v1/search/illust?offset=30", "https://app-api.pixiv.net/v1/novel/ranking?offset=30&offset=60", "https://app-api.pixiv.net/v1/novel/ranking?offset=30&unknown=1", "https://app-api.pixiv.net/v1/novel/ranking?offset=30&mode=changed&filter=changed", "https://app-api.pixiv.net/v1/novel/ranking?offset=30#fragment", "https://app-api.pixiv.net/v1/novel/ranking?offset=%zz", "https://app-api.pixiv.net/v1/novel/ranking?offset=30;mode=day", 1} {
		rows = append(rows, migrationNovelRankingRow{Name: fmt.Sprintf("next:%v", next), Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, body := range []string{`{}`, `{"novels":null}`, `{"novels":{}}`, `{"novels":[]}`, `{"novels":[null]}`, `{"novels":[{}]}`, `{"novels":[{"id":-1}]}`, `{"novels":[{"id":1,"title":1}]}`, `{"novels":[{"id":1,"create_date":"invalid"}]}`} {
		rows = append(rows, migrationNovelRankingRow{Name: "body:" + body, Bodies: []json.RawMessage{[]byte(body)}})
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
		rows = append(rows, migrationNovelRankingRow{Name: fmt.Sprintf("field:%s:%v", mutation.name, mutation.value), Bodies: []json.RawMessage{encoded}})
	}
	digest := fmt.Sprintf("%x", sha256.Sum256([]byte("filter=for_android&mode=day&")))
	for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"offset","v":30,"s":1}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"last_order","v":30}`, `{}`, `null`, `{"k":"offset","v":30,"p":{"offset":["30"]}}`, `{"p":{"offset":["30"]}}`, `{"k":"offset","v":"30"}`} {
		cursor, err := sdk.NewCursor("pixiv", "NovelRanking", 1, digest, []byte(payload), sdk.WithCursorIdentity("foreign"))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationNovelRankingRow{Name: "cursor:" + payload, Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	rows = append(rows, migrationNovelRankingRow{Name: "binding:mode", NextMode: "week", Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/novel/ranking?offset=30"), final}})
	for index := range rows {
		row := &rows[index]
		row.Results = []migrationNovelRankingResult{}
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "GET" || req.URL.Path != "/v1/novel/ranking" {
				t.Fatal("unexpected ranking request")
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
		request := pixiv.NovelRankingRequest{Mode: pixiv.RankingMode(row.Mode)}
		if row.Cursor != "" {
			request.Cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			page, err := client.NovelRanking(context.Background(), request)
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
			if row.NextMode != "" {
				request.Mode = pixiv.RankingMode(row.NextMode)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "novel-ranking.json")
	if *migrationUpdateNovelRanking {
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

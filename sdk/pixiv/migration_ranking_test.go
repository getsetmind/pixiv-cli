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

var migrationUpdateRanking = flag.Bool("migration-update-ranking", false, "capture artwork ranking pages and validation")

type migrationRankingRow struct {
	Name     string                  `json:"name"`
	Mode     string                  `json:"mode"`
	Date     string                  `json:"date"`
	Cursor   string                  `json:"cursor"`
	NextMode string                  `json:"next_mode"`
	NextDate string                  `json:"next_date"`
	Bodies   []json.RawMessage       `json:"bodies"`
	Results  []migrationSearchResult `json:"results"`
	Queries  []url.Values            `json:"queries"`
}

func TestMigrationArtworkRankingPreservesModesDatesPagesAndCursorBinding(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "search-pages.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err := json.Unmarshal(data, &sources); err != nil {
		t.Fatal(err)
	}
	batch := func(next any) json.RawMessage {
		var body map[string]any
		if err := json.Unmarshal(sources[0].Bodies[0], &body); err != nil {
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
	var rows []migrationRankingRow
	for _, mode := range []string{"", "day", "day_male", "day_female", "week", "week_original", "week_rookie", "month", "day_manga", "week_manga", "month_manga", "week_rookie_manga", "day_r18", "day_male_r18", "day_female_r18", "week_r18", "week_r18g", "invalid", "DAY", " day", "day "} {
		rows = append(rows, migrationRankingRow{Name: "mode:" + mode, Mode: mode, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/illust/ranking?offset=30&mode=day"), final}})
	}
	for _, date := range []string{"2024-02-29", "2023-02-29", "2024-01-32", "2024-1-01", "2024-01-1", "0000-01-01", "9999-12-31", "2024-01-01T00:00:00Z", " 2024-01-01"} {
		rows = append(rows, migrationRankingRow{Name: "date:" + date, Date: date, Bodies: []json.RawMessage{final}})
	}
	for _, next := range []any{nil, "", "https://app-api.pixiv.net/v1/illust/ranking?offset=30", "https://app-api.pixiv.net:/v1/illust/ranking?offset=%2B30", "https://app-api.pixiv.net/v1/illust/ranking?offset=0", "https://app-api.pixiv.net/v1/illust/ranking?offset=-1", "https://app-api.pixiv.net/v1/illust/ranking?offset=9223372036854775808", "http://app-api.pixiv.net/v1/illust/ranking?offset=30", "https://APP-API.PIXIV.NET/v1/illust/ranking?offset=30", "https://app-api.pixiv.net:443/v1/illust/ranking?offset=30", "https://user@app-api.pixiv.net/v1/illust/ranking?offset=30", "https://app-api.pixiv.net/v1/search/illust?offset=30", "https://app-api.pixiv.net/v1/illust/ranking?offset=30&offset=60", "https://app-api.pixiv.net/v1/illust/ranking?offset=30&unknown=1", "https://app-api.pixiv.net/v1/illust/ranking?offset=30&mode=changed&date=invalid", "https://app-api.pixiv.net/v1/illust/ranking?offset=30#fragment", "https://app-api.pixiv.net/v1/illust/ranking?offset=%zz", "https://app-api.pixiv.net/v1/illust/ranking?offset=30;mode=day", 1} {
		rows = append(rows, migrationRankingRow{Name: fmt.Sprintf("next:%v", next), Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, body := range []string{`{}`, `{"illusts":null}`, `{"illusts":{}}`, `{"illusts":[]}`, `{"illusts":[null]}`, `{"illusts":[{}]}`, `{"illusts":[{"id":-1}]}`, `{"illusts":[{"id":1,"title":1}]}`, `{"illusts":[{"id":1,"create_date":"invalid"}]}`} {
		rows = append(rows, migrationRankingRow{Name: "body:" + body, Bodies: []json.RawMessage{[]byte(body)}})
	}
	digest := fmt.Sprintf("%x", sha256.Sum256([]byte("mode=day&")))
	for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"offset","v":30,"s":1}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"last_order","v":30}`, `{}`, `null`, `{"k":"offset","v":30,"p":{"offset":["30"]}}`, `{"p":{"offset":["30"]}}`, `{"k":"offset","v":"30"}`} {
		cursor, err := sdk.NewCursor("pixiv", "ArtworkRanking", 1, digest, []byte(payload), sdk.WithCursorIdentity("foreign"))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationRankingRow{Name: "cursor:" + payload, Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	rows = append(rows, migrationRankingRow{Name: "binding:mode", NextMode: "week", Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/illust/ranking?offset=30"), final}}, migrationRankingRow{Name: "binding:date", NextDate: "2024-01-01", Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/illust/ranking?offset=30"), final}})
	for index := range rows {
		row := &rows[index]
		row.Results = []migrationSearchResult{}
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "GET" || req.URL.Path != "/v1/illust/ranking" {
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
		request := pixiv.ArtworkRankingRequest{Mode: pixiv.RankingMode(row.Mode), Date: row.Date}
		if row.Cursor != "" {
			request.Cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			page, err := client.ArtworkRanking(context.Background(), request)
			result := migrationSearchResult{Items: []pixiv.ArtworkDTO{}}
			if err != nil {
				result.Reason = sdk.ReasonOf(err)
				result.Message = err.Error()
			} else {
				for _, item := range page.Items {
					result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
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
			if row.NextDate != "" {
				request.Date = row.NextDate
			}
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "artwork-ranking.json")
	if *migrationUpdateRanking {
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
		t.Fatal("artwork ranking differs from fixed Go reference")
	}
}

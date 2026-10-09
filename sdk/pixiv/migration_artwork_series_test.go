package pixiv_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"testing"
)

var migrationUpdateArtworkSeries = flag.Bool("migration-update-artwork-series", false, "capture fixed Go artwork series contracts")

type migrationArtworkSeriesRow struct {
	Name         string            `json:"name"`
	SeriesID     int64             `json:"series_id"`
	Cursor       string            `json:"cursor"`
	NextSeriesID int64             `json:"next_series_id"`
	Bodies       []json.RawMessage `json:"bodies"`
	Results      []any             `json:"results"`
	Queries      []url.Values      `json:"queries"`
}

func TestMigrationArtworkSeries(t *testing.T) {
	batch := func(next any) json.RawMessage {
		body := map[string]any{}
		if err := json.Unmarshal([]byte(`{"illust_series_detail":{"user":{"id":31}},"illusts":[{"id":9301,"title":"chapter","caption":"<p>caption</p>","type":"manga","page_count":2,"width":100,"height":200,"total_bookmarks":3,"total_view":4,"x_restrict":1,"illust_ai_type":2,"ai_type":1,"tools":["pen"],"tags":[{"name":"tag","translated_name":"translated"}],"user":{"id":31,"name":"artist","profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"create_date":"2026-09-01T03:04:05.123456789+09:00","image_urls":{"large":"https://i.pximg.net/cover.jpg"},"meta_pages":[{"width":100,"height":200,"image_urls":{"original":"https://i.pximg.net/page0.jpg"}},{"image_urls":{"original":"https://i.pximg.net/page1.png"}}]}]}`), &body); err != nil {
			t.Fatal(err)
		}
		body["next_url"] = next
		data, _ := json.Marshal(body)
		return data
	}
	final := batch(nil)
	rows := []migrationArtworkSeriesRow{}
	for _, id := range []int64{21, 0, -1, 9223372036854775807} {
		rows = append(rows, migrationArtworkSeriesRow{Name: fmt.Sprintf("id:%d", id), SeriesID: id, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/illust/series?last_order=9&illust_series_id=21"), final}})
	}
	for _, next := range []any{nil, "", "https://app-api.pixiv.net/v1/illust/series?last_order=9", "https://app-api.pixiv.net:/v1/illust/series?last_order=%2B9", "https://app-api.pixiv.net/v1/illust/series?last_order=0", "https://app-api.pixiv.net/v1/illust/series?last_order=-1", "https://app-api.pixiv.net/v1/illust/series?last_order=9223372036854775808", "http://app-api.pixiv.net/v1/illust/series?last_order=9", "https://APP-API.PIXIV.NET/v1/illust/series?last_order=9", "https://app-api.pixiv.net:443/v1/illust/series?last_order=9", "https://user@app-api.pixiv.net/v1/illust/series?last_order=9", "https://app-api.pixiv.net/v1/novel/ranking?last_order=9", "https://app-api.pixiv.net/v1/illust/series?last_order=9&last_order=10", "https://app-api.pixiv.net/v1/illust/series?last_order=9&unknown=1", "https://app-api.pixiv.net/v1/illust/series?last_order=9&illust_series_id=changed", "https://app-api.pixiv.net/v1/illust/series?last_order=9#fragment", "https://app-api.pixiv.net/v1/illust/series?last_order=%zz", "https://app-api.pixiv.net/v1/illust/series?last_order=9;illust_series_id=21", 1} {
		rows = append(rows, migrationArtworkSeriesRow{Name: fmt.Sprintf("next:%v", next), SeriesID: 21, Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, suffix := range []string{"offset=30", "offset=9223372036854775807", "last_order=9223372036854775807", "offset=0", "offset=-1", "offset=9223372036854775808", "offset=%2B30", "offset=00030", "offset=30&last_order=9", "offset=0&last_order=9", "offset=30&offset=31", "offset=30&illust_series_id=changed", "offset=30&illust_series_id=21&illust_series_id=22", "illust_series_id=21", "last_order=9#", "last_order=9&unknown=", "last_order=9&", "last_order=9&&", "last_order=9&illust_series_id=", "last_order=9&%6Fffset=30"} {
		next := "https://app-api.pixiv.net/v1/illust/series?" + suffix
		rows = append(rows, migrationArtworkSeriesRow{Name: "next-extra:" + suffix, SeriesID: 21, Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, body := range []string{`{}`, `null`, `{"illust_series_detail":null,"illusts":[]}`, `{"illust_series_detail":{"user":{"id":31}},"illusts":[]}`, `{"illust_series_detail":{"user":null},"illusts":[]}`, `{"illust_series_detail":{"user":{"id":31}}}`, `{"illust_series_detail":{"user":{"id":31}},"illusts":null}`, `{"illust_series_detail":{"user":{"id":31}},"illusts":[null]}`, `{"illust_series_detail":{"user":{"id":31}},"illusts":{}}`} {
		rows = append(rows, migrationArtworkSeriesRow{Name: "body:" + body, SeriesID: 21, Bodies: []json.RawMessage{[]byte(body)}})
	}

	for _, field := range []struct {
		name  string
		value any
	}{{"id", 0}, {"user", nil}, {"create_date", "invalid"}, {"tags", []any{nil}}, {"title", 7}, {"image_urls", map[string]any{"large": "https://evil.example/cover.jpg"}}} {
		var body map[string]any
		json.Unmarshal(final, &body)
		body["illusts"].([]any)[0].(map[string]any)[field.name] = field.value
		data, _ := json.Marshal(body)
		rows = append(rows, migrationArtworkSeriesRow{Name: fmt.Sprintf("field:%s:%v", field.name, field.value), SeriesID: 21, Bodies: []json.RawMessage{data}})
	}
	digest := fmt.Sprintf("%x", sha256.Sum256([]byte("illust_series_id=21&")))
	for _, payload := range []string{`{"k":"last_order","v":9}`, `{"k":"last_order","v":0}`, `{"k":"last_order","v":-1}`, `{"k":"last_order","v":9,"s":1}`, `{"k":"last_order","v":9,"s":-1}`, `{"k":"offset","v":9}`, `{"k":"offset","v":0}`, `{"k":"offset","v":9223372036854775807}`, `{"k":"last_order","v":9223372036854775807}`, `{"k":"future","v":9}`, `{"k":"future","v":0}`, `{"k":"offset","v":9,"p":{}}`, `{"k":"offset","v":9,"p":{"unused":null}}`, `{"k":"offset","v":null}`, `{"k":"offset","v":9,"s":9223372036854775807}`, `{}`, `null`, `{"k":"last_order","v":9,"p":{"last_order":["9"]}}`, `{"p":{"last_order":["9"]}}`, `{"k":"last_order","v":"9"}`} {
		cursor, err := sdk.NewCursor("pixiv", "ArtworkSeries", 1, digest, []byte(payload), sdk.WithCursorIdentity("foreign"))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationArtworkSeriesRow{Name: "cursor:" + payload, SeriesID: 21, Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	for _, binding := range []struct {
		name, product, operation, digest string
		version                          int
	}{
		{"product", "fanbox", "ArtworkSeries", digest, 1}, {"operation", "pixiv", "ArtworkRanking", digest, 1}, {"version", "pixiv", "ArtworkSeries", digest, 2}, {"query", "pixiv", "ArtworkSeries", "changed", 1},
	} {
		cursor, err := sdk.NewCursor(binding.product, binding.operation, binding.version, binding.digest, []byte(`{"k":"offset","v":30}`))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationArtworkSeriesRow{Name: "binding:" + binding.name, SeriesID: 21, Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	rows = append(rows, migrationArtworkSeriesRow{Name: "binding:series", SeriesID: 21, NextSeriesID: 22, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v1/illust/series?last_order=9"), final}})
	for index := range rows {
		row := &rows[index]
		row.Results = []any{}
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "GET" || req.URL.Path != "/v1/illust/series" {
				t.Fatal("unexpected series request")
			}
			row.Queries = append(row.Queries, req.URL.Query())
			body := row.Bodies[0]
			if len(row.Bodies) > 1 && (req.URL.Query().Get("last_order") != "" || req.URL.Query().Get("offset") != "") {
				body = row.Bodies[1]
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		request := pixiv.ArtworkSeriesRequest{SeriesID: row.SeriesID}
		if row.Cursor != "" {
			request.Cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			page, err := client.ArtworkSeries(context.Background(), request)
			result := map[string]any{"dto": nil, "cursor": nil, "reason": "", "message": ""}
			if err != nil {
				result["reason"] = sdk.ReasonOf(err)
				result["message"] = err.Error()
			} else {
				dto := []pixiv.ArtworkDTO{}
				for _, item := range page.Items {
					dto = append(dto, pixiv.ToArtworkDTO(item))
				}
				result["dto"] = dto
				result["cursor"] = migrationSearchCursor(t, page.Next)
			}
			row.Results = append(row.Results, result)
			if err != nil || page.Next.IsZero() {
				break
			}
			request.Cursor = page.Next
			if row.NextSeriesID != 0 {
				request.SeriesID = row.NextSeriesID
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "artwork-series.json")
	if *migrationUpdateArtworkSeries {
		if err := os.WriteFile(target, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("artwork series differs from fixed Go reference")
	}
}

func TestMigrationArtworkNullTags(t *testing.T) {
	rows := []any{}
	for _, operation := range []string{"Artwork", "ArtworkRanking", "SearchArtworks"} {
		for _, tags := range []any{nil, []any{nil}, []any{nil, map[string]any{"name": "tag"}, nil}, []any{7}, []any{map[string]any{"name": 7}}} {
			item := map[string]any{"id": 42, "user": map[string]any{"id": 31}, "create_date": "2026-01-01T00:00:00Z", "tags": tags}
			var envelope any = map[string]any{"illusts": []any{item}}
			if operation == "Artwork" {
				envelope = map[string]any{"illust": item}
			}
			body, err := json.Marshal(envelope)
			if err != nil {
				t.Fatal(err)
			}
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
			})}})
			if err != nil {
				t.Fatal(err)
			}
			dtos := []pixiv.ArtworkDTO{}
			switch operation {
			case "Artwork":
				var artwork pixiv.Artwork
				artwork, err = client.Artwork(context.Background(), pixiv.ArtworkRequest{ArtworkID: 42})
				if err == nil {
					dtos = append(dtos, pixiv.ToArtworkDTO(artwork))
				}
			case "ArtworkRanking":
				var page sdk.Page[pixiv.Artwork]
				page, err = client.ArtworkRanking(context.Background(), pixiv.ArtworkRankingRequest{})
				if err == nil {
					for _, artwork := range page.Items {
						dtos = append(dtos, pixiv.ToArtworkDTO(artwork))
					}
				}
			case "SearchArtworks":
				var page sdk.Page[pixiv.Artwork]
				page, err = client.SearchArtworks(context.Background(), pixiv.SearchArtworksRequest{Word: "tag"})
				if err == nil {
					for _, artwork := range page.Items {
						dtos = append(dtos, pixiv.ToArtworkDTO(artwork))
					}
				}
			}
			message := ""
			if err != nil {
				message = err.Error()
			}
			rows = append(rows, map[string]any{"operation": operation, "body": json.RawMessage(body), "dto": dtos, "reason": sdk.ReasonOf(err), "message": message})
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "artwork-null-tags.json")
	if *migrationUpdateArtworkSeries {
		if err := os.WriteFile(target, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("artwork nullable tag elements differ from fixed Go reference")
	}
}

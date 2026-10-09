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
	"time"
)

var migrationUpdateNovelSeries = flag.Bool("migration-update-novel-series", false, "capture fixed Go novel series and content contracts")

type migrationSeriesRow struct {
	Name         string            `json:"name"`
	SeriesID     int64             `json:"series_id"`
	Cursor       string            `json:"cursor"`
	NextSeriesID int64             `json:"next_series_id"`
	Bodies       []json.RawMessage `json:"bodies"`
	Results      []any             `json:"results"`
	Queries      []url.Values      `json:"queries"`
}

func TestMigrationNovelSeriesAndContent(t *testing.T) {
	batch := func(next any) json.RawMessage {
		body := map[string]any{}
		if err := json.Unmarshal([]byte(`{"novel_series_detail":{"id":21,"title":"series","caption":"<p>caption</p>","is_concluded":true,"user":{"id":31,"name":"writer","profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}}},"novels":[{"id":9301,"title":"chapter","user":{"id":31},"create_date":"2026-09-01T03:04:05.123456789+09:00","image_urls":{"original":"https://i.pximg.net/novel.jpg"}}]}`), &body); err != nil {
			t.Fatal(err)
		}
		body["next_url"] = next
		data, _ := json.Marshal(body)
		return data
	}
	final := batch(nil)
	rows := []migrationSeriesRow{}
	for _, id := range []int64{21, 0, -1, 9223372036854775807} {
		rows = append(rows, migrationSeriesRow{Name: fmt.Sprintf("id:%d", id), SeriesID: id, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v2/novel/series?last_order=9&series_id=21"), final}})
	}
	for _, next := range []any{nil, "", "https://app-api.pixiv.net/v2/novel/series?last_order=9", "https://app-api.pixiv.net:/v2/novel/series?last_order=%2B9", "https://app-api.pixiv.net/v2/novel/series?last_order=0", "https://app-api.pixiv.net/v2/novel/series?last_order=-1", "https://app-api.pixiv.net/v2/novel/series?last_order=9223372036854775808", "http://app-api.pixiv.net/v2/novel/series?last_order=9", "https://APP-API.PIXIV.NET/v2/novel/series?last_order=9", "https://app-api.pixiv.net:443/v2/novel/series?last_order=9", "https://user@app-api.pixiv.net/v2/novel/series?last_order=9", "https://app-api.pixiv.net/v1/novel/ranking?last_order=9", "https://app-api.pixiv.net/v2/novel/series?last_order=9&last_order=10", "https://app-api.pixiv.net/v2/novel/series?last_order=9&unknown=1", "https://app-api.pixiv.net/v2/novel/series?last_order=9&series_id=changed", "https://app-api.pixiv.net/v2/novel/series?last_order=9#fragment", "https://app-api.pixiv.net/v2/novel/series?last_order=%zz", "https://app-api.pixiv.net/v2/novel/series?last_order=9;series_id=21", 1} {
		rows = append(rows, migrationSeriesRow{Name: fmt.Sprintf("next:%v", next), SeriesID: 21, Bodies: []json.RawMessage{batch(next), final}})
	}
	for _, body := range []string{`{}`, `{"novel_series_detail":null,"novels":[]}`, `{"novel_series_detail":{"id":21,"user":{"id":31}},"novels":[]}`, `{"novel_series_detail":{"id":0,"user":{"id":31}},"novels":[]}`, `{"novel_series_detail":{"id":21,"user":null},"novels":[]}`, `{"novel_series_detail":{"id":21,"user":{"id":31}}}`, `{"novel_series_detail":{"id":21,"user":{"id":31}},"novels":null}`, `{"novel_series_detail":{"id":21,"user":{"id":31}},"novels":[null]}`} {
		rows = append(rows, migrationSeriesRow{Name: "body:" + body, SeriesID: 21, Bodies: []json.RawMessage{[]byte(body)}})
	}
	for _, field := range []struct {
		name  string
		value any
	}{{"id", 0}, {"user", nil}, {"create_date", "invalid"}, {"tags", []any{nil}}, {"title", 7}, {"image_urls", map[string]any{"original": "https://evil.example/cover.jpg"}}} {
		var body map[string]any
		json.Unmarshal(final, &body)
		body["novels"].([]any)[0].(map[string]any)[field.name] = field.value
		data, _ := json.Marshal(body)
		rows = append(rows, migrationSeriesRow{Name: fmt.Sprintf("field:%s:%v", field.name, field.value), SeriesID: 21, Bodies: []json.RawMessage{data}})
	}
	digest := fmt.Sprintf("%x", sha256.Sum256([]byte("series_id=21&")))
	for _, payload := range []string{`{"k":"last_order","v":9}`, `{"k":"last_order","v":0}`, `{"k":"last_order","v":-1}`, `{"k":"last_order","v":9,"s":1}`, `{"k":"last_order","v":9,"s":-1}`, `{"k":"offset","v":9}`, `{}`, `null`, `{"k":"last_order","v":9,"p":{"last_order":["9"]}}`, `{"p":{"last_order":["9"]}}`, `{"k":"last_order","v":"9"}`} {
		cursor, err := sdk.NewCursor("pixiv", "NovelSeries", 1, digest, []byte(payload), sdk.WithCursorIdentity("foreign"))
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, migrationSeriesRow{Name: "cursor:" + payload, SeriesID: 21, Cursor: cursor.String(), Bodies: []json.RawMessage{final}})
	}
	rows = append(rows, migrationSeriesRow{Name: "binding:series", SeriesID: 21, NextSeriesID: 22, Bodies: []json.RawMessage{batch("https://app-api.pixiv.net/v2/novel/series?last_order=9"), final}})
	for index := range rows {
		row := &rows[index]
		row.Results = []any{}
		row.Queries = []url.Values{}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.Method != "GET" || req.URL.Path != "/v2/novel/series" {
				t.Fatal("unexpected series request")
			}
			row.Queries = append(row.Queries, req.URL.Query())
			body := row.Bodies[0]
			if len(row.Bodies) > 1 && req.URL.Query().Get("last_order") != "" {
				body = row.Bodies[1]
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		request := pixiv.NovelSeriesRequest{SeriesID: row.SeriesID}
		if row.Cursor != "" {
			request.Cursor, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			page, err := client.NovelSeries(context.Background(), request)
			result := map[string]any{"dto": nil, "cursor": nil, "reason": "", "message": ""}
			if err != nil {
				result["reason"] = sdk.ReasonOf(err)
				result["message"] = err.Error()
			} else {
				dto := pixiv.ToNovelSeriesResultDTO(page)
				dto.Novels.Next = ""
				result["dto"] = dto
				result["cursor"] = migrationSearchCursor(t, page.Novels.Next)
			}
			row.Results = append(row.Results, result)
			if err != nil || page.Novels.Next.IsZero() {
				break
			}
			request.Cursor = page.Novels.Next
			if row.NextSeriesID != 0 {
				request.SeriesID = row.NextSeriesID
			}
		}
	}
	content := []any{}
	client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(*http.Request) (*http.Response, error) {
		t.Fatal("NovelContent must not use HTTP")
		return nil, nil
	})}})
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []int64{-1, 0, 1, 9223372036854775807} {
		_, err := client.NovelContent(context.Background(), pixiv.NovelContentRequest{NovelID: id})
		content = append(content, map[string]any{"novel_id": id, "reason": sdk.ReasonOf(err), "message": err.Error()})
	}
	payload := map[string]string{"safe": "value"}
	body := pixiv.NovelContent{NovelID: 42, Title: "title", Caption: "caption", Blocks: []pixiv.NovelBlock{
		{Kind: pixiv.NovelBlockParagraph, Text: "text", Marks: []pixiv.NovelMark{{Kind: pixiv.NovelMarkStrong, Text: "strong"}, {Kind: pixiv.NovelMarkRuby, Ruby: &pixiv.NovelRuby{Text: "漢字", Furigana: "かんじ"}}, {Kind: pixiv.NovelMarkLink, Href: "https://example.com"}, {Kind: pixiv.NovelMarkCustom, Class: "custom"}, {Kind: pixiv.NovelMarkKind("future"), Text: "future"}}},
		{Kind: pixiv.NovelBlockHeader, Text: "header"},
		{Kind: pixiv.NovelBlockImage, Image: &pixiv.NovelImageBlock{Caption: "image", Width: 12, Height: 34}},
		{Kind: pixiv.NovelBlockFile, File: &pixiv.NovelFileBlock{Filename: "file", Caption: "caption", Size: 99}},
		{Kind: pixiv.NovelBlockUnknown, Unknown: &pixiv.NovelUnknownBlock{RawType: "widget", Payload: payload}},
		{Kind: pixiv.NovelBlockKind("future"), Unknown: &pixiv.NovelUnknownBlock{RawType: "future"}},
	}}
	dto := pixiv.ToNovelContentDTO(body)
	payload["safe"] = "changed"
	if dto.Blocks[4].Unknown.Payload["safe"] != "value" {
		t.Fatal("DTO did not copy payload")
	}
	contentDTO := []pixiv.NovelContentDTO{pixiv.ToNovelContentDTO(pixiv.NovelContent{}), dto}
	ref, err := sdk.NewResourceRef("pixiv", []byte(`{"kind":"novel_image","id":42,"index":0}`))
	if err != nil {
		t.Fatal(err)
	}
	expires := time.Date(2026, 10, 9, 1, 2, 3, 0, time.UTC)
	imageResource := sdk.Resource{Ref: ref, URL: "https://i.pximg.net/runtime-image.jpg", RequestHeaders: map[string]string{"Referer": "https://www.pixiv.net/"}, ExpiresAt: &expires, RequiresCredentials: true}
	fileResource := imageResource.Copy()
	fileResource.URL = "https://i.pximg.net/runtime-file.bin"
	fileResource.RequiresCredentials = false
	variantsPayload := map[string]string{"safe": "copied"}
	variants := pixiv.NovelContent{NovelID: 42, Blocks: []pixiv.NovelBlock{
		{Kind: pixiv.NovelBlockImage, Text: "variants", Marks: []pixiv.NovelMark{{Kind: pixiv.NovelMarkEmphasis, Text: "emphasis"}, {Kind: pixiv.NovelMarkDelete, Text: "delete"}, {Kind: pixiv.NovelMarkUnknown, Text: "unknown", Ruby: &pixiv.NovelRuby{Text: "base", Furigana: "reading"}, Href: "https://example.com/optional", Class: "optional"}}, Image: &pixiv.NovelImageBlock{Resource: imageResource, Caption: "resource image", Width: 56, Height: 78}, File: &pixiv.NovelFileBlock{Resource: fileResource, Filename: "file.bin", Caption: "resource file", Size: 123}, Unknown: &pixiv.NovelUnknownBlock{RawType: "mixed", Payload: variantsPayload}},
		{Kind: pixiv.NovelBlockFile, File: &pixiv.NovelFileBlock{Resource: fileResource, Filename: "file.bin", Size: 123}},
	}}
	variantsDTO := pixiv.ToNovelContentDTO(variants)
	variantsPayload["safe"] = "changed"
	if variantsDTO.Blocks[0].Unknown.Payload["safe"] != "copied" {
		t.Fatal("resource DTO did not copy payload")
	}
	encoded, err := json.Marshal(variantsDTO)
	if err != nil {
		t.Fatal(err)
	}
	for _, private := range []string{"runtime-image", "runtime-file", "Referer", "expires_at", "request_headers", "url"} {
		if bytes.Contains(encoded, []byte(private)) {
			t.Fatalf("content DTO leaked %s", private)
		}
	}
	if variantsDTO.Blocks[0].Image.Resource.Ref != ref.String() || !variantsDTO.Blocks[0].Image.Resource.RequiresCredentials || variantsDTO.Blocks[0].File.Resource.RequiresCredentials {
		t.Fatal("resource DTO lost reference or credentials requirement")
	}
	contentDTO = append(contentDTO, variantsDTO)
	data, err := json.MarshalIndent(map[string]any{"series": rows, "content": content, "content_dto": contentDTO}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "novel-series.json")
	if *migrationUpdateNovelSeries {
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
		t.Fatal("novel series/content differs from fixed Go reference")
	}
}

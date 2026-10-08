package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateArtwork = flag.Bool("migration-update-artwork", false, "capture artwork detail contracts from the fixed Go reference")

type migrationArtworkTransport func(*http.Request) (*http.Response, error)

func (f migrationArtworkTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestMigrationArtworkMatchesFrozenDTOAndRequests(t *testing.T) {
	type request struct {
		Method  string            `json:"method"`
		Host    string            `json:"host"`
		Path    string            `json:"path"`
		Query   url.Values        `json:"query"`
		Headers map[string]string `json:"headers"`
	}
	type row struct {
		Name     string          `json:"name"`
		ID       int64           `json:"id"`
		Body     json.RawMessage `json:"body"`
		DTO      json.RawMessage `json:"dto"`
		Reason   sdk.Reason      `json:"reason"`
		Message  string          `json:"message"`
		Requests []request       `json:"requests"`
	}
	base := map[string]any{"id": int64(42), "title": "日本語 <title>", "caption": "<p>fixture</p>", "type": "illust", "create_date": "2026-01-02T03:04:05.123456789+09:00", "page_count": 1, "width": 640, "height": 480, "total_bookmarks": 12, "total_view": 34, "x_restrict": 1, "illust_ai_type": 2, "ai_type": 1, "tools": []string{"pen", "brush"}, "tags": []any{map[string]any{"name": "tag", "translated_name": "訳"}, map[string]any{"name": "untranslated"}}, "user": map[string]any{"id": 7, "name": "author", "account": "fixture", "comment": "profile", "is_followed": true, "profile_image_urls": map[string]any{"medium": "https://i.pximg.net/profile.jpg?fixture-signature=secret"}}, "image_urls": map[string]any{"original": "https://i.pximg.net/cover.jpg?fixture-signature=secret", "large": "https://i.pximg.net/large.jpg"}, "meta_single_page": map[string]any{"original_image_url": "https://i.pximg.net/original.jpg?fixture-signature=secret"}}
	type input struct {
		name   string
		id     int64
		modify func(map[string]any)
	}
	inputs := []input{
		{"complete", 42, func(map[string]any) {}},
		{"minimal", 42, func(v map[string]any) {
			for k := range v {
				if k != "id" && k != "create_date" {
					delete(v, k)
				}
			}
		}},
		{"manga-pages", 42, func(v map[string]any) {
			v["type"] = "manga"
			v["page_count"] = 3
			v["meta_pages"] = []any{map[string]any{"page_index": 99, "width": 100, "height": 200, "image_urls": map[string]any{"large": "https://i.pximg.net/page0.jpg"}}, map[string]any{"width": 300, "height": 400, "image_urls": map[string]any{"original": "https://i.pximg.net/page1.png"}}}
		}},
		{"unknown-kind", 42, func(v map[string]any) { v["type"] = "future_kind" }},
		{"ugoira", 42, func(v map[string]any) { v["type"] = "ugoira" }},
		{"legacy-ai", 42, func(v map[string]any) { delete(v, "illust_ai_type") }},
		{"nulls", 42, func(v map[string]any) {
			for _, k := range []string{"title", "caption", "tags", "tools", "user", "image_urls", "meta_single_page", "meta_pages", "page_count", "illust_ai_type"} {
				v[k] = nil
			}
		}},
		{"negative-counters", 42, func(v map[string]any) {
			for _, k := range []string{"width", "height", "total_bookmarks", "total_view", "page_count", "x_restrict", "illust_ai_type"} {
				v[k] = -1
			}
		}},
		{"large-identities", 42, func(v map[string]any) {
			v["id"] = int64(9223372036854775807)
			v["user"].(map[string]any)["id"] = int64(9223372036854775807)
		}},
		{"invalid-profile-ignored", 42, func(v map[string]any) {
			v["user"].(map[string]any)["profile_image_urls"] = map[string]any{"medium": "https://evil.invalid/profile.jpg"}
		}},
		{"invalid-cover", 42, func(v map[string]any) { v["image_urls"] = map[string]any{"original": "https://evil.invalid/image.jpg"} }},
		{"missing-page-image", 42, func(v map[string]any) { v["meta_pages"] = []any{map[string]any{}} }},
		{"invalid-time", 42, func(v map[string]any) { v["create_date"] = "not-a-date" }},
		{"missing-time", 42, func(v map[string]any) { delete(v, "create_date") }},
		{"invalid-time-and-page", 42, func(v map[string]any) { v["create_date"] = "not-a-date"; v["meta_pages"] = []any{map[string]any{}} }},
		{"invalid-id", 42, func(v map[string]any) { v["id"] = 0 }},
		{"invalid-field-type", 42, func(v map[string]any) { v["width"] = "wide" }},
		{"zero-request", 0, func(map[string]any) {}},
		{"negative-request", -1, func(map[string]any) {}},
	}
	rows := make([]row, 0, len(inputs))
	for _, input := range inputs {
		cloned, err := json.Marshal(base)
		if err != nil {
			t.Fatal(err)
		}
		var value map[string]any
		decoder := json.NewDecoder(bytes.NewReader(cloned))
		decoder.UseNumber()
		if err := decoder.Decode(&value); err != nil {
			t.Fatal(err)
		}
		input.modify(value)
		body, err := json.Marshal(map[string]any{"illust": value})
		if err != nil {
			t.Fatal(err)
		}
		item := row{Name: input.name, ID: input.id, Body: body, Requests: []request{}}
		transport := migrationArtworkTransport(func(r *http.Request) (*http.Response, error) {
			headers := map[string]string{}
			for _, key := range []string{"User-Agent", "App-OS", "App-OS-Version", "App-Version", "Referer", "Authorization"} {
				headers[key] = r.Header.Get(key)
			}
			item.Requests = append(item.Requests, request{r.Method, r.URL.Host, r.URL.Path, r.URL.Query(), headers})
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(body)), Request: r}, nil
		})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.Artwork(context.Background(), pixiv.ArtworkRequest{ArtworkID: input.id})
		item.Reason = sdk.ReasonOf(err)
		if err != nil {
			item.Message = err.Error()
		} else {
			item.DTO, err = json.Marshal(pixiv.ToArtworkDTO(result))
			if err != nil {
				t.Fatal(err)
			}
		}
		rows = append(rows, item)
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "artwork-detail.json")
	if *migrationUpdateArtwork {
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
		t.Fatal("artwork DTO and requests differ from the fixed Go reference")
	}
}

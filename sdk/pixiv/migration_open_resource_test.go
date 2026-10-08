package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateOpenResource = flag.Bool("migration-update-open-resource", false, "capture resource open contracts from the fixed Go reference")

func TestMigrationOpenResourceMatchesFrozenMetadataResolution(t *testing.T) {
	type row struct {
		Name        string          `json:"name"`
		Payload     string          `json:"payload"`
		Product     string          `json:"product"`
		Zero        bool            `json:"zero"`
		Method      string          `json:"method"`
		Preload     bool            `json:"preload"`
		Body        json.RawMessage `json:"body"`
		Error       json.RawMessage `json:"error"`
		APIRequests []string        `json:"api_requests"`
		URLs        []string        `json:"urls"`
		Headers     []http.Header   `json:"headers"`
	}
	single := json.RawMessage(`{"illust":{"id":42,"meta_single_page":{"original_image_url":"https://i.pximg.net/img-original/img/2026/10/08/42_p0.png?fixture=secret#fragment"}}}`)
	multi := json.RawMessage(`{"illust":{"id":42,"meta_pages":[{"image_urls":{"original":"https://i.pximg.net/first.png","large":"https://i.pximg.net/first-large.jpg"}},{"image_urls":{"medium":"https://i.pximg.net/second.jpg"}}],"image_urls":{"large":"https://i.pximg.net/cover.jpg"},"meta_single_page":{"original_image_url":"https://i.pximg.net/fallback.png"}}}`)
	rows := []row{}
	for _, body := range []struct {
		name string
		body json.RawMessage
	}{{"single", single}, {"multi", multi}} {
		for _, page := range []int{-1, 0, 1, 2} {
			for _, variant := range []string{"", "original", "large", "regular", "small", "thumb", "mini", "unsupported"} {
				payload, _ := json.Marshal(map[string]any{"k": "artwork", "id": 42, "p": page, "v": variant})
				rows = append(rows, row{Name: body.name, Payload: string(payload), Product: "pixiv", Body: body.body})
			}
		}
	}
	for _, payload := range []string{`{}`, `null`, `[]`, `{"k":"artwork","id":0}`, `{"k":"artwork","id":-1}`, `{"k":"artwork","id":42,"p":-2}`, `{"k":"artwork","id":42,"p":"0"}`, `{"k":"artwork","id":42,"v":5}`, `{"k":"unknown","id":42}`, `{"k":"artwork","id":42,"p":null}`, `{"k":"artwork","id":42,"id":0}`} {
		rows = append(rows, row{Name: "identity", Payload: payload, Product: "pixiv", Body: single})
	}
	rows = append(rows,
		row{Name: "zero", Zero: true, Body: single},
		row{Name: "other-product", Product: "fanbox", Payload: `{"k":"artwork","id":42}`, Body: single},
		row{Name: "method-priority", Zero: true, Method: "POST", Body: single},
		row{Name: "head", Product: "pixiv", Payload: `{"k":"artwork","id":42}`, Method: "HEAD", Body: single},
		row{Name: "preloaded", Product: "pixiv", Payload: `{"k":"artwork","id":42,"v":"original"}`, Preload: true, Body: single},
		row{Name: "forbidden-url", Product: "pixiv", Payload: `{"k":"artwork","id":42}`, Body: json.RawMessage(`{"illust":{"id":42,"meta_single_page":{"original_image_url":"http://i.pximg.net/forbidden.png"}}}`)},
		row{Name: "missing-id", Product: "pixiv", Payload: `{"k":"artwork","id":42}`, Body: json.RawMessage(`{"illust":{"meta_single_page":{"original_image_url":"https://i.pximg.net/first.png"}}}`)},
	)
	for _, metadata := range []struct{ kind, body string }{
		{"novel_cover", `{"novel":{"id":42,"user":{"id":7},"image_urls":{"original":"https://i.pximg.net/novel.png","large":"https://i.pximg.net/novel-large.jpg","medium":"https://i.pximg.net/novel-medium.jpg","square_medium":"https://i.pximg.net/novel-square.jpg"}}}`},
		{"user_profile", `{"user":{"id":42,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile":{},"profile_publicity":{"gender":"public","region":false},"workspace":{}}`},
		{"stamp", `{"stamps":[{"stamp_id":1,"stamp_url":"https://i.pximg.net/other.png"},{"stamp_id":42,"stamp_url":"https://s.pximg.net/stamp.png"}]}`},
	} {
		for _, variant := range []string{"", "original", "large", "medium", "square_medium", "regular", "unsupported"} {
			payload, err := json.Marshal(map[string]any{"k": metadata.kind, "id": 42, "p": 123, "v": variant})
			if err != nil {
				t.Fatal(err)
			}
			rows = append(rows, row{Name: metadata.kind, Product: "pixiv", Payload: string(payload), Body: json.RawMessage(metadata.body)})
		}
	}
	for _, metadata := range []struct{ kind, body string }{
		{"novel_cover", `{}`},
		{"novel_cover", `{"novel":{"id":42,"user":{"id":7}}}`},
		{"novel_cover", `{"novel":{"id":42,"user":{"id":0},"image_urls":{"original":"https://i.pximg.net/novel.png"}}}`},
		{"novel_cover", `{"novel":{"id":42,"user":{"id":7},"image_urls":{"large":"https://i.pximg.net/novel.jpg"}}}`},
		{"novel_cover", `{"novel":{"id":42,"user":{"id":7},"image_urls":{"original":"http://i.pximg.net/novel.png"}}}`},
		{"novel_cover", `{"novel":{"id":42,"user":{"id":7},"image_urls":{"original":"https://i.pximg.net/novel.png"}},"series_next":{"id":0}}`},
		{"novel_cover", `{"novel":{"id":42,"user":{"id":7},"image_urls":{"original":"https://i.pximg.net/novel.png"},"tags":[{"name":5}]}}`},
		{"user_profile", `{}`},
		{"user_profile", `{"user":{"id":42},"profile":{},"profile_publicity":{},"workspace":{}}`},
		{"user_profile", `{"user":{"id":42,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile":null,"profile_publicity":{},"workspace":{}}`},
		{"user_profile", `{"user":{"id":42,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile":{},"profile_publicity":{"gender":null},"workspace":{}}`},
		{"user_profile", `{"user":{"id":42,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile":{},"profile_publicity":{"gender":"hidden"},"workspace":{}}`},
		{"user_profile", `{"user":{"id":42,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile":{"birth_year":"2026"},"profile_publicity":{},"workspace":{}}`},
		{"user_profile", `{"user":{"id":42,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile":{},"profile_publicity":{},"workspace":{"pc":1}}`},
		{"stamp", `{}`},
		{"stamp", `{"stamps":null}`},
		{"stamp", `{"stamps":[]}`},
		{"stamp", `{"stamps":[{"stamp_id":42,"stamp_url":"https://s.pximg.net/stamp.png"}],"next_url":"https://app-api.pixiv.net/next"}`},
		{"stamp", `{"stamps":[{"stamp_id":42,"stamp_url":"https://s.pximg.net/stamp.png"},{"stamp_id":0,"stamp_url":"https://s.pximg.net/other.png"}]}`},
		{"stamp", `{"stamps":[{"stamp_id":42,"stamp_url":"http://s.pximg.net/stamp.png"}]}`},
		{"stamp", `{"stamps":[{"stamp_id":42,"stamp_url":"https://media.fixture.invalid/stamp.png"}]}`},
	} {
		payload, err := json.Marshal(map[string]any{"k": metadata.kind, "id": 42})
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{Name: metadata.kind + "-invalid", Product: "pixiv", Payload: string(payload), Body: json.RawMessage(metadata.body)})
	}
	for _, variant := range []string{"", "original", "medium", "large", "unsupported"} {
		payload, err := json.Marshal(map[string]any{"k": "ugoira_archive", "id": 42, "p": 123, "v": variant})
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{Name: "ugoira-variant", Product: "pixiv", Payload: string(payload), Body: json.RawMessage(`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/original.zip","medium":"https://i.pximg.net/medium.zip"},"frames":[{"file":"000000.jpg","delay":100}]}}`)})
	}
	for _, frames := range []string{
		`[{"file":"000000.jpg","delay":0}]`, `[{"file":"000000.jpg","delay":-1}]`,
		`[{"file":"000000.jpg"}]`, `[{"file":"000000.jpg","delay":null}]`,
		`[{"file":"000000.jpg","delay":9223372036854775807}]`,
		`[{"file":"000000.jpg","delay":"100"}]`, `[{"file":"000000.jpg","delay":1.5}]`,
		`[{"file":"000000.jpg","delay":9223372036854775808}]`,
		`[]`, `null`, `[null]`, `[{}]`,
		`[{"file":"../000000.jpg"}]`, `[{"file":"/000000.jpg"}]`, `[{"file":"C:000000.jpg"}]`,
		`[{"file":"dir//000000.jpg"}]`, `[{"file":"dir/./000000.jpg"}]`, `[{"file":"dir/../000000.jpg"}]`,
		`[{"file":"dir/"}]`, `[{"file":"000\u000000.jpg"}]`,
		`[{"file":"dir/000000.jpg"},{"file":"dir\\000000.jpg"}]`,
		`[{"file":"000000.jpg"},{"file":"000000.jpg"}]`,
		`[{"file":"A.jpg"},{"file":"a.jpg"}]`, `[{"file":"dir\\000000.jpg"}]`,
		`[{"file":"日本語/000000.jpg"}]`, `[{"file":"%2e%2e/000000.jpg"}]`,
	} {
		body := `{"ugoira_metadata":{"zip_urls":{"medium":"https://i.pximg.net/medium.zip"},"frames":` + frames + `}}`
		rows = append(rows, row{Name: "ugoira-frames", Product: "pixiv", Payload: `{"k":"ugoira_archive","id":42}`, Body: json.RawMessage(body)})
	}
	for _, body := range []string{
		`{}`, `{"ugoira_metadata":null}`, `{"ugoira_metadata":{"zip_urls":{},"frames":[{"file":"0.jpg"}]}}`,
		`{"ugoira_metadata":{"zip_urls":null,"frames":[{"file":"0.jpg"}]}}`,
		`{"ugoira_metadata":{"zip_urls":{"medium":42},"frames":[{"file":"0.jpg"}]}}`,
		`{"ugoira_metadata":{"zip_urls":{"medium":"https://i.pximg.net/medium.zip"}}}`,
		`{"ugoira_metadata":{"zip_urls":{"original":"http://i.pximg.net/original.zip","medium":"https://i.pximg.net/medium.zip"},"frames":[{"file":"0.jpg"}]}}`,
	} {
		rows = append(rows, row{Name: "ugoira-envelope", Product: "pixiv", Payload: `{"k":"ugoira_archive","id":42}`, Body: json.RawMessage(body)})
	}
	for index := range rows {
		input := &rows[index]
		input.APIRequests = []string{}
		input.URLs = []string{}
		input.Headers = []http.Header{}
		transport := migrationArtworkTransport(func(r *http.Request) (*http.Response, error) {
			if r.URL.Host == "app-api.pixiv.net" {
				input.APIRequests = append(input.APIRequests, r.URL.RequestURI())
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(input.Body)), Request: r}, nil
			}
			input.URLs = append(input.URLs, r.URL.String())
			headers := http.Header{}
			for _, name := range []string{"Range", "If-None-Match", "If-Modified-Since", "If-Range", "Referer", "User-Agent", "Authorization", "Cookie"} {
				if values := r.Header.Values(name); len(values) > 0 {
					headers[name] = values
				}
			}
			input.Headers = append(input.Headers, headers)
			return &http.Response{StatusCode: 206, Header: http.Header{"Content-Type": {"image/png"}}, Body: io.NopCloser(bytes.NewBufferString("abc")), Request: r}, nil
		})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		if err != nil {
			t.Fatal(err)
		}
		var ref sdk.ResourceRef
		if !input.Zero {
			ref, err = sdk.NewResourceRef(input.Product, []byte(input.Payload))
			if err != nil {
				t.Fatal(err)
			}
		}
		if input.Preload {
			if _, err := client.ArtworkPages(context.Background(), pixiv.ArtworkPagesRequest{ArtworkID: 42}); err != nil {
				t.Fatal(err)
			}
		}
		for attempt := 0; attempt < 2; attempt++ {
			response, err := client.OpenResource(context.Background(), sdk.OpenResourceRequest{Ref: ref, Method: sdk.ResourceMethod(input.Method), Range: "bytes=0-2", IfNoneMatch: `"fixture"`, IfModifiedSince: "Thu, 08 Oct 2026 00:00:00 GMT", IfRange: `"range"`})
			if err != nil {
				input.Error, _ = json.Marshal(err)
				break
			}
			if response.StatusCode != 206 {
				t.Fatalf("unexpected resource status: %d", response.StatusCode)
			}
			if _, err := io.Copy(io.Discard, response.Body); err != nil {
				t.Fatal(err)
			}
			if err := response.Body.Close(); err != nil {
				t.Fatal(err)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "artwork-open-resource.json")
	if *migrationUpdateOpenResource {
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
		t.Fatal("artwork resource opens differ from the fixed Go reference")
	}
}

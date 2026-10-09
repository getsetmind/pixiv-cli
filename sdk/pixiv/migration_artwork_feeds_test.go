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

var migrationUpdateArtworkFeeds = flag.Bool("migration-update-artwork-feeds", false, "capture related and recommended artwork contracts")

type migrationArtworkFeedRow struct {
	Name       string                  `json:"name"`
	Operation  string                  `json:"operation"`
	ID         int64                   `json:"id"`
	UserID     int64                   `json:"user_id"`
	Cursor     string                  `json:"cursor"`
	NextMode   string                  `json:"next_mode"`
	Bodies     []json.RawMessage       `json:"bodies"`
	Results    []migrationSearchResult `json:"results"`
	Queries    []url.Values            `json:"queries"`
	RawQueries []string                `json:"raw_queries"`
}

func TestMigrationArtworkFeedsPreserveStructuredContinuation(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "search-pages.json"))
	if err != nil {
		t.Fatal(err)
	}
	var sources []struct {
		Bodies []json.RawMessage `json:"bodies"`
	}
	if err = json.Unmarshal(data, &sources); err != nil {
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
	rows := []migrationArtworkFeedRow{}
	for _, op := range []string{"RelatedArtworks", "RecommendedArtworks"} {
		endpoint := "/v1/illust/recommended"
		base := ""
		if op == "RelatedArtworks" {
			endpoint = "/v2/illust/related"
			base = "illust_id=123&"
		}
		prefix := "https://app-api.pixiv.net" + endpoint + "?"
		nexts := []any{nil, "", prefix + base + "offset=30", prefix + base + "offset=0", prefix + base + "offset=-1", prefix + base + "offset=%2B30", prefix + base + "offset=0030", prefix + base + "offset=9223372036854775807", prefix + base + "offset=9223372036854775808", prefix + base + "offset=30&offset=40", prefix + base + "offset=30&unknown=1", prefix + base + "offset=30#fragment", prefix + base + "offset=%zz", prefix + base + "offset=30;x=1", "http://app-api.pixiv.net" + endpoint + "?" + base + "offset=30", "https://APP-API.PIXIV.NET" + endpoint + "?" + base + "offset=30", "https://app-api.pixiv.net:443" + endpoint + "?" + base + "offset=30", "https://app-api.pixiv.net:" + endpoint + "?" + base + "offset=30", "https://user@app-api.pixiv.net" + endpoint + "?" + base + "offset=30", prefix + base + "offset=30#", 1}
		if op == "RelatedArtworks" {
			nexts = append(nexts, prefix+"illust_id=123&seed_illust_ids[1]=789&viewed[1]=456&seed_illust_ids[0]=456&viewed[0]=123", prefix+"illust_id=123&seed_illust_ids[0]=456&viewed[0]=123", prefix+"illust_id=123&seed_illust_ids[1]=456&viewed[0]=123", prefix+"illust_id=123&seed_illust_ids[]=456&viewed[]=123", prefix+"illust_id=123&seed_illust_ids[0]=0&viewed[0]=123", prefix+"illust_id=124&offset=30", prefix+"offset=30", prefix+"illust_id=123&offset=30&viewed[0]=123")
		} else {
			nexts = append(nexts, prefix+"offset=0&min_bookmark_id_for_recent_illust=0012&max_bookmark_id_for_recommend=%2B34&include_ranking_illusts=true&include_privacy_policy=false&viewed[0]=123&viewed[0]=456", prefix+"include_privacy_policy=true", prefix+"max_bookmark_id_for_recommend=-1", prefix+"include_ranking_illusts=TRUE", prefix+"viewed[]=123", prefix+"offset=0&viewed[]=123", prefix+"offset=0&viewed[bad]=")
		}
		for _, next := range nexts {
			rows = append(rows, migrationArtworkFeedRow{Name: op + fmt.Sprintf(":next:%v", next), Operation: op, ID: 123, Bodies: []json.RawMessage{batch(next), final}})
		}
		for _, body := range []string{`{}`, `{"illusts":null}`, `{"illusts":[]}`, `{"illusts":{}}`, `{"illusts":[null]}`, `{"illusts":[{}]}`, `{"illusts":[{"id":-1}]}`, `{"illusts":[{"id":1,"title":1}]}`, `{"illusts":[{"id":1,"create_date":"invalid"}]}`} {
			rows = append(rows, migrationArtworkFeedRow{Name: op + ":body:" + body, Operation: op, ID: 123, Bodies: []json.RawMessage{[]byte(body)}})
		}
		digest := fmt.Sprintf("%x", sha256.Sum256([]byte(base)))
		payloads := []string{`{"k":"offset","v":30}`, `{}`, `null`, `{"k":"offset","v":-1}`, `{"p":{"offset":["30"]},"s":-1}`, `{"p":{"offset":["30"]},"s":1}`, `{"p":{"offset":["30"]},"v":-1}`, `{"k":"offset","p":{"offset":["30"]}}`, `{"p":{"offset":["0"]}}`, `{"p":{"offset":["30","40"]}}`, `{"p":{"offset":null}}`, `{"p":{"unknown":["30"]}}`, `{"p":{"include_privacy_policy":["true"]}}`, `{"p":{"illust_id":["123"],"offset":["30"]}}`, `{"p":{"illust_id":["124"],"offset":["30"]}}`, `{"p":{"illust_id":["123"],"seed_illust_ids[]":["789","456","789"],"viewed[]":["123","456"]}}`}
		for _, payload := range payloads {
			for _, identity := range []string{"0", "foreign"} {
				cur, err := sdk.NewCursor("pixiv", op, 2, digest, []byte(payload), sdk.WithCursorIdentity(identity))
				if err != nil {
					t.Fatal(err)
				}
				rows = append(rows, migrationArtworkFeedRow{Name: op + ":cursor:" + identity + ":" + payload, Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
			}
		}
		for _, version := range []int{1, 3} {
			cur, err := sdk.NewCursor("pixiv", op, version, digest, []byte(`{"p":{"offset":["30"]}}`), sdk.WithCursorIdentity("0"))
			if err != nil {
				t.Fatal(err)
			}
			rows = append(rows, migrationArtworkFeedRow{Name: fmt.Sprintf("%s:version:%d", op, version), Operation: op, ID: 123, Cursor: cur.String(), Bodies: []json.RawMessage{final}})
		}
		for _, mode := range []string{"same_account", "other_account", "anonymous_account"} {
			rows = append(rows, migrationArtworkFeedRow{Name: op + ":identity:" + mode, Operation: op, ID: 123, UserID: 7, NextMode: mode, Bodies: []json.RawMessage{batch(prefix + base + "offset=30"), final}})
		}
		for _, mode := range []string{"other_client", "other_query"} {
			rows = append(rows, migrationArtworkFeedRow{Name: op + ":binding:" + mode, Operation: op, ID: 123, NextMode: mode, Bodies: []json.RawMessage{batch(prefix + base + "offset=30"), final}})
		}
	}
	rows = append(rows, migrationArtworkFeedRow{Name: "RelatedArtworks:invalid-id", Operation: "RelatedArtworks", ID: 0, Bodies: []json.RawMessage{final}}, migrationArtworkFeedRow{Name: "RelatedArtworks:negative-id", Operation: "RelatedArtworks", ID: -1, Bodies: []json.RawMessage{final}})
	for index := range rows {
		row := &rows[index]
		row.Results = []migrationSearchResult{}
		row.Queries = []url.Values{}
		row.RawQueries = []string{}
		userID := row.UserID
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				payload := fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d,"name":"fixture"}}`, userID)
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(payload)), Request: req}, nil
			}
			endpoint := "/v1/illust/recommended"
			if row.Operation == "RelatedArtworks" {
				endpoint = "/v2/illust/related"
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
			cur, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		id := row.ID
		for step := 0; step < 2; step++ {
			var page sdk.Page[pixiv.Artwork]
			var callErr error
			if row.Operation == "RelatedArtworks" {
				page, callErr = client.RelatedArtworks(context.Background(), pixiv.RelatedArtworksRequest{ArtworkID: id, Cursor: cur})
			} else {
				page, callErr = client.RecommendedArtworks(context.Background(), pixiv.RecommendedArtworksRequest{Cursor: cur})
			}
			result := migrationSearchResult{Items: []pixiv.ArtworkDTO{}}
			if callErr != nil {
				result.Reason = sdk.ReasonOf(callErr)
				result.Message = callErr.Error()
			} else {
				for _, item := range page.Items {
					result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
				}
				result.Cursor = migrationSearchCursor(t, page.Next)
			}
			row.Results = append(row.Results, result)
			if callErr != nil || page.Next.IsZero() {
				break
			}
			cur = page.Next
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
			if row.NextMode == "other_query" {
				id++
			}
		}
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join(path, "artwork-feeds.json")
	if *migrationUpdateArtworkFeeds {
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

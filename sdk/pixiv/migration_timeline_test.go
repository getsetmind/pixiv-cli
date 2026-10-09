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

type migrationTimelineResult struct {
	Items   []any      `json:"items"`
	Cursor  any        `json:"cursor"`
	Reason  sdk.Reason `json:"reason"`
	Message string     `json:"message"`
}

var migrationUpdateTimeline = flag.Bool("migration-update-timeline", false, "capture following and latest timeline contracts")

type migrationTimelineRow struct {
	Kind       string                    `json:"kind"`
	NextKind   string                    `json:"next_kind"`
	Name       string                    `json:"name"`
	Operation  string                    `json:"operation"`
	ID         int64                     `json:"id"`
	UserID     int64                     `json:"user_id"`
	Cursor     string                    `json:"cursor"`
	NextMode   string                    `json:"next_mode"`
	Bodies     []json.RawMessage         `json:"bodies"`
	Results    []migrationTimelineResult `json:"results"`
	Queries    []url.Values              `json:"queries"`
	RawQueries []string                  `json:"raw_queries"`
}

func TestMigrationTimelinePreserveContract(t *testing.T) {
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	var rows []migrationTimelineRow
	var reference []migrationUserWorksRow
	data, err := os.ReadFile(filepath.Join(path, "user-works.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, &reference); err != nil {
		t.Fatal(err)
	}
	for _, op := range []string{"FollowingArtworks", "FollowingNovels", "LatestArtworks", "LatestNovels"} {
		key, endpoint, continuation, base := "novels", "/v1/novel/new", "max_novel_id", "filter=for_android&"
		following := op == "FollowingArtworks" || op == "FollowingNovels"
		artwork := op == "FollowingArtworks" || op == "LatestArtworks"
		if following {
			continuation, base = "offset", "restrict=public&"
		}
		if artwork {
			key, endpoint = "illusts", "/v1/illust/new"
			if following {
				endpoint = "/v2/illust/follow"
			} else {
				continuation, base = "max_illust_id", "content_type=illust&"
			}
		} else if following {
			endpoint = "/v1/novel/follow"
		}
		var item json.RawMessage
		for _, row := range reference {
			if (row.Operation == "UserArtworks") == artwork {
				var body map[string]json.RawMessage
				if err := json.Unmarshal(row.Bodies[0], &body); err != nil {
					t.Fatal(err)
				}
				var items []json.RawMessage
				if err := json.Unmarshal(body[key], &items); err != nil {
					t.Fatal(err)
				}
				item = items[0]
				break
			}
		}
		batch := func(next any) json.RawMessage {
			data, err := json.Marshal(map[string]any{key: []json.RawMessage{item}, "next_url": next})
			if err != nil {
				t.Fatal(err)
			}
			return data
		}
		final := batch(nil)
		prefix := "https://app-api.pixiv.net" + endpoint + "?"
		appendRow := func(name string, bodies ...json.RawMessage) {
			rows = append(rows, migrationTimelineRow{Name: op + ":" + name, Operation: op, Bodies: bodies})
		}
		for _, next := range []any{nil, "", prefix + continuation + "=30", prefix + continuation + "=0", prefix + continuation + "=-1", prefix + continuation + "=%2B30", prefix + continuation + "=9223372036854775807", prefix + continuation + "=9223372036854775808", prefix + continuation + "=30&" + continuation + "=40", prefix + continuation + "=30&unknown=1", prefix + continuation + "=30#fragment", prefix + continuation + "=%zz", "http://app-api.pixiv.net" + endpoint + "?" + continuation + "=30", prefix + "offset=30", prefix + "max_novel_id=30", prefix + "max_illust_id=30"} {
			appendRow(fmt.Sprintf("next:%v", next), batch(next), final)
		}
		for _, body := range []string{`null`, `{}`, fmt.Sprintf(`{"%s":null}`, key), fmt.Sprintf(`{"%s":[]}`, key), fmt.Sprintf(`{"%s":{}}`, key), fmt.Sprintf(`{"%s":[null]}`, key), fmt.Sprintf(`{"%s":[{}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"title":1}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"create_date":"invalid"}]}`, key)} {
			appendRow("body:"+body, json.RawMessage(body))
		}
		if op == "FollowingArtworks" {
			appendRow("ignored-novels-field", json.RawMessage(`{"illusts":[],"novels":"ignored"}`))
		}
		if op == "FollowingNovels" {
			appendRow("ignored-illusts-field", json.RawMessage(`{"novels":[],"illusts":"ignored"}`))
		}
		digest := fmt.Sprintf("%x", sha256.Sum256([]byte(base)))
		makeCursor := func(product, operation string, version int, digest, payload string, opts ...sdk.CursorOption) string {
			cur, err := sdk.NewCursor(product, operation, version, digest, []byte(payload), opts...)
			if err != nil {
				t.Fatal(err)
			}
			return cur.String()
		}
		for _, payload := range []string{fmt.Sprintf(`{"k":%q,"v":30}`, continuation), fmt.Sprintf(`{"k":%q,"v":0}`, continuation), fmt.Sprintf(`{"k":%q,"v":-1}`, continuation), `{"k":"offset","v":30}`, `{"k":"other","v":0}`, `{"k":"other","v":30}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"offset","v":30,"p":{"x":["1"]}}`, `{"p":{"offset":["30"]}}`, `{}`, `null`, `{"k":"offset","v":"30"}`} {
			appendRow("cursor:"+payload, final)
			rows[len(rows)-1].Cursor = makeCursor("pixiv", op, 1, digest, payload, sdk.WithCursorIdentity("0"))
		}
		for _, mismatch := range []string{"product", "operation", "version", "query", "account", "instance"} {
			product, operation, version, query := "pixiv", op, 1, digest
			opts := []sdk.CursorOption{sdk.WithCursorIdentity("0")}
			switch mismatch {
			case "product":
				product = "fanbox"
			case "operation":
				operation = "UserArtworks"
			case "version":
				version = 2
			case "query":
				query = "different"
			case "account":
				opts = []sdk.CursorOption{sdk.WithCursorIdentity("foreign")}
			case "instance":
				opts = []sdk.CursorOption{sdk.WithCursorEphemeralInstance("foreign")}
			}
			appendRow("binding:"+mismatch, final)
			rows[len(rows)-1].Cursor = makeCursor(product, operation, version, query, fmt.Sprintf(`{"k":%q,"v":30}`, continuation), opts...)
		}
		if following || op == "LatestArtworks" {
			values := []string{"", "public", "private", "Public", " public", "bad"}
			if !following {
				values = []string{"", "illust", "manga", "all", "illust-and-ugoira", "ugoira", "Illust", " illust"}
			}
			for _, value := range values {
				appendRow("input:"+value, batch(prefix+continuation+"=30"), final)
				rows[len(rows)-1].Kind = value
			}
			appendRow("input-before-cursor", final)
			rows[len(rows)-1].Kind = "bad"
			rows[len(rows)-1].Cursor = makeCursor("pixiv", op, 2, digest, `{}`)
		}
		for _, mode := range []string{"same_account", "other_account", "anonymous_account", "other_client", "changed_kind"} {
			appendRow("replay:"+mode, batch(prefix+continuation+"=30"), final)
			rows[len(rows)-1].UserID = 7
			rows[len(rows)-1].NextMode = mode
		}
		if following {
			appendRow("anonymous-replay", batch(prefix+continuation+"=30"), final)
			appendRow("anonymous-other-client", batch(prefix+continuation+"=30"), final)
			rows[len(rows)-1].NextMode = "other_client"
		}
	}

	for index := range rows {
		row := &rows[index]
		row.Results = []migrationTimelineResult{}
		row.Queries = []url.Values{}
		row.RawQueries = []string{}
		userID := row.UserID
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "oauth.secure.pixiv.net" {
				payload := fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d,"name":"fixture"}}`, userID)
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(payload)), Request: req}, nil
			}
			endpoint := map[string]string{"FollowingArtworks": "/v2/illust/follow", "FollowingNovels": "/v1/novel/follow", "LatestArtworks": "/v1/illust/new", "LatestNovels": "/v1/novel/new"}[row.Operation]
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
			if client.UserID() != userID {
				t.Fatalf("client identity = %d, want %d", client.UserID(), userID)
			}
			return client
		}
		client := makeClient()
		originalID, originalKind := row.ID, row.Kind
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
			result := migrationTimelineResult{Items: []any{}}
			if row.Operation == "FollowingArtworks" || row.Operation == "LatestArtworks" {
				var page sdk.Page[pixiv.Artwork]
				if row.Operation == "FollowingArtworks" {
					page, callErr = client.FollowingArtworks(context.Background(), pixiv.FollowingArtworksRequest{Restrict: pixiv.Restrict(row.Kind), Cursor: cur})
				} else {
					page, callErr = client.LatestArtworks(context.Background(), pixiv.LatestArtworksRequest{ContentType: pixiv.SearchContentType(row.Kind), Cursor: cur})
				}
				if callErr == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
					}
					next = page.Next
				}
			} else {
				var page sdk.Page[pixiv.Novel]
				if row.Operation == "FollowingNovels" {
					page, callErr = client.FollowingNovels(context.Background(), pixiv.FollowingNovelsRequest{Restrict: pixiv.Restrict(row.Kind), Cursor: cur})
				} else {
					page, callErr = client.LatestNovels(context.Background(), pixiv.LatestNovelsRequest{Cursor: cur})
				}
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
			if row.NextKind != "" {
				row.Kind = row.NextKind
			}
			if row.NextMode == "changed_id" {
				row.ID = 124
			}
			if row.NextMode == "changed_kind" {
				if row.Operation == "LatestArtworks" {
					row.Kind = "manga"
				} else {
					row.Kind = "private"
				}
			}
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
		row.ID, row.Kind = originalID, originalKind
	}
	data, err = json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "timeline.json")
	if *migrationUpdateTimeline {
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
		t.Fatal("timelines differ from frozen Go reference")
	}
}

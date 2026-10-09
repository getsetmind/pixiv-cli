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
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateCommentReads = flag.Bool("migration-update-comment-reads", false, "capture read-only comment and stamp contracts")

type migrationCommentReadRow struct {
	Name      string            `json:"name"`
	Operation string            `json:"operation"`
	ID        int64             `json:"id"`
	Cursor    string            `json:"cursor"`
	NextMode  string            `json:"next_mode"`
	Bodies    []json.RawMessage `json:"bodies"`
	Results   []any             `json:"results"`
	Queries   []url.Values      `json:"queries"`
}

func TestMigrationCommentReads(t *testing.T) {
	var rows []migrationCommentReadRow
	for _, op := range []string{"ArtworkComments", "NovelComments", "Stamps"} {
		endpoint, key, queryKey := "/v2/novel/comments", "comments", "novel_id"
		item := `{"id":11,"comment":"","caption":"fallback","date":"2026-09-01T03:04:05.123456789+09:00","created_at":"ignored","user":{"id":31,"name":"writer","account":"author","comment":"hello","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"parent_comment":{"id":10,"comment":"parent","created_at":"2026-09-01T00:00:00Z"}}`
		if op == "ArtworkComments" {
			endpoint, queryKey = "/v3/illust/comments", "illust_id"
		}
		if op == "Stamps" {
			endpoint, key = "/v1/stamps", "stamps"
			item = `{"stamp_id":9,"stamp_url":"https://s.pximg.net/stamp.png?token=sentinel"}`
		}
		batch := func(next any) json.RawMessage {
			data, err := json.Marshal(map[string]any{key: []json.RawMessage{json.RawMessage(item)}, "next_url": next, "total_comments": 42, "access_control": map[string]bool{"can_comment": true, "is_locked": false}, "comment_access_control": 0})
			if err != nil {
				t.Fatal(err)
			}
			return data
		}
		add := func(name string, bodies ...json.RawMessage) {
			rows = append(rows, migrationCommentReadRow{Name: op + ":" + name, Operation: op, ID: 123, Bodies: bodies})
		}
		prefix := "https://app-api.pixiv.net" + endpoint + "?"
		for _, next := range []any{nil, "", prefix + "offset=30", prefix + "offset=0", prefix + "offset=-1", prefix + "offset=%2B30", prefix + "offset=0030", prefix + "offset=9223372036854775807", prefix + "offset=9223372036854775808", prefix + "offset=30&offset=40", prefix + "offset=30&unknown=1", prefix + "offset=30#fragment", prefix + "offset=%zz", prefix + "offset=30;x=1", "http://app-api.pixiv.net" + endpoint + "?offset=30", "https://APP-API.PIXIV.NET" + endpoint + "?offset=30", "https://app-api.pixiv.net:443" + endpoint + "?offset=30", "https://user@app-api.pixiv.net" + endpoint + "?offset=30", prefix + "offset=30&illust_id=456", prefix + "offset=30&novel_id=456", 1} {
			add(fmt.Sprintf("next:%v", next), batch(next), batch(nil))
		}
		bodies := []string{`{}`, fmt.Sprintf(`{"%s":null}`, key), fmt.Sprintf(`{"%s":[]}`, key), fmt.Sprintf(`{"%s":{}}`, key), fmt.Sprintf(`{"%s":[null]}`, key), fmt.Sprintf(`{"%s":[{}]}`, key)}
		if op == "Stamps" {
			bodies = append(bodies, `{"stamps":[{"stamp_id":9,"stamp_url":"https://s.pximg.net/stamp.png"}],"next_url":9e999,"next_url":null}`, `{"STAMPS":[{"STAMP_ID":1,"STAMP_URL":"https://s.pximg.net/stamp.png"}],"next_url":1,"NEXT_URL":null}`, `{"stamps":[{"stamp_id":1,"stamp_id":null,"stamp_url":"https://s.pximg.net/stamp.png"}]}`, `{"stamps":[{"stamp_id":1,"stamp_url":"https://s.pximg.net/stamp.png"},{"stamp_id":2,"stamp_url":"https://example.com/bad.png"}]}`, `{"stamps":[{"stamp_id":1,"stamp_url":"https://example.com/stamp.png"}]}`, `{"stamps":[{"stamp_id":1,"stamp_url":"http://s.pximg.net/stamp.png"}]}`, `{"stamps":[{"stamp_id":1,"stamp_url":"https://s.pximg.net/"}]}`)
		} else {
			bodies = append(bodies, `{"COMMENTS":[{"ID":1,"DATE":"2026-09-01T00:00:00Z","user":{"id":31},"USER":{"name":"merged"},"parent_comment":{"id":2,"date":"2026-09-01T00:00:00Z"},"PARENT_COMMENT":null}],"comment_access_control":0,"COMMENT_ACCESS_CONTROL":null,"access_control":{"can_comment":true}}`, `{"comments":[{"id":1,"id":null,"date":"2026-09-01T00:00:00Z","user":{"id":null,"name":null,"profile_image_urls":{"medium":null}}}],"total_comments":42,"TOTAL_COMMENTS":null}`, `{"comments":[{"id":1,"date":"2026-09-01T03:04:05.1234+09:00"}]}`, `{"comments":[{"id":1,"date":"invalid"}]}`, `{"comments":[{"id":1}]}`, `{"comments":[{"id":1,"date":"2026-09-01T00:00:00Z","parent_comment":{"id":0}}]}`, `{"comments":[{"id":1,"date":"2026-09-01T00:00:00Z","parent_comment":{"id":2,"date":"invalid"}}]}`, `{"comments":[{"id":1,"date":"2026-09-01T00:00:00Z","user":{"id":0}}]}`, `{"comments":[],"total_comments":-1,"access_control":{}}`, `{"comments":[],"comment_access_control":null,"access_control":{"can_comment":true,"is_locked":true}}`, `{"comments":[],"comment_access_control":"1"}`, `{"comments":[{"id":1,"date":null,"created_at":"2026-09-01T00:00:00Z","comment":null}]}`, `{"comments":[{"id":1,"date":"invalid"}],"next_url":"bad"}`)
			for _, id := range []int64{0, -1} {
				add(fmt.Sprintf("id:%d", id), batch(nil))
				rows[len(rows)-1].ID = id
			}
			digest := fmt.Sprintf("%x", sha256.Sum256([]byte(queryKey+"=123&")))
			for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"other","v":30}`, `{"k":"offset","v":30,"s":1}`, `{"p":{"offset":["30"]}}`, `{}`, `null`, `{"k":"offset","v":"30"}`} {
				cur, err := sdk.NewCursor("pixiv", op, 1, digest, []byte(payload))
				if err != nil {
					t.Fatal(err)
				}
				add("cursor:"+payload, batch(nil))
				rows[len(rows)-1].Cursor = cur.String()
			}
			for _, mode := range []string{"other_client", "changed_id", "changed_operation"} {
				add("global:"+mode, batch(prefix+"offset=30"), batch(nil))
				rows[len(rows)-1].NextMode = mode
			}
			for _, ns := range []string{"fanbox", "foreign"} {
				cur, err := sdk.NewCursor(ns, op, 1, digest, []byte(`{"k":"offset","v":30}`))
				if err != nil {
					t.Fatal(err)
				}
				add("namespace:"+ns, batch(nil))
				rows[len(rows)-1].Cursor = cur.String()
			}
		}
		for _, body := range bodies {
			add("body:"+body, json.RawMessage(body))
		}
	}
	for index := range rows {
		row := &rows[index]
		row.Results = []any{}
		row.Queries = []url.Values{}
		rt := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			endpoint := "/v2/novel/comments"
			if row.Operation == "ArtworkComments" {
				endpoint = "/v3/illust/comments"
			}
			if row.Operation == "Stamps" {
				endpoint = "/v1/stamps"
			}
			if req.Method != "GET" || req.URL.Host != "app-api.pixiv.net" || req.URL.Path != endpoint {
				t.Fatal("unexpected request", req.URL)
			}
			step := len(row.Queries)
			row.Queries = append(row.Queries, req.URL.Query())
			if step >= len(row.Bodies) {
				step = len(row.Bodies) - 1
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Bodies[step])), Request: req}, nil
		})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: rt}})
		if err != nil {
			t.Fatal(err)
		}
		cur := sdk.Cursor{}
		if row.Cursor != "" {
			cur, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		for step := 0; step < 2; step++ {
			operation, id := row.Operation, row.ID
			if step > 0 {
				switch row.NextMode {
				case "other_client":
					client, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: rt}})
					if err != nil {
						t.Fatal(err)
					}
				case "changed_id":
					id = 124
				case "changed_operation":
					if operation == "ArtworkComments" {
						operation = "NovelComments"
					} else {
						operation = "ArtworkComments"
					}
				}
			}

			result := map[string]any{"items": []any{}, "total": nil, "access_control": nil, "cursor": nil, "reason": "", "message": ""}
			var next sdk.Cursor
			var callErr error
			if operation == "Stamps" {
				var items []pixiv.Stamp
				items, callErr = client.Stamps(context.Background(), pixiv.StampsRequest{})
				dtos := []pixiv.StampDTO{}
				for _, item := range items {
					dtos = append(dtos, pixiv.ToStampDTO(item))
				}
				result["items"] = dtos
			} else {
				var page pixiv.CommentPage
				if operation == "ArtworkComments" {
					page, callErr = client.ArtworkComments(context.Background(), pixiv.ArtworkCommentsRequest{ArtworkID: id, Cursor: cur})
				} else {
					page, callErr = client.NovelComments(context.Background(), pixiv.NovelCommentsRequest{NovelID: id, Cursor: cur})
				}
				if callErr == nil {
					dto := pixiv.ToCommentPageDTO(page)
					result["items"] = dto.Page.Items
					result["total"] = dto.Total
					result["access_control"] = dto.AccessControl
					next = page.Page.Next
				}
			}
			if callErr != nil {
				result["reason"] = sdk.ReasonOf(callErr)
				result["message"] = callErr.Error()
			} else {
				result["cursor"] = migrationSearchCursor(t, next)
			}
			row.Results = append(row.Results, result)
			if callErr != nil || next.IsZero() {
				break
			}
			cur = next
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := "../../docs/migration/contracts/comment-reads.json"
	if *migrationUpdateCommentReads {
		if err = os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("comment reads differ from frozen Go reference")
	}
}

func TestMigrationStampReferencesPreserveDuplicateCacheAndResolution(t *testing.T) {
	calls := 0
	urls := []string{}
	rt := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
		if req.URL.Host == "app-api.pixiv.net" {
			calls++
			if req.Method != "GET" || req.URL.Path != "/v1/stamps" || req.URL.RawQuery != "" {
				t.Fatal("unexpected stamp read", req.URL)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(`{"stamps":[{"stamp_id":9,"stamp_url":"https://s.pximg.net/stamp.png?token=sentinel"},{"stamp_id":9,"stamp_url":"https://s.pximg.net/duplicate.png"}]}`)), Request: req}, nil
		}
		if req.URL.Host != "s.pximg.net" || req.Header.Get("Authorization") != "" {
			t.Fatal("unexpected resource request", req.URL)
		}
		urls = append(urls, req.URL.String())
		return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(bytes.NewBufferString("fixture")), Request: req}, nil
	})
	makeClient := func() *pixiv.Client {
		c, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: rt}})
		if err != nil {
			t.Fatal(err)
		}
		return c
	}
	first := makeClient()
	stamps, err := first.Stamps(context.Background(), pixiv.StampsRequest{})
	if err != nil {
		t.Fatal(err)
	}
	ref := stamps[0].Image.Resource.Ref
	for _, client := range []*pixiv.Client{first, makeClient()} {
		response, err := client.OpenResource(context.Background(), sdk.OpenResourceRequest{Ref: ref})
		if err != nil {
			t.Fatal(err)
		}
		if err = response.Body.Close(); err != nil {
			t.Fatal(err)
		}
	}
	if calls != 2 || len(urls) != 2 || urls[0] != "https://s.pximg.net/duplicate.png" || urls[1] != "https://s.pximg.net/stamp.png?token=sentinel" {
		t.Fatalf("API calls %d/resource URLs %v", calls, urls)
	}
}

func TestMigrationStampReferenceResolutionPreservesOrderedRawWire(t *testing.T) {
	cases := []struct {
		name, body string
		reason     sdk.Reason
	}{
		{"uppercase", `{"STAMPS":[{"STAMP_ID":9,"STAMP_URL":"https://s.pximg.net/stamp.png"}]}`, ""},
		{"null clearing", `{"stamps":[{"stamp_id":9,"stamp_id":null,"stamp_url":"https://s.pximg.net/stamp.png"}],"next_url":1,"NEXT_URL":null}`, ""},
		{"invalid first field", `{"stamps":[{"stamp_id":"invalid","STAMP_ID":9,"stamp_url":"https://s.pximg.net/stamp.png"}]}`, sdk.MalformedUpstreamResponse},
		{"unrelated invalid item", `{"stamps":[{"stamp_id":9,"stamp_url":"https://s.pximg.net/stamp.png"},{"stamp_id":10,"stamp_url":"https://example.com/bad.png"}]}`, sdk.MalformedUpstreamResponse},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			api, media := 0, 0
			rt := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
				body := tc.body
				if req.URL.Host == "app-api.pixiv.net" {
					api++
					if req.Method != "GET" || req.URL.Path != "/v1/stamps" || req.URL.RawQuery != "" {
						t.Fatal("unexpected API request")
					}
				} else {
					media++
					if req.URL.String() != "https://s.pximg.net/stamp.png" {
						t.Fatal(req.URL)
					}
					body = "fixture"
				}
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewBufferString(body)), Request: req}, nil
			})
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: rt}})
			if err != nil {
				t.Fatal(err)
			}
			ref, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"stamp","id":9}`))
			if err != nil {
				t.Fatal(err)
			}
			response, err := client.OpenResource(context.Background(), sdk.OpenResourceRequest{Ref: ref})
			if sdk.ReasonOf(err) != tc.reason {
				t.Fatalf("reason %q, expected %q: %v", sdk.ReasonOf(err), tc.reason, err)
			}
			if err == nil {
				_ = response.Body.Close()
			}
			if api != 1 || ((err == nil) && (media != 1)) || ((err != nil) && (media != 0)) {
				t.Fatalf("calls API/media %d/%d", api, media)
			}
		})
	}
}

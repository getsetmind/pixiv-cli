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

var migrationUpdateMyPixiv = flag.Bool("migration-update-mypixiv", false, "capture MyPixiv SDK contracts")

func TestMigrationMyPixivPreserveContract(t *testing.T) {
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
	for _, op := range []string{"MyPixivArtworks", "MyPixivNovels", "MyPixivUsers"} {
		key, endpoint := "novels", "/v1/novel/mypixiv"
		artwork := op == "MyPixivArtworks"
		users := op == "MyPixivUsers"
		if artwork {
			key, endpoint = "illusts", "/v2/illust/mypixiv"
		}
		if users {
			key, endpoint = "user_previews", "/v1/user/mypixiv"
		}
		var item json.RawMessage
		if users {
			item = json.RawMessage(`{"user":{"id":41,"name":"artist","account":"account","comment":"comment","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"illusts":7,"novels":"ignored"}`)
		} else {
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
		add := func(name string, bodies ...json.RawMessage) {
			rows = append(rows, migrationTimelineRow{Name: op + ":" + name, Operation: op, UserID: 7, Bodies: bodies})
		}
		for _, next := range []any{nil, "", prefix + "offset=30", prefix + "offset=0", prefix + "offset=-1", prefix + "offset=%2B30", prefix + "offset=9223372036854775807", prefix + "offset=9223372036854775808", prefix + "offset=30&offset=40", prefix + "offset=30&unknown=1", prefix + "offset=30#fragment", prefix + "offset=%zz", "http://app-api.pixiv.net" + endpoint + "?offset=30", prefix + "max_novel_id=30", prefix + "offset=30&restrict=public", prefix + "offset=30&user_id=99&filter=changed", 7} {
			add(fmt.Sprintf("next:%v", next), batch(next), final)
		}
		for _, body := range []string{`null`, `{}`, fmt.Sprintf(`{"%s":null}`, key), fmt.Sprintf(`{"%s":[]}`, key), fmt.Sprintf(`{"%s":{}}`, key), fmt.Sprintf(`{"%s":[null]}`, key), fmt.Sprintf(`{"%s":[{}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":0}}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"title":1}]}`, key), fmt.Sprintf(`{"%s":[{"id":1,"user":{"id":2},"create_date":"invalid"}]}`, key)} {
			add("body:"+body, json.RawMessage(body))
		}
		if artwork {
			for _, owner := range []string{`{}`, `{"user":null}`, `{"user":{"id":0}}`, `{"user":{"id":-1}}`, `{"user":{"id":null}}`} {
				var value map[string]any
				if err := json.Unmarshal([]byte(owner), &value); err != nil {
					t.Fatal(err)
				}
				value["id"] = 1
				data, err := json.Marshal(map[string]any{"illusts": []any{value}})
				if err != nil {
					t.Fatal(err)
				}
				add("owner:"+owner, data)
			}
		}

		if users {
			for _, field := range []string{"id", "name", "account", "comment", "is_followed", "profile_image_urls"} {
				for _, value := range []string{"null", "7", "false", "[]", `{}`, `"value"`} {
					add("field:"+field+":"+value, json.RawMessage(`{"user_previews":[{"user":{"id":41,"`+field+`":`+value+`}}]}`))
				}
			}
			add("ignored-users", json.RawMessage(`{"user_previews":[],"users":7}`))
		}
		digest := fmt.Sprintf("%x", sha256.Sum256(nil))
		makeCursor := func(product, operation string, version int, digest, payload string, opts ...sdk.CursorOption) string {
			cur, err := sdk.NewCursor(product, operation, version, digest, []byte(payload), opts...)
			if err != nil {
				t.Fatal(err)
			}
			return cur.String()
		}
		for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"other","v":30}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"offset","v":30,"p":{"x":["1"]}}`, `{"p":{"offset":["30"]}}`, `{}`, `null`, `{"k":"offset","v":"30"}`} {
			add("cursor:"+payload, final)
			rows[len(rows)-1].Cursor = makeCursor("pixiv", op, 1, digest, payload, sdk.WithCursorIdentity("7"))
		}
		for _, mismatch := range []string{"product", "operation", "version", "query", "account", "instance", "global"} {
			product, operation, version, query := "pixiv", op, 1, digest
			opts := []sdk.CursorOption{sdk.WithCursorIdentity("7")}
			switch mismatch {
			case "product":
				product = "fanbox"
			case "operation":
				operation = "FollowingArtworks"
			case "version":
				version = 2
			case "query":
				query = "different"
			case "account":
				opts = []sdk.CursorOption{sdk.WithCursorIdentity("foreign")}
			case "instance":
				opts = []sdk.CursorOption{sdk.WithCursorEphemeralInstance("foreign")}
			case "global":
				opts = nil
			}
			add("binding:"+mismatch, final)
			rows[len(rows)-1].Cursor = makeCursor(product, operation, version, query, `{"k":"offset","v":30}`, opts...)
		}
		for _, mode := range []string{"same_account", "other_account", "anonymous_account", "other_client"} {
			add("replay:"+mode, batch(prefix+"offset=30"), final)
			rows[len(rows)-1].NextMode = mode
		}
		add("anonymous-replay", batch(prefix+"offset=30"), final)
		rows[len(rows)-1].UserID = 0
		add("anonymous-other-client", batch(prefix+"offset=30"), final)
		rows[len(rows)-1].UserID = 0
		rows[len(rows)-1].NextMode = "other_client"
		add("identity-before-cursor", final)
		rows[len(rows)-1].UserID = 0
		rows[len(rows)-1].Cursor = makeCursor("pixiv", op, 2, "different", `{}`)
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
			endpoint := map[string]string{"MyPixivArtworks": "/v2/illust/mypixiv", "MyPixivNovels": "/v1/novel/mypixiv", "MyPixivUsers": "/v1/user/mypixiv"}[row.Operation]
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
			switch row.Operation {
			case "MyPixivArtworks":
				page, err := client.MyPixivArtworks(context.Background(), pixiv.MyPixivArtworksRequest{Cursor: cur})
				callErr = err
				if err == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToArtworkDTO(item))
					}
					next = page.Next
				}
			case "MyPixivNovels":
				page, err := client.MyPixivNovels(context.Background(), pixiv.MyPixivNovelsRequest{Cursor: cur})
				callErr = err
				if err == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToNovelDTO(item))
					}
					next = page.Next
				}
			case "MyPixivUsers":
				page, err := client.MyPixivUsers(context.Background(), pixiv.MyPixivUsersRequest{Cursor: cur})
				callErr = err
				if err == nil {
					for _, item := range page.Items {
						result.Items = append(result.Items, pixiv.ToUserPreviewDTO(item))
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
	target := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "mypixiv.json")
	if *migrationUpdateMyPixiv {
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
		t.Fatal("MyPixiv contracts differ from frozen Go reference")
	}
}

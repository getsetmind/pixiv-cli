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
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateUserRelationships = flag.Bool("migration-update-user-relationships", false, "capture relationship queries, ordered bodies, cursors and DTOs")

type migrationRelationshipRow struct {
	Name      string                      `json:"name"`
	Operation string                      `json:"operation"`
	UserID    int64                       `json:"user_id"`
	Identity  int64                       `json:"identity"`
	Restrict  string                      `json:"restrict"`
	Cursor    string                      `json:"cursor"`
	Change    string                      `json:"change"`
	Bodies    []string                    `json:"bodies"`
	Results   []migrationUserSearchResult `json:"results"`
	Queries   []url.Values                `json:"queries"`
}

func TestMigrationUserRelationshipsFreezeQueriesDTOsOrderedBodiesAndScopedCursors(t *testing.T) {
	var rows []migrationRelationshipRow
	endpoints := map[string]string{"UserFollowing": "/v1/user/following", "UserFollowers": "/v1/user/follower", "RelatedUsers": "/v1/user/related", "UserBlockedUsers": "/v2/user/list"}
	for _, op := range []string{"UserFollowing", "UserFollowers", "RelatedUsers", "UserBlockedUsers"} {
		add := func(name, body string) {
			rows = append(rows, migrationRelationshipRow{Name: op + ":" + name, Operation: op, UserID: 31, Identity: 73, Bodies: []string{body}})
		}
		final := `{"user_previews":[{"user":{"id":41,"name":"artist","account":"account","comment":"comment","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"illusts":7,"novels":"ignored"},{"user":{"id":42}}],"next_url":null}`
		add("normal", final)
		add("invalid resource", strings.ReplaceAll(final, "https://i.pximg.net/profile.jpg", "https://evil.example/image.jpg"))
		for _, id := range []int64{0, -1, 9223372036854775807} {
			add(fmt.Sprintf("id:%d", id), final)
			rows[len(rows)-1].UserID = id
		}
		if op == "UserFollowing" || op == "UserFollowers" {
			for _, r := range []string{"public", "private", "PUBLIC", " private ", "all", " "} {
				add("restrict:"+r, final)
				rows[len(rows)-1].Restrict = r
			}
		}
		for _, body := range []string{`{}`, `null`, `{"user_previews":null}`, `{"user_previews":[]}`, `{"user_previews":{}}`, `{"user_previews":[null]}`, `{"user_previews":[{}]}`, `{"user_previews":[{"user":null}]}`, `{"user_previews":[{"user":{"id":0}}]}`, `{"user_previews":[{"user":{"id":41,"name":7}}]}`, `{"USER_PREVIEWS":[{"USER":{"ID":41,"NAME":"first","name":null,"ACCOUNT":"account"},"user":{"name":"last"}}]}`, `{"user_previews":[{"user":{"id":41,"profile_image_urls":{"medium":"https://i.pximg.net/x.jpg"},"PROFILE_IMAGE_URLS":null,"is_followed":true,"IS_FOLLOWED":null}}]}`, `{"user_previews":[{"user":{"id":41,"name":7,"name":"valid"}}]}`, `{"user_previews":[{"user":{"id":41}}],"user_previews":[]}`, `{"user_previews":null,"USER_PREVIEWS":[{"user":{"id":41}}]}`, `{"user_previews":[{"user":{"id":41,"profile_image_urls":{"medium":7}}}]}`, `{"user_previews":[{"user":{"id":41,"id":null,"name":null,"account":null,"comment":null,"is_followed":null,"profile_image_urls":null}}],"ignored":9e999}`} {
			add("raw:"+body, body)
		}
		for _, field := range []string{"id", "name", "account", "comment", "is_followed", "profile_image_urls"} {
			for _, value := range []string{"null", "7", "false", "[]", "{}", "\"value\""} {
				body := `{"user_previews":[{"user":{"id":41,"` + field + `":` + value + `}}]}`
				add("field:"+field+":"+value, body)
			}
		}
		if op == "UserBlockedUsers" {
			for _, body := range []string{`{"users":[{"id":41,"name":"flat"}]}`, `{"users":[{"user":{"id":41,"name":"nested"},"id":42,"name":7}]}`, `{"users":[{"user":null,"id":41}]}`, `{"users":[{"user":{"id":41},"USER":null,"id":42}]}`, `{"users":[{"user":{"id":41,"name":"first"},"USER":{"account":"merged"}}]}`, `{"users":[{"USER":{"ID":41}}]}`, `{"users":null,"user_previews":[{"user":{"id":41}}]}`, `{"users":[],"user_previews":[{"user":{"id":41}}]}`, `{"users":[{"id":41}],"user_previews":7}`, `{"users":[{"user":7,"id":41}]}`, `{"users":[{"id":41,"id":null}]}`, `{"users":[null]}`, `{"users":[{"user":{"id":41,"name":7,"name":"valid"}}]}`, `{"users":[{"user":{},"id":41}]}`, `{"users":[{"user":null,"id":41,"name":7}]}`, `{"users":[{"id":41,"name":"first","NAME":null,"profile_image_urls":{"medium":"https://i.pximg.net/x.jpg","medium":null}}]}`} {
				add("blocked:"+body, body)
			}
		}
		for _, next := range []any{nil, "", "https://app-api.pixiv.net" + endpoints[op] + "?offset=30", "https://app-api.pixiv.net" + endpoints[op] + "?offset=0", "https://app-api.pixiv.net" + endpoints[op] + "?offset=-1", "https://app-api.pixiv.net:" + endpoints[op] + "?offset=%2B30", "https://APP-API.PIXIV.NET" + endpoints[op] + "?offset=30", "http://app-api.pixiv.net" + endpoints[op] + "?offset=30", "https://app-api.pixiv.net:443" + endpoints[op] + "?offset=30", "https://user@app-api.pixiv.net" + endpoints[op] + "?offset=30", "https://app-api.pixiv.net" + endpoints[op] + "?offset=30&offset=40", "https://app-api.pixiv.net" + endpoints[op] + "?offset=30&unknown=1", "https://app-api.pixiv.net" + endpoints[op] + "?offset=30&" + map[string]string{"UserFollowing": "user_id=999&restrict=private", "UserFollowers": "user_id=999&restrict=private", "RelatedUsers": "seed_user_id=999", "UserBlockedUsers": "user_id=999&filter=changed"}[op], "https://app-api.pixiv.net" + endpoints[op] + "?offset=30#fragment", "https://app-api.pixiv.net" + endpoints[op] + "?offset=9223372036854775808", 7} {
			value, _ := json.Marshal(next)
			add(fmt.Sprintf("next:%v", next), strings.TrimSuffix(final, `"next_url":null}`)+`"next_url":`+string(value)+`}`)
			rows[len(rows)-1].Bodies = append(rows[len(rows)-1].Bodies, final)
		}
		query := url.Values{"user_id": {"31"}}
		if op == "RelatedUsers" {
			query = url.Values{"seed_user_id": {"31"}}
		}
		if op == "UserFollowing" || op == "UserFollowers" {
			query.Set("restrict", "public")
		}
		var encoded strings.Builder
		for _, key := range []string{"restrict", "seed_user_id", "user_id"} {
			if value := query.Get(key); value != "" {
				encoded.WriteString(key + "=" + value + "&")
			}
		}
		digest := fmt.Sprintf("%x", sha256.Sum256([]byte(encoded.String())))
		for _, payload := range []string{`{"k":"offset","v":30}`, `{"k":"offset","v":0}`, `{"k":"offset","v":-1}`, `{"k":"last_order","v":30}`, `{"k":"offset","v":30,"s":-1}`, `{"k":"offset","v":30,"s":1}`, `{"k":"offset","v":30,"p":{"offset":["30"]}}`, `{"p":{"offset":["30"]}}`, `null`, `{}`, `{"k":"offset","v":"30"}`} {
			for _, binding := range []string{"same", "foreign", "global", "instance"} {
				var opts []sdk.CursorOption
				switch binding {
				case "same":
					opts = append(opts, sdk.WithCursorIdentity("73"))
				case "foreign":
					opts = append(opts, sdk.WithCursorIdentity("74"))
				case "instance":
					opts = append(opts, sdk.WithCursorEphemeralInstance("foreign"))
				}
				cur, err := sdk.NewCursor("pixiv", op, 1, digest, []byte(payload), opts...)
				if err != nil {
					t.Fatal(err)
				}
				add("cursor:"+binding+":"+payload, final)
				rows[len(rows)-1].Cursor = cur.String()
			}
		}
		for _, change := range []string{"user", "identity", "instance", "restrict", "ephemeral", "foreign-ephemeral"} {
			add("binding:"+change, strings.TrimSuffix(final, `"next_url":null}`)+`"next_url":"https://app-api.pixiv.net`+endpoints[op]+`?offset=30"}`)
			rows[len(rows)-1].Bodies = append(rows[len(rows)-1].Bodies, final)
			rows[len(rows)-1].Change = change
			if change == "ephemeral" || change == "foreign-ephemeral" {
				rows[len(rows)-1].Identity = 0
			}
		}
	}
	for index := range rows {
		row := &rows[index]
		row.Results = []migrationUserSearchResult{}
		row.Queries = []url.Values{}
		identity := row.Identity
		transport := migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			body := ""
			if req.URL.Host == "oauth.secure.pixiv.net" {
				body = fmt.Sprintf(`{"access_token":"fixture-access","refresh_token":"fixture-rotated","expires_in":3600,"user":{"id":%d}}`, identity)
			} else {
				if req.Method != "GET" || req.URL.Path != endpoints[row.Operation] {
					t.Fatal("unexpected relationship request")
				}
				step := len(row.Queries)
				row.Queries = append(row.Queries, req.URL.Query())
				if step >= len(row.Bodies) {
					step = len(row.Bodies) - 1
				}
				body = row.Bodies[step]
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(body)), Request: req}, nil
		})
		makeClient := func() *pixiv.Client {
			var c *pixiv.Client
			var err error
			if identity > 0 {
				c, _, err = pixiv.OpenWith(context.Background(), "fixture-refresh", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
			} else {
				c, err = pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
			}
			if err != nil {
				t.Fatal(err)
			}
			return c
		}
		client := makeClient()
		cur := sdk.Cursor{}
		if row.Cursor != "" {
			var err error
			cur, err = sdk.ParseCursor(row.Cursor)
			if err != nil {
				t.Fatal(err)
			}
		}
		id := row.UserID
		restrict := row.Restrict
		for step := 0; step < 2; step++ {
			var page sdk.Page[pixiv.UserPreview]
			var err error
			switch row.Operation {
			case "UserFollowing":
				page, err = client.UserFollowing(context.Background(), pixiv.UserFollowingRequest{UserID: id, Restrict: pixiv.Restrict(restrict), Cursor: cur})
			case "UserFollowers":
				page, err = client.UserFollowers(context.Background(), pixiv.UserFollowersRequest{UserID: id, Restrict: pixiv.Restrict(restrict), Cursor: cur})
			case "RelatedUsers":
				page, err = client.RelatedUsers(context.Background(), pixiv.RelatedUsersRequest{UserID: id, Cursor: cur})
			case "UserBlockedUsers":
				page, err = client.UserBlockedUsers(context.Background(), pixiv.UserBlockedUsersRequest{UserID: id, Cursor: cur})
			}
			result := migrationUserSearchResult{Items: []pixiv.UserPreviewDTO{}}
			if err != nil {
				result.Reason = sdk.ReasonOf(err)
				result.Message = err.Error()
			} else {
				for _, item := range page.Items {
					result.Items = append(result.Items, pixiv.ToUserPreviewDTO(item))
				}
				result.Cursor = migrationSearchCursor(t, page.Next)
			}
			row.Results = append(row.Results, result)
			if err != nil || page.Next.IsZero() {
				break
			}
			cur = page.Next
			switch row.Change {
			case "user":
				id++
			case "identity":
				identity++
				client = makeClient()
			case "instance", "foreign-ephemeral":
				identity = 0
				client = makeClient()
			case "restrict":
				restrict = "private"
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	target := filepath.Join("..", "..", "docs", "migration", "contracts", "user-relationships.json")
	if *migrationUpdateUserRelationships {
		if err = os.WriteFile(target, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(target)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("user relationships differ from fixed Go reference")
	}
}

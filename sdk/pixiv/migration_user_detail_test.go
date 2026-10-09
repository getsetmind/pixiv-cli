package pixiv_test

import (
	"bytes"
	"context"
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

var migrationUpdateUserDetail = flag.Bool("migration-update-user-detail", false, "capture user detail fields and requests")

func TestMigrationUserDetailPreservesFieldsVisibilityAndRequests(t *testing.T) {
	type request struct {
		Method string     `json:"method"`
		Path   string     `json:"path"`
		Query  url.Values `json:"query"`
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
	base := []byte(`{"user":{"id":42,"name":"writer\n<&>","account":"account","comment":"comment","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}},"profile_publicity":{"gender":"public","region":"private","birth_day":true,"birth_year":false,"job":"public","pawoo":"private"},"profile":{"webpage":"webpage <&>","gender":"gender <&>","birth":"birth <&>","birth_day":"birth_day <&>","birth_year":17,"region":"region <&>","address_id":17,"country_code":"country_code <&>","job":"job <&>","job_id":17,"total_follow_users":17,"total_mypixiv_users":17,"total_illusts":17,"total_manga":17,"total_novels":17,"total_illust_bookmarks_public":17,"total_illust_series":17,"total_novel_series":17,"background_image_url":"background_image_url <&>","twitter_account":"twitter_account <&>","twitter_url":"twitter_url <&>","pawoo_url":"pawoo_url <&>","is_premium":true,"is_using_custom_profile_image":true},"workspace":{"pc":"pc <&>","monitor":"monitor <&>","tool":"tool <&>","scanner":"scanner <&>","tablet":"tablet <&>","mouse":"mouse <&>","printer":"printer <&>","desktop":"desktop <&>","music":"music <&>","desk":"desk <&>","chair":"chair <&>","comment":"comment <&>","workspace_image_url":"workspace_image_url <&>"}}`)
	var rows []row
	add := func(name string, id int64, mutate func(map[string]any)) {
		var body map[string]any
		if err := json.Unmarshal(base, &body); err != nil {
			t.Fatal(err)
		}
		mutate(body)
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row{Name: name, ID: id, Body: data, Requests: []request{}})
	}
	add("rich", 42, func(map[string]any) {})
	for _, id := range []int64{0, -1, 9223372036854775807} {
		add(fmt.Sprintf("id:%d", id), id, func(map[string]any) {})
	}
	for _, section := range []string{"user", "profile", "profile_publicity", "workspace"} {
		add("missing:"+section, 42, func(body map[string]any) { delete(body, section) })
		for _, value := range []any{nil, 7, []any{}, map[string]any{}} {
			add(fmt.Sprintf("section:%s:%v", section, value), 42, func(body map[string]any) { body[section] = value })
		}
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:webpage:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["webpage"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:gender:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["gender"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:birth:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["birth"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:birth_day:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["birth_day"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:birth_year:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["birth_year"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:region:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["region"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:address_id:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["address_id"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:country_code:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["country_code"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:job:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["job"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:job_id:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["job_id"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_follow_users:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_follow_users"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_mypixiv_users:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_mypixiv_users"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_illusts:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_illusts"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_manga:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_manga"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_novels:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_novels"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_illust_bookmarks_public:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_illust_bookmarks_public"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_illust_series:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_illust_series"] = value })
	}
	for _, value := range []any{nil, "invalid"} {
		add(fmt.Sprintf("field:profile:total_novel_series:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["total_novel_series"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:background_image_url:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["background_image_url"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:twitter_account:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["twitter_account"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:twitter_url:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["twitter_url"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:pawoo_url:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["pawoo_url"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:is_premium:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["is_premium"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:profile:is_using_custom_profile_image:%v", value), 42, func(body map[string]any) { body["profile"].(map[string]any)["is_using_custom_profile_image"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:pc:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["pc"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:monitor:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["monitor"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:tool:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["tool"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:scanner:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["scanner"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:tablet:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["tablet"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:mouse:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["mouse"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:printer:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["printer"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:desktop:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["desktop"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:music:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["music"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:desk:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["desk"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:chair:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["chair"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:comment:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["comment"] = value })
	}
	for _, value := range []any{nil, 7} {
		add(fmt.Sprintf("field:workspace:workspace_image_url:%v", value), 42, func(body map[string]any) { body["workspace"].(map[string]any)["workspace_image_url"] = value })
	}

	for _, key := range []string{"gender", "region", "birth_day", "birth_year", "job", "pawoo"} {
		for _, value := range []any{nil, true, false, "public", "private", "unknown", 1} {
			add(fmt.Sprintf("publicity:%s:%v", key, value), 42, func(body map[string]any) { body["profile_publicity"].(map[string]any)[key] = value })
		}
	}
	for _, value := range []any{0, -1, nil, "42"} {
		add(fmt.Sprintf("user:id:%v", value), 42, func(body map[string]any) { body["user"].(map[string]any)["id"] = value })
	}
	add("profile:forbidden-host", 42, func(body map[string]any) {
		body["user"].(map[string]any)["profile_image_urls"] = map[string]any{"medium": "https://evil.example/image.jpg"}
	})
	for index := range rows {
		row := &rows[index]
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			row.Requests = append(row.Requests, request{req.Method, req.URL.Path, req.URL.Query()})
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(row.Body)), Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.User(context.Background(), pixiv.UserRequest{UserID: row.ID})
		if err != nil {
			row.Reason = sdk.ReasonOf(err)
			row.Message = err.Error()
		} else {
			row.DTO, err = json.Marshal(pixiv.ToUserDetailDTO(result))
			if err != nil {
				t.Fatal(err)
			}
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "user-detail.json")
	if *migrationUpdateUserDetail {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("user detail differs from frozen Go fields and requests")
	}
}

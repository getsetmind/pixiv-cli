package pixiv_test

import (
	"bytes"
	"context"
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

var migrationUpdateUserWire = flag.Bool("migration-update-user-wire", false, "capture ordered raw user wire bodies and SDK results")

type migrationUserWireRequest struct {
	Method string     `json:"method"`
	Path   string     `json:"path"`
	Query  url.Values `json:"query"`
}

type migrationUserWireRow struct {
	Name      string                     `json:"name"`
	Operation string                     `json:"operation"`
	Body      string                     `json:"body"`
	DTO       json.RawMessage            `json:"dto"`
	Cursor    any                        `json:"cursor,omitempty"`
	Reason    sdk.Reason                 `json:"reason,omitempty"`
	Message   string                     `json:"message,omitempty"`
	Requests  []migrationUserWireRequest `json:"requests"`
}

func migrationUserWireResult(t *testing.T, row migrationUserWireRow) migrationUserWireRow {
	t.Helper()
	paths := map[string]string{"SearchUsers": "/v1/search/user", "User": "/v1/user/detail"}
	queries := map[string]url.Values{"SearchUsers": {"word": {"artist"}}, "User": {"user_id": {"31"}}}
	row.Requests = []migrationUserWireRequest{}
	client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(request *http.Request) (*http.Response, error) {
		if request.Method != http.MethodGet || request.URL.Path != paths[row.Operation] || request.URL.Query().Encode() != queries[row.Operation].Encode() || request.Header.Get("Authorization") != "Bearer fixture-access" {
			t.Fatalf("unexpected %s request: %s %s", row.Operation, request.Method, request.URL)
		}
		row.Requests = append(row.Requests, migrationUserWireRequest{request.Method, request.URL.Path, request.URL.Query()})
		return &http.Response{StatusCode: http.StatusOK, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(row.Body)), Request: request}, nil
	})}})
	if err != nil {
		t.Fatal(err)
	}
	var dto any
	switch row.Operation {
	case "SearchUsers":
		var page sdk.Page[pixiv.UserPreview]
		page, err = client.SearchUsers(context.Background(), pixiv.SearchUsersRequest{Word: "artist"})
		if err != nil && (len(page.Items) != 0 || !page.Next.IsZero()) {
			t.Fatalf("failed user search exposed partial page: %#v", page)
		}
		items := make([]pixiv.UserPreviewDTO, 0, len(page.Items))
		for _, item := range page.Items {
			items = append(items, pixiv.ToUserPreviewDTO(item))
		}
		dto = items
		if err == nil {
			row.Cursor = migrationSearchCursor(t, page.Next)
		}
	case "User":
		var detail pixiv.UserDetail
		detail, err = client.User(context.Background(), pixiv.UserRequest{UserID: 31})
		if err != nil && detail.User.ID != 0 {
			t.Fatalf("failed user detail exposed partial user: %#v", detail)
		}
		dto = pixiv.ToUserDetailDTO(detail)
	default:
		t.Fatalf("unknown user wire operation: %s", row.Operation)
	}
	if len(row.Requests) != 1 {
		t.Fatalf("%s request count = %d, want 1", row.Operation, len(row.Requests))
	}
	if err != nil {
		row.Reason, row.Message = sdk.ReasonOf(err), err.Error()
		if row.Reason != sdk.MalformedUpstreamResponse || row.Message != "pixiv:"+row.Operation+": malformed_upstream_response" {
			t.Fatalf("unexpected wire failure: %s", err)
		}
		row.DTO = json.RawMessage("null")
	} else {
		row.DTO, err = json.Marshal(dto)
		if err != nil {
			t.Fatal(err)
		}
	}
	return row
}

func TestMigrationUserSearchPreservesCaseInsensitiveFieldsAndOrderedDuplicateMerges(t *testing.T) {
	cases := []struct {
		name        string
		body        string
		wantName    string
		wantAccount string
	}{
		{"uppercase envelope and user", `{"USER_PREVIEWS":[{"USER":{"ID":31,"NAME":"artist","ACCOUNT":"account"}}]}`, "artist", "account"},
		{"mixed case fields", `{"User_Previews":[{"User":{"Id":31,"Name":"artist","Account":"account"}}]}`, "artist", "account"},
		{"later uppercase name", `{"user_previews":[{"user":{"id":31,"name":"first","NAME":"last"}}]}`, "last", ""},
		{"later lowercase name", `{"user_previews":[{"user":{"id":31,"NAME":"first","name":"last"}}]}`, "last", ""},
		{"duplicate user object merges", `{"user_previews":[{"user":{"id":31,"name":"first","account":"account"},"USER":{"name":"last"}}]}`, "last", "account"},
	}
	for _, current := range cases {
		t.Run(current.name, func(t *testing.T) {
			row := migrationUserWireResult(t, migrationUserWireRow{Name: current.name, Operation: "SearchUsers", Body: current.body})
			var items []pixiv.UserPreviewDTO
			if err := json.Unmarshal(row.DTO, &items); err != nil {
				t.Fatal(err)
			}
			if len(items) != 1 || items[0].User.ID != 31 || items[0].User.Name != current.wantName || items[0].User.Account != current.wantAccount || row.Cursor != nil {
				t.Fatalf("case-insensitive or duplicate-merge contract differs: %#v", row)
			}
		})
	}
}

func TestMigrationUserWirePreservesRawFieldMatchingOrderAndValidation(t *testing.T) {
	const user = `{"id":31,"name":"artist","account":"account","comment":"comment","is_followed":true,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"}}`
	const detailTail = `,"profile":{},"profile_publicity":{},"workspace":{}`
	var rows []migrationUserWireRow
	add := func(operation, name, body string) {
		rows = append(rows, migrationUserWireRow{Name: name, Operation: operation, Body: body})
	}
	addUser := func(name, fields string) {
		add("SearchUsers", name, `{"user_previews":[{`+fields+`}]}`)
		add("User", name, `{`+fields+detailTail+`}`)

	}
	for _, current := range []struct {
		name, fields string
	}{
		{"baseline", `"user":` + user},
		{"uppercase_known_user_fields", `"USER":{"ID":31,"NAME":"artist","ACCOUNT":"account","COMMENT":"comment","IS_FOLLOWED":true,"PROFILE_IMAGE_URLS":{"MEDIUM":"https://i.pximg.net/profile.jpg"}}`},
		{"mixed_case_known_user_fields", `"User":{"Id":31,"NaMe":"artist","Account":"account","Comment":"comment","Is_Followed":true,"Profile_Image_Urls":{"Medium":"https://i.pximg.net/profile.jpg"}}`},
		{"escaped_known_field_names", `"\u0075ser":{"\u0049D":31,"\u006eAME":"artist","profile_image_url\u0073":{"\u006dedium":"https://i.pximg.net/profile.jpg"}}`},
		{"unicode_long_s_matches_known_fields", `"uſer":{"id":31,"iſ_followed":true,"profile_image_urlſ":{"medium":"https://i.pximg.net/profile.jpg"}}`},
		{"unknown_near_match_keys_ignored", `"user":{"id":31,"Namex":7,"ıd":{},"İD":[],"isFollowed":"bad","profileImageUrls":false,"profile_image_urls":{"médiüm":7,"mediumx":false}},"uſerx":false`},
		{"same_key_scalar_last_wins", `"user":{"id":31,"name":"first","name":"last"}`},
		{"exact_then_folded_scalar_last_wins", `"user":{"id":31,"name":"first","NAME":"last"}`},
		{"folded_then_exact_scalar_last_wins", `"user":{"id":31,"NAME":"first","name":"last"}`},
		{"three_case_variants_follow_input_order", `"user":{"id":31,"NAME":"first","name":"middle","NaMe":"last"}`},
		{"later_scalar_null_preserves_value", `"user":{"id":31,"ID":null,"name":"artist","NAME":null,"account":"account","ACCOUNT":null,"comment":"comment","COMMENT":null,"is_followed":true,"IS_FOLLOWED":null}`},
		{"later_empty_scalar_replaces_value", `"user":{"id":31,"name":"artist","NAME":"","is_followed":true,"IS_FOLLOWED":false}`},
		{"ordinary_nested_image_objects_merge", `"user":{"id":31,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"},"PROFILE_IMAGE_URLS":{}}`},
		{"later_image_object_null_preserves_struct", `"user":{"id":31,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"},"PROFILE_IMAGE_URLS":null}`},
		{"later_image_pointer_null_clears_value", `"user":{"id":31,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg","MEDIUM":null}}`},
		{"later_image_pointer_empty_clears_resource", `"user":{"id":31,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg","MEDIUM":""}}`},
		{"later_image_object_merges_pointer_null", `"user":{"id":31,"profile_image_urls":{"medium":"https://i.pximg.net/profile.jpg"},"PROFILE_IMAGE_URLS":{"MEDIUM":null}}`},
		{"duplicate_user_objects_merge_or_replace", `"user":{"id":31,"name":"first","account":"account"},"USER":{"name":"last"}`},
		{"duplicate_user_objects_with_valid_later_id", `"user":` + user + `,"USER":{"id":32}`},
		{"duplicate_id_zero_then_positive_recovers", `"user":{"id":0,"ID":31}`},
		{"duplicate_user_empty_object", `"user":` + user + `,"USER":{}`},
		{"duplicate_user_null", `"user":` + user + `,"USER":null`},
		{"null_user_then_valid_object", `"user":null,"USER":` + user},
		{"zero_id_then_valid_user_object", `"user":{"id":0},"USER":` + user},
		{"valid_user_then_zero_id_object", `"user":` + user + `,"USER":{"id":0}`},
		{"invalid_user_object_type_then_valid", `"user":false,"USER":` + user},
		{"invalid_scalar_then_valid_scalar", `"user":{"id":31,"name":7,"NAME":"artist"}`},
		{"invalid_scalar_after_valid_scalar", `"user":{"id":31,"name":"artist","NAME":7}`},
		{"invalid_nested_field_then_valid_user_object", `"user":{"id":31,"is_followed":"bad"},"USER":` + user},
		{"invalid_id_then_valid_id", `"user":{"id":"bad","ID":31}`},
		{"invalid_id_then_valid_user_object", `"user":{"id":"bad"},"USER":` + user},
		{"null_user_scalar_fields_default", `"user":{"id":31,"name":null,"account":null,"comment":null,"is_followed":null,"profile_image_urls":null}`},
		{"unknown_complex_fields_ignored", `"user":{"id":31,"unknown":{"name":false,"id":9e999},"UNKNOWN":[false,null,{}]},"unknown":{"user":false}`},
	} {
		addUser(current.name, current.fields)
	}
	for _, value := range []string{"0", "-1", "9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809", "31.0", "3.1e1", "31e0", `"31"`, "null", "true", "[]", "{}"} {
		addUser("id:"+value, `"user":{"id":`+value+`}`)
	}
	for _, current := range []struct{ key, invalid string }{
		{"name", "7"}, {"account", "false"}, {"comment", "[]"}, {"is_followed", `"true"`}, {"profile_image_urls", "[]"}, {"profile_image_urls", `{"medium":7}`},
	} {
		addUser("known_invalid_type:"+current.key+":"+current.invalid, `"user":{"id":31,"`+current.key+`":`+current.invalid+`}`)
	}
	for _, current := range []struct{ name, body string }{
		{"uppercase_envelope", `{"USER_PREVIEWS":[{"USER":{"ID":31}}],"NEXT_URL":null}`},
		{"long_s_envelope", `{"uſer_previewſ":[{"uſer":{"id":31}}]}`},
		{"unknown_near_match_envelope", `{"user_previews":[{"user":{"id":31}}],"userPreviews":7,"USER_PREVIEW":false,"nextUrl":false}`},
		{"duplicate_list_replaces_items", `{"user_previews":[{"user":{"id":31,"name":"first"}}],"USER_PREVIEWS":[{"user":{"id":32,"name":"last"}}]}`},
		{"duplicate_list_empty_replaces_items", `{"user_previews":[{"user":{"id":31}}],"USER_PREVIEWS":[]}`},
		{"duplicate_list_null_invalidates", `{"user_previews":[{"user":{"id":31}}],"USER_PREVIEWS":null}`},
		{"null_list_then_valid_recovers", `{"user_previews":null,"USER_PREVIEWS":[{"user":{"id":31}}]}`},
		{"invalid_list_then_valid_does_not_recover", `{"user_previews":{},"USER_PREVIEWS":[{"user":{"id":31}}]}`},
		{"invalid_item_type_then_valid_list_does_not_recover", `{"user_previews":[{"user":{"id":31,"name":7}}],"USER_PREVIEWS":[{"user":{"id":31}}]}`},
		{"zero_item_then_valid_list_recovers", `{"user_previews":[{"user":{"id":0}}],"USER_PREVIEWS":[{"user":{"id":31}}]}`},
		{"multiple_items_preserve_array_order", `{"user_previews":[{"user":{"id":32}},{"user":{"id":31}}]}`},
		{"ignored_samples_and_unknown_fields", `{"user_previews":[{"user":{"id":31},"ILLUSTS":false,"novels":{"id":"bad"},"unknown":9e999}]}`},
		{"later_next_null_clears_pointer", `{"user_previews":[],"next_url":"https://app-api.pixiv.net/v1/search/user?offset=30","NEXT_URL":null}`},
		{"later_next_valid_replaces_null", `{"user_previews":[],"NEXT_URL":null,"next_url":"https://app-api.pixiv.net/v1/search/user?offset=30"}`},
		{"invalid_next_string_then_null_recovers", `{"user_previews":[],"next_url":"bad-url","NEXT_URL":null}`},
		{"invalid_next_string_then_valid_recovers", `{"user_previews":[],"next_url":"bad-url","NEXT_URL":"https://app-api.pixiv.net/v1/search/user?offset=30"}`},
		{"later_next_empty_is_invalid", `{"user_previews":[],"next_url":"https://app-api.pixiv.net/v1/search/user?offset=30","NEXT_URL":""}`},
		{"invalid_next_then_null_does_not_recover", `{"user_previews":[],"next_url":7,"NEXT_URL":null}`},
	} {
		add("SearchUsers", current.name, current.body)
	}
	const richSections = `"profile":{"webpage":"web","gender":"gender","birth":"birth","birth_day":"day","birth_year":17,"region":"region","address_id":17,"country_code":"country","job":"job","job_id":17,"total_follow_users":17,"total_mypixiv_users":17,"total_illusts":17,"total_manga":17,"total_novels":17,"total_illust_bookmarks_public":17,"total_illust_series":17,"total_novel_series":17,"background_image_url":"background","twitter_account":"twitter","twitter_url":"twitter_url","pawoo_url":"pawoo","is_premium":true,"is_using_custom_profile_image":true},"profile_publicity":{"gender":"public","region":"private","birth_day":true,"birth_year":false,"job":"public","pawoo":"private"},"workspace":{"pc":"pc","monitor":"monitor","tool":"tool","scanner":"scanner","tablet":"tablet","mouse":"mouse","printer":"printer","desktop":"desktop","music":"music","desk":"desk","chair":"chair","comment":"comment","workspace_image_url":"image"}`
	add("User", "all_detail_fields_uppercase", `{"USER":`+user+`,`+migrationUserWireUppercaseKeys(richSections)+`}`)
	add("User", "unicode_kelvin_and_long_s_detail_fields", `{"uſer":{"id":31},"profile":{"total_illuſtſ":17,"iſ_premium":true},"profile_publicity":{"gender":"public"},"worKſpace":{"deſK":"desk","deſKtop":"desktop","ſcanner":"scanner","worKſpace_image_url":"image"}}`)
	add("User", "unknown_detail_section_keys_ignored", `{"user":{"id":31},"profile":{"BirthYear":"bad","totalMyPixivUsers":"bad","unknown":9e999},"profile_publicity":{"Genderx":7},"workspace":{"Deskx":false},"ProfilePublicity":false}`)
	for _, section := range []string{"profile", "profile_publicity", "workspace"} {
		first, second := `{"gender":"gender","webpage":"web"}`, `{"region":"region"}`
		if section == "profile_publicity" {
			first, second = `{"gender":"public"}`, `{"region":"public"}`
		} else if section == "workspace" {
			first, second = `{"pc":"pc","desk":"desk"}`, `{"monitor":"monitor"}`
		}
		for _, current := range []struct{ name, duplicate string }{
			{"objects_replace", first + `,"` + strings.ToUpper(section) + `":` + second},
			{"empty_object_resets", first + `,"` + strings.ToUpper(section) + `":{}`},
			{"null_invalidates", first + `,"` + strings.ToUpper(section) + `":null`},
			{"null_then_object_recovers", `null,"` + strings.ToUpper(section) + `":` + second},
			{"invalid_type_then_object_does_not_recover", `false,"` + strings.ToUpper(section) + `":` + second},
		} {
			body := `{"user":{"id":31}` + detailTail + `,"` + section + `":` + current.duplicate + `}`
			add("User", "duplicate_"+section+":"+current.name, body)
		}
	}
	for _, current := range []struct{ name, fields string }{
		{"invalid_then_valid_visibility_recovers", `"profile_publicity":{"gender":"bad","GENDER":"public"}`},
		{"overflow_number_then_valid_visibility_recovers", `"profile_publicity":{"gender":9e999,"GENDER":"public"}`},
		{"valid_then_invalid_visibility_fails", `"profile_publicity":{"gender":"public","GENDER":null}`},
		{"invalid_visibility_then_new_empty_object_recovers", `"profile_publicity":{"gender":7},"PROFILE_PUBLICITY":{}`},
		{"pointer_scalar_null_clears_and_value_null_retains", `"profile":{"webpage":"web","WEBPAGE":null,"gender":"gender","GENDER":null,"birth_year":17,"BIRTH_YEAR":null,"is_premium":true,"IS_PREMIUM":null},"workspace":{"pc":"pc","PC":null,"workspace_image_url":"image","WORKSPACE_IMAGE_URL":null}`},
		{"invalid_profile_field_then_new_valid_object_fails", `"profile":{"birth_year":"bad"},"PROFILE":{}`},
		{"invalid_workspace_field_then_new_valid_object_fails", `"workspace":{"pc":7},"WORKSPACE":{}`},
	} {
		add("User", current.name, `{"user":{"id":31}`+detailTail+`,`+current.fields+`}`)
	}
	for _, value := range []string{"9223372036854775807", "9223372036854775808", "17.0", "17e0", "-17", "null"} {
		add("User", "profile_int:"+value, `{"user":{"id":31}`+detailTail+`,"PROFILE":{"birth_year":`+value+`,"total_illusts":`+value+`}}`)
	}
	for _, fields := range []string{`"address_id":"bad"`, `"job_id":9223372036854775808`, `"birth":7`} {
		add("User", "non_dto_profile_field_still_validated:"+fields, `{"user":{"id":31}`+detailTail+`,"PROFILE":{`+fields+`}}`)
	}
	for _, depth := range []int{150, 9999, 10000} {
		add("SearchUsers", fmt.Sprintf("unknown_nested_array_depth:%d", depth), `{"user_previews":[],"unknown":`+strings.Repeat("[", depth)+`0`+strings.Repeat("]", depth)+`}`)
	}
	for _, operation := range []string{"SearchUsers", "User"} {
		for _, body := range []string{"", "null", "[]", "false", "{", `{"user_previews":[],"unknown":01}`, `{"user_previews":[],"unknown":NaN}`, `{"user_previews":[]} {}`} {
			add(operation, "whole_body:"+body, body)
		}
	}
	for index := range rows {
		t.Run(rows[index].Operation+"/"+rows[index].Name, func(t *testing.T) {
			rows[index] = migrationUserWireResult(t, rows[index])
		})
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "user-wire.json")
	if *migrationUpdateUserWire {
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
		t.Fatal("raw user wire differs from frozen Go field matching, order, validation, or SDK results")
	}
}

func migrationUserWireUppercaseKeys(body string) string {
	var result strings.Builder
	for index := 0; index < len(body); {
		if body[index] == '"' {
			end := index + 1
			for end < len(body) && body[end] != '"' {
				end++
			}
			if end+1 < len(body) && body[end+1] == ':' {
				fmt.Fprintf(&result, "\"%s\"", strings.ToUpper(body[index+1:end]))
				index = end + 1
				continue
			}
		}
		result.WriteByte(body[index])
		index++
	}
	return result.String()
}

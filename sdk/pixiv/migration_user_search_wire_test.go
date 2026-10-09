package pixiv_test

import (
	"context"
	"io"
	"net/http"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

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
			client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(request *http.Request) (*http.Response, error) {
				if request.Method != http.MethodGet || request.URL.Path != "/v1/search/user" || request.URL.Query().Get("word") != "artist" {
					t.Fatalf("unexpected request: %s %s", request.Method, request.URL.Path)
				}
				return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(current.body)), Request: request}, nil
			})}})
			if err != nil {
				t.Fatal(err)
			}
			page, err := client.SearchUsers(context.Background(), pixiv.SearchUsersRequest{Word: "artist"})
			if err != nil {
				t.Fatal(err)
			}
			if len(page.Items) != 1 || page.Items[0].User.ID != 31 || page.Items[0].User.Name != current.wantName || page.Items[0].User.Account != current.wantAccount || !page.Next.IsZero() {
				t.Fatalf("case-insensitive or duplicate-merge contract differs: %#v", page)
			}
		})
	}
}

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
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateUgoira = flag.Bool("migration-update-ugoira", false, "capture public ugoira metadata contracts from the fixed Go reference")

func TestMigrationUgoiraMetadataMatchesFrozenDTOAndValidation(t *testing.T) {
	type row struct {
		Name             string          `json:"name"`
		ID               int64           `json:"id"`
		Body             json.RawMessage `json:"body"`
		DTO              json.RawMessage `json:"dto"`
		Error            json.RawMessage `json:"error"`
		Requests         []string        `json:"requests"`
		Followup         bool            `json:"followup"`
		ResourceRequests []string        `json:"resource_requests"`
	}
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	input, err := os.ReadFile(filepath.Join(path, "artwork-open-resource.json"))
	if err != nil {
		t.Fatal(err)
	}
	var opens []struct {
		Name string          `json:"name"`
		Body json.RawMessage `json:"body"`
	}
	if err := json.Unmarshal(input, &opens); err != nil {
		t.Fatal(err)
	}
	rows := []row{}
	for _, open := range opens {
		if strings.HasPrefix(open.Name, "ugoira-") {
			rows = append(rows, row{Name: open.Name, ID: 42, Body: open.Body})
		}
	}
	normal := json.RawMessage(`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/original.zip?fixture=secret","medium":"https://i.pximg.net/medium.zip"},"frames":[{"file":"0.jpg","delay":-100},{"file":"dir/1.jpg","delay":0}]}}`)
	rows = append(rows, row{Name: "signed-delays-and-order", ID: 42, Body: normal}, row{Name: "zero-id", Body: normal}, row{Name: "negative-id", ID: -1, Body: normal})
	for _, body := range []string{
		`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/original.zip","medium":"http://i.pximg.net/medium.zip"},"frames":[{"file":"0.jpg"}]}}`,
		`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/original.zip"},"frames":[{"file":"..hidden.jpg"}]}}`,
		`{"ugoira_metadata":{"zip_urls":{"original":"http://i.pximg.net/original.zip"},"frames":[{"file":"dir\\0.jpg"}]}}`,
	} {
		rows = append(rows, row{Name: "mapping-priority", ID: 42, Body: json.RawMessage(body)})
	}
	rows = append(rows, row{Name: "unsafe-frame-cached-archive", ID: 42, Followup: true, Body: json.RawMessage(`{"ugoira_metadata":{"zip_urls":{"original":"https://i.pximg.net/original.zip"},"frames":[{"file":"..hidden.jpg"}]}}`)})
	for index := range rows {
		input := &rows[index]
		input.Requests = []string{}
		input.ResourceRequests = []string{}
		transport := migrationArtworkTransport(func(r *http.Request) (*http.Response, error) {
			if r.Method != "GET" {
				t.Fatal("unexpected ugoira request")
			}
			if r.URL.Host == "i.pximg.net" {
				input.ResourceRequests = append(input.ResourceRequests, r.URL.String())
				return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(bytes.NewReader(nil)), Request: r}, nil
			}
			if r.URL.Host != "app-api.pixiv.net" {
				t.Fatal("unexpected ugoira host")
			}
			input.Requests = append(input.Requests, r.URL.RequestURI())
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(input.Body)), Request: r}, nil
		})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.UgoiraMetadata(context.Background(), pixiv.UgoiraMetadataRequest{ArtworkID: input.ID})
		if err != nil {
			input.Error, err = json.Marshal(err)
		} else {
			input.DTO, err = json.Marshal(pixiv.ToUgoiraMetadataDTO(result))
		}
		if err != nil {
			t.Fatal(err)
		}
		if input.Followup {
			ref, err := sdk.NewResourceRef("pixiv", []byte(`{"k":"ugoira_archive","id":42,"p":-1,"v":"original"}`))
			if err != nil {
				t.Fatal(err)
			}
			response, err := client.OpenResource(context.Background(), sdk.OpenResourceRequest{Ref: ref})
			if err != nil {
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
	file := filepath.Join(path, "ugoira-metadata.json")
	if *migrationUpdateUgoira {
		if err := os.WriteFile(file, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public ugoira metadata differs from the fixed Go reference")
	}
}

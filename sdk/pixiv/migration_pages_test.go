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

var migrationUpdatePages = flag.Bool("migration-update-pages", false, "capture page and resource variant contracts from the fixed Go reference")

func TestMigrationArtworkPagesAndVariantsMatchFrozenContracts(t *testing.T) {
	type pageCase struct {
		Name         string          `json:"name"`
		ID           int64           `json:"id"`
		Body         json.RawMessage `json:"body"`
		AllowedHosts []string        `json:"allowed_hosts"`
		DTO          json.RawMessage `json:"dto"`
		Reason       sdk.Reason      `json:"reason"`
		Message      string          `json:"message"`
		Requests     int             `json:"requests"`
	}
	type variantCase struct {
		Kind    string     `json:"kind"`
		ID      int64      `json:"id"`
		Page    int        `json:"page"`
		Input   string     `json:"input"`
		Variant string     `json:"variant"`
		Ref     string     `json:"ref"`
		Reason  sdk.Reason `json:"reason"`
	}
	var inputs []pageCase
	path := filepath.Join("..", "..", "docs", "migration", "contracts")
	data, err := os.ReadFile(filepath.Join(path, "artwork-detail.json"))
	if err != nil {
		t.Fatal(err)
	}
	var detailInputs []struct {
		Name string          `json:"name"`
		ID   int64           `json:"id"`
		Body json.RawMessage `json:"body"`
	}
	if err := json.Unmarshal(data, &detailInputs); err != nil {
		t.Fatal(err)
	}
	for _, input := range detailInputs {
		inputs = append(inputs, pageCase{Name: input.Name, ID: input.ID, Body: input.Body})
	}
	for _, extra := range []struct {
		name, url string
		hosts     []string
	}{
		{"allowed-host", "https://media.fixture.invalid/page.jpg", []string{" MEDIA.FIXTURE.INVALID "}},
		{"unallowed-host", "https://media.fixture.invalid/page.jpg", nil},
		{"policy-suffix", "https://media.fixture.invalid.evil.invalid/page.jpg", []string{"media.fixture.invalid"}},
		{"http", "http://i.pximg.net/page.jpg", nil},
		{"empty-userinfo", "https://@i.pximg.net/page.jpg", nil},
		{"userinfo", "https://fixture-secret@i.pximg.net/page.jpg", nil},
		{"no-path", "https://i.pximg.net", nil},
		{"nondefault-port", "https://i.pximg.net:8443/page.jpg", nil},
	} {
		body, err := json.Marshal(map[string]any{"illust": map[string]any{"id": 42, "meta_single_page": map[string]string{"original_image_url": extra.url}}})
		if err != nil {
			t.Fatal(err)
		}
		inputs = append(inputs, pageCase{Name: extra.name, ID: 42, Body: body, AllowedHosts: extra.hosts})
	}
	contract := struct {
		Pages    []pageCase    `json:"pages"`
		Variants []variantCase `json:"variants"`
	}{Pages: make([]pageCase, 0, len(inputs))}
	for _, input := range inputs {
		input.Requests = 0
		input.DTO = nil
		input.Reason = ""
		input.Message = ""
		transport := migrationArtworkTransport(func(r *http.Request) (*http.Response, error) {
			input.Requests++
			if r.Method != "GET" || r.URL.Host != "app-api.pixiv.net" || r.URL.Path != "/v1/illust/detail" {
				t.Fatalf("unexpected page request: %s %s", r.Method, r.URL.Host)
			}
			return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(bytes.NewReader(input.Body)), Request: r}, nil
		})
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: transport}, ResourcePolicy: pixiv.ResourcePolicy{AllowedHosts: input.AllowedHosts}})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.ArtworkPages(context.Background(), pixiv.ArtworkPagesRequest{ArtworkID: input.ID})
		input.Reason = sdk.ReasonOf(err)
		if err != nil {
			input.Message = err.Error()
		} else {
			dtos := make([]pixiv.ArtworkPageDTO, 0, len(result))
			for _, page := range result {
				dtos = append(dtos, pixiv.ToArtworkPageDTO(page))
			}
			input.DTO, err = json.Marshal(dtos)
			if err != nil {
				t.Fatal(err)
			}
		}
		contract.Pages = append(contract.Pages, input)
	}
	for _, kind := range []string{"artwork", "user_profile", ""} {
		for _, id := range []int64{42, 0} {
			for _, variant := range []string{"", "original", "large", "arbitrary<&>"} {
				payload, err := json.Marshal(struct {
					Kind    string `json:"k"`
					ID      int64  `json:"id"`
					Page    int    `json:"p,omitempty"`
					Variant string `json:"v,omitempty"`
				}{kind, id, -1, "original"})
				if err != nil {
					t.Fatal(err)
				}
				ref, err := sdk.NewResourceRef("pixiv", payload)
				if err != nil {
					t.Fatal(err)
				}
				result, err := pixiv.ArtworkVariantResource(sdk.Resource{Ref: ref}, variant)
				contract.Variants = append(contract.Variants, variantCase{kind, id, -1, ref.String(), variant, result.String(), sdk.ReasonOf(err)})
			}
		}
	}
	for _, variant := range []string{"", "original", "large"} {
		result, err := pixiv.ArtworkVariantResource(sdk.Resource{}, variant)
		contract.Variants = append(contract.Variants, variantCase{Variant: variant, Ref: result.String(), Reason: sdk.ReasonOf(err)})
	}
	data, err = json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	file := filepath.Join(path, "artwork-pages.json")
	if *migrationUpdatePages {
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
		t.Fatal("page and variant contracts differ from the fixed Go reference")
	}
}

package sdk_test

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateResourceRef = flag.Bool("migration-update-resource-ref", false, "regenerate resource reference contracts from the fixed Go implementation")

type migrationRefCase struct {
	Name    string     `json:"name"`
	Product string     `json:"product"`
	Payload []byte     `json:"payload"`
	Text    string     `json:"text"`
	JSON    string     `json:"json"`
	Reason  sdk.Reason `json:"reason"`
}

func migrationRefResult(name string, ref sdk.ResourceRef, err error) migrationRefCase {
	result := migrationRefCase{Name: name, Text: ref.String(), Reason: sdk.ReasonOf(err)}
	if err == nil {
		result.Product, err = sdk.ResourceRefProduct(ref)
		if err != nil {
			panic(err)
		}
		result.Payload, err = sdk.ResourceRefPayload(ref)
		if err != nil {
			panic(err)
		}
		encoded, err := json.Marshal(ref)
		if err != nil {
			panic(err)
		}
		result.JSON = string(encoded)
	}
	return result
}

func TestMigrationResourceRefMatchesFrozenWireContract(t *testing.T) {
	var contract struct {
		Construct []migrationRefCase `json:"construct"`
		Parse     []migrationRefCase `json:"parse"`
		JSON      []migrationRefCase `json:"json"`
	}
	for _, input := range []struct {
		name, product string
		payload       []byte
	}{
		{"pixiv", "pixiv", []byte("artwork:42:page:0")},
		{"fanbox", "fanbox", []byte("post-asset:7")},
		{"binary", "pixiv", []byte{0, 255, 1, 128}},
		{"unicode", "日本語", []byte("作品:42")},
		{"html", "<>&\u2028\u2029", []byte("identity")},
		{"whitespace-product", " ", []byte("identity")},
		{"empty-product", "", []byte("identity")},
		{"empty-payload", "pixiv", nil},
	} {
		ref, err := sdk.NewResourceRef(input.product, input.payload)
		result := migrationRefResult(input.name, ref, err)
		result.Product, result.Payload = input.product, input.payload
		contract.Construct = append(contract.Construct, result)
	}
	valid := contract.Construct[0].Text
	for _, input := range []struct{ name, text string }{
		{"empty", ""}, {"raw-url", "https://i.pximg.net/image.jpg"},
		{"alphabet", "!!!"}, {"not-json", "YWJjZA"},
		{"padded", valid + "="}, {"space", valid + " "},
		{"line-breaks", valid[:5] + "\r\n" + valid[5:]},
		{"valid", valid},
	} {
		ref, err := sdk.ParseResourceRef(input.text)
		result := migrationRefResult(input.name, ref, err)
		result.Text = input.text
		contract.Parse = append(contract.Parse, result)
	}
	trailingBits := base64.RawURLEncoding.EncodeToString([]byte(`{"v":1,"p":"pixiv","d":"AA=="} `))
	if len(trailingBits)%4 == 0 {
		trailingBits = base64.RawURLEncoding.EncodeToString([]byte(`{"v":1,"p":"pixiv","d":"AA=="}  `))
	}
	alphabet := "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
	for index := range alphabet {
		if alphabet[index] == trailingBits[len(trailingBits)-1] {
			trailingBits = trailingBits[:len(trailingBits)-1] + string(alphabet[index|1])
			break
		}
	}
	ref, parseErr := sdk.ParseResourceRef(trailingBits)
	result := migrationRefResult("outer-base64-trailing-bits", ref, parseErr)
	result.Text = trailingBits
	contract.Parse = append(contract.Parse, result)
	for _, input := range []struct{ name, raw string }{
		{"envelope", `{"v":1,"p":"pixiv","d":"AA=="}`},
		{"future-version", `{"v":2,"p":"pixiv","d":"AA=="}`},
		{"absent-version", `{"p":"pixiv","d":"AA=="}`},
		{"absent-product", `{"v":1,"d":"AA=="}`},
		{"empty-payload", `{"v":1,"p":"pixiv","d":""}`},
		{"payload-array", `{"v":1,"p":"pixiv","d":[0,255,1]}`},
		{"payload-null-element", `{"v":1,"p":"pixiv","d":[null,255]}`},
		{"duplicate-payload-null-element", `{"v":1,"p":"pixiv","d":[17],"d":[null]}`},
		{"payload-overflow", `{"v":1,"p":"pixiv","d":[256]}`},
		{"payload-fraction", `{"v":1,"p":"pixiv","d":[1.0]}`},
		{"null-payload", `{"v":1,"p":"pixiv","d":null}`},
		{"null-envelope", `null`},
		{"array-envelope", `[]`},
		{"uppercase-keys", `{"V":1,"P":"pixiv","D":"AA=="}`},
		{"unknown-key", `{"v":1,"p":"pixiv","d":"AA==","future":{"value":1}}`},
		{"duplicate-product", `{"v":1,"p":"old","p":"pixiv","d":"AA=="}`},
		{"null-product-after-value", `{"v":1,"p":"pixiv","p":null,"d":"AA=="}`},
		{"invalid-product-before-value", `{"v":1,"p":17,"p":"pixiv","d":"AA=="}`},
		{"null-version-after-value", `{"v":1,"v":null,"p":"pixiv","d":"AA=="}`},
		{"fraction-version", `{"v":1.0,"p":"pixiv","d":"AA=="}`},
		{"inner-base64-lines", `{"v":1,"p":"pixiv","d":"A\r\nA=="}`},
		{"inner-base64-trailing-bits", `{"v":1,"p":"pixiv","d":"AB=="}`},
		{"unpaired-surrogate", `{"v":1,"p":"\ud800","d":"AA=="}`},
		{"unpaired-low-surrogate", `{"v":1,"p":"\udc00","d":"AA=="}`},
		{"paired-surrogates", `{"v":1,"p":"\ud83d\ude00","d":"AA=="}`},
		{"escaped-backslash", `{"v":1,"p":"\\ud800","d":"AA=="}`},
		{"invalid-utf8", "{\"v\":1,\"p\":\"\xff\",\"d\":\"AA==\"}"},
		{"incomplete-utf8", "{\"v\":1,\"p\":\"\xe1\x80\",\"d\":\"AA==\"}"},
		{"trailing-json", `{"v":1,"p":"pixiv","d":"AA=="} {}`},
	} {
		text := base64.RawURLEncoding.EncodeToString([]byte(input.raw))
		ref, err := sdk.ParseResourceRef(text)
		result := migrationRefResult(input.name, ref, err)
		result.Text = text
		contract.Parse = append(contract.Parse, result)
	}
	for _, input := range []struct{ name, raw string }{
		{"deep-unknown-field", `{"v":1,"p":"pixiv","d":"AA==","x":` + string(bytes.Repeat([]byte{'['}, 200)) + `0` + string(bytes.Repeat([]byte{']'}, 200)) + `}`},
		{"maximum-json-depth", `{"v":1,"p":"pixiv","d":"AA==","x":` + string(bytes.Repeat([]byte{'['}, 9999)) + `0` + string(bytes.Repeat([]byte{']'}, 9999)) + `}`},
		{"excessive-json-depth", `{"v":1,"p":"pixiv","d":"AA==","x":` + string(bytes.Repeat([]byte{'['}, 10000)) + `0` + string(bytes.Repeat([]byte{']'}, 10000)) + `}`},
		{"huge-unknown-number", `{"v":1,"p":"pixiv","d":"AA==","x":1e999}`},
	} {
		text := base64.RawURLEncoding.EncodeToString([]byte(input.raw))
		ref, err := sdk.ParseResourceRef(text)
		result := migrationRefResult(input.name, ref, err)
		result.Text = text
		contract.Parse = append(contract.Parse, result)
	}
	for _, input := range []struct{ name, raw string }{
		{"string", `"` + valid + `"`}, {"null", `null`},
		{"number", `42`}, {"object", `{}`}, {"empty-string", `""`},
	} {
		var ref sdk.ResourceRef
		err := json.Unmarshal([]byte(input.raw), &ref)
		result := migrationRefResult(input.name, ref, err)
		result.JSON = input.raw
		contract.JSON = append(contract.JSON, result)
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "docs", "migration", "contracts", "resource-ref.json")
	if *migrationUpdateResourceRef {
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
		t.Fatal("resource reference wire behavior differs from the frozen Go contract")
	}
}

package auth

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"strings"
	"testing"
)

func TestMigrationAuthBundleEncoding(t *testing.T) {
	b := &authExportBundle{Schema: authExportBundleSchema, Version: 1, Accounts: []authExportSecretAccount{{UserID: 7, Username: "<&>\u2028\u2029", RefreshToken: "synthetic"}}}
	body, err := encodeAuthExportBundle(b)
	if err != nil {
		t.Fatal(err)
	}
	want := `{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7,"username":"\u003c\u0026\u003e\u2028\u2029","refresh_token":"synthetic"}]}`
	if string(body) != want {
		t.Fatalf("got %s", body)
	}
	for _, b := range []*authExportBundle{nil, {}} {
		if _, err := encodeAuthExportBundle(b); err == nil {
			t.Fatal("empty export accepted")
		}
	}
}

func TestMigrationAuthBundleDecode(t *testing.T) {
	prefix := `{"schema":"pixiv-cli.auth-export","version":1,`
	tests := []struct{ name, body, want string }{
		{"valid", prefix + `"accounts":[{"user_id":7,"refresh_token":"synthetic"}]}`, ""},
		{"folded_order", prefix + `"VERSION":null,"ACCOUNTS":[{"user_id":1,"USER_ID":7,"refresh_token":"synthetic","USERNAME":null}]}`, ""},
		{"space_token", prefix + `"accounts":[{"user_id":7,"refresh_token":" "}]}`, ""},
		{"duplicate_key", prefix + `"accounts":[],"accounts":[]}`, `invalid auth export bundle: auth export bundle JSON has duplicate object key "accounts"`},
		{"duplicate_precedes_unknown", prefix + `"unknown":1,"accounts":[{"user_id":7,"user_id":8}]}`, `invalid auth export bundle: auth export bundle JSON has duplicate object key "user_id"`},
		{"unknown", prefix + `"unknown":1,"accounts":[]}`, `invalid auth export bundle: json: unknown field "unknown"`},
		{"null", "null", "unsupported auth export bundle schema or version"},
		{"empty", prefix + `"accounts":null}`, "auth export bundle has no accounts"},
		{"null_account", prefix + `"accounts":[null]}`, "auth export bundle contains an invalid account"},
		{"duplicate_uid", prefix + `"accounts":[{"user_id":7,"refresh_token":"s"},{"user_id":7,"refresh_token":"s"}]}`, "auth export bundle contains duplicate account 7"},
		{"bad_default", prefix + `"default_user_id":8,"accounts":[{"user_id":7,"refresh_token":"s"}]}`, "auth export bundle default does not name an included account"},
		{"trailing", prefix + `"accounts":[{"user_id":7,"refresh_token":"s"}]} {}`, "auth export bundle has trailing JSON"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			b, err := decodeAuthExportBundle([]byte(tt.body))
			if tt.want == "" {
				if err != nil {
					t.Fatal(err)
				}
				if b.Accounts[0].UserID != 7 {
					t.Fatal(b.Accounts[0].UserID)
				}
			} else if err == nil || err.Error() != tt.want {
				t.Fatalf("got %v want %s", err, tt.want)
			}
		})
	}
}

func TestMigrationAuthBundleSourceScannerPanic(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Fatal("expected frozen source scanner nil-map panic")
		}
	}()
	_, _ = decodeAuthExportBundle([]byte(`{"schema":"pixiv-cli.auth-export","version":1,"unknown":[{},"synthetic"]}`))
}

func TestMigrationAuthBundleTypeErrorsRedactValue(t *testing.T) {
	_, err := decodeAuthExportBundle([]byte(`{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":"synthetic-secret","refresh_token":"s"}]}`))
	if err == nil || strings.Contains(err.Error(), "synthetic-secret") {
		t.Fatalf("unsafe error: %v", err)
	}
}

func TestMigrationAuthBundleTypedDiagnostics(t *testing.T) {
	tests := []struct{ body, want string }{
		{`{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":"synthetic-secret","refresh_token":"s"}]}`, `invalid auth export bundle: json: cannot unmarshal string into Go struct field authExportBundle.accounts.0.user_id of type int64`},
		{`{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7.0,"refresh_token":"s"}]}`, `invalid auth export bundle: json: cannot unmarshal number 7.0 into Go struct field authExportBundle.accounts.0.user_id of type int64`},
		{`{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":9223372036854775808,"refresh_token":"s"}]}`, `invalid auth export bundle: json: cannot unmarshal number 9223372036854775808 into Go struct field authExportBundle.accounts.0.user_id of type int64`},
		{`{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7,"refresh_token":"s"}],"ACCOUNTS":null}`, `auth export bundle has no accounts`},
		{`{"schema":"pixiv-cli.auth-export","version":1,"schema":`, `invalid auth export bundle: auth export bundle JSON has duplicate object key "schema"`},
	}
	for _, tt := range tests {
		_, err := decodeAuthExportBundle([]byte(tt.body))
		if err == nil || err.Error() != tt.want {
			t.Errorf("got %v want %s", err, tt.want)
		}
	}
	body := []byte(`{"schema":"pixiv-cli.auth-export","version":1,"accounts":[{"user_id":7,"username":"\ud800","refresh_token":"s"}]}`)
	b, err := decodeAuthExportBundle(body)
	if err != nil || b.Accounts[0].Username != "\ufffd" {
		t.Fatalf("surrogate decode: %v", err)
	}
	body = []byte("{\"schema\":\"pixiv-cli.auth-export\",\"version\":1,\"accounts\":[{\"user_id\":7,\"username\":\"\xff\xff\",\"refresh_token\":\"s\"}]}")
	b, err = decodeAuthExportBundle(body)
	if err != nil || b.Accounts[0].Username != "\ufffd\ufffd" {
		t.Fatalf("UTF8 decode: %v", err)
	}
}

func TestMigrationAuthBundleKeyQuoting(t *testing.T) {
	tests := []struct{ key, quoted string }{
		{"<&>", `"<&>"`},
		{"\x00\a\b\t\n\v\f\r\x1f\x7f", `"\x00\a\b\t\n\v\f\r\x1f\x7f"`},
		{"é日😀", `"é日😀"`},
		{"\u00a0\u200b\u2028\ue000", `"\u00a0\u200b\u2028\ue000"`},
		{"\U000f0000", `"\U000f0000"`},
		{"\U000323b0", "\"\U000323b0\""},
	}
	for _, tt := range tests {
		key, _ := json.Marshal(tt.key)
		prefix := `{"schema":"pixiv-cli.auth-export","version":1,`
		_, err := decodeAuthExportBundle([]byte(prefix + string(key) + `:0}`))
		want := "invalid auth export bundle: json: unknown field " + tt.quoted
		if err == nil || err.Error() != want {
			t.Errorf("unknown got %v want %s", err, want)
		}
		_, err = decodeAuthExportBundle([]byte(prefix + string(key) + `:0,` + string(key) + `:1}`))
		want = "invalid auth export bundle: auth export bundle JSON has duplicate object key " + tt.quoted
		if err == nil || err.Error() != want {
			t.Errorf("duplicate got %v want %s", err, want)
		}
	}
}

func TestMigrationAuthBundleAllScalarQuotes(t *testing.T) {
	var key strings.Builder
	for code := rune(0); code <= 0x10ffff; code++ {
		if code < 0xd800 || code > 0xdfff {
			key.WriteRune(code)
		}
	}
	encoded, _ := json.Marshal(key.String())
	for _, duplicate := range []bool{false, true} {
		body := []byte(`{` + string(encoded) + `:0}`)
		if duplicate {
			body = []byte(`{` + string(encoded) + `:0,` + string(encoded) + `:1}`)
		}
		_, err := decodeAuthExportBundle(body)
		if err == nil {
			t.Fatal("all-scalar unknown key accepted")
		}
		digest := sha256.Sum256([]byte(err.Error()))
		want := "78264dd9ca8a15047dba309785b941535e68935901c6f344be0ce65f13cab314"
		if duplicate {
			want = "83e09deef6235851859d4605380d173ffdea2a250d4c9d95f25140eb38e77db4"
		}
		if hex.EncodeToString(digest[:]) != want {
			t.Fatalf("duplicate=%v diagnostic sha256=%x", duplicate, digest)
		}
	}
}

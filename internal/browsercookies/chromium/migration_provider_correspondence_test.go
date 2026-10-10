//go:build linux

package chromium

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/csv"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"testing"
)

import "github.com/FlanChanXwO/pixiv-cli/internal/browsercookies"

func migrationProviderHex(t *testing.T, value any) []byte {
	t.Helper()
	encoded, ok := value.(string)
	if !ok {
		t.Fatalf("hex input is not a string: %T", value)
	}
	body, err := hex.DecodeString(encoded)
	if err != nil {
		t.Fatal(err)
	}
	return body
}

func migrationProviderKeys(t *testing.T, value any) [][]byte {
	t.Helper()
	var keys [][]byte
	switch values := value.(type) {
	case []any:
		for _, value := range values {
			keys = append(keys, migrationProviderHex(t, value))
		}
	case []string:
		for _, value := range values {
			keys = append(keys, migrationProviderHex(t, value))
		}
	default:
		t.Fatalf("key vector is not an array: %T", value)
	}
	return keys
}

func migrationProviderCopy(input map[string]any) map[string]any {
	out := make(map[string]any, len(input)+1)
	for key, value := range input {
		out[key] = value
	}
	return out
}

func migrationProviderDecrypt(t *testing.T, input map[string]any) map[string]any {
	t.Helper()
	ctx, cancel := migrationCryptoContext(input["context"].(string))
	defer cancel()
	keys := migrationProviderKeys(t, input["keys_hex"])
	calls := 0
	p := &provider{encryptionKeyOverride: func(context.Context) ([][]byte, error) {
		calls++
		if input["cancel_inside_key_hook"] == true {
			cancel()
		}
		if input["key_error"] == "access" {
			return nil, browsercookies.ErrSecretServiceAccess
		}
		return keys, nil
	}}
	value, err := p.decryptEncrypted(ctx, migrationProviderHex(t, input["blob_hex"]))
	out := migrationCryptoBytes(value, err)
	out["key_calls"] = calls
	return out
}

func migrationProviderLinux(t *testing.T, input map[string]any, decrypt bool) map[string]any {
	t.Helper()
	trace, ready := migrationCryptoSecretTool(t, input["secret_mode"].(string), migrationProviderHex(t, input["password_hex"]))
	p, err := newProvider(input["browser"].(string), migrationCryptoWriteState(t, input["state_mode"].(string), migrationProviderHex(t, input["state_body_hex"])))
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := migrationCryptoContext(input["context"].(string))
	defer cancel()
	var out map[string]any
	run := func() {
		if decrypt {
			value, err := p.decryptEncrypted(ctx, migrationProviderHex(t, input["blob_hex"]))
			out = migrationCryptoBytes(value, err)
		} else {
			keys, err := p.encryptionKeys(ctx)
			out = migrationCryptoKeys(keys, err)
		}
	}
	if input["secret_mode"] == "wait" {
		done := make(chan struct{})
		go func() { run(); close(done) }()
		migrationCryptoWaitAndCancel(t, ready, cancel)
		<-done
	} else {
		run()
	}
	out["command_args"] = migrationCryptoReadTrace(t, trace)
	return out
}

func TestMigrationChromiumProviderSQLiteCommandHelper(t *testing.T) {
	if os.Getenv("PIXIV_MIGRATION_PROVIDER_SQLITE_HELPER") != "1" {
		return
	}
	args := os.Args
	for i, arg := range args {
		if arg == "--" {
			args = args[i+1:]
			break
		}
	}
	body, err := json.Marshal(args)
	if err != nil {
		os.Exit(90)
	}
	if err := os.WriteFile(os.Getenv("PIXIV_MIGRATION_PROVIDER_SQLITE_TRACE"), body, 0o600); err != nil {
		os.Exit(91)
	}
	body, err = hex.DecodeString(os.Getenv("PIXIV_MIGRATION_PROVIDER_SQLITE_CSV_HEX"))
	if err != nil {
		os.Exit(92)
	}
	_, _ = os.Stdout.Write(body)
	os.Exit(0)
}

func migrationProviderSQLite(t *testing.T, dir string, body []byte) string {
	t.Helper()
	trace := filepath.Join(dir, "sqlite-args.json")
	t.Setenv("PATH", dir)
	t.Setenv("PIXIV_MIGRATION_PROVIDER_SQLITE_HELPER", "1")
	t.Setenv("PIXIV_MIGRATION_PROVIDER_SQLITE_TRACE", trace)
	t.Setenv("PIXIV_MIGRATION_PROVIDER_SQLITE_CSV_HEX", hex.EncodeToString(body))
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	script := "#!/bin/sh\nexec " + strconv.Quote(executable) + " -test.run=^TestMigrationChromiumProviderSQLiteCommandHelper$ -- \"$@\"\n"
	if err := os.WriteFile(filepath.Join(dir, "sqlite3"), []byte(script), 0o700); err != nil {
		t.Fatal(err)
	}
	return trace
}

func migrationProviderCSV(t *testing.T, value any) []byte {
	t.Helper()
	var body bytes.Buffer
	writer := csv.NewWriter(&body)
	for _, value := range value.([]any) {
		fields := []string{}
		for _, field := range value.([]any) {
			fields = append(fields, string(migrationProviderHex(t, field)))
		}
		if err := writer.Write(fields); err != nil {
			t.Fatal(err)
		}
	}
	writer.Flush()
	if err := writer.Error(); err != nil {
		t.Fatal(err)
	}
	return body.Bytes()
}

func migrationProviderRead(t *testing.T, input map[string]any) map[string]any {
	t.Helper()
	root := migrationCryptoWriteState(t, "file", migrationProviderHex(t, input["state_body_hex"]))
	profile := input["profile_id"].(string)
	if input["database_mode"] != "missing" {
		mustWriteFile(t, filepath.Join(root, "Profile 1", cookiesFile))
	}
	ctx, cancel := migrationCryptoContext(input["context"].(string))
	defer cancel()
	dir := t.TempDir()
	secretTrace := ""
	if input["key_source"] == "linux_secret_tool" {
		secretTrace, _ = migrationCryptoSecretTool(t, input["secret_mode"].(string), migrationProviderHex(t, input["password_hex"]))
		dir = filepath.Dir(secretTrace)
	}
	trace := migrationProviderSQLite(t, dir, migrationProviderHex(t, input["csv_hex"]))
	p, err := newProvider("chrome", root)
	if err != nil {
		t.Fatal(err)
	}
	calls := 0
	if input["key_source"] == "encryption_key_override" {
		keys := migrationProviderKeys(t, input["keys_hex"])
		p.encryptionKeyOverride = func(context.Context) ([][]byte, error) {
			calls++
			if input["cancel_inside_key_hook"] == true {
				cancel()
			}
			return keys, nil
		}
	}
	values, err := p.Read(ctx, browsercookies.CookieQuery{Host: input["query_host"].(string), Name: input["query_name"].(string)}, profile)
	out := migrationCryptoError(err)
	for _, candidate := range []struct {
		err   error
		class string
	}{{browsercookies.ErrQueryInvalid, "query_invalid"}, {browsercookies.ErrInvalidProfileID, "invalid_profile_id"}, {browsercookies.ErrDatabaseNotFound, "database_not_found"}} {
		if errors.Is(err, candidate.err) {
			out["class"] = candidate.class
		}
	}
	encoded, redacted := []string{}, []string{}
	for _, value := range values {
		encoded = append(encoded, hex.EncodeToString([]byte(value.Value())))
		redacted = append(redacted, value.String())
	}
	out["values_hex"], out["values_nil"], out["secret_strings"], out["key_calls"] = encoded, values == nil, redacted, calls
	args := migrationCryptoReadTrace(t, trace)
	parameters := []string{}
	if len(args) != 0 {
		if len(args) != 13 || !bytes.Equal([]byte(args[4]), []byte("\n")) || args[11] != filepath.Join(root, "Profile 1", cookiesFile) || args[12] != selectCookiesSQL {
			t.Fatalf("unexpected actual Read sqlite boundary arguments: %#v", args)
		}
		for i := 5; i < 11; i += 2 {
			if args[i] != "-cmd" {
				t.Fatalf("sqlite parameter flag: %#v", args)
			}
			parameters = append(parameters, args[i+1])
		}
		sort.Strings(parameters)
		out["sqlite_flags"] = args[:5]
		out["sqlite_sql"] = args[12]
	} else {
		out["sqlite_flags"] = []string{}
		out["sqlite_sql"] = ""
	}
	out["sqlite_called"], out["sqlite_parameters"] = len(args) != 0, parameters
	if secretTrace != "" {
		out["secret_command_args"] = migrationCryptoReadTrace(t, secretTrace)
	}
	return out
}

func TestMigrationChromiumProviderCorrespondence(t *testing.T) {
	root := filepath.Clean(filepath.Join("..", "..", ".."))
	oldPath := "crates/pixiv-cli/tests/fixtures/browser-chromium-crypto.json"
	oldBody, err := os.ReadFile(filepath.Join(root, oldPath))
	if err != nil {
		t.Fatal(err)
	}
	if got := fmt.Sprintf("%x", sha256.Sum256(oldBody)); got != "aca537839752502488fcb6075605d911c89d7a326ff435dd090c1f6401597abf" {
		t.Fatalf("published 159-case crypto fixture changed: %s", got)
	}
	var old migrationCryptoFixture
	if err := json.Unmarshal(oldBody, &old); err != nil {
		t.Fatal(err)
	}
	if len(old.Cases) != 159 {
		t.Fatal("published crypto case count changed")
	}
	fixture := migrationCryptoFixture{
		Reference: old.Reference, Environment: runtime.GOOS + "/" + runtime.GOARCH, GoVersion: runtime.Version(),
		Sources: migrationProviderSourceHashes(t, root, old.Sources),
		Boundaries: map[string]string{
			"provider_cipher": "Actual unchanged provider.decryptEncrypted for all 51 existing blob/key helper vectors and all 10 existing provider vectors; existing encryptionKeyOverride is the named original key-acquisition dependency boundary; outputs are recaptured, never projected from helper outputs",
			"linux_keys":      "Actual unchanged provider.encryptionKeys and decryptEncrypted using the existing owned synthetic secret-tool process, real LookPath/CommandContext/Output, and owned Local State files; no real keyring, browser, account, credentials or network",
			"local_state":     "Actual unchanged localStateEncryptedKey and encryptionKeys for bounded nested duplicate objects, null-merging, type-error ordering, Unicode simple-fold field names, malformed UTF-8 and unpaired surrogate Go JSON replacement",
			"public_read":     "Actual unchanged provider.Read, sqliteio.Query, CSV parser, rowsToSnapshot and decryptEncrypted with an owned test-binary sqlite3 CSV process and owned empty Cookies placeholder; this tests process/CSV/Read correspondence, not native SQLite query execution; original encryptionKeyOverride or owned secret-tool boundary is named per case",
			"read_trace":      "Exact sqlite flags, SQL and sorted parameter commands; only the owned absolute database path is asserted in Go rather than stored; map iteration order is not a contract",
			"scope":           "Go Linux runtime only; no Darwin or Windows runtime claim; unreachable PBKDF2 primitive iteration/key-length shapes remain in the existing Go-only helper corpus",
		}, Cases: []migrationCryptoCase{},
	}
	add := func(id, operation string, input, output map[string]any) {
		fixture.Cases = append(fixture.Cases, migrationCryptoCase{ID: id, Operation: operation, Input: input, Output: output})
	}
	for _, source := range old.Cases {
		input := migrationProviderCopy(source.Input)
		input["source_case_id"] = source.ID
		switch source.Operation {
		case "decrypt_gcm", "decrypt_cbc", "decrypt_chromium_value", "decrypt_legacy_value":
			input["context"], input["key_error"], input["cancel_inside_key_hook"] = "active", "", false
			input["keys_hex"] = []string{input["key_hex"].(string)}
			delete(input, "key_hex")
			input["key_source"] = "encryption_key_override"
			add("cipher-"+source.ID, "decrypt_encrypted", input, migrationProviderDecrypt(t, input))
		case "decrypt_encrypted":
			input["key_source"] = "encryption_key_override"
			add("existing-"+source.ID, "decrypt_encrypted", input, migrationProviderDecrypt(t, input))
		case "linux_encryption_keys":
			add("existing-"+source.ID, "linux_encryption_keys", input, migrationProviderLinux(t, input, false))
		case "rows_to_snapshot":
			input["csv_hex"] = hex.EncodeToString(migrationProviderCSV(t, input["rows_hex"]))
			input["key_source"], input["cancel_inside_key_hook"], input["database_mode"], input["state_body_hex"] = "encryption_key_override", false, "owned_placeholder", "7b7d"
			add("read-"+source.ID, "provider_read", input, migrationProviderRead(t, input))
		}
	}
	migrationProviderLocalStates(t, add)
	migrationProviderLinuxDecryptCases(t, add)
	migrationProviderAdditionalReadCases(t, add, old.Cases)
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "browser-chromium-provider.json")
	if os.Getenv("PIXIV_CAPTURE_BROWSER_CHROMIUM_PROVIDER") == "1" {
		if err := os.WriteFile(path, body, 0o600); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d actual Go Chromium provider correspondence cases", len(fixture.Cases))
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(body, want) {
		actual := filepath.Join(t.TempDir(), "browser-chromium-provider-actual.json")
		_ = os.WriteFile(actual, body, 0o600)
		t.Fatalf("provider correspondence changed; actual at %s", actual)
	}
	t.Logf("replayed %d actual Go Chromium provider correspondence cases", len(fixture.Cases))
}

func migrationProviderSourceHashes(t *testing.T, root string, sources map[string]string) map[string]string {
	t.Helper()
	wants := make(map[string]string, len(sources)+3)
	for path, want := range sources {
		wants[path] = want
	}
	wants["internal/browsercookies/sqliteio/sqliteio.go"] = "d922ed106d3a70cd4c0fa79297edb57cbe9adff051816aa3cc8218aaf4b2c902"
	wants["internal/browsercookies/chromium/migration_cookie_crypto_test.go"] = "1bf0ff6b1963bac2a5c47fdd911ab5ab095cd89bd1482c045ba65f2deee312ba"
	wants["crates/pixiv-cli/tests/fixtures/browser-chromium-crypto.json"] = "aca537839752502488fcb6075605d911c89d7a326ff435dd090c1f6401597abf"
	for path, want := range wants {
		body, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(body)); got != want {
			t.Fatalf("frozen provider source changed: %s: %s", path, got)
		}
	}
	return wants
}

func migrationProviderLocalStates(t *testing.T, add func(string, string, map[string]any, map[string]any)) {
	t.Helper()
	for _, spec := range []struct{ id, body string }{
		{"duplicate-objects-last-key", `{"os_crypt":{"encrypted_key":"AAE="},"os_crypt":{"encrypted_key":"AgM="}}`},
		{"duplicate-objects-empty-retains", `{"os_crypt":{"encrypted_key":"AAE="},"os_crypt":{}}`},
		{"duplicate-object-null-retains", `{"os_crypt":{"encrypted_key":"AAE="},"os_crypt":null}`},
		{"null-then-object", `{"os_crypt":null,"os_crypt":{"encrypted_key":"AAE="}}`},
		{"duplicate-key-null-retains", `{"os_crypt":{"encrypted_key":"AAE=","encrypted_key":null}}`},
		{"null-string-last-string", `{"os_crypt":{"encrypted_key":null,"encrypted_key":"AAE=","encrypted_key":"AgM="}}`},
		{"string-null-empty-erases", `{"os_crypt":{"encrypted_key":"AAE=","encrypted_key":null,"encrypted_key":""}}`},
		{"type-error-before-valid-key", `{"os_crypt":{"encrypted_key":1,"encrypted_key":"AAE="}}`},
		{"type-error-after-valid-key", `{"os_crypt":{"encrypted_key":"AAE=","encrypted_key":1,"encrypted_key":null}}`},
		{"crypt-type-error-before-object", `{"os_crypt":1,"os_crypt":{"encrypted_key":"AAE="}}`},
		{"crypt-type-error-after-object", `{"os_crypt":{"encrypted_key":"AAE="},"os_crypt":[]}`},
		{"long-s-fold-field", `{"oſ_crypt":{"encrypted_key":"AAE="}}`},
		{"kelvin-fold-field", `{"os_crypt":{"encrypted_Key":"AAE="}}`},
		{"escaped-fold-fields", `{"o\u017f_crypt":{"encrypted_\u212aey":"AAE="}}`},
		{"invalid-utf8-unknown-value-accepted", "{\"ignored\":\"\xff\",\"os_crypt\":{\"encrypted_key\":\"AAE=\"}}"},
		{"invalid-utf8-field-replaced-unmatched", "{\"o\xffs_crypt\":{\"encrypted_key\":\"AAE=\"}}"},
		{"invalid-utf8-key-value-replaced", "{\"os_crypt\":{\"encrypted_key\":\"AA\xffE=\"}}"},
		{"unpaired-high-surrogate-ignored-accepted", `{"ignored":"\ud800","os_crypt":{"encrypted_key":"AAE="}}`},
		{"unpaired-low-surrogate-ignored-accepted", `{"ignored":"\udfff","os_crypt":{"encrypted_key":"AAE="}}`},
		{"unpaired-surrogate-key-value-replaced", `{"os_crypt":{"encrypted_key":"\ud800"}}`},
		{"null-key-legacy-fallback", `{"os_crypt":{"encrypted_key":null}}`},
	} {
		body := []byte(spec.body)
		input := map[string]any{"state_mode": "file", "state_body_hex": hex.EncodeToString(body)}
		value, present, err := (&provider{root: migrationCryptoWriteState(t, "file", body)}).localStateEncryptedKey()
		out := migrationCryptoBytes(value, err)
		out["present"] = present
		add("state-"+spec.id, "local_state_encrypted_key", input, out)
		input = migrationProviderCopy(input)
		input["browser"], input["context"], input["password_hex"], input["secret_mode"] = "chrome", "active", hex.EncodeToString([]byte("synthetic-password\r\n")), "success"
		add("keys-state-"+spec.id, "linux_encryption_keys", input, migrationProviderLinux(t, input, false))
	}
}

func migrationProviderLinuxDecryptCases(t *testing.T, add func(string, string, map[string]any, map[string]any)) {
	t.Helper()
	key := []byte("0123456789abcdef")
	password := []byte("synthetic-password\r\n")
	legacyKeys := chromiumKeyCandidates([]byte("synthetic-password"))
	plain := []byte{0, 0xff, 0xfe, '\r', '\n', ',', '"', 0x80}
	rawState := migrationCryptoStateBody(t, key)
	for _, spec := range []struct {
		id, stateMode, secretMode, contextMode string
		state, blob                            []byte
	}{
		{"raw-state-gcm", "file", "success", "active", rawState, migrationCryptoGCM(t, "v10", key, plain, nil)},
		{"raw-state-cbc", "file", "success", "active", rawState, migrationCryptoCBC(t, "v11", key, plain, true, false)},
		{"wrapped-state-gcm", "file", "success", "active", migrationCryptoStateBody(t, migrationCryptoGCM(t, "v11", legacyKeys[1], key, nil)), migrationCryptoGCM(t, "v11", key, plain, nil)},
		{"missing-state-cbc-second-candidate", "missing", "success", "active", nil, migrationCryptoCBC(t, "v10", legacyKeys[1], plain, true, false)},
		{"missing-state-gcm-third-candidate", "missing", "success", "active", nil, migrationCryptoGCM(t, "v11", legacyKeys[2], plain, nil)},
		{"secret-failure-before-malformed-blob-state", "file", "fail", "active", []byte("{"), []byte("v10")},
		{"unknown-layout-before-missing-secret", "file", "missing", "active", []byte("{"), []byte("v20")},
		{"pre-cancel-before-format-secret-state", "file", "missing", "canceled", []byte("{"), []byte("v20")},
		{"cancel-running-secret", "file", "wait", "active", rawState, migrationCryptoGCM(t, "v10", key, plain, nil)},
	} {
		input := map[string]any{"browser": "chrome", "context": spec.contextMode, "password_hex": hex.EncodeToString(password), "secret_mode": spec.secretMode, "state_mode": spec.stateMode, "state_body_hex": hex.EncodeToString(spec.state), "blob_hex": hex.EncodeToString(spec.blob), "key_source": "linux_secret_tool"}
		add("linux-decrypt-"+spec.id, "linux_decrypt_encrypted", input, migrationProviderLinux(t, input, true))
	}
}

func migrationProviderAdditionalReadCases(t *testing.T, add func(string, string, map[string]any, map[string]any), old []migrationCryptoCase) {
	t.Helper()
	var encrypted, plaintext, acquisition map[string]any
	for _, source := range old {
		switch source.ID {
		case "rows-encrypted-overrides-plaintext":
			encrypted = source.Input
		case "rows-plain-raw-no-keys":
			plaintext = source.Input
		case "linux-keys-raw-key":
			acquisition = source.Input
		}
	}
	base := migrationProviderCopy(encrypted)
	base["csv_hex"] = hex.EncodeToString(migrationProviderCSV(t, base["rows_hex"]))
	base["key_source"], base["cancel_inside_key_hook"], base["database_mode"], base["state_body_hex"] = "encryption_key_override", false, "owned_placeholder", "7b7d"
	for _, spec := range []struct{ id, change string }{
		{"empty-success-non-nil", "empty"}, {"short-row-active", "short"}, {"malformed-csv", "csv"},
		{"query-before-profile-cancel", "query"}, {"profile-before-cancel", "profile"}, {"missing-db-before-cancel", "database"},
		{"cancel-inside-key-then-plaintext-success", "cancel-plain"}, {"cancel-inside-key-then-encrypted-discards", "cancel-encrypted"},
		{"real-linux-key-and-read", "linux"}, {"real-linux-secret-failure-discards-plaintext", "linux-fail"},
	} {
		input := migrationProviderCopy(base)
		switch spec.change {
		case "empty":
			input["csv_hex"] = ""
		case "short":
			input["csv_hex"] = hex.EncodeToString([]byte(".fanbox.cc,plain\n"))
		case "csv":
			input["csv_hex"] = hex.EncodeToString([]byte("\"unterminated\n"))
		case "query":
			input["query_host"], input["profile_id"], input["context"] = "", "../bad", "canceled"
		case "profile":
			input["profile_id"], input["context"] = "../bad", "canceled"
		case "database":
			input["database_mode"], input["context"] = "missing", "canceled"
		case "cancel-plain", "cancel-encrypted":
			rows := []any{encrypted["rows_hex"].([]any)[0]}
			if spec.change == "cancel-plain" {
				rows = append(rows, plaintext["rows_hex"].([]any)[0])
			} else {
				rows = append(rows, encrypted["rows_hex"].([]any)[0])
			}
			input["csv_hex"], input["cancel_inside_key_hook"] = hex.EncodeToString(migrationProviderCSV(t, rows)), true
		case "linux", "linux-fail":
			input["key_source"] = "linux_secret_tool"
			for key, value := range acquisition {
				input[key] = value
			}
			if spec.change == "linux-fail" {
				input["secret_mode"] = "fail"
				rows := []any{plaintext["rows_hex"].([]any)[0], encrypted["rows_hex"].([]any)[0]}
				input["csv_hex"] = hex.EncodeToString(migrationProviderCSV(t, rows))
			}
		}
		delete(input, "rows_hex")
		add("read-"+spec.id, "provider_read", input, migrationProviderRead(t, input))
	}
}

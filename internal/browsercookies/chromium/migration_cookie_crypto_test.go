//go:build linux

package chromium

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies"
	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies/secret"
)

type migrationCryptoCase struct {
	ID        string         `json:"id"`
	Operation string         `json:"operation"`
	Input     map[string]any `json:"input"`
	Output    map[string]any `json:"output"`
}

type migrationCryptoFixture struct {
	Reference   string                `json:"reference"`
	Environment string                `json:"environment"`
	GoVersion   string                `json:"go_version"`
	Sources     map[string]string     `json:"sources"`
	Boundaries  map[string]string     `json:"boundaries"`
	Cases       []migrationCryptoCase `json:"cases"`
}

func migrationCryptoError(err error) map[string]any {
	out := map[string]any{"error": "", "class": ""}
	if err == nil {
		return out
	}
	out["error"] = err.Error()
	for _, candidate := range []struct {
		err   error
		class string
	}{
		{context.Canceled, "context_canceled"},
		{context.DeadlineExceeded, "context_deadline_exceeded"},
		{browsercookies.ErrEncryptedMalformed, "encrypted_malformed"},
		{browsercookies.ErrEncryptedFormatUnknown, "encrypted_format_unknown"},
		{browsercookies.ErrPermissionDenied, "permission_denied"},
		{browsercookies.ErrSecretServiceUnavailable, "secret_service_unavailable"},
		{browsercookies.ErrSecretServiceAccess, "secret_service_access"},
		{browsercookies.ErrQueryFailed, "query_failed"},
		{secret.ErrNotAvailableOnBuild, "secret_not_available"},
		{secret.ErrInvalidItem, "secret_invalid_item"},
		{secret.ErrEmptyPassword, "secret_empty_password"},
		{secret.ErrSecretService, "secret_service_lookup"},
	} {
		if errors.Is(err, candidate.err) {
			out["class"] = candidate.class
			return out
		}
	}
	out["class"] = "other"
	return out
}

func migrationCryptoBytes(value []byte, err error) map[string]any {
	out := migrationCryptoError(err)
	out["value_hex"] = hex.EncodeToString(value)
	out["value_nil"] = value == nil
	return out
}

func migrationCryptoKeys(keys [][]byte, err error) map[string]any {
	out := migrationCryptoError(err)
	encoded := make([]string, 0, len(keys))
	for _, key := range keys {
		encoded = append(encoded, hex.EncodeToString(key))
	}
	out["keys_hex"] = encoded
	out["keys_nil"] = keys == nil
	return out
}

func migrationCryptoGCM(t *testing.T, prefix string, key, plain, aad []byte) []byte {
	t.Helper()
	block, err := aes.NewCipher(key)
	if err != nil {
		t.Fatal(err)
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		t.Fatal(err)
	}
	nonce := []byte{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11}
	blob := append([]byte(prefix), nonce...)
	return append(blob, gcm.Seal(nil, nonce, plain, aad)...)
}

func migrationCryptoCBC(t *testing.T, prefix string, key, plain []byte, padded bool, zeroIV bool) []byte {
	t.Helper()
	plain = append([]byte(nil), plain...)
	if padded {
		padding := aes.BlockSize - len(plain)%aes.BlockSize
		plain = append(plain, bytes.Repeat([]byte{byte(padding)}, padding)...)
	}
	if len(plain)%aes.BlockSize != 0 {
		t.Fatal("synthetic CBC plaintext must be block aligned")
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		t.Fatal(err)
	}
	iv := bytes.Repeat([]byte{' '}, aes.BlockSize)
	if zeroIV {
		iv = make([]byte, aes.BlockSize)
	}
	ciphertext := make([]byte, len(plain))
	cipher.NewCBCEncrypter(block, iv).CryptBlocks(ciphertext, plain)
	return append([]byte(prefix), ciphertext...)
}

func migrationCryptoContext(mode string) (context.Context, context.CancelFunc) {
	ctx, cancel := context.WithCancel(context.Background())
	switch mode {
	case "canceled":
		cancel()
	case "deadline":
		cancel()
		return context.WithDeadline(context.Background(), time.Unix(0, 0))
	}
	return ctx, cancel
}

func migrationCryptoWriteState(t *testing.T, mode string, body []byte) string {
	t.Helper()
	root := t.TempDir()
	path := filepath.Join(root, "Local State")
	switch mode {
	case "missing":
	case "directory":
		if err := os.Mkdir(path, 0o700); err != nil {
			t.Fatal(err)
		}
	default:
		if err := os.WriteFile(path, body, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

func migrationCryptoStateBody(t *testing.T, key []byte) []byte {
	t.Helper()
	body, err := json.Marshal(map[string]any{"os_crypt": map[string]string{"encrypted_key": base64.StdEncoding.EncodeToString(key)}})
	if err != nil {
		t.Fatal(err)
	}
	return body
}

func TestMigrationChromiumCryptoCommandHelper(t *testing.T) {
	if os.Getenv("PIXIV_MIGRATION_CRYPTO_HELPER") != "1" {
		return
	}
	if os.Getenv("PIXIV_MIGRATION_CRYPTO_MODE") == "state-permission" {
		p := &provider{root: os.Getenv("PIXIV_MIGRATION_CRYPTO_STATE_ROOT")}
		value, present, err := p.localStateEncryptedKey()
		out := migrationCryptoBytes(value, err)
		out["present"] = present
		body, marshalErr := json.Marshal(out)
		if marshalErr != nil {
			os.Exit(94)
		}
		_, _ = os.Stdout.Write(body)
		os.Exit(0)
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
	if err := os.WriteFile(os.Getenv("PIXIV_MIGRATION_CRYPTO_TRACE"), body, 0o600); err != nil {
		os.Exit(91)
	}
	switch os.Getenv("PIXIV_MIGRATION_CRYPTO_MODE") {
	case "fail":
		_, _ = os.Stderr.Write([]byte("synthetic-private-stderr\xff"))
		os.Exit(17)
	case "wait":
		if err := os.WriteFile(os.Getenv("PIXIV_MIGRATION_CRYPTO_READY"), nil, 0o600); err != nil {
			os.Exit(92)
		}
		for {
			time.Sleep(time.Hour)
		}
	default:
		password, err := hex.DecodeString(os.Getenv("PIXIV_MIGRATION_CRYPTO_PASSWORD_HEX"))
		if err != nil {
			os.Exit(93)
		}
		_, _ = os.Stdout.Write(password)
		os.Exit(0)
	}
}

func migrationCryptoSecretTool(t *testing.T, mode string, password []byte) (string, string) {
	t.Helper()
	dir := t.TempDir()
	trace := filepath.Join(dir, "args.json")
	ready := filepath.Join(dir, "ready")
	t.Setenv("PATH", dir)
	t.Setenv("PIXIV_MIGRATION_CRYPTO_HELPER", "1")
	t.Setenv("PIXIV_MIGRATION_CRYPTO_TRACE", trace)
	t.Setenv("PIXIV_MIGRATION_CRYPTO_READY", ready)
	t.Setenv("PIXIV_MIGRATION_CRYPTO_MODE", mode)
	t.Setenv("PIXIV_MIGRATION_CRYPTO_PASSWORD_HEX", hex.EncodeToString(password))
	if mode != "missing" {
		executable, err := os.Executable()
		if err != nil {
			t.Fatal(err)
		}
		script := "#!/bin/sh\nexec " + strconv.Quote(executable) + " -test.run=^TestMigrationChromiumCryptoCommandHelper$ -- \"$@\"\n"
		if err := os.WriteFile(filepath.Join(dir, "secret-tool"), []byte(script), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	return trace, ready
}

func migrationCryptoReadTrace(t *testing.T, path string) []string {
	t.Helper()
	body, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return []string{}
	}
	if err != nil {
		t.Fatal(err)
	}
	var args []string
	if err := json.Unmarshal(body, &args); err != nil {
		t.Fatal(err)
	}
	return args
}

func migrationCryptoWaitAndCancel(t *testing.T, ready string, cancel context.CancelFunc) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for {
		if _, err := os.Stat(ready); err == nil {
			cancel()
			return
		}
		if time.Now().After(deadline) {
			cancel()
			t.Fatal("owned secret-tool helper did not reach its command boundary")
		}
		time.Sleep(time.Millisecond)
	}
}

func TestMigrationChromiumCookieCrypto(t *testing.T) {
	fixture := migrationCryptoFixture{
		Reference:   "4b4426487ef18bed276706daec385e0d0a6979f9",
		Environment: runtime.GOOS + "/" + runtime.GOARCH,
		GoVersion:   runtime.Version(),
		Sources:     map[string]string{},
		Boundaries: map[string]string{
			"crypto":          "Unchanged Go private functions with standard-library AES/CBC/GCM and synthetic byte inputs; no real browser, account, keyring or credentials",
			"linux_secret":    "Unchanged SecretService.GetPassword and provider.encryptionKeys; real LookPath/CommandContext/Output against an owned synthetic secret-tool process",
			"candidate_dedup": "Actual chromiumKeyCandidates order and unique output for fixed passwords; no cryptographic collision fabricated to force its unreachable duplicate branch",
			"darwin":          "Source SHA only, not executed: fixed security arguments, trailing CR/LF trim; exact could not be found stderr maps item-not-found; provider maps all other password errors including canceled context to keychain-access",
			"windows":         "Source SHA only, not executed: Local State first, optional DPAPI prefix, any non-v10/v11 nonempty legacy blob invokes DPAPI; DATA_BLOB u32 bounds, flags/null ABI, successful output copied then LocalFree, context before/after call; zero-size/null output exits before free defer",
			"permissions":     "Actual Local State mode-000 owned-file error; when root, an owned copy of the Go test binary runs as UID/GID 65534 in a child process; no persistent account or security setting changes",
			"cancellation":    "Actual pre-cancellation, expired deadline, cancellation while synthetic Linux secret command runs, plaintext/invalid-hex precedence, and cancellation inside existing encryptionKeyOverride; no native OS cancellation claim",
		},
		Cases: []migrationCryptoCase{},
	}
	root := filepath.Clean(filepath.Join("..", "..", ".."))
	for path, want := range map[string]string{
		"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/browsercookies/browsercookies.go":               "1b1e373c3a240f54d8b0d255ad5ae5b753b2eae0c8e506df1615e3b9190a3e6b",
		"internal/browsercookies/chromium/chromecrypto.go":        "24c0323d3d5bef2e2f0ab6d120391ea622494995374b71515069daab25ea0cf7",
		"internal/browsercookies/chromium/chromium.go":            "bca8e41df3d25be82fa0fa0a1f85e42c5fa354d4602ccb943bb69845c57bb877",
		"internal/browsercookies/chromium/decrypt.go":             "dde205f1b323b991ee0eef6ef73ce72e427981edcc46da3d64dc59224cf52da4",
		"internal/browsercookies/chromium/legacy_blob_other.go":   "5b7a4339a3ff0a001e63e1b6b4383c788ba84ab30527794c2c6db462cd143275",
		"internal/browsercookies/chromium/legacy_blob_windows.go": "95bb94121cad6da1451e6bb0b39e86de984debc9f8725f0dce7cbe39e32c8257",
		"internal/browsercookies/chromium/legacy_keys_unix.go":    "d37eaa8ee973929ef43f45fbd041e2fd37a4829ee26027c5e6b798100a5a69f7",
		"internal/browsercookies/chromium/linux.go":               "890b6bcc30417743625b5248a6902d868a5db2edefcde02a7437a57d24070247",
		"internal/browsercookies/chromium/darwin.go":              "7a4f1b58488b12d362734e4a6c23a3f79daa6644fa138cae14920a155cc71bd3",
		"internal/browsercookies/chromium/windows.go":             "d3d470187901a5f3f60017ab11ef9badf823d687b494873fc170992f59b442f9",
		"internal/browsercookies/secret/secret.go":                "a273cc81348c6e66e9bbc92d6152a8a657f59474e7624322b68fa0263c723798",
		"internal/browsercookies/secret/linux.go":                 "cd3005379d2328b3a0edbfe513d3fc804f75f16d0446d303527f295b8b05a627",
		"internal/browsercookies/secret/darwin.go":                "cbe58d241f11be2e7a0312ee6ddc2857338050ee3cdbc278607f78ba261a3e47",
		"internal/browsercookies/secret/windows.go":               "00cdc3350afabdc27921d7aca15c55dba0330a02bd82b6dc2c204721231ca654",
	} {
		body, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		got := fmt.Sprintf("%x", sha256.Sum256(body))
		if got != want {
			t.Fatalf("frozen source changed: %s: %s", path, got)
		}
		fixture.Sources[path] = got
	}
	add := func(id, operation string, input, output map[string]any) {
		fixture.Cases = append(fixture.Cases, migrationCryptoCase{id, operation, input, output})
	}
	key16 := []byte("0123456789abcdef")
	key24 := []byte("0123456789abcdefghijklmn")
	key32 := []byte("0123456789abcdefghijklmnopqrstuv")
	if len(key24) != 24 || len(key32) != 32 {
		t.Fatal("synthetic key length")
	}
	rawValue := []byte{0, 0xff, 0xfe, '\r', '\n', ',', '"', 0x80}
	for _, password := range [][]byte{nil, []byte("test-password"), {0, 0xff, '\r', '\n'}} {
		id := "derive-" + hex.EncodeToString(password)
		add(id, "derive_chrome_key", map[string]any{"password_hex": hex.EncodeToString(password)}, migrationCryptoBytes(deriveChromeKey(password), nil))
		keys := chromiumKeyCandidates(password)
		seen := map[string]bool{}
		for _, key := range keys {
			if seen[string(key)] {
				t.Fatal("candidate output contains duplicate")
			}
			seen[string(key)] = true
		}
		add("candidates-"+hex.EncodeToString(password), "chromium_key_candidates", map[string]any{"password_hex": hex.EncodeToString(password)}, migrationCryptoKeys(keys, nil))
	}
	for _, spec := range []struct{ iterations, length int }{{0, 16}, {-1, 16}, {1, 0}, {1, -1}, {1, 16}, {2, 41}, {1003, 16}} {
		in := map[string]any{"password_hex": hex.EncodeToString(rawValue), "salt_hex": hex.EncodeToString([]byte("saltysalt")), "iterations": spec.iterations, "key_length": spec.length}
		add(fmt.Sprintf("pbkdf2-%d-%d", spec.iterations, spec.length), "derive_pbkdf2_sha1", in, migrationCryptoBytes(derivePBKDF2SHA1(rawValue, []byte("saltysalt"), spec.iterations, spec.length), nil))
	}
	crypto := func(id, operation string, blob, key []byte) {
		var value []byte
		var err error
		switch operation {
		case "decrypt_gcm":
			value, err = decryptChromiumGCM(blob, key)
		case "decrypt_cbc":
			value, err = decryptChromiumCBC(blob, key)
		case "decrypt_legacy_value":
			value, err = decryptCookieValue(blob, key)
		default:
			value, err = decryptChromiumValue(blob, key)
		}
		add(id, operation, map[string]any{"blob_hex": hex.EncodeToString(blob), "key_hex": hex.EncodeToString(key)}, migrationCryptoBytes(value, err))
	}
	for _, key := range [][]byte{key16, key24, key32} {
		for _, prefix := range []string{"v10", "v11"} {
			blob := migrationCryptoGCM(t, prefix, key, rawValue, nil)
			crypto(fmt.Sprintf("gcm-%s-aes%d-raw", prefix, len(key)*8), "decrypt_gcm", blob, key)
			cbc := migrationCryptoCBC(t, prefix, key, rawValue, true, false)
			crypto(fmt.Sprintf("cbc-%s-aes%d-raw", prefix, len(key)*8), "decrypt_cbc", cbc, key)
		}
	}
	gcm := migrationCryptoGCM(t, "v10", key16, rawValue, nil)
	cbc := migrationCryptoCBC(t, "v11", key16, rawValue, true, false)
	crypto("value-gcm-before-cbc", "decrypt_chromium_value", gcm, key16)
	crypto("value-gcm-fails-cbc-succeeds", "decrypt_chromium_value", cbc, key16)
	crypto("gcm-empty-plaintext", "decrypt_gcm", migrationCryptoGCM(t, "v11", key32, nil, nil), key32)
	crypto("cbc-full-padding-empty-plaintext", "decrypt_cbc", migrationCryptoCBC(t, "v10", key16, nil, true, false), key16)
	for _, length := range []int{0, 15, 17, 23, 25, 31, 33} {
		invalid := bytes.Repeat([]byte{'k'}, length)
		crypto(fmt.Sprintf("gcm-invalid-key-%d", length), "decrypt_gcm", gcm, invalid)
	}
	for _, length := range []int{0, 15, 17, 31} {
		crypto(fmt.Sprintf("cbc-invalid-key-%d", length), "decrypt_cbc", cbc, bytes.Repeat([]byte{'k'}, length))
	}
	cbc32 := migrationCryptoCBC(t, "v10", key32, rawValue, true, false)
	crypto("cbc-key-33-truncated-to-32", "decrypt_cbc", cbc32, append(append([]byte(nil), key32...), 0xff))
	crypto("cbc-key-48-truncated-to-32", "decrypt_cbc", cbc32, append(append([]byte(nil), key32...), bytes.Repeat([]byte{0xff}, 16)...))
	corrupt := append([]byte(nil), gcm...)
	corrupt[len(corrupt)-1] ^= 1
	crypto("gcm-tag-corruption", "decrypt_gcm", corrupt, key16)
	corruptNonce := append([]byte(nil), gcm...)
	corruptNonce[3] ^= 1
	crypto("gcm-nonce-corruption", "decrypt_gcm", corruptNonce, key16)
	crypto("gcm-rejects-aad-bound-ciphertext", "decrypt_gcm", migrationCryptoGCM(t, "v10", key16, rawValue, []byte("synthetic-aad")), key16)
	for _, blob := range [][]byte{nil, []byte("v10"), append([]byte("v11"), make([]byte, 27)...), append([]byte("v10"), make([]byte, 15)...), append([]byte("v10"), make([]byte, 17)...), append([]byte("v20"), gcm[3:]...), append([]byte("V10"), gcm[3:]...)} {
		crypto("value-layout-"+hex.EncodeToString(blob), "decrypt_chromium_value", blob, key16)
	}
	for _, plain := range [][]byte{nil, {1, 2, 3}, make([]byte, 16), append(bytes.Repeat([]byte{'x'}, 15), 17), append(bytes.Repeat([]byte{'x'}, 15), 2), bytes.Repeat([]byte{16}, 16), append(bytes.Repeat([]byte{'x'}, 15), 1)} {
		value, err := unpadCBC(plain)
		add("unpad-"+hex.EncodeToString(plain), "unpad_cbc", map[string]any{"plain_hex": hex.EncodeToString(plain)}, migrationCryptoBytes(value, err))
		if len(plain) == 16 {
			crypto("cbc-padding-"+hex.EncodeToString(plain), "decrypt_cbc", migrationCryptoCBC(t, "v10", key16, plain, false, false), key16)
		}
	}
	legacyPlain := append(append([]byte("v11"), bytes.Repeat([]byte{0x82}, 29)...), bytes.Repeat([]byte{0xff}, 16)...)
	legacy := migrationCryptoCBC(t, "", key16, legacyPlain, false, true)
	crypto("legacy-zero-iv-no-unpad", "decrypt_legacy_value", legacy, key16)
	crypto("legacy-key-over-16-truncated", "decrypt_legacy_value", legacy, append(append([]byte(nil), key16...), rawValue...))
	crypto("legacy-key-short", "decrypt_legacy_value", legacy, key16[:15])
	for _, blob := range [][]byte{nil, {1, 2, 3}, make([]byte, 16), make([]byte, 31)} {
		crypto("legacy-length-"+strconv.Itoa(len(blob)), "decrypt_legacy_value", blob, key16)
		add("legacy-supported-"+strconv.Itoa(len(blob)), "legacy_blob_supported", map[string]any{"blob_hex": hex.EncodeToString(blob)}, map[string]any{"supported": legacyBlobSupported(blob)})
	}
	add("legacy-supported-32", "legacy_blob_supported", map[string]any{"blob_hex": hex.EncodeToString(legacy[:32])}, map[string]any{"supported": legacyBlobSupported(legacy[:32])})
	for _, plain := range [][]byte{nil, []byte("v10"), make([]byte, 32), legacyPlain, append([]byte("v20"), legacyPlain[3:]...)} {
		value, err := stripChromiumPrefix(plain)
		add("legacy-prefix-"+hex.EncodeToString(plain), "strip_legacy_prefix", map[string]any{"plain_hex": hex.EncodeToString(plain)}, migrationCryptoBytes(value, err))
	}
	host := ".fanbox.cc"
	digest := sha256.Sum256([]byte(host))
	withDigest := append(append([]byte(nil), digest[:]...), rawValue...)
	for _, spec := range []struct {
		id, host string
		plain    []byte
	}{{"matching", host, withDigest}, {"exact-host-dot-mismatch", "fanbox.cc", withDigest}, {"short", host, rawValue}, {"only-digest", host, digest[:]}, {"unrelated-prefix", host, bytes.Repeat([]byte{0xff}, 40)}} {
		add("host-digest-"+spec.id, "strip_host_digest", map[string]any{"host_hex": hex.EncodeToString([]byte(spec.host)), "plain_hex": hex.EncodeToString(spec.plain)}, migrationCryptoBytes(stripChromiumHostDigest(spec.plain, spec.host), nil))
	}
	for _, spec := range []struct{ id, mode, body string }{
		{"missing", "missing", ""}, {"directory", "directory", ""}, {"malformed", "file", "{"}, {"top-null", "file", "null"}, {"top-array", "file", "[]"}, {"absent", "file", "{}"}, {"null-crypt", "file", `{"os_crypt":null}`}, {"empty", "file", `{"os_crypt":{"encrypted_key":""}}`}, {"whitespace", "file", `{"os_crypt":{"encrypted_key":" \t\r\n\u00a0"}}`}, {"padded-trimmed", "file", `{"os_crypt":{"encrypted_key":" \tAAH/\r\n"}}`}, {"raw-unpadded", "file", `{"os_crypt":{"encrypted_key":"AAE"}}`}, {"padded", "file", `{"os_crypt":{"encrypted_key":"AAE="}}`}, {"line-break-in-base64", "file", `{"os_crypt":{"encrypted_key":"AA\nE="}}`}, {"space-in-base64", "file", `{"os_crypt":{"encrypted_key":"AA E="}}`}, {"url-base64-rejected", "file", `{"os_crypt":{"encrypted_key":"_w=="}}`}, {"invalid-base64", "file", `{"os_crypt":{"encrypted_key":"!!!!"}}`}, {"noncanonical-pad-bits", "file", `{"os_crypt":{"encrypted_key":"AB=="}}`}, {"wrong-value-type", "file", `{"os_crypt":{"encrypted_key":1}}`}, {"duplicate-last-value", "file", `{"os_crypt":{"encrypted_key":"AAE=","encrypted_key":"AgM="}}`},
	} {
		p := &provider{root: migrationCryptoWriteState(t, spec.mode, []byte(spec.body))}
		value, present, err := p.localStateEncryptedKey()
		out := migrationCryptoBytes(value, err)
		out["present"] = present
		add("local-state-"+spec.id, "local_state_encrypted_key", map[string]any{"state_mode": spec.mode, "state_body_hex": hex.EncodeToString([]byte(spec.body))}, out)
	}
	add("local-state-permission-denied", "local_state_encrypted_key", map[string]any{"state_mode": "permission_denied", "state_body_hex": hex.EncodeToString([]byte("{}"))}, migrationCryptoStatePermission(t))
	migrationCryptoProviderCases(t, &fixture, key16, key32, rawValue, withDigest, legacy)
	migrationCryptoLinuxCases(t, &fixture, key16, rawValue)
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	fixturePath := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "browser-chromium-crypto.json")
	if os.Getenv("PIXIV_CAPTURE_BROWSER_CHROMIUM_CRYPTO") == "1" {
		if err := os.WriteFile(fixturePath, body, 0o600); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d real Go Chromium crypto cases", len(fixture.Cases))
		return
	}
	want, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(body, want) {
		actual := filepath.Join(t.TempDir(), "browser-chromium-crypto-actual.json")
		_ = os.WriteFile(actual, body, 0o600)
		t.Fatalf("Chromium crypto contract changed; actual at %s", actual)
	}
	t.Logf("replayed %d real Go Chromium crypto cases", len(fixture.Cases))
}

func migrationCryptoProviderCases(t *testing.T, fixture *migrationCryptoFixture, key16, key32, rawValue, withDigest, legacy []byte) {
	t.Helper()
	add := func(id, operation string, input, output map[string]any) {
		fixture.Cases = append(fixture.Cases, migrationCryptoCase{id, operation, input, output})
	}
	gcm := migrationCryptoGCM(t, "v10", key16, rawValue, nil)
	for _, spec := range []struct {
		id, contextMode, keyError string
		blob                      []byte
		keys                      [][]byte
		cancelInside              bool
	}{
		{"candidate-second-succeeds", "active", "", gcm, [][]byte{key32, key16}, false},
		{"duplicate-candidates-kept-by-override", "active", "", gcm, [][]byte{key32, key32, key16, key16}, false},
		{"empty-candidates", "active", "", gcm, nil, false},
		{"key-error-before-malformed", "active", "access", []byte("v10"), nil, false},
		{"short-legacy-before-key-acquisition", "active", "access", []byte("v20"), nil, false},
		{"legacy-valid", "active", "", legacy, [][]byte{key32, key16}, false},
		{"legacy-exhausted", "active", "", make([]byte, 32), [][]byte{key16}, false},
		{"cancel-before-format", "canceled", "access", nil, nil, false},
		{"deadline-before-format", "deadline", "access", []byte("v10"), nil, false},
		{"cancel-inside-key-hook-not-polled", "active", "", gcm, [][]byte{key16}, true},
	} {
		ctx, cancel := migrationCryptoContext(spec.contextMode)
		calls := 0
		p := &provider{encryptionKeyOverride: func(context.Context) ([][]byte, error) {
			calls++
			if spec.cancelInside {
				cancel()
			}
			if spec.keyError != "" {
				return nil, browsercookies.ErrSecretServiceAccess
			}
			return spec.keys, nil
		}}
		value, err := p.decryptEncrypted(ctx, spec.blob)
		cancel()
		in := map[string]any{"blob_hex": hex.EncodeToString(spec.blob), "keys_hex": migrationCryptoKeys(spec.keys, nil)["keys_hex"], "context": spec.contextMode, "key_error": spec.keyError, "cancel_inside_key_hook": spec.cancelInside}
		out := migrationCryptoBytes(value, err)
		out["key_calls"] = calls
		add("provider-"+spec.id, "decrypt_encrypted", in, out)
	}
	withHost := migrationCryptoGCM(t, "v11", key16, withDigest, nil)
	for _, spec := range []struct {
		id, contextMode string
		rows            [][]string
	}{
		{"plain-raw-no-keys", "active", [][]string{{".fanbox.cc", string(rawValue), ""}}},
		{"plain-canceled-still-returned", "canceled", [][]string{{".fanbox.cc", string(rawValue), " \t"}}},
		{"encrypted-overrides-plaintext", "active", [][]string{{".fanbox.cc", "synthetic-unused-plaintext", hex.EncodeToString(withHost)}}},
		{"row-host-exact-digest-mismatch", "active", [][]string{{"fanbox.cc", "", hex.EncodeToString(withHost)}}},
		{"encrypted-trims-hex-whitespace", "active", [][]string{{".fanbox.cc", "", " \t" + hex.EncodeToString(gcm) + "\r\n"}}},
		{"malformed-hex-before-cancel", "canceled", [][]string{{".fanbox.cc", "", "zz"}}},
		{"encrypted-cancel-before-key", "canceled", [][]string{{".fanbox.cc", "", hex.EncodeToString(gcm)}}},
		{"short-row-before-cancel", "canceled", [][]string{{".fanbox.cc", "plain"}}},
		{"extra-column-ignored", "active", [][]string{{".fanbox.cc", string(rawValue), "", "extra"}}},
		{"partial-on-later-error", "active", [][]string{{".fanbox.cc", string(rawValue), ""}, {".fanbox.cc", "", "z"}}},
		{"per-row-key-acquisition", "active", [][]string{{".fanbox.cc", "", hex.EncodeToString(gcm)}, {".fanbox.cc", "", hex.EncodeToString(gcm)}}},
		{"empty-row-set", "canceled", nil},
	} {
		ctx, cancel := migrationCryptoContext(spec.contextMode)
		calls := 0
		p := &provider{encryptionKeyOverride: func(context.Context) ([][]byte, error) { calls++; return [][]byte{key16}, nil }}
		snapshot, err := p.rowsToSnapshot(ctx, browsercookies.DefaultQuery, "Profile 1", spec.rows)
		cancel()
		values := []string{}
		redacted := []string{}
		for _, cookie := range snapshot.Cookies {
			values = append(values, hex.EncodeToString([]byte(cookie.Value.Value())))
			redacted = append(redacted, cookie.Value.String())
		}
		rows := make([][]string, 0, len(spec.rows))
		for _, row := range spec.rows {
			encoded := []string{}
			for _, field := range row {
				encoded = append(encoded, hex.EncodeToString([]byte(field)))
			}
			rows = append(rows, encoded)
		}
		out := migrationCryptoError(err)
		out["cookie_values_hex"], out["secret_strings"], out["profile_id"], out["key_calls"] = values, redacted, snapshot.ProfileID, calls
		add("rows-"+spec.id, "rows_to_snapshot", map[string]any{"rows_hex": rows, "keys_hex": []string{hex.EncodeToString(key16)}, "context": spec.contextMode, "profile_id": "Profile 1", "query_host": ".fanbox.cc", "query_name": "FANBOXSESSID"}, out)
	}
}

func migrationCryptoLinuxCases(t *testing.T, fixture *migrationCryptoFixture, key16, rawValue []byte) {
	t.Helper()
	add := func(id, operation string, input, output map[string]any) {
		fixture.Cases = append(fixture.Cases, migrationCryptoCase{id, operation, input, output})
	}
	for _, spec := range []struct {
		id, application, mode, contextMode string
		password                           []byte
	}{
		{"chrome-raw-trailing-crlf", "chrome", "success", "active", append(append([]byte(nil), rawValue...), '\r', '\n', '\r', '\n')},
		{"edge-spaces-preserved", "microsoft-edge", "success", "active", []byte(" synthetic-password \t\r\n")},
		{"trim-validation-keeps-original-argument", " chrome ", "success", "active", []byte("password\n")},
		{"missing-command", "chrome", "missing", "active", nil},
		{"failure-redacted", "chrome", "fail", "active", nil},
		{"empty-output", "chrome", "success", "active", nil},
		{"crlf-only-empty", "chrome", "success", "active", []byte("\r\n\r\n")},
		{"invalid-application-before-command", "unknown", "missing", "active", nil},
		{"pre-cancel-before-invalid-application", "unknown", "missing", "canceled", nil},
		{"expired-deadline-before-command", "chrome", "missing", "deadline", nil},
		{"cancel-running-command", "chrome", "wait", "active", nil},
	} {
		trace, ready := migrationCryptoSecretTool(t, spec.mode, spec.password)
		ctx, cancel := migrationCryptoContext(spec.contextMode)
		var value []byte
		var err error
		if spec.mode == "wait" {
			done := make(chan struct{})
			go func() { value, err = (secret.SecretService{}).GetPassword(ctx, spec.application); close(done) }()
			migrationCryptoWaitAndCancel(t, ready, cancel)
			<-done
		} else {
			value, err = (secret.SecretService{}).GetPassword(ctx, spec.application)
		}
		cancel()
		out := migrationCryptoBytes(value, err)
		out["command_args"] = migrationCryptoReadTrace(t, trace)
		add("linux-secret-"+spec.id, "linux_secret_password", map[string]any{"application": spec.application, "secret_mode": spec.mode, "password_hex": hex.EncodeToString(spec.password), "context": spec.contextMode}, out)
	}
	password := []byte("synthetic-password")
	legacyKeys := chromiumKeyCandidates(password)
	stateRaw := migrationCryptoStateBody(t, key16)
	stateWrappedSecond := migrationCryptoStateBody(t, migrationCryptoGCM(t, "v11", legacyKeys[1], key16, nil))
	stateWrappedEmpty := migrationCryptoStateBody(t, migrationCryptoGCM(t, "v10", legacyKeys[0], nil, nil))
	stateCBC := migrationCryptoStateBody(t, migrationCryptoCBC(t, "v10", legacyKeys[1], key16, true, false))
	for _, spec := range []struct {
		id, browser, stateMode, secretMode, contextMode string
		body                                            []byte
	}{
		{"chrome-missing-state-legacy-candidates", "chrome", "missing", "success", "active", nil},
		{"edge-missing-state-legacy-candidates", "edge", "missing", "success", "active", nil},
		{"raw-key", "chrome", "file", "success", "active", stateRaw},
		{"raw-unrecognized-state-version", "chrome", "file", "success", "active", migrationCryptoStateBody(t, []byte("v20"))},
		{"wrapped-key-second-candidate", "chrome", "file", "success", "active", stateWrappedSecond},
		{"wrapped-empty-key-rejected", "chrome", "file", "success", "active", stateWrappedEmpty},
		{"wrapped-cbc-not-unwrapped", "chrome", "file", "success", "active", stateCBC},
		{"dpapi-key-format-unknown", "chrome", "file", "success", "active", migrationCryptoStateBody(t, []byte("DPAPIblob"))},
		{"malformed-state-after-secret-success", "chrome", "file", "success", "active", []byte("{")},
		{"missing-secret-before-malformed-state", "chrome", "file", "missing", "active", []byte("{")},
		{"failing-secret-before-malformed-state", "edge", "file", "fail", "active", []byte("{")},
		{"pre-cancel-before-state", "chrome", "file", "success", "canceled", stateRaw},
		{"cancel-running-secret", "chrome", "file", "wait", "active", stateRaw},
	} {
		trace, ready := migrationCryptoSecretTool(t, spec.secretMode, append(append([]byte(nil), password...), '\r', '\n'))
		p, err := newProvider(spec.browser, migrationCryptoWriteState(t, spec.stateMode, spec.body))
		if err != nil {
			t.Fatal(err)
		}
		ctx, cancel := migrationCryptoContext(spec.contextMode)
		var keys [][]byte
		if spec.secretMode == "wait" {
			done := make(chan struct{})
			go func() { keys, err = p.encryptionKeys(ctx); close(done) }()
			migrationCryptoWaitAndCancel(t, ready, cancel)
			<-done
		} else {
			keys, err = p.encryptionKeys(ctx)
		}
		cancel()
		out := migrationCryptoKeys(keys, err)
		out["command_args"] = migrationCryptoReadTrace(t, trace)
		add("linux-keys-"+spec.id, "linux_encryption_keys", map[string]any{"browser": spec.browser, "state_mode": spec.stateMode, "state_body_hex": hex.EncodeToString(spec.body), "secret_mode": spec.secretMode, "password_hex": hex.EncodeToString(append(append([]byte(nil), password...), '\r', '\n')), "context": spec.contextMode}, out)
	}
	for _, err := range []error{secret.ErrNotAvailableOnBuild, secret.ErrItemNotFound, secret.ErrEmptyPassword, secret.ErrInvalidItem, secret.ErrSecretService, context.Canceled, context.DeadlineExceeded} {
		add("linux-map-"+strings.ReplaceAll(err.Error(), " ", "-"), "map_linux_secret_error", map[string]any{"error": err.Error()}, migrationCryptoError(mapSecretError(err)))
	}
}

func migrationCryptoStatePermission(t *testing.T) map[string]any {
	t.Helper()
	root := migrationCryptoWriteState(t, "file", []byte("{}"))
	path := filepath.Join(root, "Local State")
	if err := os.Chmod(path, 0); err != nil {
		t.Fatal(err)
	}
	defer func() { _ = os.Chmod(path, 0o600) }()
	if os.Geteuid() != 0 {
		value, present, err := (&provider{root: root}).localStateEncryptedKey()
		out := migrationCryptoBytes(value, err)
		out["present"] = present
		return out
	}
	for _, dir := range []string{root, filepath.Dir(root)} {
		if err := os.Chmod(dir, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile(executable)
	if err != nil {
		t.Fatal(err)
	}
	helper := filepath.Join(root, "owned-go-test-helper")
	if err := os.WriteFile(helper, binary, 0o755); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(helper, "-test.run=^TestMigrationChromiumCryptoCommandHelper$")
	cmd.Env = append(os.Environ(), "PIXIV_MIGRATION_CRYPTO_HELPER=1", "PIXIV_MIGRATION_CRYPTO_MODE=state-permission", "PIXIV_MIGRATION_CRYPTO_STATE_ROOT="+root)
	cmd.SysProcAttr = &syscall.SysProcAttr{Credential: &syscall.Credential{Uid: 65534, Gid: 65534}}
	body, err := cmd.Output()
	if err != nil {
		t.Fatalf("owned unprivileged Local State helper: %v", err)
	}
	out := map[string]any{}
	if err := json.Unmarshal(body, &out); err != nil {
		t.Fatal(err)
	}
	if out["class"] != "permission_denied" {
		t.Fatalf("owned Local State permission case: %#v", out)
	}
	return out
}

package fanbox_test

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxIdentityJSONBytes = flag.Bool("migration-capture-fanbox-identity-json-bytes", false, "capture frozen Go public FANBOX identity byte and depth contracts")

type migrationFanboxJSONBytesStep struct {
	Status     int    `json:"status"`
	BodyHex    string `json:"body_hex"`
	BodyLength int    `json:"body_length"`
	BodySHA256 string `json:"body_sha256"`
	ReadError  string `json:"read_error,omitempty"`
	CloseError string `json:"close_error,omitempty"`
}

type migrationFanboxJSONBytesInput struct {
	Operation          string                         `json:"operation"`
	JSONContainerDepth int                            `json:"json_container_depth,omitempty"`
	Steps              []migrationFanboxJSONBytesStep `json:"steps"`
}

type migrationFanboxJSONBytesCase struct {
	Name        string                        `json:"name"`
	Input       migrationFanboxJSONBytesInput `json:"input"`
	Observation map[string]any                `json:"observation"`
}

type migrationFanboxJSONBytesBody struct {
	reader   *bytes.Reader
	step     migrationFanboxJSONBytesStep
	Injected bool     `json:"injected_body"`
	Reads    int      `json:"go_only_read_calls"`
	Bytes    int      `json:"bytes_read"`
	Closes   int      `json:"close_calls"`
	Events   []string `json:"go_only_events"`
}

func (b *migrationFanboxJSONBytesBody) Read(p []byte) (int, error) {
	b.Reads++
	b.Events = append(b.Events, "read")
	n, err := b.reader.Read(p)
	b.Bytes += n
	if b.step.ReadError != "" && b.reader.Len() == 0 {
		return n, migrationFanboxInjectedError(b.step.ReadError)
	}
	return n, err
}

func (b *migrationFanboxJSONBytesBody) Close() error {
	b.Closes++
	b.Events = append(b.Events, "close")
	return migrationFanboxInjectedError(b.step.CloseError)
}

type migrationFanboxJSONBytesTransport struct {
	t         *testing.T
	step      migrationFanboxJSONBytesStep
	requests  []map[string]any
	bodies    []*migrationFanboxJSONBytesBody
	idleCalls int
}

func (r *migrationFanboxJSONBytesTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	if len(r.requests) != 0 {
		r.t.Fatalf("unexpected additional identity request to %s", req.URL)
	}
	headers := req.Header.Clone()
	headers.Set("Cookie", migrationFanboxCookieProjection(headers.Get("Cookie")))
	_, deadline := req.Context().Deadline()
	r.requests = append(r.requests, map[string]any{
		"method": req.Method, "url": req.URL.String(), "headers": headers,
		"context_value":    req.Context().Value(migrationFanboxContextKey{}),
		"context_canceled": req.Context().Err() != nil, "context_has_deadline": deadline,
		"request_has_body": req.Body != nil,
	})
	data := migrationFanboxJSONBytesDecode(r.t, r.step)
	body := &migrationFanboxJSONBytesBody{reader: bytes.NewReader(data), step: r.step, Injected: true, Events: []string{}}
	r.bodies = append(r.bodies, body)
	return &http.Response{StatusCode: r.step.Status, Header: make(http.Header), Body: body, Request: req}, nil
}

func (r *migrationFanboxJSONBytesTransport) CloseIdleConnections() { r.idleCalls++ }

func migrationFanboxJSONBytesDecode(t *testing.T, step migrationFanboxJSONBytesStep) []byte {
	t.Helper()
	data, err := hex.DecodeString(step.BodyHex)
	if err != nil {
		t.Fatal(err)
	}
	if hex.EncodeToString(data) != step.BodyHex || len(data) != step.BodyLength || fmt.Sprintf("%x", sha256.Sum256(data)) != step.BodySHA256 {
		t.Fatal("hex response input did not preserve its exact source bytes")
	}
	return data
}

func migrationFanboxObserveJSONBytes(t *testing.T, input migrationFanboxJSONBytesInput) map[string]any {
	t.Helper()
	if input.Operation != "sdk_current_user" || len(input.Steps) != 1 {
		t.Fatal("byte supplement is limited to one public identity response")
	}
	ctx := context.WithValue(context.Background(), migrationFanboxContextKey{}, "synthetic-context")
	transport := &migrationFanboxJSONBytesTransport{t: t, step: input.Steps[0], requests: []map[string]any{}, bodies: []*migrationFanboxJSONBytesBody{}}
	jar := &migrationFanboxJar{}
	redirectCalls := 0
	client := &http.Client{Transport: transport, Jar: jar, CheckRedirect: func(*http.Request, []*http.Request) error {
		redirectCalls++
		return errors.New("synthetic caller redirect rejection")
	}}
	sdkClient, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationFanboxSessionValue}, fanbox.Options{HTTPClient: client})
	result := map[string]any{"constructed": sdkClient != nil, "constructor_error": migrationFanboxPublicHTMLError(err)}
	if err != nil {
		t.Fatal(err)
	}
	user, currentErr := sdkClient.CurrentUser(ctx, fanbox.CurrentUserRequest{})
	result["outcomes"] = []map[string]any{{"dto": fanbox.ToUserDTO(user), "error": migrationFanboxPublicHTMLError(currentErr)}}
	sdkClient.CloseIdleConnections()
	sdkClient.CloseIdleConnections()
	result["requests"] = transport.requests
	result["bodies"] = transport.bodies
	result["close_idle_calls"] = transport.idleCalls
	result["jar_calls"] = jar.calls
	result["redirect_callback_calls"] = redirectCalls
	result["go_only_injected_client_unchanged"] = client.Transport == transport && client.Jar == jar && client.CheckRedirect != nil && client.Timeout == 0
	if len(transport.requests) != 1 || len(transport.bodies) != 1 || transport.bodies[0].Bytes != input.Steps[0].BodyLength || transport.bodies[0].Closes != 1 {
		t.Fatal("public identity response byte consumption or body ownership changed")
	}
	return result
}

func migrationFanboxJSONBytesDocument(metadata []byte) []byte {
	content := append([]byte(nil), metadata...)
	for _, escape := range []struct{ from, to string }{{"&", "&amp;"}, {"'", "&#39;"}, {"<", "&lt;"}, {">", "&gt;"}} {
		content = bytes.ReplaceAll(content, []byte(escape.from), []byte(escape.to))
	}
	document := append([]byte(`<meta name='metadata' content='`), content...)
	return append(document, []byte(`'>`)...)
}

func migrationFanboxJSONBytesDepth(metadata []byte) int {
	depth, maximum := 0, 0
	quoted, escaped := false, false
	for _, b := range metadata {
		if quoted {
			if escaped {
				escaped = false
			} else if b == '\\' {
				escaped = true
			} else if b == '"' {
				quoted = false
			}
			continue
		}
		switch b {
		case '"':
			quoted = true
		case '{', '[':
			depth++
			if depth > maximum {
				maximum = depth
			}
		case '}', ']':
			depth--
		}
	}
	return maximum
}

func migrationFanboxJSONBytesNested(kind string, count int, leaf string) string {
	var opening, closing strings.Builder
	for index := 0; index < count; index++ {
		if kind == "object" || kind == "mixed" && index%2 == 1 {
			opening.WriteString(`{"":`)
			closing.WriteByte('}')
		} else {
			opening.WriteByte('[')
			closing.WriteByte(']')
		}
	}
	end := []byte(closing.String())
	for left, right := 0, len(end)-1; left < right; left, right = left+1, right-1 {
		end[left], end[right] = end[right], end[left]
	}
	return opening.String() + leaf + string(end)
}

func migrationFanboxIdentityJSONBytesCases(t *testing.T) []migrationFanboxJSONBytesCase {
	t.Helper()
	rows := []migrationFanboxJSONBytesCase{}
	addDocument := func(name string, document []byte, depth int, readError, closeError string) {
		rows = append(rows, migrationFanboxJSONBytesCase{Name: name, Input: migrationFanboxJSONBytesInput{
			Operation: "sdk_current_user", JSONContainerDepth: depth,
			Steps: []migrationFanboxJSONBytesStep{{Status: http.StatusOK, BodyHex: hex.EncodeToString(document), BodyLength: len(document), BodySHA256: fmt.Sprintf("%x", sha256.Sum256(document)), ReadError: readError, CloseError: closeError}},
		}})
	}
	add := func(name string, metadata []byte, depth int, readError, closeError string) {
		if depth != 0 && migrationFanboxJSONBytesDepth(metadata) != depth {
			t.Fatalf("%s synthetic container accounting differs: %d", name, migrationFanboxJSONBytesDepth(metadata))
		}
		addDocument(name, migrationFanboxJSONBytesDocument(metadata), depth, readError, closeError)
	}
	nameMetadata := func(value string) []byte {
		return []byte(`{"context":{"user":{"userId":42,"name":` + value + `}}}`)
	}
	for _, sample := range []struct{ name, value string }{
		{"display_high", `"n\uD800"`},
		{"display_low", `"n\uDC00"`},
		{"display_pair", `"n\uD83D\uDE80"`},
		{"display_mixed", `"A\uD800B\uDC00C\uD83D\uDE80D"`},
		{"display_reversed_pair", `"n\uDE80\uD83D"`},
		{"display_high_then_bmp", `"n\uD800\u0041"`},
		{"display_high_high_low", `"n\uD800\uD83D\uDE80"`},
		{"display_literal_escape", `"n\\uD800"`},
	} {
		add("surrogate/"+sample.name, nameMetadata(sample.value), 0, "", "")
	}
	add("surrogate/unknown_values_and_keys", []byte(`{"context":{"user":{"userId":42,"name":"n","unknown":"\uD800\uDC00\uD800","low":"\uDC00","pair":"\uD83D\uDE80","\uD800":"\uDC00","\uDC00":1,"\uD83D\uDE80":true}}}`), 0, "", "")
	add("surrogate/escaped_known_keys_and_creator_fields", []byte(`{"\u0063ontext":{"\u0075ser":{"userId":42,"\u006e\u0061\u006d\u0065":"n","creatorId":"c\uD800","creatorStatus":"s\uDC00\uD83D\uDE80"}}}`), 0, "", "")
	add("surrogate/member_suffix_is_unknown", []byte(`{"context":{"user":{"userId":42,"name\uD800":"n"}}}`), 0, "", "")
	for _, sample := range []struct{ name, value string }{
		{"unicode_no_digits", `"n\u"`},
		{"unicode_short", `"n\u12"`},
		{"unicode_nonhex", `"n\u12G4"`},
		{"unicode_uppercase_escape", `"n\U0001"`},
		{"unicode_bad_second_escape", `"n\uD800\uZZZZ"`},
	} {
		add("syntax/"+sample.name, nameMetadata(sample.value), 0, "", "")
	}
	add("syntax/unknown_bad_escape", []byte(`{"context":{"user":{"userId":42,"name":"n","unknown":"\uD800\uZZZZ"}}}`), 0, "", "")
	add("syntax/member_bad_escape", []byte(`{"context":{"user":{"userId":42,"name":"n","\u12G4":1}}}`), 0, "", "")
	for _, sample := range []struct {
		name  string
		bytes []byte
	}{
		{"isolated", []byte{0x80}},
		{"overlong", []byte{0xc0, 0xaf}},
		{"surrogate_scalar", []byte{0xed, 0xa0, 0x80}},
		{"truncated_two_byte", []byte{0xc2}},
		{"truncated_three_byte", []byte{0xe2, 0x82}},
		{"truncated_four_byte", []byte{0xf0, 0x9f, 0x92}},
		{"malformed_continuation", []byte{0xe2, 0x28, 0xa1}},
		{"above_unicode_maximum", []byte{0xf4, 0x90, 0x80, 0x80}},
		{"valid_multibyte", []byte("日本語🚀é")},
	} {
		value := append([]byte(`"n`), sample.bytes...)
		value = append(value, '"')
		add("utf8/"+sample.name, nameMetadata(string(value)), 0, "", "")
	}
	unknown := append([]byte(`{"context":{"user":{"userId":42,"name":"n","unknown":"`), 0xe2, 0x82)
	unknown = append(unknown, []byte(`","`)...)
	unknown = append(unknown, 0xed, 0xa0, 0x80)
	unknown = append(unknown, []byte(`":1}}}`)...)
	add("utf8/unknown_values_and_keys", unknown, 0, "", "")
	outside := append([]byte(`{"context":{"user":{"userId":42,"name":"n"}},"unknown":`), 0x80)
	outside = append(outside, '}')
	add("utf8/outside_string_is_syntax_error", outside, 0, "", "")
	for _, sample := range []struct{ name, document string }{
		{"entity_encoded_json_escape", `<meta name='metadata' content='{"context":{"user":{"userId":42,"name":"n&#92;uD800"}}}'>`},
		{"numeric_surrogate_entity", `<meta name='metadata' content='{"context":{"user":{"userId":42,"name":"n&#xD800;"}}}'>`},
	} {
		addDocument("html_preprocessing/"+sample.name, []byte(sample.document), 0, "", "")
	}
	nul := append([]byte(`"n`), 0)
	nul = append(nul, '"')
	add("html_preprocessing/raw_nul_in_attribute", nameMetadata(string(nul)), 0, "", "")
	invalidPrefix := append([]byte{0xe2, 0x82}, migrationFanboxJSONBytesDocument(nameMetadata(`"n"`))...)
	addDocument("html_preprocessing/invalid_utf8_outside_metadata", invalidPrefix, 0, "", "")
	for _, kind := range []string{"array", "object", "mixed"} {
		for _, depth := range []int{10000, 10001} {
			metadata := `{"context":{"user":{"userId":42,"name":"n"}},"ignored":` + migrationFanboxJSONBytesNested(kind, depth-1, `0`) + `}`
			add(fmt.Sprintf("depth/root_%s_%d", kind, depth), []byte(metadata), depth, "", "")
		}
	}
	for _, depth := range []int{10000, 10001} {
		metadata := `{"context":{"user":{"userId":42,"name":"n","ignored":` + migrationFanboxJSONBytesNested("array", depth-3, `0`) + `}}}`
		add(fmt.Sprintf("depth/user_array_%d", depth), []byte(metadata), depth, "", "")
	}
	metadata := `{"context":{"user":{"userId":42,"name":"n"}},"ignored":` + migrationFanboxJSONBytesNested("mixed", 9999, `"[{\"}]]\\["`) + `}`
	add("depth/string_delimiters_ignored_10000", []byte(metadata), 10000, "", "")
	malformed := nameMetadata(`"n\uD800\uZZZZ"`)
	add("precedence/malformed_read_failure", malformed, 0, "raw", "")
	add("precedence/malformed_close_failure", malformed, 0, "", "raw")
	add("precedence/malformed_read_and_close_failure", malformed, 0, "raw", "raw")
	overdepth := `{"context":{"user":{"userId":42,"name":"n"}},"ignored":` + migrationFanboxJSONBytesNested("array", 10000, `0`) + `}`
	add("precedence/overdepth_close_failure", []byte(overdepth), 10001, "", "raw")
	add("precedence/late_syntax_before_invalid_user_id", []byte(`{"context":{"user":{"userId":0,"name":"n\uD800","unknown":"\uZZZZ"}}}`), 0, "", "")
	return rows
}

func migrationFanboxJSONBytesFrozenProduction(t *testing.T, root, reference string) map[string]any {
	t.Helper()
	listing, err := exec.Command("git", "-C", root, "ls-tree", "-r", "--name-only", reference).Output()
	if err != nil {
		t.Fatal(err)
	}
	paths := []string{}
	var input strings.Builder
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if strings.HasSuffix(path, ".go") && !strings.HasSuffix(path, "_test.go") || path == "go.mod" || path == "go.sum" {
			paths = append(paths, path)
			input.WriteString(reference + ":" + path + "\n")
		}
	}
	if len(paths) != 434 {
		t.Fatalf("frozen Go production/module inventory changed: %d", len(paths))
	}
	command := exec.Command("git", "-C", root, "cat-file", "--batch")
	command.Stdin = strings.NewReader(input.String())
	batch, err := command.Output()
	if err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(bytes.NewReader(batch))
	hashes := map[string]string{}
	for _, path := range paths {
		header, err := reader.ReadString('\n')
		if err != nil {
			t.Fatal(err)
		}
		fields := strings.Fields(header)
		if len(fields) != 3 || fields[1] != "blob" {
			t.Fatalf("invalid frozen source object header: %s", header)
		}
		size, err := strconv.Atoi(fields[2])
		if err != nil {
			t.Fatal(err)
		}
		original := make([]byte, size)
		if _, err := io.ReadFull(reader, original); err != nil {
			t.Fatal(err)
		}
		if separator, err := reader.ReadByte(); err != nil || separator != '\n' {
			t.Fatal("invalid frozen source object separator")
		}
		current, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, current) {
			t.Fatalf("Go production/module source differs from frozen commit: %s", path)
		}
		hashes[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	encoded, err := json.Marshal(hashes)
	if err != nil {
		t.Fatal(err)
	}
	return map[string]any{"path_count": len(paths), "byte_equal_to_frozen_commit": true, "path_sha256_map_json_sha256": fmt.Sprintf("%x", sha256.Sum256(encoded))}
}

func TestMigrationFanboxIdentityJSONBytesFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	frozenProduction := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	stdlib := map[string]string{
		"encoding/json/decode.go":  "1632161a34c8286722716a48ba0b5e0c3d117a2e017e79ace676403808a16e6e",
		"encoding/json/scanner.go": "2b16dd215274dfa8e0806b53cca144684d5b9c8a02319cf5ce10120c50e6eef2",
	}
	for path, want := range stdlib {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("frozen Go JSON source changed: %s", path)
		}
	}
	protected := map[string]string{
		"crates/pixiv-sdk/tests/fixtures/fanbox-context-ownership.json":              "6c0d721eeb35314058c8f928114a9ea2c6bf76e92d468502fc543fd7651cc705",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-protocol.json":              "cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-public-html.json":           "fe0eee2fc4f5d08c717a223f0df043c003458c1893fe55e95f66ac4df043e388",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-tokenizer-regressions.json": "732d71a38a98407dbf874586c3f77412d4b34006ac916590e5593321da79129a",
		"crates/pixiv-sdk/tests/fixtures/fanbox-media-body.json":                     "a3b48317ef6f4dba2c5ca6d6f5d935467eee273147b88ebeff776b286f60bce2",
		"crates/pixiv-sdk/tests/fixtures/fanbox-native-profile.json":                 "df753a715efd28c016a2532fe544e992b00a1fe5ce58b756d807c13b9522dd62",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-public-aliases.json":          "81e88cc966ae90cd837a8bde8ea7ad9fdeb83c4d3419f2509d55a64c19e745cf",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-public.json":                  "a908d47882462e698b67d80f2f48cfab578964e675029afe973a8a024711ba80",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json":                "6a8db01180325dc9074984b4b763250d0029118889ed7e5141a8ae1554709651",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver.json":                         "45ed3d463ce1159290dbb6686490e754e30d4540a108754571a926b8bba6f382",
		"sdk/fanbox/migration_identity_protocol_test.go":                             "65784f3eaf202bc3799e7ee252c6c75e8e6b3a2ab259ef0b273db9f1e56b7015",
		"sdk/fanbox/migration_identity_public_html_test.go":                          "b71cd78abd9d2e98b26cef97a8cbcbcd648b8b53a506389c669525c28c3b09c9",
		"sdk/fanbox/migration_identity_tokenizer_regression_test.go":                 "f27059d6a92cd3f16a8067929a254a53f058ce68eca19eef04ec46666745ae90",
		"sdk/fanbox/migration_solver_public_aliases_test.go":                         "bbca2490257049b75b4883b97428b44d794bc0d290e668040d30151e040f9a80",
		"sdk/fanbox/migration_solver_public_test.go":                                 "a21a5a150c8a644d7874f5e94cbcf78716742279048a41f680d1ede0a5d6fa34",
	}
	for path, want := range protected {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("existing FANBOX source contract changed: %s", path)
		}
	}
	rows := migrationFanboxIdentityJSONBytesCases(t)
	if len(rows) != 47 {
		t.Fatalf("bounded byte and depth supplement must have 47 rows, got %d", len(rows))
	}
	seen := map[string]bool{}
	families := map[string]int{}
	for index := range rows {
		row := &rows[index]
		if seen[row.Name] {
			t.Fatalf("duplicate byte supplement case: %s", row.Name)
		}
		seen[row.Name] = true
		families[strings.Split(row.Name, "/")[0]]++
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationFanboxObserveJSONBytes(t, row.Input) })
	}
	if t.Failed() {
		return
	}
	contract := map[string]any{
		"source_commit": reference.SourceCommit, "go_version": runtime.Version(), "source_sha256": reference.SourceSHA256,
		"go_stdlib_sha256": reference.GoStdlibSHA256, "go_json_stdlib_sha256": stdlib,
		"dependencies": reference.Dependencies, "frozen_go_production_guard": frozenProduction,
		"protected_published_files_sha256": protected,
		"preserved_case_counts":            map[string]int{"original_identity": 309, "public_html": 67, "public_solver": 93, "identity_tokenizer_regressions": 14, "solver_unicode_aliases": 7},
		"public_operation":                 "fanbox.Client.CurrentUser", "input_body_encoding": "lowercase hexadecimal raw HTML bytes", "cases": rows,
		"case_family_counts": families, "go_only_projections": reference.GoOnlyProjections,
		"evidence": "Actual frozen public OpenWith/CurrentUser with owned raw bytes.Reader response bodies and a genuine injected http.Client/RoundTripper. Response input is decoded from lowercase hex directly to bytes; it never enters a JSON string field before injection. Body lengths and SHA256 are checked before every request and after fixture serialization. Surrogates, malformed escapes and invalid UTF-8 are supplied inside actual HTML metadata JSON attributes. Separate HTML preprocessing inputs distinguish entity-encoded JSON escapes, numeric surrogate entity replacement, raw NUL attribute behavior and ignored invalid UTF-8 outside metadata. Global object/array depth includes the envelope: root ignored containers add to depth 1, and user ignored containers add to depth 3. The test-only quote-aware depth counter validates input accounting and never supplies an expected SDK outcome.",
		"limitations": []string{
			"Forty-seven bounded input rows are observations, not top-level test counts, parser completeness or platform parity.",
			"Owned injected 200 response bytes and synthetic session values only; no sockets, native transport, external authentication, media, HEAD, upload, browser, account or trust changes.",
			"Original identity309, public HTML67, public solver93, identity14, solver7 and all other existing FANBOX fixture bytes are guarded unchanged; no private parser observation is transformed into a public SDK expectation.",
			"Go-only concrete error identities/tree, injected client pointer ownership and read-call/event topology remain projections. DTOs, SDK classification/text/source, request/header details, total bytes and close counts are actual public observations.",
			"Solver control JSON/date contracts and generalized HTML/JSON/URL grammar are separate work. The denied supplemental native multiplex/unfinished HEAD/upload probe is neither retried nor reconstructed.",
		},
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	var roundtrip struct {
		Cases []migrationFanboxJSONBytesCase `json:"cases"`
	}
	if err := json.Unmarshal(data, &roundtrip); err != nil {
		t.Fatal(err)
	}
	if len(roundtrip.Cases) != len(rows) {
		t.Fatal("fixture serialization changed the raw response input inventory")
	}
	for index, row := range roundtrip.Cases {
		if row.Input.Steps[0] != rows[index].Input.Steps[0] || !bytes.Equal(migrationFanboxJSONBytesDecode(t, row.Input.Steps[0]), migrationFanboxJSONBytesDecode(t, rows[index].Input.Steps[0])) {
			t.Fatalf("fixture serialization changed response bytes: %s", row.Name)
		}
	}
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-identity-json-bytes.json")
	if *migrationCaptureFanboxIdentityJSONBytes {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public identity byte/depth behavior differs from frozen Go fixture; review actual Go behavior before recapture")
	}
	names := []string{}
	for name := range families {
		names = append(names, name)
	}
	sort.Strings(names)
	t.Logf("frozen Go %s public byte/depth supplement: %d cases; fixture bytes=%d sha256=%x; frozen Go production/module paths=434", runtime.Version(), len(rows), len(data), sha256.Sum256(data))
	for _, name := range names {
		t.Logf("%s: %d", name, families[name])
	}
}

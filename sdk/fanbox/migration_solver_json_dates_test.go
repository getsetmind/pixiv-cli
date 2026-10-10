package fanbox_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

var migrationCaptureFanboxSolverJSONDates = flag.Bool("migration-capture-fanbox-solver-json-dates", false, "capture frozen Go public solver JSON bytes, depth and expiry dates")

type migrationSolverJSONDatesInput struct {
	Native             []migrationFanboxStep `json:"native_steps"`
	ControlResponseHex string                `json:"control_response_hex"`
	Calls              int                   `json:"calls"`
}

type migrationSolverJSONDatesRow struct {
	Name        string                        `json:"name"`
	Family      string                        `json:"family"`
	SourceForm  string                        `json:"source_form,omitempty"`
	GlobalDepth int                           `json:"first_value_global_depth,omitempty"`
	Input       migrationSolverJSONDatesInput `json:"input"`
	Observation map[string]any                `json:"observation"`
}

func migrationSolverJSONDatesObserve(t *testing.T, input migrationSolverJSONDatesInput) map[string]any {
	t.Helper()
	body, err := hex.DecodeString(input.ControlResponseHex)
	if err != nil || hex.EncodeToString(body) != input.ControlResponseHex {
		t.Fatal("control response is not canonical exact-byte hex")
	}
	return migrationPublicSolverObserve(t, migrationPublicSolverInput{
		Native: input.Native, Calls: input.Calls,
		Control: []migrationFanboxStep{{Status: http.StatusOK, Body: string(body)}},
	})
}

func TestMigrationFanboxSolverJSONDatesFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	protected := map[string]string{
		"sdk/fanbox/migration_identity_public_html_test.go":                          "b71cd78abd9d2e98b26cef97a8cbcbcd648b8b53a506389c669525c28c3b09c9",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-public-html.json":           "fe0eee2fc4f5d08c717a223f0df043c003458c1893fe55e95f66ac4df043e388",
		"sdk/fanbox/migration_identity_tokenizer_regression_test.go":                 "f27059d6a92cd3f16a8067929a254a53f058ce68eca19eef04ec46666745ae90",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-tokenizer-regressions.json": "732d71a38a98407dbf874586c3f77412d4b34006ac916590e5593321da79129a",
		"sdk/fanbox/migration_solver_public_test.go":                                 "a21a5a150c8a644d7874f5e94cbcf78716742279048a41f680d1ede0a5d6fa34",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-public.json":                  "a908d47882462e698b67d80f2f48cfab578964e675029afe973a8a024711ba80",
		"sdk/fanbox/migration_solver_public_aliases_test.go":                         "bbca2490257049b75b4883b97428b44d794bc0d290e668040d30151e040f9a80",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-public-aliases.json":          "81e88cc966ae90cd837a8bde8ea7ad9fdeb83c4d3419f2509d55a64c19e745cf",
		"internal/services/fanbox/protocol/migration_solver_test.go":                 "c9a27c01aafbf271dc7a6b33cab8959a7b8e721b9b3d0edc6d41941d7aae1f56",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver.json":                         "45ed3d463ce1159290dbb6686490e754e30d4540a108754571a926b8bba6f382",
		"internal/services/fanbox/protocol/migration_solver_redirect_test.go":        "40a96c8b674c9f5f9185f9b9be49616b6ccd108c9375c76531d54de7f4896da2",
		"crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json":                "6a8db01180325dc9074984b4b763250d0029118889ed7e5141a8ae1554709651",
	}
	var original struct {
		Cases []migrationPublicSolverRow `json:"cases"`
	}
	for path, want := range protected {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("published contract changed: %s", path)
		}
		if strings.HasSuffix(path, "/fanbox-solver-public.json") {
			if err := json.Unmarshal(data, &original); err != nil {
				t.Fatal(err)
			}
		}
	}
	stdlib := map[string]string{
		"encoding/json/decode.go":  "1632161a34c8286722716a48ba0b5e0c3d117a2e017e79ace676403808a16e6e",
		"encoding/json/scanner.go": "2b16dd215274dfa8e0806b53cca144684d5b9c8a02319cf5ce10120c50e6eef2",
		"encoding/json/stream.go":  "065501364e4954cf1c8f1887900248d4c0983593b8c058b2fe7997d9635b793d",
		"time/format.go":           "f2b75d1440a46231523d3be86925b52c59c5f23657658e24514f2daa588e575d",
		"time/format_rfc3339.go":   "14b2a58fa295cca1a4f99467f5a5059418ed266a2298316124af97ffa014f271",
	}
	for path, want := range stdlib {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("frozen Go JSON/date source changed: %s", path)
		}
	}
	if len(original.Cases) != 93 {
		t.Fatal("public93 input inventory changed")
	}
	var baseline migrationPublicSolverInput
	for _, row := range original.Cases {
		if row.Name == "cache/native_future" {
			baseline = row.Input
		}
	}
	if len(baseline.Native) != 3 || len(baseline.Control) != 1 || baseline.Calls != 2 || !strings.Contains(baseline.Control[0].Body, `"expiry":4102444800`) {
		t.Fatal("reused future-cache public input missing")
	}
	document := baseline.Control[0].Body
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-solver-json-dates.json")
	recorded := map[string]migrationSolverJSONDatesRow{}
	if !*migrationCaptureFanboxSolverJSONDates {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var saved struct {
			Cases []migrationSolverJSONDatesRow `json:"cases"`
		}
		if err := json.Unmarshal(data, &saved); err != nil {
			t.Fatal(err)
		}
		for _, row := range saved.Cases {
			if _, exists := recorded[row.Name]; exists {
				t.Fatalf("duplicate saved case: %s", row.Name)
			}
			recorded[row.Name] = row
		}
	}
	rows := []migrationSolverJSONDatesRow{}
	seen := map[string]bool{}
	add := func(family, name, sourceForm, body string, depth int) {
		name = family + "/" + name
		if seen[name] {
			t.Fatalf("duplicate bounded case: %s", name)
		}
		seen[name] = true
		input := migrationSolverJSONDatesInput{
			Native: append([]migrationFanboxStep{}, baseline.Native...), Calls: baseline.Calls,
			ControlResponseHex: hex.EncodeToString([]byte(body)),
		}
		if !*migrationCaptureFanboxSolverJSONDates {
			row, exists := recorded[name]
			want, wantErr := json.Marshal(input)
			got, gotErr := json.Marshal(row.Input)
			if !exists || wantErr != nil || gotErr != nil || !bytes.Equal(want, got) || row.Family != family || row.SourceForm != sourceForm || row.GlobalDepth != depth {
				t.Fatalf("saved exact-byte input differs: %s", name)
			}
			input = row.Input
		}
		var observation map[string]any
		t.Run(name, func(t *testing.T) { observation = migrationSolverJSONDatesObserve(t, input) })
		rows = append(rows, migrationSolverJSONDatesRow{Name: name, Family: family, SourceForm: sourceForm, GlobalDepth: depth, Input: input, Observation: observation})
	}
	userAgent := func(raw string) string {
		return strings.Replace(document, `"synthetic-solver-agent"`, raw, 1)
	}
	ignored := func(raw string) string {
		return `{"ignored":` + raw + `,` + document[1:]
	}
	cookie := func(raw string) string {
		return strings.Replace(document, `"cookies":[`, `"cookies":[`+raw+`,`, 1)
	}
	for _, row := range []struct{ name, raw string }{
		{"user_agent_lone_high", `"agent\uD800"`},
		{"user_agent_lone_low", `"agent\uDEAD"`},
		{"user_agent_valid_pair", `"agent\uD83D\uDE00"`},
		{"user_agent_high_then_plain", `"agent\uD800x"`},
		{"user_agent_reversed_pair", `"agent\uDE00\uD83D"`},
		{"user_agent_short_escape", `"agent\uD80"`},
		{"user_agent_invalid_hex", `"agent\uD80g"`},
		{"user_agent_unknown_escape", `"agent\q"`},
		{"user_agent_unescaped_newline", "\"agent\n\""},
		{"user_agent_replacement_then_newline", `"agent\uD800\n"`},
		{"user_agent_replacement_then_cr", `"agent\uDEAD\r"`},
		{"user_agent_replacement_then_del", `"agent\uD800\u007f"`},
	} {
		add("json_string", row.name, "JSON string escapes", userAgent(row.raw), 0)
	}
	for _, row := range []struct{ name, raw string }{
		{"ignored_lone_surrogate", `"ignored\uD800"`},
		{"ignored_valid_pair", `"ignored\uD83D\uDE00"`},
		{"ignored_malformed_escape", `"ignored\uD80g"`},
	} {
		add("json_string", row.name, "ignored JSON string", ignored(row.raw), 0)
	}
	for _, row := range []struct{ name, raw string }{
		{"ignored_cookie_name_surrogate", `{"name":"other\uD800","value":"ordinary"}`},
		{"ignored_cookie_name_pair", `{"name":"other\uD83D\uDE00","value":"ordinary"}`},
		{"ignored_cookie_value_surrogate_and_newline", `{"name":"other","value":"ignored\uD800\n"}`},
		{"ignored_cookie_value_pair", `{"name":"other","value":"ignored\uD83D\uDE00"}`},
		{"ignored_cookie_malformed_escape", `{"name":"other","value":"ignored\uD80g"}`},
	} {
		add("json_string", row.name, "discarded solver cookie", cookie(row.raw), 0)
	}
	add("json_string", "clearance_name_surrogate", "clearance cookie name", strings.Replace(document, `"cf_clearance"`, `"cf_clearance\uD800"`, 1), 0)
	add("json_string", "clearance_value_surrogate", "clearance cookie value", strings.Replace(document, `"synthetic-clearance"`, `"clearance\uD800"`, 1), 0)
	add("json_string", "clearance_value_pair", "clearance cookie value", strings.Replace(document, `"synthetic-clearance"`, `"clearance\uD83D\uDE00"`, 1), 0)
	for _, row := range []struct{ name, value string }{
		{"invalid_leading", "\xff"},
		{"truncated", "\xe2\x82"},
		{"invalid_continuation", "\xe2\x28\xa1"},
		{"overlong", "\xc0\xaf"},
		{"utf8_encoded_surrogate", "\xed\xa0\x80"},
		{"outside_unicode", "\xf4\x90\x80\x80"},
	} {
		add("json_utf8", "user_agent_"+row.name, "raw invalid UTF-8 JSON string bytes", userAgent(`"agent`+row.value+`"`), 0)
	}
	add("json_utf8", "ignored_value", "raw invalid UTF-8 ignored string bytes", ignored("\"ignored\xff\""), 0)
	add("json_utf8", "ignored_member_name", "raw invalid UTF-8 member-name bytes", `{"ignored`+"\xff"+`":true,`+document[1:], 0)
	add("json_utf8", "ignored_cookie_name", "raw invalid UTF-8 discarded cookie name", cookie("{\"name\":\"other\xff\",\"value\":\"ordinary\"}"), 0)
	add("json_utf8", "ignored_cookie_value", "raw invalid UTF-8 discarded cookie value", cookie("{\"name\":\"other\",\"value\":\"ignored\xff\"}"), 0)
	add("json_utf8", "clearance_value", "raw invalid UTF-8 clearance value", strings.Replace(document, `"synthetic-clearance"`, "\"clearance\xff\"", 1), 0)
	add("json_utf8", "replacement_then_newline", "raw invalid UTF-8 followed by escaped newline", userAgent("\"agent\xff\\n\""), 0)

	nested := func(depth int) string { return strings.Repeat("[", depth) + "0" + strings.Repeat("]", depth) }
	for _, depth := range []int{10000, 10001} {
		add("json_depth", fmt.Sprintf("ignored_root_%d", depth), "ignored root array", ignored(nested(depth-1)), depth)
		body := strings.Replace(document, `"solution":{`, `"solution":{"ignored":`+nested(depth-2)+`,`, 1)
		add("json_depth", fmt.Sprintf("ignored_solution_%d", depth), "ignored solution array", body, depth)
		body = strings.Replace(document, `"name":"cf_clearance"`, `"ignored":`+nested(depth-4)+`,"name":"cf_clearance"`, 1)
		add("json_depth", fmt.Sprintf("ignored_cookie_%d", depth), "ignored clearance-cookie array", body, depth)
	}
	for _, row := range []struct{ name, trailing string }{
		{"trailing_invalid_escape", ` "\uD80g"`},
		{"trailing_invalid_utf8", " \xff"},
		{"trailing_deep_complete_second_value", " " + nested(10001)},
		{"trailing_deep_incomplete_second_value", " " + strings.Repeat("[", 10001)},
	} {
		add("json_stream", row.name, "first complete JSON value with unparsed trailing bytes", document+row.trailing, 4)
	}
	add("json_stream", "incomplete_first_document", "unterminated first JSON object", document[:len(document)-1], 4)
	add("json_stream", "incomplete_ignored_first_value", "unterminated ignored array in first value", `{"ignored":[0,`+document, 6)
	for _, row := range []struct{ name, body string }{
		{"array", `[]`},
		{"boolean", `true`},
		{"number", `4102444800`},
		{"string", `"ok"`},
	} {
		add("json_root", row.name, "non-object first JSON value", row.body, 0)
	}

	expiry := func(raw string) string { return strings.Replace(document, `4102444800`, raw, 1) }
	for _, row := range []struct{ name, value string }{
		{"seconds_59", "2035-01-01T00:00:59Z"},
		{"seconds_60", "2035-01-01T00:00:60Z"},
		{"nano_seconds_59", "2035-01-01T00:00:59.123456789Z"},
		{"nano_seconds_60", "2035-01-01T00:00:60.123456789Z"},
		{"fraction_comma", "2035-01-01T00:00:59,125Z"},
		{"fraction_beyond_nanoseconds", "2035-01-01T00:00:59.123456789123Z"},
		{"fraction_missing_digits", "2035-01-01T00:00:59.Z"},
		{"fraction_positive_zone", "2035-01-01T00:00:59.125+09:30"},
		{"fraction_negative_zone", "2035-01-01T00:00:59.125-05:45"},
		{"zone_hour_24", "2035-01-01T00:00:59.125+24:00"},
		{"zone_hour_25", "2035-01-01T00:00:59.125+25:00"},
		{"zone_minute_60", "2035-01-01T00:00:59.125+00:60"},
		{"zone_minute_61", "2035-01-01T00:00:59.125+00:61"},
		{"two_digit_year_35", "35-01-01T00:00:59Z"},
	} {
		raw, err := json.Marshal(row.value)
		if err != nil {
			t.Fatal(err)
		}
		add("expiry_rfc3339", row.name, "RFC3339/RFC3339Nano source text", expiry(string(raw)), 0)
	}
	future := time.Date(2035, time.January, 1, 0, 0, 59, 0, time.UTC)
	gmt := future.In(time.FixedZone("GMT", 0))
	gmtDate := gmt.Format(http.TimeFormat)
	for _, row := range []struct{ name, source, value string }{
		{"timeformat_valid", "http.TimeFormat", gmtDate},
		{"timeformat_weekday_mismatch", "http.TimeFormat", "Tue" + gmtDate[3:]},
		{"timeformat_weekday_invalid", "http.TimeFormat", "Bad" + gmtDate[3:]},
		{"timeformat_seconds_60", "http.TimeFormat", strings.Replace(gmtDate, "00:00:59", "00:00:60", 1)},
		{"timeformat_fraction", "http.TimeFormat", strings.Replace(gmtDate, "00:00:59", "00:00:59.125", 1)},
		{"timeformat_two_digit_year_35", "http.TimeFormat with two-digit year", strings.Replace(gmtDate, "2035", "35", 1)},
		{"timeformat_two_digit_year_69", "http.TimeFormat with two-digit year", strings.Replace(gmtDate, "2035", "69", 1)},
		{"rfc1123_gmt", "time.RFC1123 formatted in GMT", gmt.Format(time.RFC1123)},
		{"rfc1123_utc", "time.RFC1123 formatted in UTC", future.Format(time.RFC1123)},
		{"rfc1123_other_zone", "time.RFC1123 formatted in named fixed zone", future.In(time.FixedZone("JST", 9*60*60)).Format(time.RFC1123)},
		{"rfc1123z", "time.RFC1123Z", future.Format(time.RFC1123Z)},
		{"rfc850_year_35", "time.RFC850 (two-digit year 35)", gmt.Format(time.RFC850)},
		{"rfc850_year_69", "time.RFC850 (two-digit year 69)", time.Date(2069, time.January, 1, 0, 0, 59, 0, time.UTC).In(time.FixedZone("GMT", 0)).Format(time.RFC850)},
		{"ansic", "time.ANSIC", future.Format(time.ANSIC)},
	} {
		raw, err := json.Marshal(row.value)
		if err != nil {
			t.Fatal(err)
		}
		add("expiry_http_date", row.name, row.source, expiry(string(raw)), 0)
	}
	if t.Failed() {
		return
	}
	if !*migrationCaptureFanboxSolverJSONDates && len(recorded) != len(rows) {
		t.Fatal("saved bounded input inventory differs")
	}
	fixture := map[string]any{
		"source_commit": reference.SourceCommit, "go_version": reference.GoVersion, "source_sha256": reference.SourceSHA256,
		"go_json_date_stdlib_sha256": stdlib, "protected_published_files_sha256": protected,
		"source_public_fixture_case_count": 93, "reused_public_case": "cache/native_future",
		"public_operation": "fanbox.Client.CurrentUser", "control_response_encoding": "lowercase hexadecimal exact bytes", "cases": rows,
		"evidence": "Actual unchanged frozen public OpenWith/CurrentUser and ordinary owned anonymous HTTP/1 loopback control through the published public93 harness. Only solver response bytes change from the existing future-cache input. Test-only hex preserves malformed UTF-8 through fixture marshaling and decodes directly to the bytes written by the loopback handler; every row is replayed from canonical hex. Native/API identity responses, two public calls, error/DTO/source, native header state/cache and diagnostics are actual public observations. Total JSON object/array depth includes the enclosing root, solution and cookie envelopes. Stream rows distinguish the first complete decoded value from unparsed malformed, deeply nested or incomplete second values. Expiry uses only the actual solver's RFC3339 and http.TimeFormat layouts, with source-formatted RFC1123/RFC1123Z/RFC850/ANSIC rejection or acceptance boundaries captured at the same public operation.",
		"go_only_boundaries": []string{
			"Original Go production, identity309/public67/tokenizer14, solver124/public93/aliases7 and redirect3 evidence remain unchanged",
			"Concrete Go error/sentinel identities, reader call topology and default control User-Agent/HTTP version projections retain the published public93 boundaries",
			"Date acceptance and retained native clearance are observed; the private parsed expiry timestamp is not exposed or synthesized. All accepted date samples are future 2035 dates; numeric baseline expiry stays 4102444800",
			"A valid HTTP date weekday token may disagree with the date. The solver does not call http.ParseTime, and two-digit RFC850 years do not acquire its broader parsing or pivot semantics",
		},
		"limitations": []string{
			"This bounded string/byte/depth/stream/date supplement does not claim full encoding/json or general date parser parity",
			"Only anonymous owned HTTP/1 loopback control and injected native/API responses execute; no native HTTP/2, external authentication, media, account, browser, host trust, HEAD or upload work",
			"No private Solver/session seam, runtime clock injection or private expiry helper is used. Exact fractional expiry nanoseconds and timezone offsets are not individually observable through this fixed future-cache public workflow",
		},
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if *migrationCaptureFanboxSolverJSONDates {
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public solver JSON/date observations differ from actual frozen Go fixture")
	}
	t.Logf("public solver JSON/dates: %d rows, bytes=%d sha256=%x", len(rows), len(data), sha256.Sum256(data))
}

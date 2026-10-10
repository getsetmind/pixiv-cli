package pipeline

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
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	record "github.com/FlanChanXwO/pixiv-cli/internal/shared/record"
	"github.com/spf13/cobra"
)

var migrationUpdateDownloadRecordCodec = flag.Bool("migration-update-download-record-codec", false, "capture download record input contracts from the fixed Go reference")

const downloadRecordFrozenGo = "4b4426487ef18bed276706daec385e0d0a6979f9"

var downloadRecordSourceSHA = map[string]string{
	"internal/cli/pipeline/records.go":                  "c4a94d230db4ca95661baeac26c6f912ab6ade7c8a3808bff05878609152eb29",
	"internal/cli/pipeline/action.go":                   "74c540329878d2f87db3643056dc0f1d58cde5d4f98d90e9bec4421cd189161c",
	"internal/cli/pipeline/pipeline.go":                 "294d15b516da78e677dfde04a59daf97d5c1adff76e0328bb43d0d3bfe7e7d3c",
	"internal/shared/record/json.go":                    "8a41ca7fa1e7d2841532cf435e5ae08560bb6f677794b4ba4c4b7e113c0ab858",
	"internal/shared/record/record.go":                  "fce91df3075c9ea3a86822ebb602d42749f6039e7bd563fedfa4377bc94c5d20",
	"internal/utils/parse/parse.go":                     "e99a8188c2d8a5fc9f4d053a47ff8c0ed8bbaa539d4abd79ba30f06d778e7bb1",
	"internal/cli/commands/pixiv/download/execution.go": "236460386b2847dc03a00f10ac1e955ad1fca9f18c173ae0a1523f960812fdf0",
}

type downloadRecordCodecFixture struct {
	FrozenGo  string                    `json:"frozen_go"`
	SourceSHA map[string]string         `json:"source_sha256"`
	Cases     []downloadRecordCodecCase `json:"cases"`
}

type downloadRecordCodecCase struct {
	Name          string                    `json:"name"`
	Boundary      string                    `json:"boundary"`
	InputHex      string                    `json:"input_hex"`
	Args          []string                  `json:"args,omitempty"`
	OnError       string                    `json:"on_error"`
	Reader        string                    `json:"reader"`
	Writer        string                    `json:"writer"`
	ActionFailure string                    `json:"action_failure"`
	FailureID     int64                     `json:"failure_id"`
	Cancellation  string                    `json:"cancellation"`
	Actual        downloadRecordCodecResult `json:"actual"`
}

type downloadRecordCodecResult struct {
	Error              string                     `json:"error"`
	Cause              string                     `json:"cause"`
	Pipeline           bool                       `json:"pipeline"`
	Stderr             string                     `json:"stderr"`
	Diagnostics        []recordDiagnostic         `json:"diagnostics"`
	ActionIDs          []int64                    `json:"action_ids"`
	ActionContextError []string                   `json:"action_context_errors"`
	ReadCalls          int                        `json:"read_calls"`
	ReadBytes          int                        `json:"read_bytes"`
	WriteFailures      int                        `json:"write_failures"`
	Mode               Mode                       `json:"mode"`
	TextHex            string                     `json:"text_hex"`
	ReplayHex          string                     `json:"replay_hex"`
	Args               []string                   `json:"args"`
	Lines              []downloadRecordCodecLine  `json:"lines"`
	Parsed             []downloadRecordCodecValue `json:"parsed"`
	ParseConsumerError string                     `json:"parse_consumer_error"`
}

type downloadRecordCodecValue struct {
	ID              string `json:"id"`
	Type            string `json:"type"`
	URL             string `json:"url"`
	RequiredID      int64  `json:"required_id"`
	RequiredIDError string `json:"required_id_error"`
}

type downloadRecordCodecLine struct {
	Line         int64                    `json:"line"`
	ParseError   string                   `json:"parse_error"`
	Value        downloadRecordCodecValue `json:"value"`
	IdentityID   string                   `json:"identity_id"`
	IdentityType string                   `json:"identity_type"`
}

var downloadRecordReadFailure = errors.New("fixture reader failure")
var downloadRecordWriteFailure = errors.New("fixture writer failure")
var downloadRecordBusinessFailure = errors.New("fixture business failure")
var downloadRecordFatalFailure = errors.New("fixture fatal failure")

type downloadRecordOwnedReader struct {
	data     []byte
	mode     string
	position int
	calls    int
	terminal bool
	onRead   func()
}

func (r *downloadRecordOwnedReader) Read(p []byte) (int, error) {
	r.calls++
	if r.onRead != nil {
		r.onRead()
		r.onRead = nil
	}
	if r.position == len(r.data) {
		if !r.terminal && (r.mode == "error-after-data" || r.mode == "data-error") {
			r.terminal = true
			return 0, downloadRecordReadFailure
		}
		return 0, io.EOF
	}
	if r.mode == "one-byte" && len(p) > 1 {
		p = p[:1]
	}
	n := copy(p, r.data[r.position:])
	r.position += n
	if r.position == len(r.data) && !r.terminal {
		switch r.mode {
		case "data-eof":
			r.terminal = true
			return n, io.EOF
		case "data-error":
			r.terminal = true
			return n, downloadRecordReadFailure
		}
	}
	return n, nil
}

type downloadRecordOwnedWriter struct {
	bytes.Buffer
	mode     string
	calls    int
	failures int
}

func (w *downloadRecordOwnedWriter) Write(p []byte) (int, error) {
	w.calls++
	if w.mode == "always" || (w.mode == "first" && w.calls == 1) {
		w.failures++
		return 0, downloadRecordWriteFailure
	}
	return w.Buffer.Write(p)
}

func downloadRecordErrorText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func downloadRecordValue(value record.Record) downloadRecordCodecValue {
	id, err := RequiredRecordID(value)
	return downloadRecordCodecValue{ID: value.ID(), Type: value.Type(), URL: value.URL(), RequiredID: id, RequiredIDError: downloadRecordErrorText(err)}
}

func downloadRecordCodecPlans() []downloadRecordCodecCase {
	var cases []downloadRecordCodecCase
	add := func(name, input string) int {
		cases = append(cases, downloadRecordCodecCase{Name: name, Boundary: "action", InputHex: hex.EncodeToString([]byte(input)), OnError: "skip", Reader: "plain", Writer: "plain"})
		return len(cases) - 1
	}
	valid := func(id, typ, url string) string {
		return `{"id":` + id + `,"type":` + typ + `,"url":` + url + `}`
	}
	artwork := func(id string) string { return valid(id, `"artwork"`, `"https://www.pixiv.net/artworks/42"`) }
	first := artwork(`"42"`)
	second := artwork(`"43"`)
	for _, typ := range []string{"artwork", "illust", "manga", "ugoira"} {
		add("accepted-"+typ, valid(`"42"`, fmt.Sprintf("%q", typ), `"https://misleading.invalid/not-an-artwork/7"`)+"\n")
	}
	add("accepted-numeric-id", artwork(`42`))
	add("repeated-id-not-deduplicated", first+"\n"+first+"\n")
	add("url-not-parsed-or-matched", valid(`"42"`, `"artwork"`, `" \t"`))
	add("empty-stream", "")
	add("blank-line", "\n")
	add("ascii-whitespace-line", " \t\r\n")
	add("unicode-whitespace-line", "\u00a0\u2003\u3000\n")
	add("crlf-final-without-newline", first+"\r\n"+second)
	add("physical-lines-and-success-prefix", first+"\n\n \t\n{broken}\n"+second+"\n")
	i := add("physical-lines-fail-fast", first+"\n\n \t\n{broken}\n"+second+"\n")
	cases[i].OnError = "fail-fast"
	add("pretty-multiline-is-physical-lines", "{\n\"id\":\"42\",\n\"type\":\"artwork\",\n\"url\":\"x\"\n}\n"+second)
	add("array-is-not-expanded", "["+first+"]\n"+second)
	add("aggregate-is-not-expanded", `{"records":[`+first+`]}`+"\n")
	for _, scalar := range []string{"null", "true", "42", `"text"`} {
		add("scalar-"+hex.EncodeToString([]byte(scalar)), scalar+"\n")
	}
	for _, input := range []struct{ name, value string }{
		{"missing-all", `{}`},
		{"missing-id-before-invalid-type-url", `{"type":null,"url":false}`},
		{"null-id", `{"id":null,"type":"artwork","url":"x"}`},
		{"bool-id", `{"id":true,"type":"artwork","url":"x"}`},
		{"array-id", `{"id":[],"type":"artwork","url":"x"}`},
		{"object-id", `{"id":{},"type":"artwork","url":"x"}`},
		{"empty-id-before-missing-type", `{"id":""}`},
		{"missing-type-before-url", `{"id":"42"}`},
		{"null-type-before-missing-url", `{"id":"42","type":null}`},
		{"numeric-type", `{"id":"42","type":7,"url":"x"}`},
		{"bool-type", `{"id":"42","type":false,"url":"x"}`},
		{"array-type", `{"id":"42","type":[],"url":"x"}`},
		{"object-type", `{"id":"42","type":{},"url":"x"}`},
		{"empty-type", `{"id":"42","type":"","url":"x"}`},
		{"missing-url", `{"id":"42","type":"artwork"}`},
		{"null-url", `{"id":"42","type":"artwork","url":null}`},
		{"numeric-url", `{"id":"42","type":"artwork","url":7}`},
		{"bool-url", `{"id":"42","type":"artwork","url":false}`},
		{"array-url", `{"id":"42","type":"artwork","url":[]}`},
		{"object-url", `{"id":"42","type":"artwork","url":{}}`},
		{"empty-url", `{"id":"42","type":"artwork","url":""}`},
		{"field-name-case-sensitive", `{"ID":"42","Type":"artwork","URL":"x"}`},
	} {
		add("required-"+input.name, input.value)
	}
	for _, id := range []struct{ name, value string }{
		{"string-plus", `"+42"`}, {"string-leading-zero", `"0042"`}, {"string-many-leading-zeros", `"00000000000000000000000000000000000000042"`},
		{"string-minus", `"-42"`}, {"string-zero", `"0"`}, {"string-negative-zero", `"-0"`}, {"string-plus-zero", `"+0"`},
		{"string-whitespace", `" 42 "`}, {"string-tab", `"\t42"`}, {"string-unicode-whitespace", `"\u00a042\u00a0"`},
		{"string-decimal", `"42.0"`}, {"string-exponent", `"4.2e1"`}, {"string-hex", `"0x2a"`}, {"string-unicode-digits", `"４２"`},
		{"string-i64-max", `"9223372036854775807"`}, {"string-i64-overflow", `"9223372036854775808"`},
		{"numeric-zero", `0`}, {"numeric-negative-zero", `-0`}, {"numeric-negative", `-42`}, {"numeric-decimal", `42.0`},
		{"numeric-exponent", `4.2e1`}, {"numeric-positive-exponent", `42e+0`}, {"numeric-large-exponent", `1e99999`},
		{"numeric-i64-max", `9223372036854775807`}, {"numeric-i64-overflow", `9223372036854775808`},
		{"numeric-u64-max", `18446744073709551615`}, {"numeric-huge", strings.Repeat("9", 180)},
		{"invalid-plus-number", `+42`}, {"invalid-leading-zero-number", `0042`},
	} {
		add("id-"+id.name, artwork(id.value))
	}
	for _, typ := range []string{"user", "novel", "resource", "illustration", "Artwork", " artwork ", "manga\n", "\u2028", "\U0001f600"} {
		add("unsupported-type-"+hex.EncodeToString([]byte(typ)), valid(`" 42 "`, fmt.Sprintf("%q", typ), `"x"`))
	}
	add("duplicate-id-last-numeric-wins", `{"id":"bad","id":42,"type":"artwork","url":"x"}`)
	add("duplicate-id-last-null-wins", `{"id":"42","id":null,"type":"artwork","url":"x"}`)
	add("duplicate-type-last-accepted-wins", `{"id":"42","type":"user","type":"manga","url":"x"}`)
	add("duplicate-type-last-rejected-wins", `{"id":"42","type":"manga","type":"user","url":"x"}`)
	add("duplicate-url-last-empty-wins", `{"id":"42","type":"artwork","url":"x","url":""}`)
	add("trailing-second-object-keeps-first-identity", first+second)
	add("trailing-garbage-keeps-first-identity", first+"garbage")
	add("trailing-number-keeps-raw-identity", valid(`4.200e+01`, `"user"`, `"x"`)+" 99")
	add("trailing-huge-integer-keeps-raw-identity", valid(strings.Repeat("9", 180), `"manga"`, `"x"`)+" false")
	add("trailing-incomplete-first-loses-identity", `{"id":42,"type":"artwork","url":"x"`)
	add("html-diagnostic-escaping", valid(`"<42>&"`, `"<user>&"`, `"x"`))
	add("unknown-fields-do-not-change-action", `{"id":42,"type":"artwork","url":"x","version":7,"api_version":"x","metadata":{"SCHEMA":{},"conversion":"kept","n":123456789012345678901234567890},"unknown":[{"sdk-version":7}]}`)
	for _, depth := range []int{129, 256, 9999, 10000} {
		add(fmt.Sprintf("unknown-array-depth-%d", depth), `{"id":"42","type":"artwork","url":"x","unknown":`+strings.Repeat("[", depth)+"0"+strings.Repeat("]", depth)+"}")
	}
	add("record-exceeds-64-kib", `{"id":"42","type":"artwork","url":"x","unknown":"`+strings.Repeat("x", 70*1024)+`"}`+"\n"+second)
	add("unicode-trimspace-surrounding-record", "\u00a0\u1680\u2003\u2028\u2029\u202f\u205f\u3000"+first+"\u00a0\u2003\u3000\r\n")
	add("trimspace-vertical-tab-form-feed-next-line", "\v\f\u0085"+first+"\v\f\u0085")
	add("unicode-bom-is-not-trimspace", "\ufeff"+first)
	add("unicode-zero-width-is-not-trimspace", "\u200b"+first)
	add("unicode-trimspace-before-unsupported-loses-identity", "\u00a0"+valid(`"42"`, `"user"`, `"x"`)+"\u00a0")
	add("unicode-trimspace-after-unsupported-keeps-identity", valid(`"42"`, `"user"`, `"x"`)+"\u00a0")
	add("surrogate-lone-id", artwork(`"\ud800"`))
	add("surrogate-lone-type", valid(`"42"`, `"\udc00"`, `"x"`))
	add("surrogate-valid-pair-type", valid(`"42"`, `"\ud83d\ude00"`, `"x"`))
	add("surrogate-lone-url-accepted", valid(`"42"`, `"artwork"`, `"\ud800"`))
	add("surrogate-high-high-low-type", valid(`"42"`, `"\ud800\ud800\udc00"`, `"x"`))
	add("escaped-backslash-surrogate-not-repaired", valid(`"42"`, `"\\ud800"`, `"x"`))
	for _, field := range []string{"id", "type", "url", "unknown"} {
		input := `{"id":"42","type":"artwork","url":"x","` + field + `":"` + string([]byte{0xff, 0xc0, 0xaf}) + `"}`
		add("invalid-utf8-in-"+field, input)
	}
	add("invalid-utf8-outside-string", string([]byte{0xff})+first)
	add("invalid-utf8-truncated-multibyte-type", valid(`"42"`, `"`+string([]byte{0xe2, 0x82})+`"`, `"x"`))
	add("invalid-utf8-surrogate-sequence-type", valid(`"42"`, `"`+string([]byte{0xed, 0xa0, 0x80})+`"`, `"x"`))
	add("raw-control-in-string", valid(`"42"`, `"art`+string([]byte{0})+`work"`, `"x"`))
	for _, mode := range []string{"skip", "fail-fast"} {
		i := add("mixed-failures-"+mode, "{broken}\n"+first+"\n"+valid(`"42"`, `"user"`, `"x"`)+"\n"+artwork(`" 42 "`)+"\n"+second+"\n"+artwork(`"44"`))
		cases[i].OnError, cases[i].ActionFailure, cases[i].FailureID = mode, "business", 43
	}
	for _, failure := range []string{"business", "coded", "joined-coded", "fatal"} {
		i := add("callback-"+failure, first+"\n"+second)
		cases[i].ActionFailure, cases[i].FailureID = failure, 42
		i = add("consumer-callback-"+failure, first+"\n"+second)
		cases[i].Boundary, cases[i].ActionFailure, cases[i].FailureID = "consumer", failure, 42
	}
	for _, writer := range []string{"first", "always"} {
		i := add("diagnostic-writer-"+writer, "{broken}\n"+first)
		cases[i].Writer = writer
		i = add("callback-writer-"+writer, first+"\n"+second)
		cases[i].Writer, cases[i].ActionFailure, cases[i].FailureID = writer, "write", 42
	}
	for _, cancel := range []string{"before", "success", "error", "fatal", "during-read"} {
		i := add("cancel-"+cancel, first+"\n"+second)
		cases[i].Cancellation = cancel
	}
	for _, reader := range []string{"plain", "one-byte", "data-eof", "data-error", "error-after-data"} {
		i := add("reader-final-valid-"+reader, first)
		cases[i].Reader = reader
		i = add("reader-complete-prefix-incomplete-"+reader, first+"\n"+second)
		cases[i].Reader = reader
	}
	i = add("reader-data-error-after-complete-lines", first+"\n"+second+"\n")
	cases[i].Reader = "data-error"
	i = add("reader-error-without-data", "")
	cases[i].Reader = "error-after-data"
	i = add("invalid-strategy-before-read", first)
	cases[i].OnError = "SKIP"
	for _, input := range []struct{ name, value string }{
		{"empty", ""}, {"ascii-record-prefix", " \t\r\n" + first}, {"unicode-leading-record", "\u00a0" + first},
		{"unicode-after-ascii-leading-record", " \t\u2003" + first}, {"text-one-trailing-crlf", " 42 \r\n"},
		{"text-two-final-newlines", "42\n\n"}, {"text-bare-cr", "42\r"}, {"array-text", "[" + first + "]\n"},
		{"scalar-text", "null\n"}, {"ascii-whitespace-only", " \t\r\n"}, {"bom-text", "\ufeff" + first},
		{"vertical-tab-text", "\v" + first}, {"form-feed-text", "\f" + first}, {"next-line-text", "\u0085" + first},
	} {
		i = add("classify-"+input.name, input.value)
		cases[i].Boundary = "classify"
	}
	for _, input := range []struct{ name, value string }{{"whitespace", " \t"}, {"object-byte", "{"}, {"text-byte", "4"}} {
		i = add("classify-data-error-"+input.name, input.value)
		cases[i].Boundary, cases[i].Reader = "classify", "data-error"
	}
	i = add("explicit-source-leaves-stdin-unread", "{broken}\n")
	cases[i].Boundary, cases[i].Reader, cases[i].Args = "explicit", "error-after-data", []string{"42"}
	return cases
}

func observeDownloadRecordCodec(t *testing.T, row downloadRecordCodecCase) downloadRecordCodecResult {
	t.Helper()
	input, err := hex.DecodeString(row.InputHex)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	reader := &downloadRecordOwnedReader{data: input, mode: row.Reader}
	writer := &downloadRecordOwnedWriter{mode: row.Writer}
	result := downloadRecordCodecResult{Diagnostics: []recordDiagnostic{}, ActionIDs: []int64{}, ActionContextError: []string{}, Args: []string{}, Lines: []downloadRecordCodecLine{}, Parsed: []downloadRecordCodecValue{}}
	if row.Cancellation == "before" {
		cancel()
	}
	if row.Cancellation == "during-read" {
		reader.onRead = cancel
	}
	switch row.Boundary {
	case "classify":
		var replay io.Reader
		var text string
		result.Mode, replay, text, err = resolveTextOrRecord(reader)
		result.TextHex = hex.EncodeToString([]byte(text))
		if replay != nil {
			var data []byte
			data, err = io.ReadAll(replay)
			result.ReplayHex = hex.EncodeToString(data)
		}
	case "explicit":
		var resolved resolvedInput
		resolved, err = resolve(&cobra.Command{}, row.Args, InputSpec{Codec: TextOrRecord, MinArgs: 1, MaxArgs: -1, FillPosition: 0, Reader: reader})
		result.Mode, result.Args = resolved.mode, resolved.args
	case "consumer":
		err = ConsumeNDJSONRecords(ctx, reader, writer, "download", row.OnError == "fail-fast", func(ctx context.Context, value record.Record) error {
			id, err := RequiredRecordID(value)
			if err != nil {
				return err
			}
			result.ActionIDs = append(result.ActionIDs, id)
			result.ActionContextError = append(result.ActionContextError, downloadRecordErrorText(ctx.Err()))
			if row.FailureID == id {
				switch row.ActionFailure {
				case "business":
					return downloadRecordBusinessFailure
				case "coded":
					return NewRecordActionError("fixture_code", downloadRecordBusinessFailure)
				case "joined-coded":
					return errors.Join(NewRecordActionError("fixture_code", downloadRecordBusinessFailure), errors.New("fixture secondary failure"))
				case "fatal":
					return FatalRecordPipeline(downloadRecordFatalFailure)
				}
			}
			return nil
		})
	case "action":
		err = ConsumeActionRecords(ctx, reader, writer, "download", row.OnError,
			map[string]struct{}{"artwork": {}, "illust": {}, "manga": {}, "ugoira": {}},
			func(ctx context.Context, id int64) error {
				result.ActionIDs = append(result.ActionIDs, id)
				result.ActionContextError = append(result.ActionContextError, downloadRecordErrorText(ctx.Err()))
				if len(result.ActionIDs) == 1 {
					switch row.Cancellation {
					case "success":
						cancel()
					case "error":
						cancel()
						return downloadRecordBusinessFailure
					case "fatal":
						cancel()
						return FatalRecordPipeline(downloadRecordFatalFailure)
					}
				}
				if row.FailureID == id {
					switch row.ActionFailure {
					case "business":
						return downloadRecordBusinessFailure
					case "coded":
						return NewRecordActionError("fixture_code", downloadRecordBusinessFailure)
					case "joined-coded":
						return errors.Join(NewRecordActionError("fixture_code", downloadRecordBusinessFailure), errors.New("fixture secondary failure"))
					case "fatal":
						return FatalRecordPipeline(downloadRecordFatalFailure)
					case "write":
						_, err := writer.Write([]byte("fixture action output\n"))
						return err
					}
				}
				return nil
			}, func(err error) error { return fmt.Errorf("fixture usage: %w", err) })
	default:
		t.Fatalf("unknown boundary %q", row.Boundary)
	}
	result.Error = downloadRecordErrorText(err)
	var pipelineErr *PipelineDiagnosticError
	result.Pipeline = errors.As(err, &pipelineErr)
	for _, cause := range []struct {
		name string
		err  error
	}{{"cancel", context.Canceled}, {"reader", downloadRecordReadFailure}, {"writer", downloadRecordWriteFailure}, {"fatal", downloadRecordFatalFailure}, {"business", downloadRecordBusinessFailure}} {
		if errors.Is(err, cause.err) {
			result.Cause = cause.name
			break
		}
	}
	result.Stderr, result.ReadCalls, result.ReadBytes, result.WriteFailures = writer.String(), reader.calls, reader.position, writer.failures
	for _, line := range bytes.Split(writer.Bytes(), []byte{'\n'}) {
		var diagnostic recordDiagnostic
		if json.Unmarshal(line, &diagnostic) == nil && diagnostic.Kind == "record_error" {
			result.Diagnostics = append(result.Diagnostics, diagnostic)
		}
	}
	lineReader := bufio.NewReader(bytes.NewReader(input))
	for number := int64(1); ; number++ {
		line, readErr := lineReader.ReadBytes('\n')
		if len(line) != 0 {
			value, parseErr := record.ParseRecordJSON(bytes.TrimSpace(line))
			id, typ := diagnosticRecordIdentity(line)
			observation := downloadRecordCodecLine{Line: number, ParseError: downloadRecordErrorText(parseErr), IdentityID: id, IdentityType: typ}
			if parseErr == nil {
				observation.Value = downloadRecordValue(value)
			}
			result.Lines = append(result.Lines, observation)
		}
		if readErr != nil {
			break
		}
	}
	parseErr := ConsumeNDJSONRecords(context.Background(), bytes.NewReader(input), io.Discard, "download", false, func(_ context.Context, value record.Record) error {
		result.Parsed = append(result.Parsed, downloadRecordValue(value))
		return nil
	})
	result.ParseConsumerError = downloadRecordErrorText(parseErr)
	return result
}

func TestMigrationDownloadRecordCodecMatchesFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..", "..")
	for path, expected := range downloadRecordSourceSHA {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprintf("%x", sha256.Sum256(data)); got != expected {
			t.Fatalf("frozen Go source changed: %s: got %s want %s", path, got, expected)
		}
	}
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "download_record_codec.json")
	actual := downloadRecordCodecFixture{FrozenGo: downloadRecordFrozenGo, SourceSHA: downloadRecordSourceSHA, Cases: downloadRecordCodecPlans()}
	for i := range actual.Cases {
		actual.Cases[i].Actual = observeDownloadRecordCodec(t, actual.Cases[i])
	}
	if *migrationUpdateDownloadRecordCodec {
		data, err := json.MarshalIndent(actual, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected downloadRecordCodecFixture
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	if expected.FrozenGo != actual.FrozenGo || !reflect.DeepEqual(expected.SourceSHA, actual.SourceSHA) || len(expected.Cases) != len(actual.Cases) {
		t.Fatal("frozen download record fixture provenance or case count differs")
	}
	for i, row := range actual.Cases {
		t.Run(row.Name, func(t *testing.T) {
			if !reflect.DeepEqual(row, expected.Cases[i]) {
				got, _ := json.MarshalIndent(row, "", "  ")
				want, _ := json.MarshalIndent(expected.Cases[i], "", "  ")
				t.Errorf("download record input contract differs\ngot: %s\nwant: %s", got, want)
			}
		})
	}
	t.Logf("replayed %d frozen download record input cases; fixture sha256 %x", len(actual.Cases), sha256.Sum256(data))
}

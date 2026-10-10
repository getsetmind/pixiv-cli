package protocol

import (
	"bytes"
	"compress/flate"
	"compress/gzip"
	"compress/zlib"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/andybalholm/brotli"
	fhttp "github.com/bogdanfinn/fhttp"
	tlsclient "github.com/bogdanfinn/tls-client"
	"github.com/klauspost/compress/zstd"
)

var migrationUpdateMediaBody = flag.Bool("migration-update-fanbox-media-body", false, "capture fixed Go in-memory media and production decoder contracts")

type migrationMediaSource struct {
	Root    string `json:"root"`
	Path    string `json:"path"`
	SHA256  string `json:"sha256"`
	GoFiles int    `json:"go_files,omitempty"`
}

type migrationMediaError struct {
	Type                  string `json:"type"`
	Message               string `json:"message"`
	EOF                   bool   `json:"eof"`
	UnexpectedEOF         bool   `json:"unexpected_eof"`
	Canceled              bool   `json:"canceled"`
	Deadline              bool   `json:"deadline"`
	SourceFailureRetained bool   `json:"source_failure_retained"`
}

type migrationMediaBodySpec struct {
	Status             int         `json:"status"`
	Encoding           string      `json:"encoding"`
	Wire               []byte      `json:"wire"`
	Header             http.Header `json:"header"`
	Chunk              int         `json:"chunk"`
	ReadError          string      `json:"read_error"`
	ErrorWithLastBytes bool        `json:"error_with_last_bytes"`
	CloseError         bool        `json:"close_error"`
	NilBody            bool        `json:"nil_body"`
	Length             int64       `json:"content_length"`
}

type migrationMediaAction struct {
	Kind string `json:"kind"`
	Size int    `json:"size,omitempty"`
}

type migrationMediaInput struct {
	Name           string                   `json:"name"`
	Family         string                   `json:"family"`
	URL            string                   `json:"url"`
	Request        MediaRequest             `json:"request"`
	Boundary       string                   `json:"boundary"`
	Bodies         []migrationMediaBodySpec `json:"bodies"`
	TransportError string                   `json:"transport_error"`
	Context        string                   `json:"context"`
	Actions        []migrationMediaAction   `json:"actions"`
}

type migrationMediaRequest struct {
	Method        string              `json:"method"`
	URL           string              `json:"url"`
	Host          string              `json:"host"`
	Headers       http.Header         `json:"headers"`
	ContextError  migrationMediaError `json:"context_error"`
	HasBody       bool                `json:"has_body"`
	ContentLength int64               `json:"content_length"`
}

type migrationMediaSourceRead struct {
	Requested int                 `json:"requested"`
	Returned  int                 `json:"returned"`
	Error     migrationMediaError `json:"error"`
}

type migrationMediaOwnership struct {
	BytesRead int                        `json:"bytes_read"`
	Reads     []migrationMediaSourceRead `json:"reads"`
	Closes    int                        `json:"closes"`
}

type migrationMediaStep struct {
	Action  migrationMediaAction      `json:"action"`
	Bytes   int                       `json:"bytes"`
	SHA256  string                    `json:"sha256"`
	Data    []byte                    `json:"data,omitempty"`
	Text    string                    `json:"text,omitempty"`
	Error   migrationMediaError       `json:"error"`
	Panic   string                    `json:"go_only_panic,omitempty"`
	Sources []migrationMediaOwnership `json:"sources"`
}

type migrationMediaResponse struct {
	Returned         bool        `json:"returned"`
	Status           int         `json:"status"`
	Headers          http.Header `json:"headers"`
	ContentLength    int64       `json:"content_length"`
	Uncompressed     bool        `json:"uncompressed"`
	Proto            string      `json:"proto"`
	ProtoMajor       int         `json:"proto_major"`
	ProtoMinor       int         `json:"proto_minor"`
	TransferEncoding []string    `json:"transfer_encoding"`
	Close            bool        `json:"close"`
	BodyType         string      `json:"go_only_body_type"`
	DecoderTypes     []string    `json:"go_only_decoder_types"`
}

type migrationMediaDecoderOperation struct {
	Body      int                 `json:"body"`
	Kind      string              `json:"kind"`
	Requested int                 `json:"requested"`
	Returned  int                 `json:"returned"`
	Error     migrationMediaError `json:"go_only_dependency_error"`
	Panic     string              `json:"go_only_panic,omitempty"`
}

type migrationMediaDecoderObserver struct {
	body       io.ReadCloser
	index      int
	operations *[]migrationMediaDecoderOperation
}

func (o *migrationMediaDecoderObserver) Read(p []byte) (n int, err error) {
	n, err = o.body.Read(p)
	*o.operations = append(*o.operations, migrationMediaDecoderOperation{Body: o.index, Kind: "read", Requested: len(p), Returned: n, Error: migrationMediaObserveError(err)})
	return n, err
}

func (o *migrationMediaDecoderObserver) Close() (err error) {
	operation := migrationMediaDecoderOperation{Body: o.index, Kind: "close"}
	defer func() {
		if value := recover(); value != nil {
			operation.Panic = fmt.Sprintf("%T: %v", value, value)
			*o.operations = append(*o.operations, operation)
			panic(value)
		}
		operation.Error = migrationMediaObserveError(err)
		*o.operations = append(*o.operations, operation)
	}()
	return o.body.Close()
}

type migrationMediaResult struct {
	DependencyOperations []migrationMediaDecoderOperation `json:"go_only_dependency_operations"`
	Requests             []migrationMediaRequest          `json:"requests"`
	Response             migrationMediaResponse           `json:"response"`
	OpenError            migrationMediaError              `json:"open_error"`
	AtReturn             []migrationMediaOwnership        `json:"ownership_at_return"`
	Steps                []migrationMediaStep             `json:"steps"`
	FinalSources         []migrationMediaOwnership        `json:"final_ownership"`
}

type migrationMediaCase struct {
	Input  migrationMediaInput  `json:"input"`
	Result migrationMediaResult `json:"result"`
}

type migrationMediaFixture struct {
	Reference   string                 `json:"reference"`
	GoVersion   string                 `json:"go_version"`
	GOMAXPROCS  int                    `json:"gomaxprocs"`
	Sources     []migrationMediaSource `json:"sources"`
	Evidence    string                 `json:"evidence"`
	Limitations []string               `json:"limitations"`
	Cases       []migrationMediaCase   `json:"cases"`
}

var migrationMediaSyntheticFailure = errors.New("synthetic transport/body failure canary")

type migrationMediaBody struct {
	spec      migrationMediaBodySpec
	position  int
	ownership migrationMediaOwnership
}

func (b *migrationMediaBody) Read(p []byte) (int, error) {
	count := len(p)
	if b.spec.Chunk > 0 && count > b.spec.Chunk {
		count = b.spec.Chunk
	}
	if count > len(b.spec.Wire)-b.position {
		count = len(b.spec.Wire) - b.position
	}
	copy(p, b.spec.Wire[b.position:b.position+count])
	b.position += count
	var err error
	if b.position == len(b.spec.Wire) && (count == 0 || b.spec.ErrorWithLastBytes) {
		err = migrationMediaInjectedError(b.spec.ReadError)
		if err == nil {
			err = io.EOF
		}
	}
	b.ownership.BytesRead += count
	b.ownership.Reads = append(b.ownership.Reads, migrationMediaSourceRead{len(p), count, migrationMediaObserveError(err)})
	return count, err
}

func (b *migrationMediaBody) Close() error {
	b.ownership.Closes++
	if b.spec.CloseError {
		return migrationMediaSyntheticFailure
	}
	return nil
}

type migrationMediaTransport func(*http.Request) (*http.Response, error)

func (f migrationMediaTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type migrationMediaTLSClient struct {
	tlsclient.HttpClient
	do func(*fhttp.Request) (*fhttp.Response, error)
}

func (c *migrationMediaTLSClient) Do(r *fhttp.Request) (*fhttp.Response, error) { return c.do(r) }
func (*migrationMediaTLSClient) CloseIdleConnections()                          {}

func TestMigrationFanboxMediaBodyContract(t *testing.T) {
	const reference = "4b4426487ef18bed276706daec385e0d0a6979f9"
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("requires fixed Go1.27.1, got %s", runtime.Version())
	}
	previous := runtime.GOMAXPROCS(1)
	defer runtime.GOMAXPROCS(previous)
	root := filepath.Join("..", "..", "..", "..")
	sources := migrationMediaVerifySources(t, root, reference)
	fixture := migrationMediaFixture{
		Reference: reference, GoVersion: runtime.Version(), GOMAXPROCS: 1, Sources: sources,
		Evidence: "Actual Session GET/standard Client.Do and browserTransport execute over owned in-memory injected HTTP/TLS-client boundaries. The TLS-client mock invokes the exact exported fhttp.DecompressBody dependency helper used by production; native HTTP1/H2 dispatch and wire framing are not executed.",
		Limitations: []string{
			"No socket, network, native TLS/HTTP peer, external media, authenticated account, trust/browser, HEAD or upload operation is executed.",
			"fhttp helper execution proves decoder/read/close behavior after native dispatch, not native Accept-Encoding insertion, HTTP1 header deletion, initial HEADERS END_STREAM, physical stream CANCEL or connection reuse. Those remain separate source/native-wire gaps.",
			"GOMAXPROCS=1 makes the production zstd default streaming decoder synchronous for stable ownership traces. Concurrent Go Read/Close and the asynchronous default under larger GOMAXPROCS remain separate Go-only/API gaps.",
			"Protocol media URLs accept an empty/root path; the resource endpoint adds a nonempty asset-path constraint. Opaque ref resolution, SDK once-owned response wrapper, file persistence and resource save/reopen are outside this slice.",
			"Decoder private type strings and recovered uninitialized-deflate Close panic types/messages are Go-only source observations, not Rust panic requirements.",
			"Header-only over-limit zstd frames reject before allocation. The 512MiB window and 64GiB memory defaults are source-anchored; no giant input or output allocation is attempted. Ordinary bounded decompressed streaming has no product-defined body cap.",
			"Recognized deflate eagerly buffers and closes its source; unrecognized/truncated sniffing loses consumed prefix bytes. The ignored io.Copy/Close failure and lazy-decoder Close panic are preserved source debts, not corrected expectations.",
			"Synthetic transport bodies deliberately define their own post-Close read behavior. Wrapper forwarding is observed; it does not establish native response-body behavior after closure.",
		},
		Cases: []migrationMediaCase{},
	}
	for _, input := range migrationMediaInputs(t) {
		t.Run(input.Name, func(t *testing.T) {
			result := migrationMediaCapture(t, input)
			migrationMediaCheckRequirement(t, input, result)
			fixture.Cases = append(fixture.Cases, migrationMediaCase{input, result})
		})
	}
	if t.Failed() {
		return
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-media-body.json")
	if *migrationUpdateMediaBody {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
	} else {
		frozen, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(data, frozen) {
			t.Fatal("media/body observations differ from fixed Go fixture")
		}
	}
	t.Logf("captured/replayed %d in-memory media/body rows", len(fixture.Cases))
}

func migrationMediaInputs(t *testing.T) []migrationMediaInput {
	t.Helper()
	const asset = "https://downloads.fanbox.cc/media/owned.bin"
	readAll := migrationMediaAction{Kind: "read_all"}
	closeBody := migrationMediaAction{Kind: "close"}
	readOne := migrationMediaAction{Kind: "read", Size: 1}
	plain := []byte("owned media payload")
	makeSpec := func(wire []byte, encoding string) migrationMediaBodySpec {
		return migrationMediaBodySpec{Status: 200, Encoding: encoding, Wire: wire, Header: http.Header{"X-Bridge": {"original", "second"}}, Length: int64(len(wire))}
	}
	makeInput := func(name, family, boundary string, spec migrationMediaBodySpec, actions ...migrationMediaAction) migrationMediaInput {
		return migrationMediaInput{Name: name, Family: family, URL: asset, Boundary: boundary, Bodies: []migrationMediaBodySpec{spec}, Actions: actions}
	}
	var inputs []migrationMediaInput
	for _, item := range []struct{ name, url string }{
		{"downloads_cookie", asset}, {"downloads_uppercase_cookie", "https://DOWNLOADS.FANBOX.CC:443/media/a"},
		{"cdn_session_free", "https://i.pximg.net/media/a"}, {"cdn_subdomain_session_free", "https://s.pximg.net/media/a"},
		{"pixiv_fanbox_session_free", "https://fanbox.pixiv.net/media/a"}, {"pixiv_fanbox_subdomain_session_free", "https://cdn.fanbox.pixiv.net/media/a"},
		{"www_session_free", "https://www.fanbox.cc/media/a"}, {"apex_root_protocol_only", "https://fanbox.cc/"},
		{"empty_path_protocol_only", "https://i.pximg.net"}, {"http_rejected", "http://i.pximg.net/media/a"},
		{"userinfo_rejected", "https://synthetic@downloads.fanbox.cc/media/a"}, {"suffix_confusion_rejected", "https://downloads.fanbox.cc.invalid/media/a"},
		{"pixiv_apex_rejected", "https://pixiv.net/media/a"},
	} {
		input := makeInput(item.name, "media_url_headers", "standard_injected", makeSpec(plain, ""), readAll, closeBody, closeBody)
		input.URL = item.url
		input.Request = MediaRequest{Method: " get ", Range: "bytes=1-3", IfNoneMatch: `"owned-etag"`, IfModifiedSince: "Wed, 21 Oct 2015 07:28:00 GMT", IfRange: `"owned-range"`}
		inputs = append(inputs, input)
	}
	redirect := makeSpec([]byte("owned redirect"), "")
	redirect.Status = 302
	redirect.Header.Set("Location", "https://i.pximg.net/media/final")
	input := makeInput("redirect_drops_session", "redirect_ownership", "standard_injected", redirect, readAll, closeBody)
	input.Request = MediaRequest{Range: "bytes=0-4", IfNoneMatch: `"owned-etag"`}
	input.Bodies = append(input.Bodies, makeSpec(plain, ""))
	inputs = append(inputs, input)
	for _, item := range []struct {
		name, location string
		closeFailure   bool
	}{
		{"redirect_rejected_host", "https://invalid.example/media/a", false},
		{"redirect_close_error_wins", "https://invalid.example/media/a", true},
		{"redirect_same_downloads_drops_session", "https://downloads.fanbox.cc/media/final", false},
	} {
		spec := redirect
		spec.Header = redirect.Header.Clone()
		spec.Header.Set("Location", item.location)
		spec.CloseError = item.closeFailure
		input := makeInput(item.name, "redirect_ownership", "standard_injected", spec, readAll, closeBody)
		if strings.Contains(item.name, "same_downloads") {
			input.Bodies = append(input.Bodies, makeSpec(plain, ""))
		}
		inputs = append(inputs, input)
	}
	for _, item := range []struct {
		name   string
		status int
		wire   string
	}{
		{"partial_content", 206, "owned media payload"}, {"not_modified", 304, ""}, {"unauthorized", 401, "ignored"},
		{"forbidden_scans", 403, "ordinary refusal"}, {"challenge_scans", 403, "prefix cf-chl suffix"}, {"server_error_no_scan", 500, "ignored"},
	} {
		spec := makeSpec([]byte(item.wire), "")
		spec.Status = item.status
		inputs = append(inputs, makeInput(item.name, "status_ownership", "standard_injected", spec, readAll, closeBody))
	}
	for _, item := range []struct {
		name     string
		boundary string
		size     int64
	}{
		{"nil_body_zero_length", "standard_injected", 0}, {"nil_body_positive_length", "standard_injected", 9},
		{"bridge_nil_body_zero_length", "fhttp_dependency_helper", 0},
	} {
		spec := makeSpec(nil, "")
		spec.NilBody = true
		spec.Length = item.size
		inputs = append(inputs, makeInput(item.name, "nil_body_client_contract", item.boundary, spec, readAll, closeBody))
	}
	for _, item := range []struct {
		name, readErr, ctx  string
		closeErr, withBytes bool
	}{
		{"partial_bytes_safe_failure", "failure", "", false, true}, {"partial_bytes_canceled", "canceled", "", false, true},
		{"partial_bytes_deadline", "deadline", "", false, true}, {"source_eof_bytes", "", "", false, true},
		{"read_and_close_errors", "failure", "", true, false}, {"canceled_context_overrides_stream", "failure", "cancel_after_open", true, false},
		{"close_error_repeated", "", "", true, false},
	} {
		spec := makeSpec(plain, "")
		spec.ReadError = item.readErr
		spec.ErrorWithLastBytes = item.withBytes
		spec.CloseError = item.closeErr
		input := makeInput(item.name, "safe_stream_errors", "standard_injected", spec, readAll, closeBody, closeBody)
		input.Context = item.ctx
		inputs = append(inputs, input)
	}
	for _, item := range []struct{ name, ctx, err string }{
		{"transport_secret_safe", "", "failure"}, {"transport_canceled", "", "canceled"},
		{"transport_context_overrides", "canceled", "failure"}, {"canceled_context_ignored_by_transport", "canceled", ""},
	} {
		input := makeInput(item.name, "transport_error_context", "standard_injected", makeSpec(plain, ""), readAll, closeBody)
		input.Context = item.ctx
		input.TransportError = item.err
		inputs = append(inputs, input)
	}
	for _, encoding := range []string{"gzip", "deflate", "br", "zstd"} {
		wire := migrationMediaCompress(t, encoding, plain)
		for _, stage := range []string{"complete", "unread_close", "partial_close", "read_after_close"} {
			actions := []migrationMediaAction{readAll, closeBody, closeBody}
			switch stage {
			case "unread_close":
				actions = []migrationMediaAction{closeBody, closeBody}
			case "partial_close":
				actions = []migrationMediaAction{readOne, closeBody, closeBody}
			case "read_after_close":
				actions = []migrationMediaAction{readOne, closeBody, readAll, closeBody}
			}
			inputs = append(inputs, makeInput(encoding+"_"+stage, "production_decoder_ownership", "fhttp_dependency_helper", makeSpec(wire, encoding), actions...))
		}
		bad := append([]byte(nil), wire[:len(wire)-3]...)
		inputs = append(inputs, makeInput(encoding+"_truncated", "production_decoder_error", "fhttp_dependency_helper", makeSpec(bad, encoding), readAll, readOne, closeBody, closeBody))
		failed := makeSpec(wire, encoding)
		failed.ReadError = "failure"
		failed.CloseError = true
		inputs = append(inputs, makeInput(encoding+"_source_errors", "production_decoder_error", "fhttp_dependency_helper", failed, readAll, closeBody, closeBody))
	}
	for _, item := range []struct {
		name, encoding string
		wire           []byte
	}{
		{"gzip_invalid_header", "gzip", []byte("invalid gzip")}, {"gzip_checksum_error", "gzip", nil},
		{"deflate_raw_prefix_lost", "deflate", migrationMediaCompress(t, "raw_deflate", plain)},
		{"deflate_short_sniff_lost", "deflate", []byte{0x78}},
		{"deflate_unrecognized_sniff_lost", "deflate", []byte("raw body")},
		{"deflate_raw_78_branch", "deflate", []byte{0x78, 0x00, 0xff, 0xff, 0x01, 0x00, 0x00, 0xff, 0xff}},
		{"br_invalid_header", "br", []byte{0xff, 0xff, 0xff}},
		{"br_excessive_input", "br", append(migrationMediaCompress(t, "br", plain), 0x00)},
		{"zstd_invalid_magic", "zstd", []byte("invalid zstd")},
		{"zstd_window_above_default", "zstd", []byte{0x28, 0xb5, 0x2f, 0xfd, 0x00, 0xa0, 0x01, 0x00, 0x00}},
		{"zstd_single_segment_above_memory_default", "zstd", nil},
		{"encoding_case_sensitive_passthrough", "GZIP", migrationMediaCompress(t, "gzip", plain)},
		{"encoding_list_passthrough", "gzip, br", migrationMediaCompress(t, "gzip", plain)},
		{"unknown_encoding_passthrough", "owned-unknown", plain},
	} {
		if item.name == "gzip_checksum_error" {
			item.wire = migrationMediaCompress(t, "gzip", plain)
			item.wire[len(item.wire)-8] ^= 0xff
		}
		if item.name == "zstd_single_segment_above_memory_default" {
			item.wire = []byte{0x28, 0xb5, 0x2f, 0xfd, 0xe0}
			item.wire = binary.LittleEndian.AppendUint64(item.wire, (64<<30)+1)
			item.wire = append(item.wire, 1, 0, 0)
		}
		inputs = append(inputs, makeInput(item.name, "decoder_format_boundaries", "fhttp_dependency_helper", makeSpec(item.wire, item.encoding), readAll, readOne, closeBody, closeBody))
	}
	input = makeInput("standard_injection_does_not_decode", "injection_boundary_distinction", "standard_injected", makeSpec(migrationMediaCompress(t, "gzip", plain), "gzip"), readAll, closeBody)
	inputs = append(inputs, input)
	input = makeInput("bridge_metadata_is_copied", "bridge_metadata_ownership", "fhttp_dependency_helper", makeSpec(plain, ""), migrationMediaAction{Kind: "mutate_dependency_metadata"}, readAll, closeBody)
	inputs = append(inputs, input)
	for _, size := range []int{5, 65536} {
		payload := bytes.Repeat([]byte("B"), size)
		spec := makeSpec(migrationMediaCompress(t, "br", payload), "br")
		input := makeInput(fmt.Sprintf("br_buffered_tiny_reads_%d", size), "buffered_brotli_output", "fhttp_dependency_helper", spec, readOne, readOne, migrationMediaAction{Kind: "read", Size: 3}, readAll, closeBody, closeBody)
		inputs = append(inputs, input)
	}
	large := bytes.Repeat([]byte("owned bounded uncapped body\n"), 4096)
	inputs = append(inputs, makeInput("gzip_bounded_expansion_no_product_cap", "bounded_stream_size", "fhttp_dependency_helper", makeSpec(migrationMediaCompress(t, "gzip", large), "gzip"), readAll, closeBody))
	return inputs
}

func migrationMediaCompress(t *testing.T, encoding string, payload []byte) []byte {
	t.Helper()
	var buffer bytes.Buffer
	var writer io.WriteCloser
	var err error
	switch encoding {
	case "gzip":
		writer = gzip.NewWriter(&buffer)
	case "deflate":
		writer = zlib.NewWriter(&buffer)
	case "raw_deflate":
		writer, err = flate.NewWriter(&buffer, flate.DefaultCompression)
	case "br":
		writer = brotli.NewWriterLevel(&buffer, 5)
	case "zstd":
		writer, err = zstd.NewWriter(&buffer, zstd.WithEncoderConcurrency(1))
	default:
		t.Fatalf("unknown synthetic encoder %s", encoding)
	}
	if err != nil {
		t.Fatal(err)
	}
	if _, err = writer.Write(payload); err != nil {
		t.Fatal(err)
	}
	if err = writer.Close(); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func migrationMediaCapture(t *testing.T, input migrationMediaInput) migrationMediaResult {
	t.Helper()
	result := migrationMediaResult{Requests: []migrationMediaRequest{}, Steps: []migrationMediaStep{}, DependencyOperations: []migrationMediaDecoderOperation{}}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if input.Context == "canceled" {
		cancel()
	}
	bodies := make([]*migrationMediaBody, len(input.Bodies))
	for i, spec := range input.Bodies {
		bodies[i] = &migrationMediaBody{spec: spec, ownership: migrationMediaOwnership{Reads: []migrationMediaSourceRead{}}}
	}
	snapshot := func() []migrationMediaOwnership {
		out := make([]migrationMediaOwnership, len(bodies))
		for i, b := range bodies {
			out[i] = b.ownership
			out[i].Reads = append([]migrationMediaSourceRead{}, b.ownership.Reads...)
		}
		return out
	}
	index := 0
	var dependencyResponse *fhttp.Response
	decoderTypes := []string{}
	next := func(request migrationMediaRequest) (*migrationMediaBody, error) {
		result.Requests = append(result.Requests, request)
		if err := migrationMediaInjectedError(input.TransportError); err != nil {
			return nil, err
		}
		if index >= len(bodies) {
			t.Fatal("unexpected in-memory transport request")
		}
		body := bodies[index]
		index++
		return body, nil
	}
	var transport http.RoundTripper
	if input.Boundary == "standard_injected" {
		transport = migrationMediaTransport(func(r *http.Request) (*http.Response, error) {
			b, err := next(migrationMediaRequest{r.Method, r.URL.String(), r.Host, r.Header.Clone(), migrationMediaObserveError(r.Context().Err()), r.Body != nil, r.ContentLength})
			if err != nil {
				return nil, err
			}
			header := b.spec.Header.Clone()
			if b.spec.Encoding != "" {
				header.Set("Content-Encoding", b.spec.Encoding)
			}
			header.Set("Content-Length", fmt.Sprint(b.spec.Length))
			var body io.ReadCloser = b
			if b.spec.NilBody {
				body = nil
			}
			return &http.Response{StatusCode: b.spec.Status, Header: header, Body: body, ContentLength: b.spec.Length, Request: r}, nil
		})
	} else {
		transport = &browserTransport{client: &migrationMediaTLSClient{do: func(r *fhttp.Request) (*fhttp.Response, error) {
			header := make(http.Header, len(r.Header))
			for k, v := range r.Header {
				header[k] = append([]string(nil), v...)
			}
			b, err := next(migrationMediaRequest{r.Method, r.URL.String(), r.Host, header, migrationMediaObserveError(r.Context().Err()), r.Body != nil, r.ContentLength})
			if err != nil {
				return nil, err
			}
			fh := make(fhttp.Header)
			for k, v := range b.spec.Header {
				fh[k] = append([]string(nil), v...)
			}
			if b.spec.Encoding != "" {
				fh.Set("Content-Encoding", b.spec.Encoding)
			}
			fh.Set("Content-Length", fmt.Sprint(b.spec.Length))
			var body io.ReadCloser = b
			if b.spec.NilBody {
				body = nil
			}
			response := &fhttp.Response{Status: "owned synthetic response", StatusCode: b.spec.Status, Header: fh, Body: body, ContentLength: b.spec.Length, Proto: "HTTP/2.0", ProtoMajor: 2, ProtoMinor: 0, TransferEncoding: []string{"owned-transfer"}, Close: true, Request: r}
			if body != nil {
				response.Body = fhttp.DecompressBody(response)
			}
			decoderTypes = append(decoderTypes, fmt.Sprintf("%T", response.Body))
			if response.Body != nil {
				response.Body = &migrationMediaDecoderObserver{body: response.Body, index: index - 1, operations: &result.DependencyOperations}
			}
			dependencyResponse = response
			return response, nil
		}}}
	}
	session, err := NewSessionWithOptions("FANBOXSESSID=synthetic-owned-media", SessionOptions{HTTPClient: &http.Client{Transport: transport}, UserAgent: "owned-media-agent"})
	if err != nil {
		t.Fatal(err)
	}
	response, err := session.OpenMediaWithRequest(ctx, input.URL, input.Request)
	result.OpenError = migrationMediaObserveError(err)
	result.AtReturn = snapshot()
	if input.Context == "cancel_after_open" {
		cancel()
	}
	if response != nil {
		for _, action := range input.Actions {
			step := migrationMediaStep{Action: action}
			func() {
				defer func() {
					if value := recover(); value != nil {
						step.Panic = fmt.Sprintf("%T: %v", value, value)
					}
				}()
				var data []byte
				var err error
				switch action.Kind {
				case "read_all":
					data, err = io.ReadAll(response.Body)
				case "read":
					data = make([]byte, action.Size)
					var count int
					count, err = response.Body.Read(data)
					data = data[:count]
				case "close":
					err = response.Body.Close()
				case "mutate_dependency_metadata":
					dependencyResponse.Header.Set("X-Bridge", "mutated")
					dependencyResponse.TransferEncoding[0] = "mutated"
				default:
					t.Fatalf("unknown action %s", action.Kind)
				}
				step.Bytes = len(data)
				if data != nil {
					step.SHA256 = fmt.Sprintf("%x", sha256.Sum256(data))
					if len(data) <= 256 {
						step.Data = append([]byte{}, data...)
						if utf8.Valid(data) {
							step.Text = string(data)
						}
					}
				}
				step.Error = migrationMediaObserveError(err)
			}()
			step.Sources = snapshot()
			result.Steps = append(result.Steps, step)
		}
		result.Response = migrationMediaResponse{true, response.StatusCode, response.Header.Clone(), response.ContentLength, response.Uncompressed, response.Proto, response.ProtoMajor, response.ProtoMinor, append([]string{}, response.TransferEncoding...), response.Close, fmt.Sprintf("%T", response.Body), decoderTypes}
	} else {
		result.Response.Headers = http.Header{}
		result.Response.TransferEncoding = []string{}
		result.Response.DecoderTypes = decoderTypes
	}
	result.FinalSources = snapshot()
	return result
}

func migrationMediaInjectedError(kind string) error {
	switch kind {
	case "failure":
		return migrationMediaSyntheticFailure
	case "canceled":
		return fmt.Errorf("synthetic wrapped cancellation: %w", context.Canceled)
	case "deadline":
		return fmt.Errorf("synthetic wrapped deadline: %w", context.DeadlineExceeded)
	case "":
		return nil
	default:
		panic("unknown synthetic error kind")
	}
}

func migrationMediaObserveError(err error) migrationMediaError {
	if err == nil {
		return migrationMediaError{}
	}
	return migrationMediaError{fmt.Sprintf("%T", err), err.Error(), errors.Is(err, io.EOF), errors.Is(err, io.ErrUnexpectedEOF), errors.Is(err, context.Canceled), errors.Is(err, context.DeadlineExceeded), errors.Is(err, migrationMediaSyntheticFailure)}
}

func migrationMediaCheckRequirement(t *testing.T, input migrationMediaInput, result migrationMediaResult) {
	t.Helper()
	for _, step := range result.Steps {
		if step.Error.SourceFailureRetained || strings.Contains(step.Error.Message, "canary") {
			t.Fatal("safe media wrapper retained synthetic external failure")
		}
	}
	if result.OpenError.SourceFailureRetained || strings.Contains(result.OpenError.Message, "canary") {
		t.Fatal("safe open retained synthetic external failure")
	}
	if strings.HasSuffix(input.Name, "_complete") {
		if !result.Response.Returned || len(result.Steps) != 3 || result.Steps[0].Text != "owned media payload" || result.Steps[0].Error.Message != "" {
			t.Fatal("complete production decoder failed to reproduce media payload")
		}
	}
	if input.Family == "buffered_brotli_output" {
		if result.Steps[0].Bytes != 1 || result.Steps[1].Bytes != 1 || result.Steps[2].Bytes != 3 || len(result.Steps[0].Sources[0].Reads) != len(result.Steps[1].Sources[0].Reads) || len(result.Steps[1].Sources[0].Reads) != len(result.Steps[2].Sources[0].Reads) {
			t.Fatal("buffered Brotli output refilled source during tiny reads")
		}
	}
	if strings.HasSuffix(input.Name, "_complete") {
		closes := result.FinalSources[0].Closes
		if strings.HasPrefix(input.Name, "deflate_") {
			if result.AtReturn[0].Closes != 1 || closes != 1 {
				t.Fatal("recognized deflate must own eager source close")
			}
		} else if result.AtReturn[0].Closes != 0 || closes != 2 {
			t.Fatal("repeated decoder Close must forward to owned source twice")
		}
	}
	if input.Name == "deflate_source_errors" && (result.AtReturn[0].Closes != 1 || result.Steps[0].Error.Message != "" || result.Steps[1].Error.Message != "") {
		t.Fatal("recognized deflate eager copy/close errors must remain ignored")
	}
	if input.Name == "zstd_window_above_default" && result.DependencyOperations[0].Error.Message != "window size exceeded" {
		t.Fatal("wrong source zstd window-limit failure")
	}
	if input.Name == "zstd_single_segment_above_memory_default" && result.DependencyOperations[0].Error.Message != "decompressed size exceeds configured limit" {
		t.Fatal("wrong source zstd memory-limit failure")
	}
	if input.Name == "bridge_metadata_is_copied" && (result.Response.Headers.Get("X-Bridge") != "original" || result.Response.TransferEncoding[0] != "owned-transfer") {
		t.Fatal("browser transport aliased dependency response metadata")
	}
	if strings.Contains(input.Name, "_unread_close") && strings.HasPrefix(input.Name, "deflate") && (result.Steps[0].Panic == "" || result.Steps[1].Panic == "") {
		t.Fatal("uninitialized deflate Close source observation changed")
	}
	if strings.Contains(input.Name, "above_") && (len(result.Steps) == 0 || result.Steps[0].Error.Message != "read FANBOX media failed" || result.Steps[0].Bytes != 0) {
		t.Fatal("header-only zstd over-limit frame did not reject safely")
	}
}

func migrationMediaVerifySources(t *testing.T, root, reference string) []migrationMediaSource {
	t.Helper()
	sources := []migrationMediaSource{
		{"repo", "internal/services/fanbox/protocol/protocol.go", "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3", 0},
		{"repo", "internal/services/fanbox/protocol/solver.go", "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea", 0},
		{"repo", "internal/services/fanbox/protocol/cookie.go", "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9", 0},
		{"repo", "internal/shared/diagnostics/diagnostics.go", "aecd4045f50f6bcf4cd20f316d680cbfb28e657f919ce1c0700dfdad8d5ddc76", 0},
		{"repo", "go.mod", "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c", 0},
		{"repo", "go.sum", "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e", 0},
		{"module", "github.com/bogdanfinn/fhttp@v0.6.8", "4e627e357d5623ce22dd89ade3a5b104af6e5a92446379b16ba0c02dbf4af9d0", 82},
		{"module", "github.com/andybalholm/brotli@v1.2.0", "682cce4a6de0df33b8c0e185a5820f6809ef5a4bad41ea7ce8c2e518582511e5", 68},
		{"module", "github.com/klauspost/compress@v1.18.2", "952a01484046f35b7e96779b74d59c966dac86721ea5d5b65ed5a5956f733cb0", 134},
		{"stdlib", "compress/gzip", "d132a3032c0cc38890c7b85a3f08dce9317de0d5b935b258bce3933d817a89cc", 2},
		{"stdlib", "compress/flate", "3955db2e51012edc80a879fa2b2dd9a446f86660c165a270c0ec32d305b2af31", 16},
		{"stdlib", "compress/zlib", "f7d61b59b7f18ebaf5dbd6deb2906633266acc944daf3240418b51f552ad251e", 2},
		{"stdlib", "net/http/client.go", "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9", 0},
		{"stdlib", "net/http/response.go", "0b32a0b4ee51e00e3410764f3b2ffe1163dccb9cad548b097f47db85893a2f44", 0},
	}
	moduleRoot := os.Getenv("GOMODCACHE")
	if moduleRoot == "" {
		t.Fatal("requires canonical GOMODCACHE")
	}
	for _, source := range sources {
		base := root
		switch source.Root {
		case "module":
			base = moduleRoot
		case "stdlib":
			base = filepath.Join(runtime.GOROOT(), "src")
		}
		path := filepath.Join(base, source.Path)
		var hash string
		if source.GoFiles == 0 {
			data, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			hash = fmt.Sprintf("%x", sha256.Sum256(data))
			if source.Root == "repo" {
				command := exec.Command("git", "show", reference+":"+source.Path)
				command.Dir = root
				frozen, err := command.Output()
				if err != nil {
					t.Fatal(err)
				}
				if !bytes.Equal(data, frozen) {
					t.Fatalf("production/module source differs from frozen Go: %s", source.Path)
				}
			}
		} else {
			var entries []string
			err := filepath.WalkDir(path, func(name string, entry fs.DirEntry, err error) error {
				if err != nil {
					return err
				}
				if entry.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
					return nil
				}
				data, err := os.ReadFile(name)
				if err != nil {
					return err
				}
				relative, err := filepath.Rel(path, name)
				if err != nil {
					return err
				}
				entries = append(entries, filepath.ToSlash(relative)+"\x00"+hex.EncodeToString(migrationMediaSHA(data))+"\n")
				return nil
			})
			if err != nil {
				t.Fatal(err)
			}
			sort.Strings(entries)
			if len(entries) != source.GoFiles {
				t.Fatalf("source inventory count changed: %s", source.Path)
			}
			hash = fmt.Sprintf("%x", sha256.Sum256([]byte(strings.Join(entries, ""))))
		}
		if hash != source.SHA256 {
			t.Fatalf("source hash changed: %s got %s", source.Path, hash)
		}
	}
	return sources
}

func migrationMediaSHA(data []byte) []byte { hash := sha256.Sum256(data); return hash[:] }

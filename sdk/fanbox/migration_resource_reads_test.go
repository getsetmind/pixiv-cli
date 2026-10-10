package fanbox_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
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
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxResourceReads = flag.Bool("migration-capture-fanbox-resource-reads", false, "capture frozen Go public FANBOX resource reads with an owned in-memory transport")

const migrationResourceSession = "resource-session-secret-canary"
const migrationResourceSecret = "resource-raw-error-secret-canary"

type migrationResourceBodySpec struct {
	Status             int         `json:"status"`
	Header             http.Header `json:"header"`
	Data               string      `json:"data"`
	Wire               []byte      `json:"wire,omitempty"`
	Chunk              int         `json:"chunk"`
	ReadError          string      `json:"read_error"`
	ErrorWithLastBytes bool        `json:"error_with_last_bytes"`
	CloseError         string      `json:"close_error"`
	NilBody            bool        `json:"nil_body"`
	Length             int64       `json:"transport_content_length"`
	CancelOnRead       bool        `json:"cancel_on_read"`
	CancelOnClose      bool        `json:"cancel_on_close"`
}

type migrationResourceAction struct {
	Kind string `json:"kind"`
	Size int    `json:"size,omitempty"`
}

type migrationResourceInput struct {
	Kind                     string                      `json:"kind"`
	Mode                     string                      `json:"mode"`
	ProducerDocument         string                      `json:"producer_document"`
	ReopenDocument           string                      `json:"reopen_document"`
	RefProduct               string                      `json:"ref_product"`
	RefPayload               string                      `json:"ref_payload"`
	ParseText                string                      `json:"parse_text"`
	ParseOnly                bool                        `json:"parse_only"`
	Method                   string                      `json:"method"`
	Range                    string                      `json:"range"`
	IfNoneMatch              string                      `json:"if_none_match"`
	IfModifiedSince          string                      `json:"if_modified_since"`
	IfRange                  string                      `json:"if_range"`
	Context                  string                      `json:"context"`
	TransportError           string                      `json:"transport_error"`
	TransportErrorAtMetadata bool                        `json:"transport_error_at_metadata"`
	Metadata                 migrationResourceBodySpec   `json:"metadata"`
	Media                    []migrationResourceBodySpec `json:"media"`
	Actions                  []migrationResourceAction   `json:"actions"`
	Repeat                   int                         `json:"repeat"`
	MutateResourceHeader     bool                        `json:"mutate_resource_header"`
}

type migrationResourceRow struct {
	Name        string                 `json:"name"`
	Input       migrationResourceInput `json:"input"`
	Observation map[string]any         `json:"observation"`
}

type migrationResourceBody struct {
	spec   migrationResourceBodySpec
	name   string
	offset int
	closes int
	reads  []map[string]any
	cancel context.CancelFunc
}

func migrationResourceRawError(kind string) error {
	switch kind {
	case "":
		return nil
	case "EOF":
		return io.EOF
	case "wrapped_EOF":
		return fmt.Errorf("%s: %w", migrationResourceSecret, io.EOF)
	case "unexpected_EOF":
		return io.ErrUnexpectedEOF
	case "canceled":
		return fmt.Errorf("%s: %w", migrationResourceSecret, context.Canceled)
	case "deadline":
		return fmt.Errorf("%s: %w", migrationResourceSecret, context.DeadlineExceeded)
	case "canceled_then_deadline":
		return errors.Join(context.Canceled, context.DeadlineExceeded, errors.New(migrationResourceSecret))
	case "deadline_then_canceled":
		return errors.Join(context.DeadlineExceeded, context.Canceled, errors.New(migrationResourceSecret))
	default:
		return errors.New(migrationResourceSecret)
	}
}

func (b *migrationResourceBody) Read(p []byte) (int, error) {
	data := b.spec.Wire
	if data == nil {
		data = []byte(b.spec.Data)
	}
	n := len(data) - b.offset
	if n > len(p) {
		n = len(p)
	}
	if b.spec.Chunk > 0 && n > b.spec.Chunk {
		n = b.spec.Chunk
	}
	copy(p, data[b.offset:b.offset+n])
	b.offset += n
	var err error
	if b.offset == len(data) && (n == 0 || b.spec.ErrorWithLastBytes) {
		err = migrationResourceRawError(b.spec.ReadError)
		if err == nil && n == 0 {
			err = io.EOF
		}
	}
	if b.spec.CancelOnRead {
		b.cancel()
	}
	code := ""
	if err != nil {
		code = b.spec.ReadError
		if code == "" {
			code = "EOF"
		}
	}
	b.reads = append(b.reads, map[string]any{"requested": len(p), "returned": n, "error_kind": code, "after_close": b.closes > 0})
	return n, err
}

func (b *migrationResourceBody) Close() error {
	b.closes++
	if b.spec.CancelOnClose {
		b.cancel()
	}
	return migrationResourceRawError(b.spec.CloseError)
}

func (b *migrationResourceBody) observation() map[string]any {
	return map[string]any{"name": b.name, "bytes_read": b.offset, "read_calls": b.reads, "close_calls": b.closes}
}

type migrationResourceTransport struct {
	t          *testing.T
	role       string
	document   string
	input      migrationResourceInput
	requests   []map[string]any
	bodies     []*migrationResourceBody
	mediaIndex int
	idleCalls  int
	cancel     context.CancelFunc
}

func (r *migrationResourceTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	r.requests = append(r.requests, map[string]any{"client": r.role, "method": request.Method, "url": request.URL.String(), "host": request.Host, "headers": request.Header.Clone(), "has_body": request.Body != nil, "content_length": request.ContentLength, "context_error": migrationResourceError(request.Context().Err()), "context_value": request.Context().Value(migrationResourceContextKey{})})
	metadata := request.URL.Hostname() == "api.fanbox.cc" && (request.URL.Path == "/creator.get" || request.URL.Path == "/post.info")
	if r.input.TransportError != "" && (!metadata || (r.role == "consumer" && r.input.TransportErrorAtMetadata)) {
		return nil, migrationResourceRawError(r.input.TransportError)
	}
	var spec migrationResourceBodySpec
	var name string
	if metadata {
		spec = r.input.Metadata
		if r.role == "producer" {
			spec = migrationResourceBodySpec{}
		}
		spec.Data = r.document
		if spec.Status == 0 {
			spec.Status = 200
		}
		if spec.Header == nil {
			spec.Header = http.Header{"Content-Type": {"application/json"}}
		}
		name = r.role + "/metadata"
	} else {
		if r.mediaIndex >= len(r.input.Media) {
			r.t.Fatalf("unexpected synthetic media request %d: %s", r.mediaIndex, request.URL.String())
		}
		spec = r.input.Media[r.mediaIndex]
		name = fmt.Sprintf("%s/media/%d", r.role, r.mediaIndex)
		r.mediaIndex++
	}
	var body io.ReadCloser
	if !spec.NilBody {
		source := &migrationResourceBody{spec: spec, name: name, cancel: r.cancel, reads: []map[string]any{}}
		r.bodies = append(r.bodies, source)
		body = source
	}
	return &http.Response{StatusCode: spec.Status, Header: spec.Header.Clone(), Body: body, ContentLength: spec.Length, Request: request}, nil
}

func (r *migrationResourceTransport) CloseIdleConnections() { r.idleCalls++ }

type migrationResourceContextKey struct{}

func migrationResourceError(err error) map[string]any {
	out := map[string]any{"message": "", "reason": sdk.ReasonOf(err), "canceled": errors.Is(err, context.Canceled), "deadline_exceeded": errors.Is(err, context.DeadlineExceeded), "eof": errors.Is(err, io.EOF), "exact_eof": err == io.EOF, "unexpected_eof": errors.Is(err, io.ErrUnexpectedEOF), "go_only_error_tree": []map[string]string{}}
	if err == nil {
		return out
	}
	out["message"] = err.Error()
	var tree []map[string]string
	var walk func(error)
	walk = func(e error) {
		if e == nil {
			return
		}
		tree = append(tree, map[string]string{"type": fmt.Sprintf("%T", e), "message": e.Error()})
		if joined, ok := e.(interface{ Unwrap() []error }); ok {
			for _, child := range joined.Unwrap() {
				walk(child)
			}
		} else {
			walk(errors.Unwrap(e))
		}
	}
	walk(err)
	out["go_only_error_tree"] = tree
	var classified *sdk.Error
	if errors.As(err, &classified) {
		out["sdk"] = map[string]any{"product": classified.Product, "operation": classified.Operation, "detail": classified.Detail, "http_status": classified.HTTPStatus, "transport": classified.Transport, "retry_safe": classified.Retry.Safe, "retry_has_after": classified.Retry.HasAfter}
	}
	for _, entry := range tree {
		if strings.Contains(entry["message"], migrationResourceSecret) || strings.Contains(entry["message"], migrationResourceSession) {
			panic("public error chain leaked a synthetic secret")
		}
	}
	return out
}

func migrationResourceSnapshot(t *testing.T, resource sdk.Resource) map[string]any {
	t.Helper()
	payload, err := sdk.ResourceRefPayload(resource.Ref)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(payload, []byte("https://")) || bytes.Contains(payload, []byte("signature")) || bytes.Contains(payload, []byte(migrationResourceSession)) {
		t.Fatal("generated reference leaked locator or credentials")
	}
	return map[string]any{"ref": resource.Ref.String(), "payload": string(payload), "url": resource.URL, "request_headers": resource.Copy().RequestHeaders, "expires_at": resource.ExpiresAt, "requires_credentials": resource.RequiresCredentials, "dto": sdk.ToResourceDTO(resource)}
}

func migrationResourceGenerate(t *testing.T, ctx context.Context, client *fanbox.Client, kind string) (sdk.Resource, []map[string]any, error) {
	t.Helper()
	all := []map[string]any{}
	if strings.HasPrefix(kind, "creator_") {
		creator, err := client.Creator(ctx, fanbox.CreatorRequest{CreatorID: "resource-creator"})
		if err != nil {
			return sdk.Resource{}, all, err
		}
		if !creator.Icon.Resource.Ref.IsZero() {
			all = append(all, migrationResourceSnapshot(t, creator.Icon.Resource))
		}
		if !creator.Cover.Resource.Ref.IsZero() {
			all = append(all, migrationResourceSnapshot(t, creator.Cover.Resource))
		}
		if kind == "creator_icon" {
			return creator.Icon.Resource, all, nil
		}
		return creator.Cover.Resource, all, nil
	}
	post, err := client.Post(ctx, fanbox.PostRequest{PostID: "resource-post"})
	if err != nil {
		return sdk.Resource{}, all, err
	}
	var selected sdk.Resource
	if post.Body != nil {
		for _, asset := range post.Body.Assets {
			all = append(all, migrationResourceSnapshot(t, asset.Resource))
			if "post_"+string(asset.Kind) == kind {
				selected = asset.Resource
			}
		}
	}
	if !selected.Ref.IsZero() {
		return selected, all, nil
	}
	return sdk.Resource{}, all, errors.New("synthetic producer did not emit selected resource")
}

func migrationResourceObserve(t *testing.T, input migrationResourceInput) map[string]any {
	t.Helper()
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationResourceContextKey{}, "resource-context"))
	defer cancel()
	var stopDeadline context.CancelFunc = func() {}
	producerTransport := &migrationResourceTransport{t: t, role: "producer", document: input.ProducerDocument, input: input, requests: []map[string]any{}, bodies: []*migrationResourceBody{}, cancel: cancel}
	consumerTransport := &migrationResourceTransport{t: t, role: "consumer", document: input.ReopenDocument, input: input, requests: []map[string]any{}, bodies: []*migrationResourceBody{}, cancel: cancel}
	open := func(transport *migrationResourceTransport) *fanbox.Client {
		client, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationResourceSession}, fanbox.Options{HTTPClient: &http.Client{Transport: transport}, UserAgent: "resource-injected-agent"})
		if err != nil {
			t.Fatal(err)
		}
		return client
	}
	producer := open(producerTransport)
	client := producer
	var consumer *fanbox.Client
	if input.Mode == "fresh" {
		consumer = open(consumerTransport)
		client = consumer
	}
	out := map[string]any{"generated_resources": []map[string]any{}, "generation_error": migrationResourceError(nil), "reference_error": migrationResourceError(nil), "outcomes": []map[string]any{}}
	var ref sdk.ResourceRef
	if input.Kind != "" {
		resource, all, err := migrationResourceGenerate(t, ctx, producer, input.Kind)
		out["generation_error"] = migrationResourceError(err)
		out["generated_resources"] = all
		ref = resource.Ref
		if input.MutateResourceHeader && resource.RequestHeaders != nil {
			resource.RequestHeaders["Cookie"] = migrationResourceSecret
			resource.RequestHeaders["Referer"] = "https://untrusted.invalid/"
		}
		if err != nil {
			goto finished
		}
	} else if input.RefPayload != "" {
		var err error
		ref, err = sdk.NewResourceRef(input.RefProduct, []byte(input.RefPayload))
		out["reference_error"] = migrationResourceError(err)
		if err != nil {
			goto finished
		}
	} else if input.ParseOnly || input.ParseText != "" {
		var err error
		ref, err = sdk.ParseResourceRef(input.ParseText)
		out["reference_error"] = migrationResourceError(err)
		if err != nil || input.ParseOnly {
			goto finished
		}
	}
	out["selected_ref"] = ref.String()
	if input.Context == "canceled" {
		cancel()
	}
	if input.Context == "deadline" {
		ctx, stopDeadline = context.WithDeadline(ctx, time.Unix(1, 0))
		defer stopDeadline()
	}
	{
		outcomes := []map[string]any{}
		repeat := input.Repeat
		if repeat == 0 {
			repeat = 1
		}
		for index := 0; index < repeat; index++ {
			response, err := client.OpenResource(ctx, sdk.OpenResourceRequest{Ref: ref, Method: sdk.ResourceMethod(input.Method), Range: input.Range, IfNoneMatch: input.IfNoneMatch, IfModifiedSince: input.IfModifiedSince, IfRange: input.IfRange})
			result := map[string]any{"returned": response != nil, "open_error": migrationResourceError(err), "steps": []map[string]any{}, "ownership_at_return": migrationResourceOwnership(producerTransport, consumerTransport)}
			if response != nil {
				header := response.Header()
				result["response"] = map[string]any{"status": response.StatusCode, "headers": header.Clone(), "content_type": response.ContentType(), "content_length": response.ContentLength(), "content_range": response.ContentRange(), "accept_ranges": response.AcceptRanges(), "etag": response.ETag(), "last_modified": response.LastModified(), "cache_control": response.CacheControl(), "body_non_nil": response.Body != nil}
				if values := header["Content-Type"]; len(values) > 0 {
					values[0] = "mutated-header"
				}
				header.Set("Set-Cookie", migrationResourceSecret)
				result["headers_after_caller_copy_mutation"] = response.Header()
				steps := []map[string]any{}
				for _, action := range input.Actions {
					step := map[string]any{"action": action, "data": "", "bytes": 0, "error": migrationResourceError(nil)}
					switch action.Kind {
					case "read":
						p := make([]byte, action.Size)
						n, readErr := response.Body.Read(p)
						step["data_bytes"] = append([]byte{}, p[:n]...)
						step["data_hex"] = fmt.Sprintf("%x", p[:n])
						step["data_sha256"] = fmt.Sprintf("%x", sha256.Sum256(p[:n]))
						if utf8.Valid(p[:n]) {
							step["data"] = string(p[:n])
						} else {
							step["data"] = nil
						}
						step["bytes"] = n
						step["error"] = migrationResourceError(readErr)
					case "close":
						step["error"] = migrationResourceError(response.Body.Close())
					case "cancel":
						cancel()
					case "idle":
						client.CloseIdleConnections()
					default:
						t.Fatalf("unknown resource action %s", action.Kind)
					}
					step["ownership"] = migrationResourceOwnership(producerTransport, consumerTransport)
					steps = append(steps, step)
				}
				result["steps"] = steps
			}
			outcomes = append(outcomes, result)
		}
		out["outcomes"] = outcomes
	}
finished:
	producer.CloseIdleConnections()
	producer.CloseIdleConnections()
	if consumer != nil {
		consumer.CloseIdleConnections()
		consumer.CloseIdleConnections()
	}
	out["requests"] = append(producerTransport.requests, consumerTransport.requests...)
	out["final_ownership"] = migrationResourceOwnership(producerTransport, consumerTransport)
	out["producer_close_idle_calls"] = producerTransport.idleCalls
	out["consumer_close_idle_calls"] = consumerTransport.idleCalls
	return out
}

func migrationResourceOwnership(transports ...*migrationResourceTransport) []map[string]any {
	out := []map[string]any{}
	for _, transport := range transports {
		for _, body := range transport.bodies {
			out = append(out, body.observation())
		}
	}
	return out
}

func migrationResourceCreator(icon, cover string) string {
	body := map[string]any{"body": map[string]any{"creatorId": "resource-creator", "user": map[string]any{"name": "Resource Creator", "iconUrl": icon}, "coverImageUrl": cover}}
	data, _ := json.Marshal(body)
	return string(data)
}

func migrationResourcePost(kind, url string) string {
	body := map[string]any{}
	if kind == "post_image" {
		body["images"] = []map[string]any{{"id": "resource-image", "originalUrl": url}}
	} else {
		body["files"] = []map[string]any{{"id": "resource-file", "name": "attachment.txt", "url": url}}
	}
	return migrationResourcePostBody(body)
}

func migrationResourcePostBody(body any) string {
	data, _ := json.Marshal(map[string]any{"body": map[string]any{"post": map[string]any{"id": "resource-post", "creatorId": "resource-creator", "title": "Resource Post", "publishedDatetime": "2024-01-02T03:04:05Z", "body": body}}})
	return string(data)
}

func migrationResourceDefault(kind string) migrationResourceInput {
	original := "https://downloads.fanbox.cc/original.bin?signature=old"
	rotated := "https://downloads.fanbox.cc/rotated.bin?signature=new"
	producer, reopen := migrationResourcePost(kind, original), migrationResourcePost(kind, rotated)
	if strings.HasPrefix(kind, "creator_") {
		producer = migrationResourceCreator(original, "https://i.pximg.net/cover-old.png")
		reopen = migrationResourceCreator(rotated, "https://i.pximg.net/cover-new.png")
	}
	return migrationResourceInput{Kind: kind, Mode: "cached", ProducerDocument: producer, ReopenDocument: reopen, Media: []migrationResourceBodySpec{{Status: 200, Header: http.Header{"Content-Type": {"application/octet-stream"}, "Content-Length": {"5"}}, Data: "media"}}, Actions: []migrationResourceAction{{Kind: "read", Size: 2}, {Kind: "read", Size: 8}, {Kind: "read", Size: 8}, {Kind: "close"}}}
}

func migrationResourceCases() []migrationResourceRow {
	rows := []migrationResourceRow{}
	add := func(name string, input migrationResourceInput) {
		rows = append(rows, migrationResourceRow{Name: name, Input: input})
	}
	for _, kind := range []string{"creator_icon", "creator_cover", "post_image", "post_file"} {
		for _, mode := range []string{"cached", "fresh"} {
			input := migrationResourceDefault(kind)
			input.Mode = mode
			add("reopening/"+kind+"/"+mode+"_rotated_locator", input)
			input.Repeat = 2
			input.Media = append(input.Media, input.Media[0])
			input.MutateResourceHeader = true
			add("reopening/"+kind+"/"+mode+"_repeat_cache_and_header_ownership", input)
		}
	}
	collision := migrationResourcePostBody(map[string]any{"blocks": []map[string]any{{"type": "image", "imageId": "same"}, {"type": "file", "fileId": "same"}}, "imageMap": map[string]any{"same": map[string]any{"id": "same", "originalUrl": "https://i.pximg.net/collision-image.png"}}, "fileMap": map[string]any{"same": map[string]any{"id": "same", "name": "collision.txt", "url": "https://downloads.fanbox.cc/collision-file.txt"}}})
	for _, kind := range []string{"post_image", "post_file"} {
		for _, mode := range []string{"cached", "fresh"} {
			input := migrationResourceDefault(kind)
			input.Mode = mode
			input.ProducerDocument = collision
			input.ReopenDocument = collision
			add("collision/"+kind+"/"+mode, input)
		}
	}
	for _, sample := range []struct{ name, kind, document string }{
		{"creator_icon_absent", "creator_icon", migrationResourceCreator("", "https://i.pximg.net/cover.png")},
		{"creator_cover_absent", "creator_cover", migrationResourceCreator("https://i.pximg.net/icon.png", "")},
		{"creator_profile_absent", "creator_icon", `{"body":{}}`},
		{"creator_null_body", "creator_icon", `{"body":null}`},
		{"creator_bad_type", "creator_icon", `{"body":{"user":{"name":42}}}`},
		{"post_null_body", "post_image", migrationResourcePostBody(nil)},
		{"post_empty_body", "post_file", migrationResourcePostBody(map[string]any{})},
		{"post_attachment_absent", "post_image", migrationResourcePost("post_file", "https://downloads.fanbox.cc/other.txt")},
		{"post_absent", "post_image", `{"body":{}}`},
		{"post_wrong_type", "post_file", `{"body":{"post":42}}`},
		{"post_invalid_publish_time_is_unused_during_reopen", "post_image", strings.ReplaceAll(migrationResourcePost("post_image", "https://i.pximg.net/rotated.png"), "2024-01-02T03:04:05Z", "not-a-time")},
		{"post_images_precede_files", "post_file", migrationResourcePostBody(map[string]any{"images": []any{}, "files": []map[string]any{{"id": "resource-file", "url": "https://downloads.fanbox.cc/file.txt"}}})},
		{"post_missing_map_asset", "post_image", migrationResourcePostBody(map[string]any{"blocks": []map[string]any{{"type": "image", "imageId": "missing"}}, "imageMap": map[string]any{}})},
	} {
		input := migrationResourceDefault(sample.kind)
		input.Mode = "fresh"
		input.ReopenDocument = sample.document
		add("metadata/"+sample.name, input)
	}
	for _, sample := range []struct{ name, payload string }{
		{"malformed_json", "{"}, {"scalar_json", "42"}, {"null_json", "null"}, {"array_json", "[]"}, {"missing_kind", `{"p":"resource-post","a":"resource-image"}`}, {"empty_kind", `{"k":""}`}, {"wrong_kind_type", `{"k":3}`}, {"unknown_kind", `{"k":"future"}`}, {"creator_missing_id", `{"k":"creator_icon"}`}, {"post_missing_id", `{"k":"post_image","a":"resource-image"}`}, {"asset_missing_id", `{"k":"post_file","p":"resource-post"}`}, {"wrong_asset_type", `{"k":"post_image","p":"resource-post","a":2}`}, {"duplicate_kind_last_wins", `{"k":"future","k":"post_image","p":"resource-post","a":"resource-image"}`}, {"unicode_field_alias", `{"K":"post_image","P":"resource-post","A":"resource-image"}`}, {"extra_locator_ignored", `{"k":"post_image","p":"resource-post","a":"resource-image","url":"https://evil.invalid/secret"}`},
	} {
		input := migrationResourceDefault("post_image")
		input.Kind = ""
		input.RefProduct = "fanbox"
		input.RefPayload = sample.payload
		input.Mode = "fresh"
		add("references/"+sample.name, input)
	}
	for _, product := range []string{"pixiv", "FANBOX", ""} {
		input := migrationResourceDefault("post_image")
		input.Kind = ""
		input.RefProduct = product
		input.RefPayload = `{"k":"post_image","p":"resource-post","a":"resource-image"}`
		add("references/product_"+map[string]string{"": "empty", "pixiv": "pixiv", "FANBOX": "uppercase"}[product], input)
	}
	zero := migrationResourceDefault("post_image")
	zero.Kind = ""
	add("references/zero", zero)
	for _, sample := range []struct{ name, text string }{
		{"empty", ""}, {"invalid_base64", "not+route/safe="}, {"invalid_json", base64.RawURLEncoding.EncodeToString([]byte("{"))}, {"missing_version", base64.RawURLEncoding.EncodeToString([]byte(`{"p":"fanbox","d":"e30="}`))}, {"future_version", base64.RawURLEncoding.EncodeToString([]byte(`{"v":2,"p":"fanbox","d":"e30="}`))}, {"missing_product", base64.RawURLEncoding.EncodeToString([]byte(`{"v":1,"d":"e30="}`))}, {"empty_payload", base64.RawURLEncoding.EncodeToString([]byte(`{"v":1,"p":"fanbox","d":""}`))}, {"wrong_payload_type", base64.RawURLEncoding.EncodeToString([]byte(`{"v":1,"p":"fanbox","d":3}`))},
	} {
		input := migrationResourceInput{ParseOnly: true, ParseText: sample.text, Mode: "fresh"}
		add("parse/"+sample.name, input)
	}
	for _, method := range []string{"POST", "get", " GET ", "DELETE"} {
		input := zero
		input.Method = method
		input.Range = "bad\nvalue"
		add("validation/method_before_headers_and_zero_ref_"+strings.TrimSpace(method), input)
	}
	for _, field := range []string{"range", "if_none_match", "if_modified_since", "if_range"} {
		for _, control := range []struct{ name, value string }{{"NUL", "\x00"}, {"tab", "\t"}, {"LF", "\n"}, {"CR", "\r"}, {"DEL", "\x7f"}} {
			input := zero
			switch field {
			case "range":
				input.Range = "prefix" + control.value + "suffix"
			case "if_none_match":
				input.IfNoneMatch = "prefix" + control.value + "suffix"
			case "if_modified_since":
				input.IfModifiedSince = "prefix" + control.value + "suffix"
			case "if_range":
				input.IfRange = "prefix" + control.value + "suffix"
			}
			add("validation/"+field+"_"+control.name+"_before_ref", input)
		}
	}
	allBad := zero
	allBad.Range = "bad\n"
	allBad.IfNoneMatch = "bad\n"
	allBad.IfModifiedSince = "bad\n"
	allBad.IfRange = "bad\n"
	add("validation/header_field_order", allBad)
	for _, sample := range []struct{ name, url string }{
		{"empty", ""}, {"http", "http://downloads.fanbox.cc/file"}, {"userinfo", "https://user:password@downloads.fanbox.cc/file"}, {"foreign", "https://evil.invalid/file"}, {"fanbox_suffix_attack", "https://fanbox.cc.evil.invalid/file"}, {"pximg_suffix_attack", "https://i.pximg.net.evil.invalid/file"}, {"no_path", "https://downloads.fanbox.cc"}, {"root_path", "https://downloads.fanbox.cc/"}, {"query_without_path", "https://downloads.fanbox.cc?file=1"}, {"fragment_without_path", "https://downloads.fanbox.cc#file"}, {"relative", "/file"}, {"invalid_escape", "https://downloads.fanbox.cc/%zz"}, {"upper_scheme", "HTTPS://downloads.fanbox.cc/file"}, {"uppercase_host", "https://DOWNLOADS.FANBOX.CC/file"}, {"explicit_port", "https://downloads.fanbox.cc:8443/file"}, {"encoded_slash", "https://downloads.fanbox.cc/%2F"}, {"dot_path", "https://downloads.fanbox.cc/."}, {"fragment", "https://downloads.fanbox.cc/file#fragment"}, {"fanbox_apex", "https://fanbox.cc/file"}, {"fanbox_subdomain", "https://creator.fanbox.cc/file"}, {"pixiv_fanbox", "https://fanbox.pixiv.net/file"}, {"pixiv_subdomain", "https://cdn.fanbox.pixiv.net/file"}, {"pximg_subdomain", "https://cdn.pximg.net/file"},
	} {
		input := migrationResourceDefault("creator_icon")
		input.ProducerDocument = migrationResourceCreator(sample.url, "")
		add("locator/generation_"+sample.name, input)
		input = migrationResourceDefault("creator_icon")
		input.Mode = "fresh"
		input.ReopenDocument = migrationResourceCreator(sample.url, "")
		add("locator/reopening_"+sample.name, input)
	}
	for _, method := range []string{"", "GET", "HEAD"} {
		input := migrationResourceDefault("post_image")
		input.Method = method
		input.Range = "bytes=1-4"
		input.IfNoneMatch = `"etag"`
		input.IfModifiedSince = "Wed, 21 Oct 2015 07:28:00 GMT"
		input.IfRange = `"range-etag"`
		add("requests/"+map[string]string{"": "default_GET", "GET": "explicit_GET", "HEAD": "injected_only_HEAD"}[method]+"_conditional", input)
	}
	for _, status := range []int{200, 201, 204, 206, 304, 400, 401, 403, 404, 429, 500} {
		input := migrationResourceDefault("post_image")
		input.Media[0].Status = status
		if status == 204 || status == 304 {
			input.Media[0].Data = ""
		}
		add(fmt.Sprintf("status/%d", status), input)
	}
	for _, sample := range []struct {
		name, status int
		length       int64
		method       string
	}{{0, 200, 0, ""}, {1, 200, 5, ""}, {2, 204, 0, ""}, {3, 304, 0, ""}, {4, 200, 5, "HEAD"}} {
		input := migrationResourceDefault("post_image")
		input.Method = sample.method
		input.Media[0].Status = sample.status
		input.Media[0].NilBody = true
		input.Media[0].Length = sample.length
		add(fmt.Sprintf("nil_body/%d", sample.name), input)
	}
	for _, sample := range []struct{ name, value string }{{"absent", ""}, {"spaces", " 17 "}, {"negative", "-1"}, {"plus", "+9"}, {"zero", "0"}, {"overflow", "9223372036854775808"}, {"malformed", "3x"}, {"empty", ""}} {
		input := migrationResourceDefault("post_file")
		if sample.name == "absent" {
			delete(input.Media[0].Header, "Content-Length")
		} else {
			input.Media[0].Header["Content-Length"] = []string{sample.value}
		}
		input.Media[0].Length = 77
		add("length/"+sample.name, input)
	}
	header := migrationResourceDefault("post_file")
	header.Media[0].Header = http.Header{"Content-Type": {"application/octet-stream", "second-type"}, "Content-Length": {"5", "99"}, "Content-Range": {"bytes 0-4/9"}, "Accept-Ranges": {"bytes"}, "Etag": {"\"first\"", "\"second\""}, "Last-Modified": {"Wed, 21 Oct 2015 07:28:00 GMT"}, "Cache-Control": {"private", "max-age=0"}, "Location": {"https://secret.invalid/"}, "Set-Cookie": {migrationResourceSecret}, "Authorization": {migrationResourceSecret}, "X-Upstream-Secret": {migrationResourceSecret}, "content-type": {"noncanonical-ignored"}}
	add("headers/allowlist_multiple_values_and_copy", header)
	encoded := migrationResourceDefault("post_file")
	encoded.Media[0].Header["Content-Encoding"] = []string{"gzip"}
	encoded.Media[0].Data = "injected-undecoded-wire"
	add("headers/injected_transport_keeps_encoding_bytes", encoded)
	for _, status := range []int{301, 302, 303, 307, 308} {
		input := migrationResourceDefault("post_image")
		input.Range = "bytes=0-4"
		input.Media = []migrationResourceBodySpec{{Status: status, Header: http.Header{"Location": {"/redirected.bin"}}, Data: "redirect"}, input.Media[0]}
		add(fmt.Sprintf("redirect/status_%d_relative_cookie_drop", status), input)
	}
	for _, sample := range []struct{ name, location string }{{"public_cdn", "https://i.pximg.net/redirected.png"}, {"foreign", "https://evil.invalid/file"}, {"http", "http://downloads.fanbox.cc/file"}, {"userinfo", "https://u:p@downloads.fanbox.cc/file"}, {"missing", ""}, {"invalid_escape", "/%zz"}, {"loop", "https://downloads.fanbox.cc/original.bin?signature=old"}, {"root_path_allowed_after_redirect", "https://downloads.fanbox.cc/"}} {
		input := migrationResourceDefault("post_image")
		input.Media = []migrationResourceBodySpec{{Status: 302, Header: http.Header{"Location": {sample.location}}, Data: "redirect"}, input.Media[0]}
		add("redirect/"+sample.name, input)
	}
	back := migrationResourceDefault("post_file")
	back.Media = []migrationResourceBodySpec{{Status: 302, Header: http.Header{"Location": {"https://i.pximg.net/intermediate.png"}}, Data: "first"}, {Status: 307, Header: http.Header{"Location": {"https://downloads.fanbox.cc/back.txt"}}, Data: "second"}, back.Media[0]}
	add("redirect/back_to_downloads_cookie_stays_dropped", back)
	badClose := migrationResourceDefault("post_image")
	badClose.Media = []migrationResourceBodySpec{{Status: 302, Header: http.Header{"Location": {"https://evil.invalid/file"}}, Data: "redirect", CloseError: "raw"}}
	add("redirect/close_error_precedes_unsafe_next_url", badClose)
	for _, kind := range []string{"raw", "EOF", "wrapped_EOF", "unexpected_EOF", "canceled", "deadline", "canceled_then_deadline", "deadline_then_canceled"} {
		for _, withBytes := range []bool{false, true} {
			input := migrationResourceDefault("post_file")
			input.Media[0].ReadError = kind
			input.Media[0].ErrorWithLastBytes = withBytes
			input.Actions = []migrationResourceAction{{Kind: "read", Size: 32}, {Kind: "read", Size: 32}, {Kind: "close"}}
			add(fmt.Sprintf("stream/%s/with_last_bytes_%t", kind, withBytes), input)
		}
	}
	for _, closeErr := range []string{"raw", "canceled", "deadline", "canceled_then_deadline", "deadline_then_canceled"} {
		input := migrationResourceDefault("post_file")
		input.Media[0].CloseError = closeErr
		input.Actions = []migrationResourceAction{{Kind: "close"}, {Kind: "close"}, {Kind: "read", Size: 32}}
		add("close/"+closeErr+"_repeat_and_read_after_close", input)
	}
	for _, kind := range []string{"raw", "canceled", "deadline", "canceled_then_deadline", "deadline_then_canceled"} {
		input := migrationResourceDefault("post_image")
		input.TransportError = kind
		add("transport/"+kind, input)
	}
	for _, ctxKind := range []string{"canceled", "deadline"} {
		for _, fault := range []string{"", "raw", "canceled", "deadline"} {
			input := migrationResourceDefault("post_image")
			input.Context = ctxKind
			input.TransportError = fault
			add("cancellation/"+ctxKind+"_before_open_transport_"+map[string]string{"": "success", "raw": "raw", "canceled": "canceled", "deadline": "deadline"}[fault], input)
		}
	}
	for _, kind := range []string{"raw", "EOF", "wrapped_EOF", "deadline"} {
		input := migrationResourceDefault("post_file")
		input.Media[0].ReadError = kind
		input.Media[0].ErrorWithLastBytes = true
		input.Media[0].CancelOnRead = true
		input.Media[0].CloseError = "raw"
		add("cancellation/read_cancel_precedes_"+kind, input)
	}
	cancelClose := migrationResourceDefault("post_file")
	cancelClose.Media[0].CancelOnClose = true
	cancelClose.Media[0].CloseError = "deadline"
	add("cancellation/close_cancel_precedes_deadline_error", cancelClose)
	cancelSuccess := migrationResourceDefault("post_file")
	cancelSuccess.Actions = []migrationResourceAction{{Kind: "cancel"}, {Kind: "read", Size: 32}, {Kind: "read", Size: 32}, {Kind: "close"}}
	add("cancellation/canceled_context_does_not_override_success_or_exact_EOF", cancelSuccess)
	ownership := migrationResourceDefault("post_image")
	ownership.Actions = []migrationResourceAction{{Kind: "idle"}, {Kind: "read", Size: 2}, {Kind: "idle"}, {Kind: "read", Size: 32}, {Kind: "close"}, {Kind: "close"}}
	add("ownership/idle_cleanup_does_not_close_caller_body", ownership)
	for _, fault := range []string{"raw", "canceled", "deadline"} {
		input := migrationResourceDefault("post_image")
		input.Mode = "fresh"
		input.Metadata.CloseError = fault
		add("metadata_ownership/close_"+fault, input)
	}
	for _, status := range []int{401, 403, 404, 500} {
		input := migrationResourceDefault("post_image")
		input.Mode = "fresh"
		input.Metadata.Status = status
		add(fmt.Sprintf("metadata_ownership/status_%d", status), input)
	}
	for _, sample := range []struct {
		name        string
		status      int
		read, close string
	}{{"close_precedes_status", 401, "", "raw"}, {"close_precedes_403_read", 403, "raw", "raw"}, {"read_precedes_403_classification", 403, "raw", ""}} {
		input := migrationResourceDefault("post_file")
		input.Media[0].Status = sample.status
		input.Media[0].ReadError = sample.read
		input.Media[0].CloseError = sample.close
		add("status_ownership/"+sample.name, input)
	}
	for _, status := range []int{204, 304} {
		input := migrationResourceDefault("post_file")
		input.Media[0].Status = status
		add(fmt.Sprintf("status/%d_injected_nonempty_body_is_preserved", status), input)
	}
	for _, kind := range []string{"raw", "canceled", "deadline", "canceled_then_deadline", "deadline_then_canceled"} {
		input := migrationResourceDefault("post_image")
		input.Mode = "fresh"
		input.TransportError = kind
		input.TransportErrorAtMetadata = true
		add("metadata_transport/"+kind, input)
	}
	for _, length := range []int64{0, 5} {
		input := migrationResourceDefault("post_file")
		input.Mode = "fresh"
		input.Metadata.NilBody = true
		input.Metadata.Length = length
		add(fmt.Sprintf("metadata_ownership/nil_body_length_%d", length), input)
	}
	for _, sample := range []struct {
		name, read, close string
		withBytes         bool
	}{
		{"late_raw_error_after_root", "raw", "", false},
		{"root_with_raw_error", "raw", "", true},
		{"root_with_canceled_error", "canceled", "", true},
		{"root_with_deadline_error", "deadline", "", true},
		{"malformed_decode_then_close", "raw", "raw", false},
	} {
		input := migrationResourceDefault("post_image")
		input.Mode = "fresh"
		input.Metadata.ReadError = sample.read
		input.Metadata.CloseError = sample.close
		input.Metadata.ErrorWithLastBytes = sample.withBytes
		if sample.name == "malformed_decode_then_close" {
			input.ReopenDocument = "{"
		}
		add("metadata_ownership/"+sample.name, input)
	}
	noncontrol := migrationResourceDefault("post_image")
	noncontrol.Range = "range-format-is-not-validated"
	noncontrol.IfNoneMatch = "é 非制御"
	noncontrol.IfModifiedSince = "not-an-http-date"
	noncontrol.IfRange = " "
	add("validation/noncontrol_header_values_are_forwarded", noncontrol)

	for _, fault := range []string{"", "raw"} {
		input := migrationResourceDefault("post_file")
		input.Media[0].Data = ""
		input.Media[0].Wire = []byte{0, 255, 195, 169, 255, 192, 175, 127, 10, 13}
		input.Media[0].Chunk = 3
		input.Media[0].ReadError = fault
		input.Media[0].ErrorWithLastBytes = true
		input.Actions = []migrationResourceAction{{Kind: "read", Size: 4}, {Kind: "read", Size: 4}, {Kind: "read", Size: 4}, {Kind: "read", Size: 4}, {Kind: "read", Size: 4}, {Kind: "close"}}
		add("stream/binary_nul_invalid_utf8_chunked_"+map[string]string{"": "EOF", "raw": "partial_error"}[fault], input)
	}

	return rows
}

func migrationResourceVerifySources(t *testing.T, root string) (map[string]string, map[string]string, map[string]string) {
	t.Helper()
	const frozen = "4b4426487ef18bed276706daec385e0d0a6979f9"
	const base = "7078b729cc4dd48ee2b28f8eedcb758bacbc3a0f"
	sources := map[string]string{}
	for _, path := range []string{"sdk/resource.go", "sdk/resource_dto.go", "sdk/ref.go", "sdk/error.go", "sdk/fanbox/fanbox.go", "sdk/fanbox/resource.go", "sdk/fanbox/ops.go", "sdk/fanbox/dto.go", "sdk/fanbox/errors.go", "sdk/fanbox/request.go", "sdk/fanbox/models.go", "internal/services/fanbox/resource/resource.go", "internal/services/fanbox/protocol/protocol.go", "internal/services/fanbox/protocol/cookie.go", "internal/services/fanbox/protocol/solver.go", "internal/services/fanbox/endpoint/creator/creators/creators.go", "internal/services/fanbox/endpoint/post/info/info.go", "internal/services/fanbox/endpoint/post/wire/wire.go"} {
		original, err := exec.Command("git", "-C", root, "show", frozen+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		current, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, current) {
			t.Fatalf("resource source differs from frozen Go: %s", path)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	stdlib := map[string]string{
		"net/http/client.go":        "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9",
		"net/http/request.go":       "c3257079994b4e4f74f01cef72508919983ba31f91fcf599d348e9d52cec1540",
		"net/http/header.go":        "b4fa959c3db03d4ca67a487b3cbb25dcefecea45ca3dd10fef13cfc32fa6d80d",
		"net/url/url.go":            "44c33f88526abf3b5955d6c97297be5c9d228e741b33e52c940dcac5a496e091",
		"encoding/base64/base64.go": "15e67707bc3f0925f7f56bd4002106681c6b64ba5bf553b4d8cc0bb8eca71aa5",
		"encoding/json/decode.go":   "1632161a34c8286722716a48ba0b5e0c3d117a2e017e79ace676403808a16e6e",
		"encoding/json/encode.go":   "8ff45e82c60c6e29d11fe57ce0c59d79f40ef0d53be28ac36617482a7463b217",
		"io/io.go":                  "3945d328a9072e4f5970b7d4bc0eb834eec29fd22889a1ed9f3fb76ad41c3274",
		"errors/wrap.go":            "098116636610ae87dd80e49c4dbe6a2b6c37918d7e1af35ded005be621d68a40",
		"context/context.go":        "971f00ffa375b79d3f65e6da33cca3a493cc3fce091c589233abfb4283e29b7d",
		"strconv/number.go":         "dda43e8c91d95f9dea0c46b2fec6637e5520cae099e40c5965dca0e41bbaab9e",
		"errors/join.go":            "87da7c120f71d17e37f110ae1dc6af6493ec841a22ee825e13a6a8024a014dba",
	}
	for path, want := range stdlib {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("official Go resource dependency changed: %s", path)
		}
	}
	preserved := map[string]string{}
	listing, err := exec.Command("git", "-C", root, "ls-tree", "-r", "--name-only", base, "crates/pixiv-sdk/tests/fixtures").Output()
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		original, err := exec.Command("git", "-C", root, "show", base+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, data) {
			t.Fatalf("published SDK fixture changed: %s", path)
		}
		preserved[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	if preserved["crates/pixiv-sdk/tests/fixtures/fanbox-media-body.json"] != "a3b48317ef6f4dba2c5ca6d6f5d935467eee273147b88ebeff776b286f60bce2" {
		t.Fatal("existing media80 must remain byte-exact")
	}
	return sources, stdlib, preserved
}

func TestMigrationFanboxResourceReadsFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	production := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	sources, stdlib, preserved := migrationResourceVerifySources(t, root)
	rows := migrationResourceCases()
	if len(rows) != 243 {
		t.Fatalf("bounded public resource inventory must contain 243 rows, got %d", len(rows))
	}
	if len(preserved) != 21 {
		t.Fatalf("published SDK fixture inventory must contain 21 files, got %d", len(preserved))
	}
	seen := map[string]bool{}
	families := map[string]int{}
	for index := range rows {
		row := &rows[index]
		if seen[row.Name] {
			t.Fatalf("duplicate public resource case: %s", row.Name)
		}
		seen[row.Name] = true
		families[strings.Split(row.Name, "/")[0]]++
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationResourceObserve(t, row.Input) })
	}
	if t.Failed() {
		return
	}
	fixture := map[string]any{
		"source_commit": reference.SourceCommit, "published_base": "7078b729cc4dd48ee2b28f8eedcb758bacbc3a0f", "go_version": runtime.Version(),
		"frozen_go_production_guard": production, "source_sha256": sources, "go_stdlib_sha256": stdlib, "dependencies": reference.Dependencies,
		"protected_published_sdk_fixtures_sha256": preserved, "case_family_counts": families, "cases": rows,
		"public_operations":   []string{"fanbox.OpenWith", "fanbox.Client.Creator", "fanbox.Client.Post", "fanbox.Client.OpenResource", "sdk.NewResourceRef", "sdk.ParseResourceRef", "sdk.ResourceRefPayload", "sdk.ToResourceDTO", "sdk.ResourceResponse.Header", "sdk.ResourceResponse.ContentLength"},
		"evidence":            "Actual frozen public SDK with owned in-memory net/http.RoundTripper responses, real endpoint decoding and public generation of all four resource kinds. Cached producer clients and independent fresh consumer clients reopen the generated stable reference; fresh metadata rotates the locator. No private resource constructor/resolver or test-only production API is called. Request method, locator, complete headers, caller context, safe errors, response fields, streamed bytes-plus-error (including raw NUL and invalid UTF-8 captured as exact base64/hex/SHA256) and per-source read/close ownership are recorded exactly. The fixture is serialized and compared byte-for-byte on every replay.",
		"go_only_projections": []string{"Concrete error tree type names and source read-call topology are Go representations; public classifications, safe cause text, cancellation/EOF matching, exact returned bytes, request order and close counts are retained as contract observations.", "An injected source deliberately permits repeated Close and Read after Close; the observed forwarding does not assert behavior of a real socket, decoder or external server."},
		"limitations":         []string{"Synthetic injected boundaries only: no live accounts, media, browser, sockets, external network, trust changes, native HEAD, HTTP2/multiplex or uploads.", "HEAD appears only as an owned in-memory request. Existing media80 remains byte-exact and is reused separately for real decoder integration; encoded bytes through net/http injection remain undecoded. Public OpenWith does not expose an injected solver-control client, so clearance state is reused from prior fixtures rather than manufactured here.", "These bounded rows establish public resource generation/reopening, not SaveResource/download expansion, native transport decoding/lifecycle, unrestricted JSON/URL grammar, physical concurrent stream Read/Close or non-Linux parity.", "All 434 frozen Go production/module paths and every SDK fixture tracked at the published base are guarded unchanged. New sibling fixtures are separate contracts and are not recaptured here."},
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-resource-reads.json")
	if *migrationCaptureFanboxResourceReads {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("public resource reads differ from frozen Go fixture; review actual behavior before any explicit recapture")
	}
	names := []string{}
	for family := range families {
		names = append(names, family)
	}
	sort.Strings(names)
	t.Logf("frozen Go %s public resource reads: %d rows; %d protected published SDK fixtures; fixture bytes=%d sha256=%x; frozen Go production/module paths=434", runtime.Version(), len(rows), len(preserved), len(data), sha256.Sum256(data))
	for _, name := range names {
		t.Logf("%s: %d", name, families[name])
	}
}

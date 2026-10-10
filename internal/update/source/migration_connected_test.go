package source

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

type updaterSourceRow struct {
	Name      string         `json:"name"`
	Operation string         `json:"operation"`
	GoOnly    bool           `json:"go_only"`
	Input     map[string]any `json:"input"`
	Output    map[string]any `json:"output"`
}
type updaterSourceFixture struct {
	Reference   string             `json:"reference"`
	Environment string             `json:"environment"`
	GoVersion   string             `json:"go_version"`
	Sources     map[string]string  `json:"sources"`
	APIMap      map[string]string  `json:"api_map"`
	Boundaries  map[string]string  `json:"boundaries"`
	Cases       []updaterSourceRow `json:"cases"`
}

func updaterSourceError(err error) map[string]any {
	out := map[string]any{"message": "", "canceled": errors.Is(err, context.Canceled), "deadline": errors.Is(err, context.DeadlineExceeded), "tree": nil}
	if err != nil {
		out["message"] = err.Error()
		out["tree"] = updaterSourceErrorTree(err)
	}
	return out
}
func updaterSourceErrorTree(err error) map[string]any {
	children := []any{}
	if joined, ok := err.(interface{ Unwrap() []error }); ok {
		for _, e := range joined.Unwrap() {
			children = append(children, updaterSourceErrorTree(e))
		}
	} else if e := errors.Unwrap(err); e != nil {
		children = append(children, updaterSourceErrorTree(e))
	}
	return map[string]any{"type": fmt.Sprintf("%T", err), "message": err.Error(), "children": children}
}
func updaterSourceIDs(sources []ReleaseSource) []string {
	out := []string{}
	for _, s := range sources {
		out = append(out, s.ID())
	}
	return out
}
func updaterSourceDescribe(sources []ReleaseSource) []any {
	out := []any{}
	for _, s := range sources {
		out = append(out, map[string]any{"id": s.ID(), "api": s.api.raw, "api_placeholder": s.api.placeholder, "asset": s.asset.raw, "asset_placeholder": s.asset.placeholder})
	}
	return out
}
func updaterSourceParse(t *testing.T, body string) []ReleaseSource {
	t.Helper()
	sources, e := ParseReleaseSources([]byte(body))
	if e != nil {
		t.Fatal(e)
	}
	return sources
}
func updaterSourceContext(mode string) (context.Context, context.CancelFunc) {
	ctx := context.WithValue(context.Background(), updaterSourceContextKey{}, "owned-context")
	switch mode {
	case "nil":
		return nil, func() {}
	case "canceled":
		c, cancel := context.WithCancel(ctx)
		cancel()
		return c, cancel
	case "deadline":
		return context.WithDeadline(ctx, time.Unix(1, 0))
	case "future-deadline":
		return context.WithDeadline(ctx, time.Now().Add(time.Hour))
	}
	return context.WithCancel(ctx)
}

type updaterSourceContextKey struct{}

func updaterSourceCall(fn func() error) (err error, panicText string) {
	defer func() {
		if p := recover(); p != nil {
			panicText = fmt.Sprint(p)
		}
	}()
	err = fn()
	return
}

func updaterSourceParserRows(t *testing.T) []updaterSourceRow {
	rows := []updaterSourceRow{}
	cases := []struct{ name, body string }{
		{"embedded", string(embeddedReleaseSources)},
		{"whitespace-comments-crlf", "\r\n # comment\r\n a \t| {url} | {url} \r\n\t# final\r\n"},
		{"raw-and-query", "prefix|https://mirror.test/{url}|http://mirror.test/{url}\nquery|https://mirror.test/api?target={url_query}|https://mirror.test/download?target={url_query}\n"},
		{"asset-only", "asset|-|{url}\n"}, {"id-leading-digit", "0a.-|{url}|{url}"},
		{"empty", ""}, {"comments-only", " # owned\n \n"}, {"field-count-two", "a|{url}"}, {"field-count-four", "a|{url}|{url}|extra"},
		{"empty-id", "|{url}|{url}"}, {"uppercase-id", "A|{url}|{url}"}, {"underscore-id", "a_b|{url}|{url}"}, {"leading-dot-id", ".a|{url}|{url}"}, {"leading-hyphen-id", "-a|{url}|{url}"}, {"unicode-id", "あ|{url}|{url}"},
		{"duplicate-id", "a|{url}|{url}\na|-|{url}"},
		{"api-no-placeholder", "a|https://mirror.test/api|{url}"}, {"asset-no-placeholder", "a|{url}|https://mirror.test/asset"}, {"asset-disabled", "a|{url}|-"}, {"api-empty", "a||{url}"}, {"asset-empty", "a|{url}|"},
		{"api-both-placeholders", "a|https://mirror.test/{url}{url_query}|{url}"}, {"asset-repeat-raw", "a|{url}|https://mirror.test/{url}/{url}"}, {"asset-repeat-query", "a|{url}|https://mirror.test/?x={url_query}&y={url_query}"},
		{"api-relative", "a|mirror/{url}|{url}"}, {"asset-ftp", "a|{url}|ftp://mirror.test/{url}"}, {"asset-userinfo", "a|{url}|https://owned:synthetic@mirror.test/{url}"}, {"asset-fragment", "a|{url}|https://mirror.test/{url}#fragment"}, {"api-malformed-escape", "a|https://mirror.test/%zz/{url}|{url}"}, {"asset-hostless", "a|{url}|https:///{url}"},
		{"parse-precedence-id-before-template", "UPPER|missing|missing"}, {"parse-precedence-duplicate-before-template", "a|{url}|{url}\na|missing|missing"}, {"parse-precedence-api-before-asset", "a|missing|missing"},
		{"later-invalid-line-number", "# first\na|{url}|{url}\n\nb|bad|{url}"},
		{"trailing-inline-comment-is-template", "a|{url}|{url} # inline"},
	}
	for _, c := range cases {
		sources, e := ParseReleaseSources([]byte(c.body))
		rows = append(rows, updaterSourceRow{Name: c.name, Operation: "parse", Input: map[string]any{"body": c.body}, Output: map[string]any{"sources": updaterSourceDescribe(sources), "nil_sources": sources == nil, "error": updaterSourceError(e)}})
	}
	first := DefaultReleaseSources()
	first[0].id = "mutated-owned"
	second := DefaultReleaseSources()
	rows = append(rows, updaterSourceRow{Name: "fresh-embedded-list", Operation: "default_sources", Input: map[string]any{}, Output: map[string]any{"sources": updaterSourceDescribe(second), "mutation_shared": second[0].ID() == first[0].ID(), "user_agent": GitHubUserAgent}})
	return rows
}
func updaterSourceTransformRows(t *testing.T) []updaterSourceRow {
	rows := []updaterSourceRow{}
	sources := updaterSourceParse(t, "raw|https://mirror.test/{url}|https://mirror.test/{url}\nquery|https://mirror.test/api?target={url_query}|https://mirror.test/asset?target={url_query}\ndirect|{url}|{url}\nasset|-|{url}")
	canonicals := []struct{ name, url string }{
		{"official-api", "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"}, {"official-asset", "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/checksums.txt"},
		{"query-space-plus-unicode", "https://github.com/FlanChanXwO/pixiv-cli/releases?next=a+b&name=雪%20x"}, {"encoded-path", "https://github.com/repos/a%2Fb/releases?x=%26"},
		{"foreign-host-allowed", "https://other.test/release"}, {"port-allowed", "https://github.com:443/releases"}, {"uppercase-scheme-host", "HTTPS://GitHub.COM/releases"}, {"empty-path", "https://github.com"}, {"bare-query-marker", "https://github.com/releases?"},
		{"http-rejected", "http://github.com/releases"}, {"relative-rejected", "/releases"}, {"hostless-rejected", "https:///releases"}, {"userinfo-rejected", "https://owned:synthetic@github.com/releases"}, {"fragment-rejected", "https://github.com/releases#owned"}, {"malformed-escape", "https://github.com/%zz"}, {"malformed-host", "https://%zz/releases"}, {"empty", ""}, {"encoded-fragment", "https://github.com/releases%23owned"}, {"empty-fragment", "https://github.com/releases#"},
	}
	for _, c := range canonicals {
		for _, s := range sources[:3] {
			api, ae := s.APIURL(c.url)
			asset, se := s.AssetURL(c.url)
			rows = append(rows, updaterSourceRow{Name: c.name + "-" + s.ID(), Operation: "transform", Input: map[string]any{"source": updaterSourceDescribe([]ReleaseSource{s})[0], "canonical": c.url}, Output: map[string]any{"api_url": api, "api_error": updaterSourceError(ae), "asset_url": asset, "asset_error": updaterSourceError(se)}})
		}
	}
	api, e := sources[3].APIURL("not a URL")
	asset, ae := sources[3].AssetURL(canonicals[1].url)
	rows = append(rows, updaterSourceRow{Name: "unsupported-api-before-canonical", Operation: "transform", Input: map[string]any{"source": updaterSourceDescribe(sources[3:])[0], "canonical": "not a URL", "asset_canonical": canonicals[1].url}, Output: map[string]any{"api_url": api, "api_error": updaterSourceError(e), "asset_url": asset, "asset_error": updaterSourceError(ae)}})
	for _, s := range DefaultReleaseSources() {
		api, e := s.APIURL(canonicals[0].url)
		asset, ae := s.AssetURL(canonicals[1].url)
		rows = append(rows, updaterSourceRow{Name: s.ID(), Operation: "embedded_transform", Input: map[string]any{"id": s.ID(), "api_canonical": canonicals[0].url, "asset_canonical": canonicals[1].url}, Output: map[string]any{"api_url": api, "api_error": updaterSourceError(e), "asset_url": asset, "asset_error": updaterSourceError(ae)}})
	}
	for _, value := range []string{"-", "", "{url}", "http://mirror.test/{url}", "https://mirror.test/?target={url_query}", "https://mirror.test/{other}/{url}", "https://mirror.test/{url}{url_query}", "https://mirror.test/#x={url_query}"} {
		template, e := parseReleaseSourceTemplate(value)
		rows = append(rows, updaterSourceRow{Name: value, Operation: "template_parse", GoOnly: true, Input: map[string]any{"value": value}, Output: map[string]any{"raw": template.raw, "placeholder": template.placeholder, "error": updaterSourceError(e)}})
	}
	for _, template := range []releaseSourceTemplate{{}, {raw: "https://mirror.test/no-target", placeholder: "{url}"}, {raw: "ftp://mirror.test/{url}", placeholder: "{url}"}, {raw: "https://owned@mirror.test/{url}", placeholder: "{url}"}, {raw: "https://mirror.test/{url}#owned", placeholder: "{url}"}, {raw: "https://mirror.test/%zz/{url}", placeholder: "{url}"}} {
		value, e := template.apply(canonicals[1].url)
		rows = append(rows, updaterSourceRow{Name: template.raw, Operation: "template_apply", GoOnly: true, Input: map[string]any{"raw": template.raw, "placeholder": template.placeholder, "canonical": canonicals[1].url}, Output: map[string]any{"url": value, "error": updaterSourceError(e)}})
	}
	return rows
}

type updaterSourceTransport func(*http.Request) (*http.Response, error)

func (f updaterSourceTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type updaterSourceBody struct {
	body   []byte
	mode   string
	trace  []string
	cancel context.CancelFunc
	err    error
	closed chan struct{}
}

func (b *updaterSourceBody) Read(p []byte) (int, error) {
	if len(b.body) > 0 {
		n := copy(p, b.body)
		b.body = b.body[n:]
		b.trace = append(b.trace, fmt.Sprintf("read:%d", n))
		switch b.mode {
		case "error-with-bytes":
			return n, errors.New("owned body read error")
		case "cancel-final-eof":
			b.cancel()
			return n, io.EOF
		case "cancel-after-bytes":
			b.cancel()
		}
		return n, nil
	}
	if b.mode == "read-error" || b.mode == "error-with-bytes" {
		b.trace = append(b.trace, "read:error")
		if b.err != nil {
			return 0, b.err
		}
		return 0, errors.New("owned body read error")
	}
	b.trace = append(b.trace, "read:eof")
	return 0, io.EOF
}
func (b *updaterSourceBody) Close() error {
	b.trace = append(b.trace, "close")
	if b.closed != nil {
		close(b.closed)
	}
	if b.mode == "close-error" {
		return errors.New("owned body close error")
	}
	return nil
}
func updaterSourceRequest(r *http.Request, parent context.Context) map[string]any {
	_, deadline := r.Context().Deadline()
	return map[string]any{"method": r.Method, "url": r.URL.String(), "header": r.Header, "nil_body": r.Body == nil, "nil_get_body": r.GetBody == nil, "content_length": r.ContentLength, "context_value": r.Context().Value(updaterSourceContextKey{}), "context_same_as_parent": r.Context() == parent, "context_deadline": deadline, "context_canceled": r.Context().Err() != nil}
}
func updaterSourceProbeRows(t *testing.T) []updaterSourceRow {
	rows := []updaterSourceRow{}
	cases := []struct {
		name, kind, body, mode, transport, context string
		status                                     int
	}{
		{name: "api-empty-array", kind: "api", body: `[]`}, {name: "api-minimal-object", kind: "api", body: `[{"tag_name":"v1.2.3","draft":false,"prerelease":true}]`}, {name: "api-empty-object", kind: "api", body: `[{}]`}, {name: "api-null-element", kind: "api", body: `[null]`}, {name: "api-unknown-field", kind: "api", body: `[{"assets":{},"unknown":9}]`}, {name: "api-null-fields", kind: "api", body: `[{"tag_name":null,"draft":null,"prerelease":null}]`},
		{name: "api-null", kind: "api", body: `null`}, {name: "api-object", kind: "api", body: `{}`}, {name: "api-string", kind: "api", body: `"owned"`}, {name: "api-number-element", kind: "api", body: `[7]`}, {name: "api-string-element", kind: "api", body: `["owned"]`}, {name: "api-tag-number", kind: "api", body: `[{"tag_name":7}]`}, {name: "api-draft-string", kind: "api", body: `[{"draft":"false"}]`}, {name: "api-prerelease-number", kind: "api", body: `[{"prerelease":1}]`},
		{name: "api-empty", kind: "api"}, {name: "api-html", kind: "api", body: `<html>owned</html>`}, {name: "api-second-array", kind: "api", body: `[] []`}, {name: "api-second-null", kind: "api", body: `[] null`}, {name: "api-trailing-junk", kind: "api", body: `[] x`}, {name: "api-trailing-whitespace", kind: "api", body: "[] \r\n\t"}, {name: "api-truncated", kind: "api", body: `[{`}, {name: "api-read-error", kind: "api", mode: "read-error"}, {name: "api-error-with-array", kind: "api", body: `[]`, mode: "error-with-bytes"}, {name: "api-close-error-ignored", kind: "api", body: `[]`, mode: "close-error"},
		{name: "api-status-204", kind: "api", body: `[]`, status: 204}, {name: "api-status-404", kind: "api", body: `[]`, status: 404}, {name: "asset-opaque", kind: "asset", body: "owned opaque bytes\x00\xff"}, {name: "asset-empty", kind: "asset"}, {name: "asset-html-accepted", kind: "asset", body: `<html>owned</html>`}, {name: "asset-two-json-values-accepted", kind: "asset", body: `[] []`}, {name: "asset-read-error", kind: "asset", mode: "read-error"}, {name: "asset-error-with-prefix", kind: "asset", body: "owned-prefix", mode: "error-with-bytes"}, {name: "asset-close-error-ignored", kind: "asset", body: "owned", mode: "close-error"}, {name: "asset-status-206", kind: "asset", body: "owned", status: 206},
		{name: "api-transport-error", kind: "api", transport: "error"}, {name: "asset-transport-error", kind: "asset", transport: "error"}, {name: "api-nil-response", kind: "api", transport: "nil-response"}, {name: "asset-nil-body", kind: "asset", transport: "nil-body"}, {name: "api-nil-body", kind: "api", transport: "nil-body"}, {name: "api-future-deadline-inherited", kind: "api", body: `[]`, context: "future-deadline"},
	}
	canonicalAPI := "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
	canonicalAsset := "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/checksums.txt"
	for _, c := range cases {
		t.Run("probe/"+c.name, func(t *testing.T) {
			ctx, cancel := updaterSourceContext(c.context)
			defer cancel()
			body := &updaterSourceBody{body: []byte(c.body), mode: c.mode, trace: []string{}}
			requests := []any{}
			status := c.status
			if status == 0 {
				status = 200
			}
			client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
				requests = append(requests, updaterSourceRequest(r, ctx))
				switch c.transport {
				case "error":
					return nil, errors.New("owned transport error")
				case "nil-response":
					return nil, nil
				}
				response := &http.Response{StatusCode: status, Status: fmt.Sprintf("%d %s", status, http.StatusText(status)), Body: body, Header: http.Header{}, Request: r}
				if c.transport == "nil-body" {
					response.Body = nil
				}
				return response, nil
			})}
			s := updaterSourceParse(t, "owned|https://mirror.test/api?target={url_query}|https://mirror.test/{url}")
			kind, canonical := ReleaseSourceAPI, canonicalAPI
			if c.kind == "asset" {
				kind, canonical = ReleaseSourceAsset, canonicalAsset
			}
			ordered, e := NewReleaseSourceSelector(s, client).Ordered(ctx, kind, canonical)
			rows = append(rows, updaterSourceRow{Name: c.name, Operation: "ordered_single", Input: map[string]any{"sources": updaterSourceDescribe(s), "kind": string(kind), "canonical": canonical, "body_hex": hex.EncodeToString([]byte(c.body)), "body_mode": c.mode, "status": status, "transport": c.transport, "context": c.context}, Output: map[string]any{"ids": updaterSourceIDs(ordered), "nil_ordered": ordered == nil, "error": updaterSourceError(e), "requests": requests, "body_trace": body.trace, "caller_canceled": ctx.Err() != nil}})
		})
	}
	for _, c := range []struct{ name, kind, canonical, mode, body string }{
		{"unsupported-api", string(ReleaseSourceAPI), canonicalAPI, "", `[]`}, {"malformed-canonical", string(ReleaseSourceAsset), "http://github.com/owned", "", "owned"}, {"nil-context", string(ReleaseSourceAsset), canonicalAsset, "", "owned"}, {"asset-cancel-final-eof", string(ReleaseSourceAsset), canonicalAsset, "cancel-final-eof", "owned"}, {"api-cancel-final-eof", string(ReleaseSourceAPI), canonicalAPI, "cancel-final-eof", `[]`}, {"unknown-kind-is-asset", "owned-unknown", canonicalAsset, "", "owned"},
	} {
		ctx, cancel := updaterSourceContext("")
		if c.name == "nil-context" {
			ctx = nil
		}
		body := &updaterSourceBody{body: []byte(c.body), mode: c.mode, cancel: cancel, trace: []string{}}
		requests := []any{}
		client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
			requests = append(requests, updaterSourceRequest(r, ctx))
			return &http.Response{StatusCode: 200, Status: "200 OK", Body: body, Header: http.Header{}, Request: r}, nil
		})}
		s := updaterSourceParse(t, "owned|{url}|{url}")[0]
		if c.name == "unsupported-api" {
			s = updaterSourceParse(t, "owned|-|{url}")[0]
		}
		selector := NewReleaseSourceSelector([]ReleaseSource{s}, client)
		e, p := updaterSourceCall(func() error { return selector.probe(ctx, s, ReleaseSourceKind(c.kind), c.canonical) })
		rows = append(rows, updaterSourceRow{Name: c.name, Operation: "probe_private", GoOnly: true, Input: map[string]any{"source": updaterSourceDescribe([]ReleaseSource{s})[0], "kind": c.kind, "canonical": c.canonical, "body_mode": c.mode, "body": c.body}, Output: map[string]any{"error": updaterSourceError(e), "panic": p, "requests": requests, "body_trace": body.trace}})
		cancel()
	}
	return rows
}

func updaterSourceWait(t *testing.T, ch <-chan struct{}, label string) {
	t.Helper()
	select {
	case <-ch:
	case <-time.After(2 * time.Second):
		t.Fatalf("owned witness timed out: %s", label)
	}
}

func updaterSourceGoroutineID(t *testing.T) int {
	t.Helper()
	buffer := make([]byte, 256)
	n := runtime.Stack(buffer, false)
	fields := strings.Fields(string(buffer[:n]))
	if len(fields) < 2 || fields[0] != "goroutine" {
		t.Fatalf("unexpected owned goroutine header: %q", buffer[:n])
	}
	id, e := strconv.Atoi(fields[1])
	if e != nil {
		t.Fatal(e)
	}
	return id
}
func updaterSourceWaitGoroutineExit(t *testing.T, id int) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for {
		buffer := make([]byte, 1<<20)
		n := runtime.Stack(buffer, true)
		if n == len(buffer) {
			t.Fatal("owned goroutine witness buffer exhausted")
		}
		active := false
		for _, line := range strings.Split(string(buffer[:n]), "\n") {
			if strings.HasPrefix(line, fmt.Sprintf("goroutine %d ", id)) {
				active = true
				break
			}
		}
		if !active {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("owned probe goroutine %d did not exit", id)
		}
		runtime.Gosched()
	}
}
func updaterSourceRaceRows(t *testing.T) []updaterSourceRow {
	rows := []updaterSourceRow{}
	sources := updaterSourceParse(t, "first|https://first.test/{url}|https://first.test/{url}\nasset-only|-|https://asset-only.test/{url}\nthird|https://third.test/{url}|https://third.test/{url}\nfourth|https://fourth.test/{url}|https://fourth.test/{url}")
	for _, kind := range []ReleaseSourceKind{ReleaseSourceAPI, ReleaseSourceAsset} {
		for _, winner := range []string{"first", "third", "fourth"} {
			t.Run("race/"+string(kind)+"/"+winner, func(t *testing.T) {
				ctx, cancel := updaterSourceContext("")
				defer cancel()
				candidates := NewReleaseSourceSelector(sources, nil).candidates(kind)
				entered, finished := map[string]chan struct{}{}, map[string]chan struct{}{}
				records := map[string]any{}
				mu := sync.Mutex{}
				releaseWinner := make(chan struct{})
				for _, s := range candidates {
					entered[s.ID()] = make(chan struct{})
					finished[s.ID()] = make(chan struct{})
				}
				client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
					id := r.URL.Hostname()
					id = strings.TrimSuffix(id, ".test")
					record := map[string]any{"request": updaterSourceRequest(r, ctx), "events": []string{"entered"}}
					mu.Lock()
					records[id] = record
					mu.Unlock()
					close(entered[id])
					if id != winner {
						<-r.Context().Done()
						mu.Lock()
						record["events"] = []string{"entered", "child-canceled"}
						record["context_error"] = r.Context().Err().Error()
						mu.Unlock()
						close(finished[id])
						return nil, r.Context().Err()
					}
					<-releaseWinner
					payload := "owned checksum bytes"
					if kind == ReleaseSourceAPI {
						payload = `[]`
					}
					body := &updaterSourceBody{body: []byte(payload), trace: []string{}, closed: finished[id]}
					mu.Lock()
					record["events"] = []string{"entered", "winner-released"}
					record["body"] = body
					mu.Unlock()
					return &http.Response{StatusCode: 200, Status: "200 OK", Body: body, Header: http.Header{}, Request: r}, nil
				})}
				canonical := "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
				if kind != ReleaseSourceAPI {
					canonical = "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/checksums.txt"
				}
				type result struct {
					sources []ReleaseSource
					err     error
				}
				done := make(chan result, 1)
				go func() {
					s, e := NewReleaseSourceSelector(sources, client).Ordered(ctx, kind, canonical)
					done <- result{s, e}
				}()
				for _, s := range candidates {
					updaterSourceWait(t, entered[s.ID()], s.ID()+" entered")
				}
				close(releaseWinner)
				var actual result
				select {
				case actual = <-done:
				case <-time.After(2 * time.Second):
					t.Fatal("winner did not complete")
				}
				for _, s := range candidates {
					updaterSourceWait(t, finished[s.ID()], s.ID()+" finished")
				}
				mu.Lock()
				for _, recordAny := range records {
					record := recordAny.(map[string]any)
					if body, ok := record["body"].(*updaterSourceBody); ok {
						record["body_trace"] = body.trace
						delete(record, "body")
					}
				}
				mu.Unlock()
				if actual.err != nil || len(actual.sources) == 0 || actual.sources[0].ID() != winner {
					t.Fatalf("actual winner = %v, %v", updaterSourceIDs(actual.sources), actual.err)
				}
				rows = append(rows, updaterSourceRow{Name: string(kind) + "-" + winner, Operation: "ordered_race", Input: map[string]any{"sources": updaterSourceDescribe(sources), "kind": string(kind), "canonical": canonical, "winner": winner, "witness": "all eligible transports entered before releasing the winner; all losers wait for actual child cancellation"}, Output: map[string]any{"ids": updaterSourceIDs(actual.sources), "error": updaterSourceError(actual.err), "requests_by_source": records, "witness_steps": []string{"all-eligible-entered", "winner-released", "ordered-returned", "all-eligible-close-or-cancel-observed"}, "caller_canceled": ctx.Err() != nil}})
			})
		}
	}
	for _, order := range [][]string{{"fourth", "first", "third"}, {"third", "fourth", "first"}} {
		t.Run("all-failure/"+strings.Join(order, "-"), func(t *testing.T) {
			ctx, cancel := updaterSourceContext("")
			defer cancel()
			candidates := NewReleaseSourceSelector(sources, nil).candidates(ReleaseSourceAPI)
			entered, gates := map[string]chan struct{}{}, map[string]chan struct{}{}
			goroutineIDs := map[string]int{}
			records := map[string]any{}
			mu := sync.Mutex{}
			bodies := map[string]*updaterSourceBody{}
			for _, s := range candidates {
				entered[s.ID()] = make(chan struct{})
				gates[s.ID()] = make(chan struct{})
			}
			client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
				id := strings.TrimSuffix(r.URL.Hostname(), ".test")
				body := &updaterSourceBody{mode: "read-error", trace: []string{}, err: errors.New("owned failure " + id)}
				mu.Lock()
				records[id] = updaterSourceRequest(r, ctx)
				goroutineIDs[id] = updaterSourceGoroutineID(t)
				bodies[id] = body
				mu.Unlock()
				close(entered[id])
				select {
				case <-gates[id]:
				case <-r.Context().Done():
					return nil, r.Context().Err()
				}
				return &http.Response{StatusCode: 200, Status: "200 OK", Body: body, Header: http.Header{}, Request: r}, nil
			})}
			canonical := "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
			type result struct {
				s []ReleaseSource
				e error
			}
			done := make(chan result, 1)
			go func() {
				s, e := NewReleaseSourceSelector(sources, client).Ordered(ctx, ReleaseSourceAPI, canonical)
				done <- result{s, e}
			}()
			for _, s := range candidates {
				updaterSourceWait(t, entered[s.ID()], s.ID()+" entered")
			}
			witness := []string{"all-eligible-entered"}
			for _, id := range order {
				close(gates[id])
				updaterSourceWaitGoroutineExit(t, goroutineIDs[id])
				witness = append(witness, "probe-completed:"+id)
			}
			var actual result
			select {
			case actual = <-done:
			case <-time.After(2 * time.Second):
				t.Fatal("all-failure did not complete")
			}
			bodyTraces := map[string]any{}
			for id, body := range bodies {
				bodyTraces[id] = body.trace
			}
			if actual.e == nil {
				t.Fatal("all-failure returned success")
			}
			message := actual.e.Error()
			last := -1
			for _, id := range order {
				position := strings.Index(message, "release source \""+id+"\"")
				if position <= last {
					t.Fatalf("actual completion order lost: %s", message)
				}
				last = position
			}
			rows = append(rows, updaterSourceRow{Name: strings.Join(order, "-"), Operation: "ordered_all_failure", Input: map[string]any{"sources": updaterSourceDescribe(sources), "kind": string(ReleaseSourceAPI), "canonical": canonical, "completion_order": order, "witness": "Actual transport captures its probe goroutine; its verified exit proves the result was enqueued before releasing the next source; the buffered result channel preserves this completion order; no sleeps, sorting, or schedule normalization"}, Output: map[string]any{"ids": updaterSourceIDs(actual.s), "nil_ordered": actual.s == nil, "error": updaterSourceError(actual.e), "requests_by_source": records, "body_trace_by_source": bodyTraces, "witness_steps": witness, "caller_canceled": ctx.Err() != nil}})
		})
	}
	t.Run("parent-cancel-in-flight", func(t *testing.T) {
		ctx, cancel := updaterSourceContext("")
		defer cancel()
		candidates := NewReleaseSourceSelector(sources, nil).candidates(ReleaseSourceAPI)
		entered, seen, finished := map[string]chan struct{}{}, map[string]chan struct{}{}, map[string]chan struct{}{}
		release := make(chan struct{})
		records := map[string]any{}
		mu := sync.Mutex{}
		for _, s := range candidates {
			entered[s.ID()] = make(chan struct{})
			seen[s.ID()] = make(chan struct{})
			finished[s.ID()] = make(chan struct{})
		}
		client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
			id := strings.TrimSuffix(r.URL.Hostname(), ".test")
			mu.Lock()
			records[id] = updaterSourceRequest(r, ctx)
			mu.Unlock()
			close(entered[id])
			<-r.Context().Done()
			close(seen[id])
			<-release
			close(finished[id])
			return nil, r.Context().Err()
		})}
		canonical := "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
		done := make(chan error, 1)
		go func() {
			_, e := NewReleaseSourceSelector(sources, client).Ordered(ctx, ReleaseSourceAPI, canonical)
			done <- e
		}()
		for _, s := range candidates {
			updaterSourceWait(t, entered[s.ID()], s.ID()+" entered")
		}
		cancel()
		var actual error
		select {
		case actual = <-done:
		case <-time.After(2 * time.Second):
			t.Fatal("parent cancellation did not complete")
		}
		for _, s := range candidates {
			updaterSourceWait(t, seen[s.ID()], s.ID()+" cancellation")
		}
		close(release)
		for _, s := range candidates {
			updaterSourceWait(t, finished[s.ID()], s.ID()+" finish")
		}
		if !errors.Is(actual, context.Canceled) {
			t.Fatalf("parent error = %v", actual)
		}
		rows = append(rows, updaterSourceRow{Name: "in-flight", Operation: "ordered_parent_cancel", Input: map[string]any{"sources": updaterSourceDescribe(sources), "kind": string(ReleaseSourceAPI), "canonical": canonical, "witness": "all eligible transports entered; cancel parent; transport returns held until Ordered has returned"}, Output: map[string]any{"error": updaterSourceError(actual), "requests_by_source": records, "witness_steps": []string{"all-eligible-entered", "parent-canceled", "ordered-returned", "all-child-contexts-canceled", "all-transports-released"}}})
	})
	return rows
}

func updaterSourceBoundaryRows(t *testing.T) []updaterSourceRow {
	rows := []updaterSourceRow{}
	parsed := updaterSourceParse(t, "a|{url}|{url}\nasset|-|{url}\nc|{url}|{url}")
	for _, c := range []struct {
		name, mode, kind              string
		nilSelector, empty, assetOnly bool
	}{
		{name: "nil-selector", nilSelector: true, kind: string(ReleaseSourceAPI)}, {name: "nil-selector-before-canceled", nilSelector: true, mode: "canceled", kind: string(ReleaseSourceAPI)},
		{name: "pre-canceled", mode: "canceled", kind: string(ReleaseSourceAPI)}, {name: "expired-deadline", mode: "deadline", kind: string(ReleaseSourceAPI)}, {name: "nil-context", mode: "nil", kind: string(ReleaseSourceAPI)},
		{name: "empty-api", empty: true, kind: string(ReleaseSourceAPI)}, {name: "empty-asset", empty: true, kind: string(ReleaseSourceAsset)}, {name: "no-api-capability", assetOnly: true, kind: string(ReleaseSourceAPI)}, {name: "canceled-before-no-capability", mode: "canceled", assetOnly: true, kind: string(ReleaseSourceAPI)},
		{name: "canonical-http", kind: string(ReleaseSourceAPI)}, {name: "unsupported-kind-uses-assets", kind: "owned-unknown"},
	} {
		ctx, cancel := updaterSourceContext(c.mode)
		s := parsed
		if c.empty {
			s = nil
		}
		if c.assetOnly {
			s = parsed[1:2]
		}
		calls := 0
		client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
			calls++
			return &http.Response{StatusCode: 200, Status: "200 OK", Body: io.NopCloser(strings.NewReader("owned")), Request: r, Header: http.Header{}}, nil
		})}
		selector := NewReleaseSourceSelector(s, client)
		if c.nilSelector {
			selector = nil
		}
		canonical := "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
		if c.name == "canonical-http" {
			canonical = "http://github.com/owned"
			s = parsed[:1]
			selector = NewReleaseSourceSelector(s, client)
		}
		if c.name == "unsupported-kind-uses-assets" {
			s = parsed[:1]
			selector = NewReleaseSourceSelector(s, client)
		}
		var ordered []ReleaseSource
		e, p := updaterSourceCall(func() error {
			var err error
			ordered, err = selector.Ordered(ctx, ReleaseSourceKind(c.kind), canonical)
			return err
		})
		rows = append(rows, updaterSourceRow{Name: c.name, Operation: "ordered_boundary", GoOnly: c.mode == "nil" || c.nilSelector, Input: map[string]any{"sources": updaterSourceDescribe(s), "context": c.mode, "kind": c.kind, "canonical": canonical, "nil_selector": c.nilSelector}, Output: map[string]any{"ids": updaterSourceIDs(ordered), "nil_ordered": ordered == nil, "error": updaterSourceError(e), "panic": p, "transport_calls": calls}})
		cancel()
	}
	for _, mode := range []string{"", "canceled", "deadline", "nil"} {
		ctx, cancel := updaterSourceContext(mode)
		e, p := updaterSourceCall(func() error { return CheckContext(ctx, "owned action") })
		rows = append(rows, updaterSourceRow{Name: mode, Operation: "check_context", GoOnly: mode == "nil", Input: map[string]any{"context": mode, "action": "owned action"}, Output: map[string]any{"error": updaterSourceError(e), "panic": p}})
		cancel()
	}
	empty := FirstReleaseSource(nil)
	first := FirstReleaseSource(parsed)
	rows = append(rows, updaterSourceRow{Name: "empty-and-first-id", Operation: "first_source_behavior", Input: map[string]any{"sources": updaterSourceIDs(parsed)}, Output: map[string]any{"nil_empty": empty == nil, "first_id": first.ID()}})

	before := first.ID()
	first.id = "owned-mutated"
	rows = append(rows, updaterSourceRow{Name: "nil-and-borrowed-first", Operation: "first_source", GoOnly: true, Input: map[string]any{"sources_before": []string{"a", "asset", "c"}}, Output: map[string]any{"nil_empty": empty == nil, "first_before": before, "mutation_updates_input": parsed[0].ID() == "owned-mutated"}})
	parsed = updaterSourceParse(t, "a|{url}|{url}\nasset|-|{url}\nc|{url}|{url}")
	defaultClient := NewReleaseSourceSelector(parsed, nil)
	ownedClient := &http.Client{}
	owned := NewReleaseSourceSelector(parsed, ownedClient)
	parsed[0].id = "owned-mutated"
	defaultSelector := NewDefaultReleaseSourceSelector(ownedClient)
	rows = append(rows, updaterSourceRow{Name: "defaults-and-owned-copy", Operation: "constructors", GoOnly: true, Input: map[string]any{}, Output: map[string]any{"nil_client_uses_default": defaultClient.httpClient == http.DefaultClient, "provided_client_preserved": owned.httpClient == ownedClient, "input_slice_copied": owned.sources[0].ID() == "a", "default_sources": updaterSourceIDs(defaultSelector.sources)}})

	for _, mutated := range []bool{false, true} {
		ctx, cancel := updaterSourceContext("")
		original := updaterSourceParse(t, "original|{url}|{url}")
		client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
			return &http.Response{StatusCode: 200, Status: "200 OK", Body: io.NopCloser(strings.NewReader(`[]`)), Header: http.Header{}, Request: r}, nil
		})}
		selector := NewReleaseSourceSelector(original, client)
		if mutated {
			original[0] = updaterSourceParse(t, "replacement|{url}|{url}")[0]
		}
		selected, e := selector.Ordered(ctx, ReleaseSourceAPI, "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases")
		rows = append(rows, updaterSourceRow{Name: fmt.Sprintf("input-slice-replaced-%t", mutated), Operation: "ordered_constructor_copy", Input: map[string]any{"original_sources": []string{"original"}, "mutate_slice_after_constructor": mutated, "input_ids_when_ordered": updaterSourceIDs(original)}, Output: map[string]any{"ids": updaterSourceIDs(selected), "error": updaterSourceError(e)}})
		cancel()
	}

	for _, kind := range []ReleaseSourceKind{ReleaseSourceAPI, ReleaseSourceAsset, "owned-unknown"} {
		rows = append(rows, updaterSourceRow{Name: string(kind), Operation: "candidates", GoOnly: true, Input: map[string]any{"sources": updaterSourceDescribe(owned.sources), "kind": string(kind)}, Output: map[string]any{"ids": updaterSourceIDs(owned.candidates(kind))}})
	}
	for _, c := range []struct {
		name  string
		first ReleaseSource
		all   []ReleaseSource
	}{{"existing-middle", owned.sources[2], owned.sources}, {"absent-first", ReleaseSource{id: "absent"}, owned.sources}, {"duplicate-ids-removed", ReleaseSource{id: "a"}, []ReleaseSource{{id: "a"}, {id: "a"}, {id: "b"}}}, {"empty-input", ReleaseSource{id: "first"}, nil}} {
		ordered := placeReleaseSourceFirst(c.first, c.all)
		rows = append(rows, updaterSourceRow{Name: c.name, Operation: "place_first", GoOnly: true, Input: map[string]any{"first": c.first.ID(), "sources": updaterSourceIDs(c.all)}, Output: map[string]any{"ids": updaterSourceIDs(ordered)}})
	}
	return rows
}

func updaterSourceRedirectRows(t *testing.T) []updaterSourceRow {
	rows := []updaterSourceRow{}
	for _, c := range []struct {
		name       string
		kind       ReleaseSourceKind
		redirects  int
		closeError bool
		final      string
	}{
		{"api-one-redirect", ReleaseSourceAPI, 1, false, `[]`},
		{"asset-one-redirect", ReleaseSourceAsset, 1, false, "owned asset bytes"},
		{"api-redirect-close-error-ignored", ReleaseSourceAPI, 1, true, `[]`},
		{"api-final-two-values", ReleaseSourceAPI, 1, false, `[] []`},
		{"api-default-ten-request-cap", ReleaseSourceAPI, 11, false, `[]`},
	} {
		ctx, cancel := updaterSourceContext("")
		sources := updaterSourceParse(t, "owned|https://mirror.test/{url}|https://mirror.test/{url}")
		requests := []any{}
		bodies := []*updaterSourceBody{}
		index := 0
		client := &http.Client{Transport: updaterSourceTransport(func(r *http.Request) (*http.Response, error) {
			requests = append(requests, updaterSourceRequest(r, ctx))
			status, payload := 200, c.final
			header := http.Header{}
			if index < c.redirects {
				status, payload = 302, "owned redirect body"
				header.Set("Location", fmt.Sprintf("https://landing.test/owned/%d", index+1))
			}
			body := &updaterSourceBody{body: []byte(payload), trace: []string{}}
			if c.closeError && index == 0 {
				body.mode = "close-error"
			}
			bodies = append(bodies, body)
			index++
			return &http.Response{StatusCode: status, Status: fmt.Sprintf("%d %s", status, http.StatusText(status)), Header: header, Body: body, Request: r}, nil
		})}
		canonical := "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
		if c.kind == ReleaseSourceAsset {
			canonical = "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/checksums.txt"
		}
		ordered, e := NewReleaseSourceSelector(sources, client).Ordered(ctx, c.kind, canonical)
		traces := []any{}
		for _, b := range bodies {
			traces = append(traces, b.trace)
		}
		rows = append(rows, updaterSourceRow{Name: c.name, Operation: "ordered_redirect", Input: map[string]any{"sources": updaterSourceDescribe(sources), "kind": string(c.kind), "canonical": canonical, "redirects": c.redirects, "redirect_close_error": c.closeError, "final_body": c.final}, Output: map[string]any{"ids": updaterSourceIDs(ordered), "nil_ordered": ordered == nil, "error": updaterSourceError(e), "requests": requests, "body_traces": traces}})
		cancel()
	}
	return rows
}

func TestMigrationUpdaterSourceSelection(t *testing.T) {
	root := filepath.Join("..", "..", "..")
	guards := map[string]string{
		"go.mod":                           "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum":                           "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/update/source/github.go": "1fd8c8abd4e0855298475ce7455ab775bc4eec6c3fda07a59b76fbb8780c617d",
		"internal/update/source/release_source_selector.go": "cbfb9bedcd32b391a52ce6a038710e45320dfbd2edf70c746ad72d22b8ad21f0",
		"internal/update/source/release_sources.go":         "5450d686f836da97e264872e80ba4178bcec7a4217fd8a9eaa85abfccf144bc8",
		"internal/update/source/source.go":                  "3d0912a3937d6cfa6f871c380b60922e60898ca23f88e3b722179c4f1ba64aa5",
		"internal/update/source/source_test.go":             "d7a8377d5ef2287b2ccef963b3dd15494d90e00fb316452391b4c842ed2f3f89",
		"internal/update/source/release_sources.txt":        "e694ddf4ca91dcf649fc599d5d54bf0f67f67340d8fca10ec2314b345f2b4588",
	}
	fixture := updaterSourceFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", Environment: runtime.GOOS + "/" + runtime.GOARCH, GoVersion: runtime.Version(), Sources: map[string]string{}, APIMap: map[string]string{
		"CheckContext":                     "check_context; nil context panic is Go-only",
		"DefaultReleaseSources":            "default_sources and embedded_transform; each call owns a fresh parsed slice",
		"FirstReleaseSource":               "first_source_behavior; separate first_source Go pointer mutation row is Go-only",
		"GitHubUserAgent":                  "default_sources and actual request headers in ordered_single/ordered_race",
		"NewDefaultReleaseSourceSelector":  "constructors (private dependency identity Go-only); actual defaults captured and transformed",
		"NewReleaseSourceSelector":         "constructors (private dependency/slice identity Go-only); ordered_constructor_copy proves copied input slice through actual public Ordered; every public Ordered row uses actual constructor",
		"ParseReleaseSources":              "parse; embedded five-source bytes, ID grammar, field/template validation and error precedence",
		"ReleaseSource.ID/APIURL/AssetURL": "transform and embedded_transform; canonical validation precedes transformed validation; no repository allowlist at source boundary",
		"ReleaseSourceSelector.Ordered":    "ordered_single, ordered_boundary, ordered_race, ordered_all_failure, ordered_parent_cancel",
		"candidates/parseReleaseSourceTemplate/releaseSourceTemplate.apply/placeReleaseSourceFirst/probe": "Go-only private rows retain direct contracts without adding production seams; public behavior also exercised through actual parser/Ordered",
	}, Boundaries: map[string]string{
		"transport":   "Actual unchanged http.Client over owned synthetic RoundTripper and Body; records actual method, transformed URL, headers, nil body, context identity/value/deadline, read/close; no sockets or external requests",
		"race":        "Actual concurrent Ordered: all eligible requests enter before winner release; losers wait for real child cancellation; records keyed by source retain each actual local trace without normalizing an uncontrolled global schedule",
		"all_failure": "Actual transport captures its probe goroutine; verified goroutine exit proves its result was enqueued before releasing next source; buffered result channel preserves order; errors.Join child order/message retained unchanged",
		"probe":       "API requires one non-null release-shaped array and EOF; asset drains arbitrary successful200 body; status/read/decode errors fail; close errors ignored",
		"ownership":   "Source constructor slice copy, default fresh slice, default HTTP client and FirstReleaseSource Go pointer identity rows are Go-only; no Rust public test seam required",
		"scope":       "Bounded frozen source API/actual private behavior on Linux/amd64 Go1.27.1; no native platform, API page download/fallback, installer verification or denied supplemental native/upload probe claim",
	}}
	for path, expected := range guards {
		body, e := os.ReadFile(filepath.Join(root, path))
		if e != nil {
			t.Fatal(e)
		}
		sum := sha256.Sum256(body)
		actual := hex.EncodeToString(sum[:])
		if actual != expected {
			t.Fatalf("frozen source changed: %s = %s, expected %s", path, actual, expected)
		}
		fixture.Sources[path] = actual
	}
	own := "internal/update/source/migration_connected_test.go"
	body, e := os.ReadFile(filepath.Join(root, own))
	if e != nil {
		t.Fatal(e)
	}
	sum := sha256.Sum256(body)
	fixture.Sources[own] = hex.EncodeToString(sum[:])
	fixture.Cases = append(fixture.Cases, updaterSourceParserRows(t)...)
	fixture.Cases = append(fixture.Cases, updaterSourceTransformRows(t)...)
	fixture.Cases = append(fixture.Cases, updaterSourceProbeRows(t)...)
	fixture.Cases = append(fixture.Cases, updaterSourceRedirectRows(t)...)
	fixture.Cases = append(fixture.Cases, updaterSourceRaceRows(t)...)
	fixture.Cases = append(fixture.Cases, updaterSourceBoundaryRows(t)...)
	if t.Failed() {
		t.Fatal("failed source witness cannot be captured or replayed")
	}
	sealed, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	sealed = append(sealed, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "updater-source-selection.json")
	if os.Getenv("PIXIV_CAPTURE_UPDATER_SOURCE") == "1" {
		if e = os.WriteFile(path, sealed, 0600); e != nil {
			t.Fatal(e)
		}
		t.Logf("captured %d rows, %d bytes", len(fixture.Cases), len(sealed))
		return
	}
	expected, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	if !bytes.Equal(expected, sealed) {
		got := filepath.Join(os.TempDir(), "updater-source-selection.actual.json")
		if e = os.WriteFile(got, sealed, 0600); e != nil {
			t.Fatal(e)
		}
		t.Fatalf("actual source observations differ from frozen fixture (%d rows/%d bytes); actual at %s", len(fixture.Cases), len(sealed), got)
	}
	t.Logf("replayed %d rows, %d bytes exactly", len(fixture.Cases), len(sealed))
}

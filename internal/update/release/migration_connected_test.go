package release

import (
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
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/update/source"
)

var updaterReleaseCapture = flag.Bool("migration-capture-updater-release-cache", false, "capture frozen connected release/cache contracts")

const updaterReleaseReference = "4b4426487ef18bed276706daec385e0d0a6979f9"
const updaterReleasePublished = "0e5f42273f69067bf2e06a130c926c4d58557fb4"
const updaterReleaseEndpoint = "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases"
const updaterReleaseNow = "2026-10-10T12:00:00Z"

type updaterReleaseReply struct {
	URL           string      `json:"url"`
	Status        int         `json:"status"`
	Headers       http.Header `json:"headers,omitempty"`
	Body          string      `json:"body"`
	RequestError  string      `json:"request_error,omitempty"`
	ReadError     string      `json:"read_error,omitempty"`
	ErrorWithData bool        `json:"error_with_data,omitempty"`
	CloseError    string      `json:"close_error,omitempty"`
	CancelAt      string      `json:"cancel_at,omitempty"`
	WaitContext   bool        `json:"wait_context,omitempty"`
}

type updaterReleaseInput struct {
	APIBaseURL         string                `json:"api_base_url,omitempty"`
	Repository         string                `json:"repository,omitempty"`
	NilCache           bool                  `json:"nil_cache,omitempty"`
	NilClient          bool                  `json:"nil_client,omitempty"`
	CacheExists        bool                  `json:"cache_exists"`
	CacheBefore        string                `json:"cache_before"`
	ReadError          string                `json:"cache_read_error,omitempty"`
	WriteError         string                `json:"cache_write_error,omitempty"`
	CacheCancelAt      string                `json:"cache_cancel_at,omitempty"`
	CacheWaitContext   bool                  `json:"cache_wait_context,omitempty"`
	CancelBefore       bool                  `json:"cancel_before,omitempty"`
	NowCancel          bool                  `json:"now_cancel,omitempty"`
	NowDateYear        int                   `json:"now_date_year,omitempty"`
	Now                string                `json:"now"`
	ParentDeadline     string                `json:"parent_deadline,omitempty"`
	Automatic          bool                  `json:"automatic"`
	IncludePrerelease  bool                  `json:"include_prerelease"`
	PublicSourceWinner string                `json:"public_source_winner,omitempty"`
	PrivateMirror      bool                  `json:"private_single_mirror,omitempty"`
	Replies            []updaterReleaseReply `json:"replies"`
}

type updaterReleaseRequest struct {
	Method   string      `json:"method"`
	URL      string      `json:"url"`
	Headers  http.Header `json:"headers"`
	HasBody  bool        `json:"has_body"`
	Deadline string      `json:"deadline"`
}

type updaterReleaseBodyState struct {
	Reads      int    `json:"reads"`
	Bytes      int    `json:"bytes"`
	Closes     int    `json:"closes"`
	ReadError  string `json:"read_error"`
	CloseError string `json:"close_error"`
}

type updaterReleaseSelected struct {
	TagName    string         `json:"tag_name"`
	Version    string         `json:"version"`
	Prerelease bool           `json:"prerelease"`
	Assets     []ReleaseAsset `json:"assets"`
}

type updaterReleaseObservation struct {
	Release             *updaterReleaseSelected   `json:"release"`
	Throttled           bool                      `json:"throttled"`
	Error               string                    `json:"error"`
	ErrorChain          []string                  `json:"error_chain"`
	Canceled            bool                      `json:"canceled"`
	DeadlineError       bool                      `json:"deadline_error"`
	Trace               []string                  `json:"trace"`
	Requests            []updaterReleaseRequest   `json:"requests"`
	Bodies              []updaterReleaseBodyState `json:"bodies"`
	Writes              []string                  `json:"cache_write_attempts"`
	CacheAfter          string                    `json:"cache_after"`
	CacheExists         bool                      `json:"cache_exists_after"`
	PortDeadlines       []string                  `json:"cache_port_deadlines"`
	ProbeLosersCanceled int                       `json:"probe_losers_canceled"`
	NowCalls            int                       `json:"now_calls"`
}

type updaterReleaseCase struct {
	Name        string                    `json:"name"`
	Boundary    string                    `json:"boundary"`
	Input       updaterReleaseInput       `json:"input"`
	Observation updaterReleaseObservation `json:"observation"`
}

type updaterReleaseFixture struct {
	Reference   string               `json:"reference"`
	Published   string               `json:"published_base"`
	Toolchain   string               `json:"toolchain"`
	Environment string               `json:"environment"`
	Sources     map[string]string    `json:"source_sha256"`
	Limitations []string             `json:"limitations"`
	Cases       []updaterReleaseCase `json:"cases"`
}

type updaterReleaseCachePort struct {
	input       updaterReleaseInput
	observation *updaterReleaseObservation
	cancel      context.CancelFunc
	parent      context.Context
}

func (c *updaterReleaseCachePort) Read(ctx context.Context) ([]byte, bool, error) {
	c.observation.Trace = append(c.observation.Trace, "cache.read")
	c.observation.PortDeadlines = append(c.observation.PortDeadlines, updaterReleaseDeadline(ctx, c.parent))
	if c.input.CacheCancelAt == "read" {
		c.cancel()
	}
	if c.input.CacheWaitContext {
		<-ctx.Done()
		return nil, false, ctx.Err()
	}
	if c.input.ReadError != "" {
		return nil, false, errors.New(c.input.ReadError)
	}
	return []byte(c.observation.CacheAfter), c.observation.CacheExists, nil
}

func (c *updaterReleaseCachePort) Write(ctx context.Context, data []byte) error {
	c.observation.Trace = append(c.observation.Trace, "cache.write")
	c.observation.PortDeadlines = append(c.observation.PortDeadlines, updaterReleaseDeadline(ctx, c.parent))
	c.observation.Writes = append(c.observation.Writes, string(data))
	if c.input.CacheCancelAt == "write" {
		c.cancel()
	}
	if c.input.WriteError != "" {
		return errors.New(c.input.WriteError)
	}
	c.observation.CacheAfter = string(data)
	c.observation.CacheExists = true
	return nil
}

type updaterReleaseTransport func(*http.Request) (*http.Response, error)

func (f updaterReleaseTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type updaterReleaseBody struct {
	reader      *strings.Reader
	reply       updaterReleaseReply
	index       int
	observation *updaterReleaseObservation
	cancel      context.CancelFunc
}

func (b *updaterReleaseBody) Read(p []byte) (int, error) {
	state := &b.observation.Bodies[b.index]
	state.Reads++
	b.observation.Trace = append(b.observation.Trace, fmt.Sprintf("body.%d.read", b.index+1))
	if b.reply.CancelAt == "read" {
		b.cancel()
	}
	if b.reply.ReadError != "" && (!b.reply.ErrorWithData || b.reader.Len() == 0) {
		state.ReadError = b.reply.ReadError
		return 0, errors.New(b.reply.ReadError)
	}
	n, err := b.reader.Read(p)
	state.Bytes += n
	if b.reply.ReadError != "" && b.reply.ErrorWithData {
		state.ReadError = b.reply.ReadError
		err = errors.New(b.reply.ReadError)
	}
	return n, err
}

func (b *updaterReleaseBody) Close() error {
	state := &b.observation.Bodies[b.index]
	state.Closes++
	b.observation.Trace = append(b.observation.Trace, fmt.Sprintf("body.%d.close", b.index+1))
	if b.reply.CancelAt == "close" {
		b.cancel()
	}
	if b.reply.CloseError != "" {
		state.CloseError = b.reply.CloseError
		return errors.New(b.reply.CloseError)
	}
	return nil
}

func updaterReleaseDeadline(ctx, parent context.Context) string {
	deadline, ok := ctx.Deadline()
	if !ok {
		return "none"
	}
	if parentDeadline, exists := parent.Deadline(); exists && deadline.Equal(parentDeadline) {
		return "parent-preserved"
	}
	if remaining := time.Until(deadline); remaining > 3*time.Second || remaining < -100*time.Millisecond {
		return "unexpected-child-deadline"
	}
	return "automatic-3s-child"
}

func updaterReleaseObserve(t *testing.T, input updaterReleaseInput) updaterReleaseObservation {
	t.Helper()
	observation := updaterReleaseObservation{
		Trace: []string{}, Requests: []updaterReleaseRequest{}, Bodies: []updaterReleaseBodyState{},
		Writes: []string{}, ErrorChain: []string{}, PortDeadlines: []string{},
		CacheAfter: input.CacheBefore, CacheExists: input.CacheExists,
	}
	parent := context.Background()
	var deadlineCancel context.CancelFunc
	if input.ParentDeadline != "" {
		duration, err := time.ParseDuration(input.ParentDeadline)
		if err != nil {
			t.Fatal(err)
		}
		parent, deadlineCancel = context.WithTimeout(parent, duration)
		defer deadlineCancel()
	}
	ctx, cancel := context.WithCancel(parent)
	defer cancel()
	if input.CancelBefore {
		cancel()
	}
	cache := &updaterReleaseCachePort{input: input, observation: &observation, cancel: cancel, parent: parent}
	winnerRecorded, loserRecorded, loserDone := make(chan struct{}), make(chan struct{}), make(chan struct{})
	var witnessMu sync.Mutex
	winnerRequests := 0
	transport := updaterReleaseTransport(func(request *http.Request) (*http.Response, error) {
		firstWinner, loser := false, false
		if input.PublicSourceWinner != "" {
			winnerURL := "https://gh-proxy.com/" + updaterReleaseEndpoint
			if input.PublicSourceWinner == "github-direct" {
				winnerURL = updaterReleaseEndpoint
			}
			loserURL := updaterReleaseEndpoint
			if input.PublicSourceWinner == "github-direct" {
				loserURL = "https://gh-proxy.com/" + updaterReleaseEndpoint
			}
			if request.URL.String() == loserURL {
				loser = true
				<-winnerRecorded
			}
			if request.URL.String() == winnerURL {
				witnessMu.Lock()
				winnerRequests++
				firstWinner = winnerRequests == 1
				witnessMu.Unlock()
				if !firstWinner {
					<-loserDone
				}
			}
		}
		index := len(observation.Requests)
		observation.Trace = append(observation.Trace, fmt.Sprintf("request.%d", index+1))
		observation.Requests = append(observation.Requests, updaterReleaseRequest{
			Method: request.Method, URL: request.URL.String(), Headers: request.Header.Clone(),
			HasBody: request.Body != nil, Deadline: updaterReleaseDeadline(request.Context(), parent),
		})
		if firstWinner {
			close(winnerRecorded)
			<-loserRecorded
		}
		if loser {
			close(loserRecorded)
		}
		if index >= len(input.Replies) {
			t.Fatalf("unexpected request %s", request.URL)
		}
		reply := input.Replies[index]
		if reply.URL != request.URL.String() {
			t.Fatalf("request URL = %q, want %q", request.URL, reply.URL)
		}
		if reply.CancelAt == "request" {
			cancel()
		}
		if reply.WaitContext {
			<-request.Context().Done()
			if loser {
				if request.Context().Err() == context.Canceled {
					observation.ProbeLosersCanceled++
				}
				observation.Trace = append(observation.Trace, "probe.loser.canceled")
				close(loserDone)
			}
			return nil, request.Context().Err()
		}
		if reply.RequestError != "" {
			return nil, errors.New(reply.RequestError)
		}
		bodyIndex := len(observation.Bodies)
		observation.Bodies = append(observation.Bodies, updaterReleaseBodyState{})
		body := &updaterReleaseBody{reader: strings.NewReader(reply.Body), reply: reply, index: bodyIndex, observation: &observation, cancel: cancel}
		return &http.Response{StatusCode: reply.Status, Status: fmt.Sprintf("%d %s", reply.Status, http.StatusText(reply.Status)), Header: reply.Headers.Clone(), Body: body, Request: request}, nil
	})
	now, err := time.Parse(time.RFC3339Nano, input.Now)
	if err != nil {
		t.Fatal(err)
	}
	if input.NowDateYear != 0 {
		now = time.Date(input.NowDateYear, time.October, 10, 12, 0, 0, 0, time.UTC)
	}
	options := ReleaseClientOptions{EnablePublicReleaseSources: input.PublicSourceWinner != "", APIBaseURL: input.APIBaseURL, Repository: input.Repository, Cache: cache, HTTPClient: &http.Client{Transport: transport}, Now: func() time.Time {
		observation.NowCalls++
		observation.Trace = append(observation.Trace, "now")
		if input.NowCancel {
			cancel()
		}
		return now
	}}
	if input.NilCache {
		options.Cache = nil
	}
	client, err := NewGitHubReleaseClient(options)
	if err == nil && input.PrivateMirror {
		routes, routeErr := source.ParseReleaseSources([]byte("fixture-mirror|https://mirror.invalid/{url}|https://mirror.invalid/{url}\n"))
		if routeErr != nil {
			t.Fatal(routeErr)
		}
		client.sourceSelector = source.NewReleaseSourceSelector(routes, options.HTTPClient)
	}
	if err == nil {
		if input.NilClient {
			client = nil
		}
		var result ReleaseCheckResult
		result, err = client.Check(ctx, ReleaseCheckOptions{Automatic: input.Automatic, IncludePrerelease: input.IncludePrerelease})
		observation.Throttled = result.Throttled
		if result.Release != nil {
			observation.Release = &updaterReleaseSelected{TagName: result.Release.TagName, Version: result.Release.Version, Prerelease: result.Release.Prerelease, Assets: result.Release.Assets}
		}
	}
	if err != nil {
		observation.Error = err.Error()
		observation.Canceled = errors.Is(err, context.Canceled)
		observation.DeadlineError = errors.Is(err, context.DeadlineExceeded)
		for cause := err; cause != nil; cause = errors.Unwrap(cause) {
			observation.ErrorChain = append(observation.ErrorChain, cause.Error())
		}
	}
	return observation
}

func updaterReleaseCacheJSON(schema int, checked, releases, pages string) string {
	if pages == "" {
		pages = "[]"
	}
	return fmt.Sprintf(`{"schema_version":%d,"checked_at":%q,"releases":%s,"pages":%s}`, schema, checked, releases, pages)
}

func updaterReleasePageJSON(url, etag, next, releases string) string {
	return fmt.Sprintf(`{"url":%q,"etag":%q,"next_url":%q,"releases":%s}`, url, etag, next, releases)
}

func updaterReleaseCases() []updaterReleaseCase {
	var cases []updaterReleaseCase
	add := func(name string, input updaterReleaseInput) {
		replies := make([]updaterReleaseReply, len(input.Replies))
		copy(replies, input.Replies)
		for index := range replies {
			replies[index].Headers = replies[index].Headers.Clone()
		}
		input.Replies = replies
		if input.Now == "" {
			input.Now = updaterReleaseNow
		}
		if input.Replies == nil {
			input.Replies = []updaterReleaseReply{}
		}
		boundary := "public-constructor-check-transport-cache"
		if input.NilClient {
			boundary = "go-nil-receiver-only"
		}
		if input.PrivateMirror {
			boundary = "private-selector-injection-connected-check"
		}
		cases = append(cases, updaterReleaseCase{Name: name, Boundary: boundary, Input: input})
	}
	query := func(body string) updaterReleaseInput {
		return updaterReleaseInput{Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 200, Body: body}}}
	}
	stable := `[{"tag_name":"v1.2.3","assets":[{"name":"opaque.bin","browser_download_url":"https://owned.invalid/opaque"}]}]`
	add("selected-assets-carried-unchanged", query(stable))
	add("empty-array-no-release-still-writes", query(`[]`))
	add("null-query-is-accepted-and-persisted-empty", query(`null`))
	add("query-first-value-ignores-second-value", query(stable+` {"second":true}`))
	add("query-first-value-ignores-invalid-trailing", query(stable+` trailing garbage`))
	add("query-invalid-first-value", query(`not JSON`))
	add("query-object-instead-of-array", query(`{}`))
	add("query-malformed-field-type", query(`[{"tag_name":42}]`))
	add("query-missing-tag-published-fails", query(`[{}]`))
	add("draft-invalid-tag-skipped-and-removed-from-cache", query(`[{"tag_name":"invalid","draft":true},{"tag_name":"v1.0.0"}]`))
	add("labelled-prerelease-invalid-tag-stable-skips-before-parse", query(`[{"tag_name":"invalid","prerelease":true},{"tag_name":"v1.0.0"}]`))
	in := query(`[{"tag_name":"invalid","prerelease":true},{"tag_name":"v1.0.0"}]`)
	in.IncludePrerelease = true
	add("labelled-prerelease-invalid-tag-include-fails", in)
	add("semantic-prerelease-stable-skips", query(`[{"tag_name":"v2.0.0-rc.1"},{"tag_name":"v1.0.0"}]`))
	in = query(`[{"tag_name":"v2.0.0-rc.1"},{"tag_name":"v1.0.0"}]`)
	in.IncludePrerelease = true
	add("semantic-prerelease-include-selects", in)
	in = query(`[{"tag_name":"v1.0.0","prerelease":true}]`)
	in.IncludePrerelease = true
	add("labelled-stable-tag-is-prerelease", in)
	add("all-draft-or-prerelease-no-candidate", query(`[{"tag_name":"bad","draft":true},{"tag_name":"bad","prerelease":true},{"tag_name":"v2.0.0-alpha"}]`))
	add("equal-build-precedence-first-release-wins", query(`[{"tag_name":"v1.2.3+first","assets":[{"name":"first","browser_download_url":"first"}]},{"tag_name":"v1.2.3+second","assets":[{"name":"second","browser_download_url":"second"}]}]`))
	add("late-malformed-published-tag-fails-after-newer", query(`[{"tag_name":"v99.0.0"},{"tag_name":"broken"}]`))
	for _, tag := range []string{"1.2.3", "v", "v1.2", "v01.2.3", "v1.02.3", "v1.2.03", "v1.-2.3", "v1.2.3-", "v1.2.3-01", "v1.2.3-alpha..1", "v1.2.3-α", "v1.2.3+", "v1.2.3+a_b", "v1.2.3+a+b", "v1.2.3.4", " v1.2.3", "v1.2.3\n"} {
		add("malformed-tag-"+fmt.Sprintf("%x", []byte(tag)), query(fmt.Sprintf(`[{"tag_name":%q}]`, tag)))
	}
	large := strings.Repeat("9", 100)
	for _, dimension := range []string{"major", "minor", "patch"} {
		first, second := "v9.0.0", "v"+large+".0.0"
		if dimension == "minor" {
			first, second = "v1.9.0", "v1."+large+".0"
		}
		if dimension == "patch" {
			first, second = "v1.0.9", "v1.0."+large
		}
		add("unbounded-"+dimension+"-decimal-order", query(fmt.Sprintf(`[{"tag_name":%q},{"tag_name":%q}]`, first, second)))
	}
	for _, pair := range [][2]string{{"v1.0.0-alpha.9", "v1.0.0-alpha." + large}, {"v1.0.0-alpha.1", "v1.0.0-alpha.beta"}, {"v1.0.0-alpha", "v1.0.0-alpha.1"}, {"v1.0.0-rc.1", "v1.0.0"}, {"v1.0.0-alpha.Z", "v1.0.0-alpha.a"}, {"v1.0.0+001", "v1.0.0+002"}, {"v1.0.0-alpha-1+001", "v1.0.0-alpha-2+000"}} {
		in = query(fmt.Sprintf(`[{"tag_name":%q},{"tag_name":%q}]`, pair[0], pair[1]))
		in.IncludePrerelease = true
		add("prerelease-build-order-"+pair[0]+"-"+pair[1], in)
	}
	page2 := updaterReleaseEndpoint + "?page=2"
	in = updaterReleaseInput{Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 200, Body: `[{"tag_name":"v1.0.0"}]`, Headers: http.Header{"Link": {`<?page=2>; REL="prev next"`}, "Etag": {`"one"`}}}, {URL: page2, Status: 200, Body: `[{"tag_name":"v3.0.0"}]`, Headers: http.Header{"Etag": {`"two"`}, "Link": {`<https://foreign.invalid/ignored>; rel="last"`}}}}}
	add("relative-two-pages-all-pages-selected", in)
	add("three-pages-tie-retains-first-and-drafts-excluded", updaterReleaseInput{Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 200, Body: `[{"tag_name":"v3.0.0+first"}]`, Headers: http.Header{"Link": {`<?page=2>; rel=next`}}}, {URL: page2, Status: 200, Body: `[{"tag_name":"bad","draft":true},{"tag_name":"v3.0.0+second"}]`, Headers: http.Header{"Link": {`<?page=3>; rel="next"`}}}, {URL: updaterReleaseEndpoint + "?page=3", Status: 200, Body: `[{"tag_name":"v2.0.0"}]`}}})
	for name, link := range map[string]string{
		"different-host":         `<https://foreign.invalid/repos/FlanChanXwO/pixiv-cli/releases?page=2>; rel="next"`,
		"different-scheme":       `<http://api.github.com/repos/FlanChanXwO/pixiv-cli/releases?page=2>; rel="next"`,
		"different-port":         `<https://api.github.com:443/repos/FlanChanXwO/pixiv-cli/releases?page=2>; rel="next"`,
		"userinfo":               `<https://u@api.github.com/repos/FlanChanXwO/pixiv-cli/releases?page=2>; rel="next"`,
		"fragment":               `<?page=2#fragment>; rel="next"`,
		"different-path":         `</user>; rel="next"`,
		"empty-link":             ``,
		"empty-comma-part":       `<?page=2>; rel=next,`,
		"missing-angle":          `?page=2; rel=next`,
		"missing-value":          `<?page=2>; rel`,
		"duplicate-next":         `<?page=2>; rel=next, <?page=3>; rel=next`,
		"canonical-dot-loop":     `</repos/FlanChanXwO/./pixiv-cli/releases>; rel=next`,
		"canonical-encoded-loop": `<https://api.github.com/repos/FlanChanXwO/%70ixiv-cli/releases>; rel=next`,
		"same-url-loop":          `<` + updaterReleaseEndpoint + `>; rel=next`,
	} {
		in = query(stable)
		in.Replies[0].Headers = http.Header{"Link": {link}}
		add("pagination-reject-"+name, in)
	}
	for name, link := range map[string]string{"relation-case-is-not-next": `<?page=2>; rel=Next`, "unrelated-origin-ignored": `<https://foreign.invalid/not-an-endpoint>; rel=last`, "missing-rel-ignored": `<?page=2>; title="next"`, "non-next-bad-parameter-still-fails": `<?page=2>; title`} {
		in = query(stable)
		in.Replies[0].Headers = http.Header{"Link": {link}}
		add("pagination-syntax-"+name, in)
	}
	in = query(stable)
	in.Replies[0].Headers = http.Header{"Link": {`<?page=%ZZ>; rel=next`}}
	in.Replies = append(in.Replies, updaterReleaseReply{URL: updaterReleaseEndpoint + "?page=%ZZ", Status: 200, Body: `[]`})
	add("pagination-invalid-percent-query-is-not-validated", in)
	in = query(stable)
	in.Replies[0].Headers = http.Header{"Link": {`<?page=2>; rel=next`, `<?page=3>; rel=next`}}
	add("pagination-duplicate-next-across-header-lines", in)
	in = query(stable)
	in.Replies[0].Headers = http.Header{"Link": {`</repos/FlanChanXwO/./pixiv-cli/releases/?page=2>; rel=next`}}
	in.Replies = append(in.Replies, updaterReleaseReply{URL: "https://api.github.com/repos/FlanChanXwO/pixiv-cli/releases/?page=2", Status: 200, Body: `[]`})
	add("normalized-path-with-trailing-slash-allowed", in)
	cachedPages := "[" + updaterReleasePageJSON(updaterReleaseEndpoint, `"one"`, page2, stable) + "," + updaterReleasePageJSON(page2, `"two"`, "", `[{"tag_name":"v4.0.0"}]`) + "]"
	cache := updaterReleaseCacheJSON(2, "2026-10-08T12:00:00Z", stable, cachedPages)
	in = updaterReleaseInput{CacheExists: true, CacheBefore: cache, Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 304, Body: "unread"}, {URL: page2, Status: 304, Body: "unread"}}}
	add("schema2-exact-cached-pages-304-reuse-assets-and-next", in)
	in = updaterReleaseInput{Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 304, Body: "unread"}}}
	add("304-without-cache-fails", in)
	in.CacheExists = true
	in.CacheBefore = updaterReleaseCacheJSON(2, "2026-10-08T12:00:00Z", stable, "[]")
	add("304-schema2-without-exact-page-fails", in)
	in.CacheBefore = updaterReleaseCacheJSON(2, "2026-10-08T12:00:00Z", stable, "["+updaterReleasePageJSON(page2, `"wrong"`, "", stable)+"]")
	add("304-schema2-different-page-fails", in)
	for _, schema := range []int{0, 1, 3} {
		in = query(stable)
		in.CacheExists = true
		in.CacheBefore = updaterReleaseCacheJSON(schema, updaterReleaseNow, stable, cachedPages)
		in.Automatic = true
		add(fmt.Sprintf("schema%d-refresh-without-etag-or-throttle", schema), in)
		in.Replies[0].Status = 304
		add(fmt.Sprintf("schema%d-304-rejected-without-page-reuse", schema), in)
	}
	for name, pages := range map[string]string{
		"relative-url":             "[" + updaterReleasePageJSON("?page=2", "e", "", stable) + "]",
		"foreign-origin":           "[" + updaterReleasePageJSON("https://foreign.invalid/x", "e", "", stable) + "]",
		"foreign-path":             "[" + updaterReleasePageJSON("https://api.github.com/user", "e", "", stable) + "]",
		"duplicate-canonical-path": "[" + updaterReleasePageJSON(updaterReleaseEndpoint, "e", "", stable) + "," + updaterReleasePageJSON("https://api.github.com/repos/FlanChanXwO/./pixiv-cli/releases", "f", "", stable) + "]",
	} {
		add("cached-page-reject-"+name, updaterReleaseInput{CacheExists: true, CacheBefore: updaterReleaseCacheJSON(2, "2026-10-08T12:00:00Z", stable, pages)})
	}
	for name, next := range map[string]string{"relative-next": "?page=2", "foreign-next": "https://foreign.invalid/x", "loop-next": updaterReleaseEndpoint} {
		in = updaterReleaseInput{CacheExists: true, CacheBefore: updaterReleaseCacheJSON(2, "2026-10-08T12:00:00Z", stable, "["+updaterReleasePageJSON(updaterReleaseEndpoint, "e", next, stable)+"]"), Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 304, Body: "unread"}}}
		add("cached-next-reject-"+name, in)
	}
	for name, checked := range map[string]string{"before-24h": "2026-10-09T12:00:00.000000001Z", "at-24h": "2026-10-09T12:00:00Z", "after-24h": "2026-10-09T11:59:59.999999999Z", "future-time": "2026-10-11T12:00:00Z"} {
		in = updaterReleaseInput{Automatic: true, CacheExists: true, CacheBefore: updaterReleaseCacheJSON(2, checked, stable, "[]")}
		if name == "at-24h" || name == "after-24h" {
			in.Replies = query(stable).Replies
		}
		add("automatic-throttle-"+name, in)
	}
	in = query(stable)
	in.CacheExists = true
	in.CacheBefore = updaterReleaseCacheJSON(2, updaterReleaseNow, stable, "[]")
	add("explicit-never-throttles", in)
	in = updaterReleaseInput{Automatic: true, CacheExists: true, CacheBefore: updaterReleaseCacheJSON(2, updaterReleaseNow, `[{"tag_name":"bad"}]`, "[]")}
	add("throttled-cache-still-validates-tags", in)
	in.CacheBefore = updaterReleaseCacheJSON(2, updaterReleaseNow, `[{"tag_name":"v1.0.0-rc.1"}]`, "[]")
	add("throttled-cache-channel-no-candidate", in)
	in.IncludePrerelease = true
	add("throttled-cache-channel-prerelease", in)
	in.CacheBefore = updaterReleaseCacheJSON(2, updaterReleaseNow, stable, "["+updaterReleasePageJSON("foreign", "e", "", stable)+"]")
	add("throttled-cache-does-not-validate-pages", in)
	for name, data := range map[string]string{"invalid-json": "{", "missing-checked-at": `{"schema_version":2}`, "invalid-time": `{"checked_at":"not-time"}`, "extra-json-value": updaterReleaseCacheJSON(2, updaterReleaseNow, stable, "[]") + ` {}`, "null-cache": `null`} {
		add("cache-decode-"+name, updaterReleaseInput{CacheExists: true, CacheBefore: data})
	}
	add("cache-read-error-stops-before-clock", updaterReleaseInput{ReadError: "owned cache read failure"})
	in = query(stable)
	in.WriteError = "owned cache write failure"
	add("cache-write-error-discards-selected-result", in)
	in = query(`[{"tag_name":"bad"}]`)
	in.WriteError = "unreached cache write failure"
	add("selection-failure-before-write", in)
	in = query(stable)
	in.ReadError = "owned cache read failure"
	add("read-failure-before-network", in)
	for _, status := range []int{204, 403, 404, 500} {
		in = query("unread")
		in.Replies[0].Status = status
		in.Replies[0].ReadError = "unreached read failure"
		in.Replies[0].CloseError = "ignored close failure"
		add(fmt.Sprintf("http-status-%d-before-body-read-close-ignored", status), in)
	}
	in = query(stable)
	in.Replies[0].RequestError = "owned request failure"
	add("request-error-before-response", in)
	in = query(stable)
	in.Replies[0].ReadError = "owned read failure"
	in.Replies[0].CloseError = "ignored close failure"
	add("read-error-precedes-close-error", in)
	in = query(stable)
	in.Replies[0].CloseError = "ignored close failure"
	add("successful-query-close-error-ignored", in)
	in = query(stable)
	in.Replies[0].ReadError = "owned read failure with complete JSON"
	in.Replies[0].ErrorWithData = true
	add("complete-first-json-value-accepts-simultaneous-read-error", in)
	in = query(`[{`)
	in.Replies[0].ReadError = "owned read failure with incomplete JSON"
	in.Replies[0].ErrorWithData = true
	add("incomplete-json-preserves-simultaneous-read-error", in)
	in = updaterReleaseInput{CacheExists: true, CacheBefore: cache, Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 304, Body: "unread", CloseError: "ignored close failure"}, {URL: page2, Status: 304, Body: "unread", CloseError: "ignored close failure"}}}
	add("304-close-error-ignored-no-body-read", in)
	add("canceled-before-check-no-port-read", updaterReleaseInput{CancelBefore: true})
	add("canceled-cache-read-valid-data", updaterReleaseInput{CacheCancelAt: "read", CacheExists: true, CacheBefore: cache})
	add("canceled-cache-read-malformed-json-decode-wins", updaterReleaseInput{CacheCancelAt: "read", CacheExists: true, CacheBefore: "{"})
	add("canceled-cache-read-port-error-wins", updaterReleaseInput{CacheCancelAt: "read", ReadError: "owned cache read failure"})
	in = query(stable)
	in.NowCancel = true
	add("canceled-by-clock-before-fetch", in)
	for _, boundary := range []string{"request", "read", "close"} {
		in = query(stable)
		in.Replies[0].CancelAt = boundary
		add("canceled-at-response-"+boundary, in)
	}
	in = query("invalid JSON")
	in.Replies[0].CancelAt = "read"
	add("canceled-during-malformed-decode-json-error-wins", in)
	in = query(stable)
	in.CacheCancelAt = "write"
	add("cache-port-cancels-but-successful-write-is-not-postchecked", in)
	in.WriteError = "owned cache write failure"
	add("cache-port-cancel-write-error-is-returned", in)
	in = query(stable)
	in.Automatic = true
	add("automatic-child-deadline-observed-across-read-request-write", in)
	in = query(stable)
	in.ParentDeadline = "1h"
	add("explicit-preserves-parent-deadline", in)
	in = query(stable)
	in.Automatic = true
	in.ParentDeadline = "1h"
	add("automatic-shortens-long-parent-deadline", in)
	in = query(stable)
	in.Automatic = true
	in.ParentDeadline = "1s"
	add("automatic-preserves-short-parent-deadline", in)
	add("automatic-total-three-second-timeout-covers-cache-read", updaterReleaseInput{Automatic: true, CacheWaitContext: true})
	in = query(stable)
	in.Automatic = true
	in.ParentDeadline = "10ms"
	in.Replies[0].WaitContext = true
	add("parent-deadline-covers-http-request", in)
	in = query(stable)
	in.PrivateMirror = true
	in.Replies = []updaterReleaseReply{{URL: "https://mirror.invalid/" + updaterReleaseEndpoint, Status: 200, Body: `[]`}, {URL: "https://mirror.invalid/" + updaterReleaseEndpoint, Status: 200, Body: stable + ` {"ignored":true}`, Headers: http.Header{"Etag": {`"mirror"`}, "Link": {`<?page=2>; rel=next`}}}, {URL: "https://mirror.invalid/" + page2, Status: 200, Body: `[{"tag_name":"v4.0.0"}]`}}
	add("private-single-mirror-probe-strict-query-first-value-canonical-cache", in)
	for name, body := range map[string]string{"null": "null", "multiple-values": "[] []", "trailing-garbage": "[] garbage", "wrong-field-type": `[{"tag_name":42}]`} {
		in = updaterReleaseInput{PrivateMirror: true, Replies: []updaterReleaseReply{{URL: "https://mirror.invalid/" + updaterReleaseEndpoint, Status: 200, Body: body}}}
		add("private-single-probe-reject-"+name, in)
	}
	in = updaterReleaseInput{PrivateMirror: true, Replies: []updaterReleaseReply{{URL: "https://mirror.invalid/" + updaterReleaseEndpoint, Status: 200, Body: `[]`}, {URL: "https://mirror.invalid/" + updaterReleaseEndpoint, Status: 503, Body: "unread"}}}
	add("private-selected-api-source-query-failure-no-fallback", in)
	in = updaterReleaseInput{CacheExists: true, CacheBefore: updaterReleaseCacheJSON(2, "2026-10-08T12:00:00Z", stable, "["+updaterReleasePageJSON("https://api.github.com/repos/FlanChanXwO/./pixiv-cli/releases", `"alias"`, "", stable)+"]"), Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 304, Body: "unread"}}}
	add("304-canonical-path-cache-identity-reuses-and-preserves-cached-url", in)
	in = updaterReleaseInput{Replies: []updaterReleaseReply{{URL: updaterReleaseEndpoint, Status: 200, Body: stable, Headers: http.Header{"Link": {`<?page=2>; rel=next`}}}, {URL: page2, Status: 200, Body: `[]`, Headers: http.Header{"Link": {`<` + updaterReleaseEndpoint + `>; rel=next`}}}}}
	add("pagination-two-page-cycle-before-third-request", in)
	in = query(stable)
	in.Replies[0].Headers = http.Header{"Link": {`<?b=2&a=1>; rel=next`}}
	in.Replies = append(in.Replies, updaterReleaseReply{URL: updaterReleaseEndpoint + "?b=2&a=1", Status: 200, Body: `[]`, Headers: http.Header{"Link": {`<?a=1&b=2>; rel=next`}}}, updaterReleaseReply{URL: updaterReleaseEndpoint + "?a=1&b=2", Status: 200, Body: `[]`})
	add("pagination-raw-query-order-is-distinct-page-identity", in)
	for _, year := range []int{-1, 10000} {
		in = query(stable)
		in.NowDateYear = year
		add(fmt.Sprintf("public-clock-year-%d-cache-marshal-error-before-write", year), in)
	}
	for _, winner := range []string{"gh-proxy", "github-direct"} {
		winnerURL, loserURL := "https://gh-proxy.com/"+updaterReleaseEndpoint, updaterReleaseEndpoint
		if winner == "github-direct" {
			winnerURL, loserURL = loserURL, winnerURL
		}
		in = updaterReleaseInput{PublicSourceWinner: winner, Replies: []updaterReleaseReply{{URL: winnerURL, Status: 200, Body: `[]`}, {URL: loserURL, WaitContext: true}, {URL: winnerURL, Status: 200, Body: stable, Headers: http.Header{"Etag": {`"default-source"`}}}}}
		add("public-default-source-"+winner+"-wins-loser-canceled-canonical-cache", in)
		in.Replies[2].Status = 503
		add("public-default-source-"+winner+"-query-failure-no-fallback", in)
	}
	in = query(`[{"tag_name":"v1.0.0","assets":[]}]`)
	add("empty-asset-array-selected-copy-is-nil-cache-remains-array", in)
	in = query(`[{"tag_name":"v1.0.0","assets":[{"name":17}]}]`)
	add("invalid-asset-field-type-decode-fails-before-selection", in)
	in = query(stable)
	in.CacheBefore = "ignored malformed bytes"
	add("nonexistent-cache-bytes-ignored", in)
	in = updaterReleaseInput{Automatic: true, CacheExists: true, CacheBefore: updaterReleaseCacheJSON(2, updaterReleaseNow, stable, "[]"), NowCancel: true}
	add("throttled-clock-cancellation-is-not-postchecked", in)
	in = query(stable)
	in.Replies[0].CancelAt = "request"
	in.Replies[0].RequestError = "owned request failure"
	add("canceled-request-transport-error-wins", in)
	in = query(stable)
	in.Replies[0].Status = 500
	in.Replies[0].CancelAt = "close"
	add("canceled-status-close-status-error-wins", in)
	add("constructor-cache-required", updaterReleaseInput{NilCache: true})
	for _, base := range []string{"relative", "://bad", "https://%zz.invalid"} {
		add("constructor-invalid-base-"+fmt.Sprintf("%x", base), updaterReleaseInput{APIBaseURL: base})
	}
	for _, repository := range []string{"owner", "/name", "owner/", "owner/name/extra"} {
		add("constructor-invalid-repository-"+fmt.Sprintf("%x", repository), updaterReleaseInput{Repository: repository})
	}
	add("nil-client-check-error", updaterReleaseInput{NilClient: true})
	sort.Slice(cases, func(i, j int) bool { return cases[i].Name < cases[j].Name })
	return cases
}

func updaterReleaseSourceAnchors(t *testing.T, root string) map[string]string {
	t.Helper()
	anchors := map[string]string{
		"go.mod":                           "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum":                           "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/update/release/cache.go": "493d46de83ad0ccae219c19a5e2abbe3a7feb2e68783e64afce912d504ca20f2",
		"internal/update/release/release_client.go":         "9de30775c343d211ea62a61f29cb9c02f33c59f71c42613620e61001f8eac8d0",
		"internal/update/release/version_policy.go":         "988c885c7c078f0e2c3d748869a55586db0fcd3ce3fcce8db377a860fdbc219a",
		"internal/update/source/github.go":                  "1fd8c8abd4e0855298475ce7455ab775bc4eec6c3fda07a59b76fbb8780c617d",
		"internal/update/source/release_source_selector.go": "cbfb9bedcd32b391a52ce6a038710e45320dfbd2edf70c746ad72d22b8ad21f0",
		"internal/update/source/release_sources.go":         "5450d686f836da97e264872e80ba4178bcec7a4217fd8a9eaa85abfccf144bc8",
		"internal/update/source/release_sources.txt":        "e694ddf4ca91dcf649fc599d5d54bf0f67f67340d8fca10ec2314b345f2b4588",
		"internal/update/installer/release_cache.go":        "9d091159d1f32b328f6244f17b2ba23a704dd7cfd487a907172042b7535512be",
	}
	for path, want := range anchors {
		body, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(body)
		got := hex.EncodeToString(sum[:])
		if got != want {
			t.Fatalf("frozen source changed: %s sha256=%s want=%s", path, got, want)
		}
	}
	body, err := os.ReadFile(filepath.Join(root, "internal/update/release/migration_connected_test.go"))
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(body)
	anchors["internal/update/release/migration_connected_test.go"] = hex.EncodeToString(sum[:])
	return anchors
}

func TestMigrationUpdaterReleaseCacheConnected(t *testing.T) {
	root, err := filepath.Abs("../../..")
	if err != nil {
		t.Fatal(err)
	}
	fixture := updaterReleaseFixture{Reference: updaterReleaseReference, Published: updaterReleasePublished, Toolchain: runtime.Version(), Environment: runtime.GOOS + "/" + runtime.GOARCH + "; in-memory canonical HTTP and synthetic cache ports; no live network or files", Sources: updaterReleaseSourceAnchors(t, root), Limitations: []string{
		"Bounded public constructor and Check flows use genuine HTTP request/response bodies and cache storage ports; no checker-result stub is used.",
		"Private single-source selector injection rows separately capture canonical cache and strict probe versus first-value query behavior; default-source rows use witness-controlled scheduling of actual default probe requests and cancellation, not native or public network timing.",
		"Nil receiver row is Go-only. Typed nil ports and arbitrary URL/JSON/SemVer grammars are not generalized from bounded observations.",
		"Cache-port writes observe exact bytes, timing and propagated errors. Physical permission, atomic replacement and native Windows effects belong to the separate installer/cache oracle.",
		"The 3-second automatic deadline is exercised while blocked in cache Read. Other ports record actual inherited deadline identity, without asserting wall-clock scheduler timing.",
		"HTTP transport is in-memory and does not establish DNS/TLS/proxy/redirect behavior or public service availability. No account, media, download, updater execution or native settings are used.",
	}, Cases: updaterReleaseCases()}
	for index := range fixture.Cases {
		row := &fixture.Cases[index]
		t.Run(row.Name, func(t *testing.T) { row.Observation = updaterReleaseObserve(t, row.Input) })
	}
	if t.Failed() {
		return
	}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join(root, "crates/pixiv-cli/tests/fixtures/updater-release-cache.json")
	if *updaterReleaseCapture {
		if err := os.WriteFile(path, body, 0o600); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d connected rows", len(fixture.Cases))
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(body) != string(want) {
		t.Fatalf("connected release/cache fixture differs; explicit capture flag is required to change frozen evidence")
	}
}

var _ io.ReadCloser = (*updaterReleaseBody)(nil)

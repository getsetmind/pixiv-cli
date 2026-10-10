package dic

import (
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
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	requirements "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands"
	dicservice "github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

var migrationUpdateDictionaryOwner = flag.Bool("migration-update-dictionary-owner", false, "capture the frozen anonymous dictionary command owner contract")

const migrationDictionaryOwnerReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

type migrationDictionaryOwnerResponse struct {
	Body   string `json:"body"`
	Status int    `json:"status"`
	Error  string `json:"error"`
}

type migrationDictionaryOwnerInput struct {
	Name           string                             `json:"name"`
	Args           []string                           `json:"args"`
	Input          string                             `json:"input"`
	ConfiguredJSON bool                               `json:"configured_json"`
	JSONError      string                             `json:"json_error"`
	MissingJSONOut bool                               `json:"missing_json_out"`
	ReaderMode     string                             `json:"reader_mode"`
	Writer         string                             `json:"writer"`
	Context        string                             `json:"context"`
	Responses      []migrationDictionaryOwnerResponse `json:"responses"`
}

type migrationDictionaryOwnerRequest struct {
	URL              string `json:"url"`
	Accept           string `json:"accept"`
	ContextInherited bool   `json:"context_inherited"`
	ContextError     string `json:"context_error"`
}

type migrationDictionaryOwnerWrite struct {
	Input            string `json:"input"`
	Count            int    `json:"count"`
	Error            string `json:"error"`
	RequestsComplete int    `json:"requests_complete"`
}

type migrationDictionaryOwnerResult struct {
	Error         string                            `json:"error"`
	Selected      string                            `json:"selected"`
	JSONChanged   bool                              `json:"json_changed"`
	JSONFlag      bool                              `json:"json_flag"`
	NDJSONFlag    bool                              `json:"ndjson_flag"`
	ErrorPipe     bool                              `json:"error_broken_pipe"`
	Usage         bool                              `json:"usage"`
	DicCode       string                            `json:"dic_code"`
	HTTPStatus    int                               `json:"http_status"`
	SDKReason     string                            `json:"sdk_reason"`
	ErrorCanceled bool                              `json:"error_canceled"`
	ErrorDeadline bool                              `json:"error_deadline"`
	ErrorCause    bool                              `json:"error_cause"`
	Stdout        string                            `json:"stdout"`
	HelpOutput    string                            `json:"help_output"`
	Diagnostics   string                            `json:"diagnostics"`
	JSONOverrides []*bool                           `json:"json_overrides"`
	Requests      []migrationDictionaryOwnerRequest `json:"requests"`
	Events        []string                          `json:"events"`
	Writes        []migrationDictionaryOwnerWrite   `json:"writes"`
	InputReads    int                               `json:"input_reads"`
}

type migrationDictionaryOwnerCase struct {
	Input  migrationDictionaryOwnerInput  `json:"input"`
	Result migrationDictionaryOwnerResult `json:"result"`
}

type migrationDictionaryOwnerFlag struct {
	Name      string `json:"name"`
	Shorthand string `json:"shorthand"`
	Type      string `json:"type"`
	Default   string `json:"default"`
	NoOpt     string `json:"no_opt"`
	Usage     string `json:"usage"`
}

type migrationDictionaryOwnerCommand struct {
	Use          string                         `json:"use"`
	Short        string                         `json:"short"`
	Aliases      []string                       `json:"aliases"`
	Flags        []migrationDictionaryOwnerFlag `json:"flags"`
	Inherited    []migrationDictionaryOwnerFlag `json:"inherited"`
	Requirements requirements.Execution         `json:"requirements"`
}

type migrationDictionaryOwnerUsage struct{ error }

func (err migrationDictionaryOwnerUsage) Unwrap() error { return err.error }

type migrationDictionaryOwnerTransport func(context.Context, string, string) ([]byte, int, error)

func (transport migrationDictionaryOwnerTransport) Get(ctx context.Context, rawURL, accept string) ([]byte, int, error) {
	return transport(ctx, rawURL, accept)
}

type migrationDictionaryOwnerInputReader struct{ reads *int }

func (input migrationDictionaryOwnerInputReader) Read([]byte) (int, error) {
	*input.reads++
	panic("dictionary owner must not read stdin")
}

type migrationDictionaryOwnerInjectedReader struct{ err error }

func (reader migrationDictionaryOwnerInjectedReader) Article(context.Context, dicservice.ArticleRequest) (dicservice.Article, error) {
	return dicservice.Article{}, reader.err
}

func (reader migrationDictionaryOwnerInjectedReader) Search(context.Context, dicservice.SearchRequest) ([]dicservice.SearchResult, error) {
	return nil, reader.err
}

type migrationDictionaryOwnerWriter struct {
	mode   string
	result *migrationDictionaryOwnerResult
	output bytes.Buffer
}

func (writer *migrationDictionaryOwnerWriter) Write(input []byte) (int, error) {
	count := len(input)
	var err error
	switch writer.mode {
	case "short-nil":
		count /= 2
	case "zero-nil":
		count = 0
	case "fail-first":
		count, err = 0, errors.New("synthetic dictionary writer failure")
	case "partial-error":
		count, err = min(5, count), errors.New("synthetic dictionary writer failure")
	case "fail-fourth":
		if len(writer.result.Writes) == 3 {
			count, err = 0, errors.New("synthetic dictionary writer failure")
		}
	case "broken-pipe":
		count, err = 0, syscall.EPIPE
	}
	write := migrationDictionaryOwnerWrite{Input: string(input), Count: count, RequestsComplete: len(writer.result.Requests)}
	if err != nil {
		write.Error = err.Error()
	}
	writer.result.Writes = append(writer.result.Writes, write)
	writer.result.Events = append(writer.result.Events, "write")
	_, _ = writer.output.Write(input[:count])
	return count, err
}

func migrationDictionaryOwnerSources(t *testing.T, root string) map[string]string {
	t.Helper()
	paths := []string{
		"internal/cli/commands/pixiv/dic/dic.go", "internal/cli/commands/pixiv/dic/article.go", "internal/cli/commands/pixiv/dic/search.go",
		"internal/cli/commands/lifecycle.go", "internal/services/dic/dic.go", "internal/services/dic/article.go",
		"internal/services/dic/search.go", "internal/services/dic/errors.go", "sdk/error.go",
	}
	identities := map[string]string{}
	for _, path := range paths {
		working, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		command := exec.Command("git", "show", migrationDictionaryOwnerReference+":"+path)
		command.Dir = root
		frozen, err := command.Output()
		if err != nil {
			t.Fatalf("read frozen %s: %v", path, err)
		}
		if !bytes.Equal(working, frozen) {
			t.Fatalf("%s differs from frozen Go reference", path)
		}
		digest := sha256.Sum256(frozen)
		identities[path] = hex.EncodeToString(digest[:])
	}
	if identities[paths[0]] != "291a50c64e94cfce5251a98bead928b3d802712521748e0061870f9f4a3c67db" {
		t.Fatal("frozen dictionary owner identity changed")
	}
	return identities
}

func migrationDictionaryOwnerArticleBody() string {
	nodes := `[null,{"tag":"text","text":"  lead & < >\u2028\u2029\r\n next  ","children":[{"tag":"text","text":"ignored"}]},{"tag":"br"},{"tag":"article_link","text":"article link"},{"tag":"external_link","text":"external link"},{"tag":"header","children":[{"tag":"text","text":"Heading"}]},{"tag":"sub_header","children":[{"tag":"text","text":"Subheading"}]},{"tag":"p","children":[{"tag":"text","text":"paragraph\n\n\n final"}]},{"tag":"list_item","children":[{"tag":"text","text":"item"}]},{"tag":"table_row","children":[{"tag":"table_header","children":[{"tag":"text","text":"Header"}]},{"tag":"table_cell","children":[{"tag":"text","text":"Cell"}]}]},{"tag":"unknown","children":[{"tag":"text","text":"tail"}]}]`
	body, err := json.Marshal(map[string]any{
		"id": 7, "tagName": "Response / <>&\u2028\u2029", "yomigana": "よみ\t\x1b[31m", "translatedTagName": "Translation\nline",
		"categories": []string{"one", "two <>&"}, "abstract": "abstract\n\t\x1b[0m & < >\u2028\u2029", "nodes": nodes,
		"recommendedArticles": []map[string]string{{"tagName": " related "}, {"tagName": "\u3000"}, {"tagName": " related "}, {"tagName": "raw\nline"}},
	})
	if err != nil {
		panic(err)
	}
	return string(body)
}

func migrationDictionaryOwnerSearchBody() string {
	return `<html><body><div id="main">
<article><img src="//images.invalid/a?x=1&amp;y=2"><div class="x info y"><a href="/a/first?x=1&amp;y=2"> First &lt;&gt;&amp; &#x2028;&#x2029; <span>title</span> </a><p class="summary"> Before <b>bold</b><a href="/more">more</a>ignored</p><ul class="data"><li>更新: 2026:10</li><li>閲覧数: 1,234</li><li>作品数: -56件</li><li>チェックリスト数: ７ 89</li></ul><div class="relation"><ul><li> related one </li><li> </li><li>related two</li></ul></div></div></article>
<article><div class="info"><a href="/a/duplicate">Duplicate</a></div></article>
<article><div class="info"><a href="/a/duplicate">Duplicate</a></div></article>
<article><div class="unrelated"><a href="/a/skip">skip missing info</a></div></article>
<article><div class="info"><a href="/a/skip"> </a></div></article>
</div></body></html>`
}

func migrationDictionaryOwnerInputs() []migrationDictionaryOwnerInput {
	article := migrationDictionaryOwnerArticleBody()
	counters := `{"articleViewCount":1234,"pixivWorkCount":-6,"commentCount":0,"checklistCount":9}`
	search := migrationDictionaryOwnerSearchBody()
	newInput := func(name string, args ...string) migrationDictionaryOwnerInput {
		responses := []migrationDictionaryOwnerResponse{}
		if len(args) > 0 && args[0] == "article" {
			responses = append(responses, migrationDictionaryOwnerResponse{Body: article, Status: 200}, migrationDictionaryOwnerResponse{Body: counters, Status: 200})
		} else if len(args) > 0 && args[0] == "search" {
			responses = append(responses, migrationDictionaryOwnerResponse{Body: search, Status: 200})
		}
		return migrationDictionaryOwnerInput{Name: name, Args: append([]string{}, args...), Input: "synthetic stdin must stay unread\n", ReaderMode: "real-client", Writer: "normal", Context: "active", Responses: responses}
	}
	inputs := []migrationDictionaryOwnerInput{}
	add := func(name string, args ...string) *migrationDictionaryOwnerInput {
		inputs = append(inputs, newInput(name, args...))
		return &inputs[len(inputs)-1]
	}
	add("group-bare")
	add("group-help", "--help")
	add("group-extra", "extra")
	add("group-unknown-flag", "--unknown")
	add("group-json-flag-rejected", "--json")
	for _, current := range []struct {
		name string
		args []string
	}{
		{"discovery-json-no-value-before-article", []string{"--json", "article", "query"}},
		{"discovery-json-equals-before-article", []string{"--json=true", "article", "query"}},
		{"discovery-language-before-article", []string{"--lang", "en", "article", "query"}},
		{"discovery-separator-before-article", []string{"--", "article", "query"}},
		{"discovery-page-before-search", []string{"--page", "2", "search", "query"}},
		{"discovery-page-equals-before-search", []string{"--page=2", "search", "query"}},
		{"discovery-no-counters-before-article", []string{"--no-counters", "article", "query"}},
		{"article-json-invalid-after-valid", []string{"article", "query", "--json=true", "--json=bad"}},
		{"search-json-invalid-after-valid", []string{"search", "query", "--json=true", "--json=bad"}},
		{"search-ndjson-invalid-after-valid", []string{"search", "query", "--ndjson=true", "--ndjson=bad"}},
		{"search-json-invalid-after-ndjson", []string{"search", "query", "--ndjson", "--json=true", "--json=bad"}},
	} {
		row := add(current.name, current.args...)
		if len(row.Responses) == 0 {
			for _, arg := range current.args {
				if arg == "article" {
					row.Responses = []migrationDictionaryOwnerResponse{{Body: article, Status: 200}, {Body: counters, Status: 200}}
					break
				}
				if arg == "search" {
					row.Responses = []migrationDictionaryOwnerResponse{{Body: search, Status: 200}}
					break
				}
			}
		}
	}
	for _, leaf := range []string{"article", "search"} {
		add(leaf+"-human", leaf, "Request / title")
		add(leaf+"-json", leaf, "Request / title", "--json")
		add(leaf+"-short-json", leaf, "Request / title", "-j")
		add(leaf+"-configured-json", leaf, "Request / title").ConfiguredJSON = true
		add(leaf+"-configured-json-explicit-false", leaf, "Request / title", "--json=false").ConfiguredJSON = true
		add(leaf+"-json-repeat-last-false", leaf, "Request / title", "--json", "--json=false")
		add(leaf+"-json-repeat-last-true", leaf, "Request / title", "--json=false", "-j")
		add(leaf+"-missing", leaf)
		add(leaf+"-missing-piped-input", leaf).Input = "{\"title\":\"from stdin\"}\n"
		add(leaf+"-extra", leaf, "one", "two")
		add(leaf+"-unknown-long", leaf, "query", "--unknown")
		add(leaf+"-unknown-short", leaf, "query", "-x")
		add(leaf+"-proxy-rejected", leaf, "query", "--proxy=http://synthetic.invalid")
		add(leaf+"-help", leaf, "--help")
		add(leaf+"-short-help", leaf, "-h")
		add(leaf+"-invalid-json", leaf, "query", "--json=invalid")
		add(leaf+"-json-space-is-positional", leaf, "query", "--json", "false")
		add(leaf+"-json-resolver-port-failure", leaf, "Request / title", "--json").JSONError = "synthetic JSON resolver failure"
		add(leaf+"-injected-reader-port-failure", leaf, "query", "--json").ReaderMode = "injected-reader-error"
		add(leaf+"-missing-reader-port", leaf, "query").ReaderMode = "missing-reader"
		add(leaf+"-nil-transport-real-client", leaf, "query").ReaderMode = "nil-transport"
		add(leaf+"-missing-json-default-human", leaf, "query").MissingJSONOut = true
		add(leaf+"-missing-json-explicit-true", leaf, "query", "--json").MissingJSONOut = true
		add(leaf+"-missing-json-explicit-false", leaf, "query", "--json=false").MissingJSONOut = true
	}
	for _, value := range []string{"1", "0", "t", "T", "TRUE", "True", "f", "F", "FALSE", "False"} {
		add("article-json-bool-"+value, "article", "query", "--json="+value, "--no-counters")
	}
	for _, value := range []string{"", "fr", " JA", "en ", "EN"} {
		add("article-language-invalid-"+fmt.Sprintf("%q", value), "article", "query", "--lang="+value)
	}
	add("article-language-en", "article", "https://dic.pixiv.net/a/Request%252Ftitle", "--lang=en", "--json")
	add("article-url-language-does-not-override-ja", "article", "https://dic.pixiv.net/en/a/Request", "--json")
	add("article-language-repeated-last-en", "article", "query", "--lang=fr", "--lang=en", "--json")
	add("article-language-invalid-before-reader", "article", "query", "--lang=fr").ReaderMode = "missing-reader"
	add("article-missing-language-value", "article", "query", "--lang")
	add("article-no-counters-json", "article", "query", "--no-counters", "--json")
	add("article-no-counters-human", "article", "query", "--no-counters")
	add("article-no-counters-false", "article", "query", "--no-counters=false", "--json")
	add("article-no-counters-repeat-last-false", "article", "query", "--no-counters", "--no-counters=false", "--json")
	add("article-no-counters-bool-one", "article", "query", "--no-counters=1", "--json")
	add("article-no-counters-invalid", "article", "query", "--no-counters=invalid")
	add("article-ndjson-rejected", "article", "query", "--ndjson")
	add("article-page-rejected", "article", "query", "--page=2")
	add("article-limit-rejected", "article", "query", "-n1")
	add("article-empty-reference", "article", " \t\u3000", "--json").JSONError = "synthetic JSON resolver must not run"
	add("search-empty-query", "search", " \t\u3000", "--json").JSONError = "synthetic JSON resolver must not run"
	add("search-ndjson", "search", " query ", "--ndjson")
	add("search-ndjson-configured-json", "search", "query", "--ndjson").ConfiguredJSON = true
	add("search-ndjson-skips-json-resolver-port", "search", "query", "--ndjson").JSONError = "synthetic JSON resolver must not run"
	add("search-ndjson-false", "search", "query", "--ndjson=false", "--json")
	add("search-ndjson-repeat-last-false", "search", "query", "--ndjson", "--ndjson=false", "--json")
	add("search-ndjson-repeat-last-true", "search", "query", "--ndjson=false", "--ndjson", "--json=false")
	add("search-ndjson-json-conflict", "search", "query", "--ndjson", "--json")
	add("search-ndjson-json-false-conflict", "search", "query", "--ndjson", "--json=false")
	add("search-ndjson-short-json-conflict", "search", "query", "--ndjson", "-j=false")
	add("search-ndjson-invalid", "search", "query", "--ndjson=invalid")
	add("search-lang-rejected", "search", "query", "--lang=en")
	add("search-no-counters-rejected", "search", "query", "--no-counters")
	add("search-page-before-limit-before-conflict", "search", "query", "--page=0", "--limit=-1", "--ndjson", "--json")
	add("search-limit-before-conflict", "search", "query", "--limit=-1", "--ndjson", "--json")
	add("search-page-before-reader", "search", "query", "--page=0").ReaderMode = "missing-reader"
	add("search-missing-page-value", "search", "query", "--page")
	add("search-missing-limit-value", "search", "query", "--limit")
	for _, value := range []string{"0", "-1", "+2", "02", "010", "0x10", "0Xf", "0b10", "0o10", "1_0", "08", "0b2", "9223372036854775807", "9223372036854775808", " 2"} {
		add("search-page-"+fmt.Sprintf("%q", value), "search", "query", "--page="+value, "--limit=1", "--json")
	}
	for _, value := range []string{"0", "1", "2", "99", "-1", "0x2", "010", "08"} {
		add("search-limit-"+value, "search", "query", "--limit="+value, "--json")
	}
	add("search-short-limit-attached", "search", "query", "-n2", "--ndjson")
	add("search-short-limit-separated", "search", "query", "-n", "2", "--ndjson")
	add("search-page-repeated", "search", "query", "--page=0", "--page=2", "--json")
	add("search-limit-repeated", "search", "query", "--limit=-1", "--limit=2", "--json")
	add("search-flags-interspersed", "search", "--page=2", "query", "--limit=2", "--json")
	add("search-separator-literal-query", "search", "--", "--json")
	for _, optional := range []string{"missing", "null", "empty"} {
		body := `{"id":9,"tagName":"minimal"}`
		if optional == "null" {
			body = `{"id":9,"tagName":"minimal","categories":null,"recommendedArticles":null,"nodes":"null"}`
		} else if optional == "empty" {
			body = `{"id":9,"tagName":"minimal","categories":[],"recommendedArticles":[],"nodes":"[]"}`
		}
		for _, mode := range []string{"human", "json"} {
			row := add("article-optional-"+optional+"-"+mode, "article", "query", "--no-counters")
			if mode == "json" {
				row.Args = append(row.Args, "--json")
			}
			row.Responses[0].Body = body
		}
	}
	for index, body := range []string{`{"id":9,"tagName":"minimal","nodes":"{"}`, `{"id":9,"tagName":"minimal","nodes":" "}`} {
		row := add("article-malformed-node-body-"+fmt.Sprint(index), "article", "query", "--json", "--no-counters")
		row.Responses[0].Body = body
	}
	for _, mode := range []string{"human", "json", "ndjson"} {
		for _, status := range []int{200, 404} {
			row := add(fmt.Sprintf("search-empty-%d-%s", status, mode), "search", "query")
			if mode != "human" {
				row.Args = append(row.Args, "--"+mode)
			}
			row.Responses[0] = migrationDictionaryOwnerResponse{Body: `<div id="main"></div>`, Status: status}
			if status == 404 {
				row.Responses[0].Body = "arbitrary malformed body\x00"
			}
		}
	}
	for _, leaf := range []string{"article", "search"} {
		for _, status := range []int{404, 429, 500} {
			row := add(fmt.Sprintf("%s-upstream-%d", leaf, status), leaf, "query", "--json")
			row.Responses[0].Status = status
			row.JSONError = "synthetic JSON resolver must not run on error"
		}
		for _, cause := range []string{"synthetic-cause", "canceled", "deadline"} {
			row := add(leaf+"-transport-"+cause, leaf, "query", "--json")
			row.Responses[0].Error = cause
			if cause != "synthetic-cause" {
				row.Context = cause
			}
		}
	}
	for _, body := range []string{"invalid upstream JSON", `{}`, `{"id":0,"tagName":"bad"}`, `{"id":7,"tagName":" \u3000"}`} {
		row := add("article-malformed-response-"+fmt.Sprint(len(body)), "article", "query", "--json")
		row.Responses[0].Body = body
	}
	add("search-missing-main", "search", "query", "--json").Responses[0].Body = `<article><div class="info"><a href="/a/ignored">ignored</a></div></article>`
	for _, failure := range []string{"status", "decode", "synthetic-cause", "canceled", "deadline"} {
		row := add("article-counter-retention-"+failure, "article", "query", "--json")
		switch failure {
		case "status":
			row.Responses[1].Status = 500
		case "decode":
			row.Responses[1].Body = "invalid counter JSON"
		default:
			row.Responses[1].Error = failure
		}
	}
	for _, leaf := range []string{"article", "search"} {
		modes := []string{"human", "json"}
		if leaf == "search" {
			modes = append(modes, "ndjson")
		}
		for _, mode := range modes {
			for _, writer := range []string{"short-nil", "zero-nil", "fail-first", "partial-error", "fail-fourth", "broken-pipe"} {
				row := add("writer-"+leaf+"-"+mode+"-"+writer, leaf, "query")
				if mode != "human" {
					row.Args = append(row.Args, "--"+mode)
				}
				row.Writer = writer
			}
		}
	}
	return inputs
}

func migrationDictionaryOwnerExecute(t *testing.T, input migrationDictionaryOwnerInput) migrationDictionaryOwnerResult {
	t.Helper()
	result := migrationDictionaryOwnerResult{JSONOverrides: []*bool{}, Requests: []migrationDictionaryOwnerRequest{}, Events: []string{}, Writes: []migrationDictionaryOwnerWrite{}}
	writer := &migrationDictionaryOwnerWriter{mode: input.Writer, result: &result}
	var help, diagnostics bytes.Buffer
	type contextKey struct{}
	ctx := context.WithValue(context.Background(), contextKey{}, "synthetic dictionary context")
	if input.Context == "canceled" {
		canceled, cancel := context.WithCancel(ctx)
		cancel()
		ctx = canceled
	} else if input.Context == "deadline" {
		deadline, cancel := context.WithDeadline(ctx, time.Unix(1, 0))
		defer cancel()
		ctx = deadline
	}
	syntheticCause := errors.New("synthetic dictionary transport cause")
	transport := migrationDictionaryOwnerTransport(func(requestContext context.Context, rawURL, accept string) ([]byte, int, error) {
		index := len(result.Requests)
		if index >= len(input.Responses) {
			t.Fatalf("%s: unexpected anonymous request: %s", input.Name, rawURL)
		}
		if !strings.HasPrefix(rawURL, "https://dic.pixiv.net/") {
			t.Fatalf("%s: request escaped dictionary origin: %s", input.Name, rawURL)
		}
		request := migrationDictionaryOwnerRequest{URL: rawURL, Accept: accept, ContextInherited: requestContext.Value(contextKey{}) == "synthetic dictionary context"}
		if requestContext.Err() != nil {
			request.ContextError = requestContext.Err().Error()
		}
		result.Requests = append(result.Requests, request)
		result.Events = append(result.Events, "get")
		response := input.Responses[index]
		var err error
		switch response.Error {
		case "synthetic-cause":
			err = syntheticCause
		case "canceled":
			err = context.Canceled
		case "deadline":
			err = context.DeadlineExceeded
		}
		return []byte(response.Body), response.Status, err
	})
	var reader Reader = dicservice.New(transport)
	switch input.ReaderMode {
	case "missing-reader":
		reader = nil
	case "nil-transport":
		reader = dicservice.New(nil)
	case "injected-reader-error":
		reader = migrationDictionaryOwnerInjectedReader{err: errors.New("synthetic dictionary Reader port failure")}
	}
	stdin := migrationDictionaryOwnerInputReader{reads: &result.InputReads}
	dependencies := Dependencies{
		Input: stdin, Output: writer, Reader: reader,
		UsageError: func(err error) error { return migrationDictionaryOwnerUsage{err} },
		JSONOut: func(override *bool) (bool, error) {
			result.Events = append(result.Events, "json-resolve")
			if override == nil {
				result.JSONOverrides = append(result.JSONOverrides, nil)
			} else {
				value := *override
				result.JSONOverrides = append(result.JSONOverrides, &value)
			}
			if input.JSONError != "" {
				return false, errors.New(input.JSONError)
			}
			if override != nil {
				return *override, nil
			}
			return input.ConfiguredJSON, nil
		},
	}
	if input.MissingJSONOut {
		dependencies.JSONOut = nil
	}
	command := New(dependencies)
	command.SilenceErrors, command.SilenceUsage = true, true
	command.SetIn(stdin)
	command.SetOut(&help)
	command.SetErr(&diagnostics)
	command.SetContext(ctx)
	command.SetArgs(input.Args)
	selected, err := command.ExecuteC()
	if selected != nil {
		result.Selected = selected.CommandPath()
		if selected.Flags().Lookup("json") != nil {
			result.JSONChanged = selected.Flags().Changed("json")
			result.JSONFlag, _ = selected.Flags().GetBool("json")
		}
		if selected.Flags().Lookup("ndjson") != nil {
			result.NDJSONFlag, _ = selected.Flags().GetBool("ndjson")
		}
	}
	if err != nil {
		result.Error = err.Error()
		var usage migrationDictionaryOwnerUsage
		result.Usage = errors.As(err, &usage)
		var classified *dicservice.Error
		if errors.As(err, &classified) {
			result.HTTPStatus = classified.StatusCode()
		}
		result.ErrorCanceled = errors.Is(err, context.Canceled)
		result.ErrorDeadline = errors.Is(err, context.DeadlineExceeded)
		result.ErrorCause = errors.Is(err, syntheticCause)
		result.ErrorPipe = errors.Is(err, syscall.EPIPE)
	}
	result.DicCode = string(dicservice.CodeOf(err))
	result.SDKReason = string(sdk.ReasonOf(err))
	result.Stdout, result.HelpOutput, result.Diagnostics = writer.output.String(), help.String(), diagnostics.String()
	if result.InputReads != 0 {
		t.Fatal("dictionary command read stdin")
	}
	return result
}

func migrationDictionaryOwnerCommands() []migrationDictionaryOwnerCommand {
	command := New(Dependencies{})
	commands := []*cobra.Command{command}
	commands = append(commands, command.Commands()...)
	rows := []migrationDictionaryOwnerCommand{}
	flags := func(set *pflag.FlagSet) []migrationDictionaryOwnerFlag {
		rows := []migrationDictionaryOwnerFlag{}
		set.VisitAll(func(flag *pflag.Flag) {
			rows = append(rows, migrationDictionaryOwnerFlag{Name: flag.Name, Shorthand: flag.Shorthand, Type: flag.Value.Type(), Default: flag.DefValue, NoOpt: flag.NoOptDefVal, Usage: flag.Usage})
		})
		return rows
	}
	for _, current := range commands {
		current.InitDefaultHelpFlag()
		rows = append(rows, migrationDictionaryOwnerCommand{Use: current.Use, Short: current.Short, Aliases: append([]string{}, current.Aliases...), Flags: flags(current.LocalFlags()), Inherited: flags(current.InheritedFlags()), Requirements: requirements.For(current)})
	}
	return rows
}

func TestMigrationDictionaryOwner(t *testing.T) {
	root := filepath.Join("..", "..", "..", "..", "..")
	fixture := struct {
		Reference string                            `json:"reference"`
		Sources   map[string]string                 `json:"sources"`
		Gaps      []string                          `json:"gaps"`
		Commands  []migrationDictionaryOwnerCommand `json:"commands"`
		Rows      []migrationDictionaryOwnerCase    `json:"rows"`
	}{
		Reference: migrationDictionaryOwnerReference,
		Sources:   migrationDictionaryOwnerSources(t, root),
		Gaps: []string{
			"Owner-only execution does not prove root startup, configuration persistence, automatic update, exit codes or error envelopes.",
			"The real anonymous dic.Client consumes finite synthetic dic.Transport responses; no live or native HTTP, proxy, TLS, redirect or lifecycle observation is claimed.",
			"Reader and JSONOut port failures are explicitly injected dependency failures and are not upstream responses.",
			"Finite node, HTML, URL, scalar and writer samples are not the complete grammar or every native-platform I/O behavior.",
		},
		Commands: migrationDictionaryOwnerCommands(), Rows: []migrationDictionaryOwnerCase{},
	}
	seen := map[string]bool{}
	for _, input := range migrationDictionaryOwnerInputs() {
		if seen[input.Name] {
			t.Fatalf("duplicate dictionary case: %s", input.Name)
		}
		seen[input.Name] = true
		fixture.Rows = append(fixture.Rows, migrationDictionaryOwnerCase{Input: input, Result: migrationDictionaryOwnerExecute(t, input)})
	}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "cli-dictionary-owner.json")
	if *migrationUpdateDictionaryOwner {
		if err := os.WriteFile(path, data, 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("dictionary owner contract differs from frozen Go capture")
	}
}

var _ io.Writer = (*migrationDictionaryOwnerWriter)(nil)

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
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxContentReads = flag.Bool("migration-capture-fanbox-content-reads", false, "capture frozen Go public FANBOX content reads")

type migrationContentCall struct {
	Operation   string `json:"operation"`
	CreatorID   string `json:"creator_id,omitempty"`
	PostID      string `json:"post_id,omitempty"`
	Tag         string `json:"tag,omitempty"`
	Kind        string `json:"kind,omitempty"`
	RawURL      string `json:"raw_url,omitempty"`
	Cursor      string `json:"cursor,omitempty"`
	Continue    bool   `json:"continue,omitempty"`
	Mutation    string `json:"mutation,omitempty"`
	FreshClient bool   `json:"fresh_client,omitempty"`
	Context     string `json:"context,omitempty"`
}
type migrationContentInput struct {
	SolverSteps []migrationFanboxStep  `json:"solver_steps,omitempty"`
	SolverProxy string                 `json:"solver_proxy,omitempty"`
	Calls       []migrationContentCall `json:"calls"`
	Steps       []migrationFanboxStep  `json:"steps"`
}
type migrationContentRow struct {
	Name        string                `json:"name"`
	Input       migrationContentInput `json:"input"`
	Observation map[string]any        `json:"observation"`
}

func migrationContentResource(r sdk.Resource) map[string]any {
	payload := ""
	if !r.Ref.IsZero() {
		raw, err := sdk.ResourceRefPayload(r.Ref)
		if err != nil {
			panic(err)
		}
		payload = string(raw)
	}
	return map[string]any{"ref": r.Ref.String(), "payload": payload, "url": r.URL, "request_headers": r.RequestHeaders, "requires_credentials": r.RequiresCredentials, "expires_at": r.ExpiresAt, "dto": sdk.ToResourceDTO(r)}
}
func migrationContentPost(p fanbox.Post) map[string]any {
	result := map[string]any{"dto": fanbox.ToPostDTO(p), "runtime_cover": migrationContentResource(p.Cover.Resource), "runtime_body_nil": p.Body == nil}
	if p.Body != nil {
		assets := []map[string]any{}
		blocks := []map[string]any{}
		for _, a := range p.Body.Assets {
			assets = append(assets, map[string]any{"resource": migrationContentResource(a.Resource), "thumbnail": migrationContentResource(a.Thumbnail.Resource)})
		}
		for _, b := range p.Body.Blocks {
			v := map[string]any{}
			if b.Image != nil {
				v["image"] = migrationContentResource(b.Image.Resource)
			}
			if b.File != nil {
				v["file"] = migrationContentResource(b.File.Resource)
			}
			blocks = append(blocks, v)
		}
		result["runtime_assets"] = assets
		result["runtime_blocks"] = blocks
		result["runtime_assets_nil"] = p.Body.Assets == nil
		result["runtime_blocks_nil"] = p.Body.Blocks == nil
	}
	return result
}
func migrationContentCursor(c sdk.Cursor) map[string]any {
	if c.IsZero() {
		return map[string]any{"text": "", "envelope": nil, "payload": "", "identity": "", "identity_present": false}
	}
	raw, err := base64.RawURLEncoding.DecodeString(c.String())
	if err != nil {
		panic(err)
	}
	var env map[string]any
	if err := json.Unmarshal(raw, &env); err != nil {
		panic(err)
	}
	payload, err := sdk.CursorPayload(c)
	if err != nil {
		panic(err)
	}
	id, present := sdk.CursorIdentity(c)
	return map[string]any{"text": c.String(), "envelope": env, "payload": string(payload), "identity": id, "identity_present": present}
}
func migrationContentMutate(t *testing.T, c sdk.Cursor, mutation string) (sdk.Cursor, error) {
	t.Helper()
	if mutation == "" {
		return c, nil
	}
	raw, err := base64.RawURLEncoding.DecodeString(c.String())
	if err != nil {
		t.Fatal(err)
	}
	var env map[string]any
	if err := json.Unmarshal(raw, &env); err != nil {
		t.Fatal(err)
	}
	switch mutation {
	case "product":
		env["p"] = "pixiv"
	case "operation":
		env["o"] = "Other"
	case "binding":
		env["b"] = 2
	case "format":
		env["v"] = 2
	case "query":
		env["q"] = "wrong-query"
	case "identity":
		env["id"] = "99"
	case "no_identity":
		delete(env, "id")
	case "empty_identity":
		env["id"] = ""
	case "bad_payload":
		env["pl"] = base64.StdEncoding.EncodeToString([]byte(`{`))
	case "empty_url":
		env["pl"] = base64.StdEncoding.EncodeToString([]byte(`{"u":""}`))
	case "null_payload":
		env["pl"] = base64.StdEncoding.EncodeToString([]byte(`null`))
	case "wrong_url_type":
		env["pl"] = base64.StdEncoding.EncodeToString([]byte(`{"u":1}`))
	case "relative_url":
		env["pl"] = base64.StdEncoding.EncodeToString([]byte(`{"u":"/next"}`))
	case "unsafe_url":
		env["pl"] = base64.StdEncoding.EncodeToString([]byte(`{"u":"https://evil.example/next"}`))
	case "empty_payload":
		env["pl"] = ""
	case "unknown_fields":
		env["unknown"] = "ignored"
		env["e"] = true
		env["i"] = "non-secret-instance"
	default:
		t.Fatalf("unknown cursor mutation %s", mutation)
	}
	raw, err = json.Marshal(env)
	if err != nil {
		t.Fatal(err)
	}
	return sdk.ParseCursor(base64.RawURLEncoding.EncodeToString(raw))
}
func migrationContentDTO() map[string]any {
	stamp := time.Date(2026, 1, 2, 3, 4, 5, 6000000, time.UTC)
	ref, err := sdk.NewResourceRef("fanbox", []byte(`{"k":"post_image","c":"dto-creator","p":"dto-post","a":"dto-asset"}`))
	if err != nil {
		panic(err)
	}
	resource := sdk.Resource{Ref: ref, URL: "https://downloads.fanbox.cc/dto-private", RequestHeaders: map[string]string{"Referer": "https://www.fanbox.cc/"}, ExpiresAt: &stamp, RequiresCredentials: true}
	image := fanbox.ImageResource{Resource: resource, Variant: "original", Width: 17, Height: 23}
	block := fanbox.PostBlock{Kind: fanbox.PostBlockUnknown, Image: &fanbox.PostImageBlock{Resource: resource, Caption: "image caption"}, File: &fanbox.PostFileBlock{Resource: resource, Name: "file name", Caption: "file caption"}, Article: &fanbox.PostArticleBlock{Text: "article"}, Video: &fanbox.PostVideoEmbed{Provider: "provider", ContentID: "content", CanonicalURL: "https://example.invalid/video", Title: "title", ThumbnailURL: "https://example.invalid/thumb", VideoID: "video", EmbeddedData: map[string]string{"key": "original"}}, Unknown: &fanbox.PostUnknownBlock{RawType: "raw", Payload: map[string]string{"key": "original"}}}
	body := fanbox.PostBody{Text: "body", Blocks: []fanbox.PostBlock{block}, Assets: []fanbox.Asset{{ID: "asset", Kind: fanbox.AssetKindImage, Name: "name", Resource: resource, Thumbnail: image}}}
	dto := fanbox.ToPostBodyDTO(body)
	body.Blocks[0].Video.EmbeddedData["key"] = "mutated"
	body.Blocks[0].Unknown.Payload["key"] = "mutated"
	body.Assets[0].Name = "mutated"
	creator := fanbox.Creator{CreatorSummary: fanbox.CreatorSummary{ID: "creator", Name: "name", Icon: image}, HasAdultContent: true, IsFollowing: true, Cover: image, PlanFee: 500, HasSupportingPlan: true}
	return map[string]any{"body": dto, "empty_body": fanbox.ToPostBodyDTO(fanbox.PostBody{}), "nil_video_map": fanbox.ToPostVideoEmbedDTO(fanbox.PostVideoEmbed{}), "nil_unknown_map": fanbox.ToPostUnknownBlockDTO(fanbox.PostUnknownBlock{}), "creator": fanbox.ToCreatorDTO(creator), "summary": fanbox.ToCreatorSummaryDTO(creator.CreatorSummary), "file_resource": fanbox.ToFileResourceDTO(fanbox.FileResource{Resource: resource, Name: "name"}), "nil_variants": fanbox.ToPostBlockDTO(fanbox.PostBlock{Kind: fanbox.PostBlockUnknown}), "tag": fanbox.ToCreatorTagDTO(fanbox.CreatorTag{Name: "tag", URL: "https://example.invalid/tag"}), "maps_independent": dto.Blocks[0].Video.EmbeddedData["key"] == "original" && dto.Blocks[0].Unknown.Payload["key"] == "original", "assets_independent": dto.Assets[0].Name == "name"}
}

type migrationContentRecordingTransport struct {
	owned  *migrationFanboxTransport
	record func(string)
}

func (r *migrationContentRecordingTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	r.record("api_or_identity:" + req.URL.String())
	return r.owned.RoundTrip(req)
}
func (r *migrationContentRecordingTransport) CloseIdleConnections() { r.owned.CloseIdleConnections() }
func migrationContentObserve(t *testing.T, input migrationContentInput) map[string]any {
	t.Helper()
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationFanboxContextKey{}, "synthetic-context"))
	defer cancel()
	transport := &migrationFanboxTransport{t: t, steps: input.Steps, requests: []map[string]any{}, bodies: []*migrationFanboxBody{}, cancel: cancel}
	jar := &migrationFanboxJar{}
	var traceMu sync.Mutex
	trace := []string{}
	record := func(event string) { traceMu.Lock(); defer traceMu.Unlock(); trace = append(trace, event) }
	ownedTransport := &migrationContentRecordingTransport{owned: transport, record: record}
	caller := &http.Client{Transport: ownedTransport, Jar: jar, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("caller redirect must remain untouched") }}
	options := fanbox.Options{HTTPClient: caller}
	var control *migrationPublicSolverControl
	if len(input.SolverSteps) > 0 {
		control = &migrationPublicSolverControl{t: t, steps: input.SolverSteps, requests: []map[string]any{}}
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			record("solver_control:" + r.Method + " " + r.URL.RequestURI())
			control.handler(w, r)
		}))
		defer server.Close()
		options.FlareSolverr = &fanbox.FlareSolverrOptions{URL: server.URL + "/", ProxyURL: input.SolverProxy}
	}
	open := func() *fanbox.Client {
		c, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationFanboxSessionValue}, options)
		if err != nil {
			t.Fatal(err)
		}
		return c
	}
	client := open()
	clients := []*fanbox.Client{client}
	outcomes := []map[string]any{}
	var previous sdk.Cursor
	for _, call := range input.Calls {
		if call.FreshClient {
			client = open()
			clients = append(clients, client)
		}
		callCtx := ctx
		if call.Context == "canceled" {
			c, stop := context.WithCancel(ctx)
			stop()
			callCtx = c
		}
		if call.Context == "deadline" {
			c, stop := context.WithDeadline(ctx, time.Unix(1, 0))
			defer stop()
			callCtx = c
		}
		cur := sdk.Cursor{}
		var err error
		if call.Continue {
			cur = previous
		}
		if call.Cursor != "" {
			cur, err = sdk.ParseCursor(call.Cursor)
		}
		if err == nil {
			cur, err = migrationContentMutate(t, cur, call.Mutation)
		}
		result := map[string]any{"operation": call.Operation}
		if err != nil {
			result["cursor_parse_error"] = migrationFanboxPublicHTMLError(err)
		} else {
			switch call.Operation {
			case "Creator":
				v, e := client.Creator(callCtx, fanbox.CreatorRequest{CreatorID: call.CreatorID})
				err = e
				result["dto"] = fanbox.ToCreatorDTO(v)
				result["runtime_icon"] = migrationContentResource(v.Icon.Resource)
				result["runtime_cover"] = migrationContentResource(v.Cover.Resource)
			case "Creators":
				v, e := client.Creators(callCtx, fanbox.CreatorsRequest{Kind: fanbox.CreatorListKind(call.Kind), Cursor: cur})
				err = e
				items := []fanbox.CreatorSummaryDTO{}
				for _, x := range v.Items {
					items = append(items, fanbox.ToCreatorSummaryDTO(x))
				}
				result["items"] = items
				result["runtime_items_nil"] = v.Items == nil
				previous = v.Next
				result["next"] = migrationContentCursor(v.Next)
			case "CreatorTags":
				v, e := client.CreatorTags(callCtx, fanbox.CreatorTagsRequest{CreatorID: call.CreatorID})
				err = e
				var items []fanbox.CreatorTagDTO
				if v != nil {
					items = []fanbox.CreatorTagDTO{}
				}
				for _, x := range v {
					items = append(items, fanbox.ToCreatorTagDTO(x))
				}
				result["items"] = items
				result["runtime_items_nil"] = v == nil
			case "CreatorPosts", "TaggedPosts", "Home", "Supporting":
				var v sdk.Page[fanbox.Post]
				switch call.Operation {
				case "CreatorPosts":
					v, err = client.CreatorPosts(callCtx, fanbox.CreatorPostsRequest{CreatorID: call.CreatorID, Cursor: cur})
				case "TaggedPosts":
					v, err = client.TaggedPosts(callCtx, fanbox.TaggedPostsRequest{CreatorID: call.CreatorID, Tag: call.Tag, Cursor: cur})
				case "Home":
					v, err = client.Home(callCtx, fanbox.HomeRequest{Cursor: cur})
				case "Supporting":
					v, err = client.Supporting(callCtx, fanbox.SupportingRequest{Cursor: cur})
				}
				items := []map[string]any{}
				for _, x := range v.Items {
					items = append(items, migrationContentPost(x))
				}
				result["items"] = items
				result["runtime_items_nil"] = v.Items == nil
				previous = v.Next
				result["next"] = migrationContentCursor(v.Next)
			case "Post":
				v, e := client.Post(callCtx, fanbox.PostRequest{PostID: call.PostID})
				err = e
				result["post"] = migrationContentPost(v)
			case "ResolveURL":
				v, e := client.ResolveURL(callCtx, fanbox.ResolveURLRequest{RawURL: call.RawURL})
				err = e
				result["reference"] = map[string]any{"kind": v.Kind, "creator_id": v.CreatorID, "post_id": v.PostID, "tag": v.Tag}
			case "DTO":
				result["conversion"] = migrationContentDTO()
			default:
				t.Fatalf("unknown content operation %s", call.Operation)
			}
		}
		result["error"] = migrationFanboxPublicHTMLError(err)
		result["requests_completed"] = len(transport.requests)
		outcomes = append(outcomes, result)
	}
	for _, c := range clients {
		c.CloseIdleConnections()
		c.CloseIdleConnections()
	}
	if len(transport.requests) != len(input.Steps) {
		t.Fatalf("consumed %d of %d response steps", len(transport.requests), len(input.Steps))
	}
	result := map[string]any{"outcomes": outcomes, "requests": transport.requests, "bodies": transport.bodies, "close_idle_calls": transport.idleCalls, "jar_calls": jar.calls, "go_only_injected_client_unchanged": caller.Transport == ownedTransport && caller.Jar == jar && caller.CheckRedirect != nil && caller.Timeout == 0}
	result["request_order"] = trace
	if control != nil {
		view := control.view()
		if len(view) != len(input.SolverSteps) {
			t.Fatalf("consumed %d of %d solver steps", len(view), len(input.SolverSteps))
		}
		result["solver_requests"] = view
	}
	return result
}

func migrationContentPostJSON(body string) string {
	suffix := ""
	if body != "omit" {
		suffix = `,"body":` + body
	}
	return `{"id":"post-1","title":" title ","publishedDatetime":"2026-01-02T03:04:05.123456789+09:00","creatorId":"creator-1","feeRequired":500,"isRestricted":false,"isPinned":true,"restrictedFor":3,"commentCount":7,"coverImageUrl":"https://downloads.fanbox.cc/ignored-cover","ignored":"synthetic-ignored"` + suffix + `}`
}
func migrationContentCases() []migrationContentRow {
	rows := []migrationContentRow{}
	add := func(name string, calls []migrationContentCall, steps ...migrationFanboxStep) {
		rows = append(rows, migrationContentRow{Name: name, Input: migrationContentInput{Calls: calls, Steps: append([]migrationFanboxStep{}, steps...)}})
	}
	ok := func(body string) migrationFanboxStep { return migrationFanboxStep{Status: 200, Body: body} }
	one := func(op string) migrationContentCall {
		return migrationContentCall{Operation: op, CreatorID: "creator-1", PostID: "post-1", Tag: "日本語 + &/", Kind: ""}
	}
	post := func(body string) string { return `{"body":{"post":` + migrationContentPostJSON(body) + `}}` }
	add("dto/all_fields_mutable_collections_and_nil_variants", []migrationContentCall{{Operation: "DTO"}})
	profile := `{"body":{"creatorId":" creator-actual ","user":{"name":" Display 名 ","iconUrl":" https://i.pximg.net/icon.jpg "},"coverImageUrl":" https://downloads.fanbox.cc/cover.jpg ","hasAdultContent":true,"isFollowing":true,"plan":{"fee":700,"hasSupportingPlan":true}}}`
	add("routes/Creator_trimmed_profile", []migrationContentCall{{Operation: "Creator", CreatorID: " creator +&/名 "}}, ok(profile))
	add("routes/Creators_default_supporting", []migrationContentCall{one("Creators")}, ok(`{"body":[{"creatorId":" creator-1 ","name":"ignored","iconUrl":"https://evil.example/ignored"}]}`))
	call := one("Creators")
	call.Kind = "following"
	add("routes/Creators_following", []migrationContentCall{call}, ok(`{"body":{"creators":[{"creatorId":"following"}]}}`))
	add("routes/CreatorTags_trimmed_query", []migrationContentCall{{Operation: "CreatorTags", CreatorID: " creator +&/名 "}}, ok(`{"body":[{"tag":" 日本語 ","url":" unvalidated://tag url "}]}`))
	for _, op := range []string{"CreatorPosts", "TaggedPosts", "Post", "Home", "Supporting"} {
		c := one(op)
		c.CreatorID = " creator +&/名 "
		c.PostID = " post +&/名 "
		c.Tag = " tag +&/名 "
		body := post(`{"text":"body"}`)
		if op != "Post" {
			body = `{"body":{"posts":[` + migrationContentPostJSON(`{"text":"body"}`) + `]}}`
		}
		add("routes/"+op, []migrationContentCall{c}, ok(body))
	}
	for _, op := range []string{"Creator", "CreatorTags", "CreatorPosts", "TaggedPosts", "Post"} {
		for _, value := range []string{"", " \t\n "} {
			c := one(op)
			if op == "Post" {
				c.PostID = value
			} else {
				c.CreatorID = value
			}
			name := "empty"
			if value != "" {
				name = "whitespace"
			}
			add("input/"+op+"_"+name, []migrationContentCall{c})
		}
	}
	for _, value := range []string{"", " \t "} {
		c := one("TaggedPosts")
		c.Tag = value
		name := "empty"
		if value != "" {
			name = "whitespace"
		}
		add("input/TaggedPosts_tag_"+name, []migrationContentCall{c})
	}
	for _, kind := range []string{"invalid", " following ", "SUPPORTING"} {
		c := one("Creators")
		c.Kind = kind
		add("input/Creators_kind_"+strings.ReplaceAll(kind, " ", "_"), []migrationContentCall{c})
	}
	for _, op := range []string{"Creator", "Creators", "CreatorTags", "CreatorPosts", "Post", "Home", "Supporting"} {
		for _, envelope := range []struct{ name, body string }{{"missing", `{}`}, {"null", `{"body":null}`}, {"array", `{"body":[]}`}, {"object", `{"body":{}}`}, {"scalar", `{"body":1}`}} {
			add("envelope/"+op+"_"+envelope.name, []migrationContentCall{one(op)}, ok(envelope.body))
		}
	}
	for _, kind := range []string{"supporting", "following"} {
		for _, wrapper := range []struct{ name, body string }{{"both", `{"plans":[],"creators":[{"creatorId":"following"}]}`}, {"plans_null", `{"plans":null,"creators":[]}`}, {"creators_null", `{"plans":[],"creators":null}`}, {"wrong_array_item", `[1]`}, {"empty_id", `[{"creatorId":" "}]`}, {"null_item", `[null]`}, {"null_id", `[{"creatorId":null}]`}, {"wrong_id", `[{"creatorId":1}]`}, {"case_alias", `[{"CREATORID":" alias "}]`}, {"last_duplicate", `[{"creatorId":"first","CREATORID":"last"}]`}} {
			c := one("Creators")
			c.Kind = kind
			add("creator_list/"+kind+"_"+wrapper.name, []migrationContentCall{c}, ok(`{"body":`+wrapper.body+`}`))
		}
	}
	for _, tag := range []struct{ name, body string }{{"wrapped", `{"tags":[{"tag":" tag ","url":"https://example.invalid/"}]}`}, {"empty", `[]`}, {"wrapped_null", `{"tags":null}`}, {"empty_tag", `[{"tag":" "}]`}, {"null_item", `[null]`}, {"wrong_tag", `[{"tag":1}]`}, {"wrong_url", `[{"tag":"tag","url":1}]`}, {"case_alias", `[{"TAG":"first","tag":"last","URL":"raw"}]`}, {"wrong_tags", `{"tags":{}}`}} {
		add("tags/"+tag.name, []migrationContentCall{one("CreatorTags")}, ok(`{"body":`+tag.body+`}`))
	}
	for _, profileCase := range []struct{ name, body string }{{"fallback_id", `{"user":{"name":" name "}}`}, {"empty_name", `{"creatorId":"id","user":{"name":" "}}`}, {"null_user", `{"user":null}`}, {"wrong_name", `{"user":{"name":1}}`}, {"wrong_fee", `{"user":{"name":"name"},"plan":{"fee":"700"}}`}, {"overflow_fee", `{"user":{"name":"name"},"plan":{"fee":9223372036854775808}}`}, {"wrong_bool", `{"user":{"name":"name"},"isFollowing":1}`}, {"case_alias", `{"CREATORID":"alias","USER":{"NAME":"Name"},"HASAduLTCONTENT":true}`}, {"bad_icon", `{"user":{"name":"name","iconUrl":"https://evil.example/icon"}}`}, {"bad_cover_path", `{"user":{"name":"name"},"coverImageUrl":"https://downloads.fanbox.cc/"}`}} {
		add("profile/"+profileCase.name, []migrationContentCall{one("Creator")}, ok(`{"body":`+profileCase.body+`}`))
	}
	for _, op := range []string{"CreatorPosts", "TaggedPosts", "Home", "Supporting"} {
		for _, body := range []struct{ name, raw string }{{"posts_empty_items_present", `{"posts":[],"items":[` + migrationContentPostJSON("null") + `]}`}, {"posts_null_items_present", `{"posts":null,"items":[` + migrationContentPostJSON("null") + `]}`}, {"items_only", `{"items":[` + migrationContentPostJSON("null") + `]}`}, {"posts_wrong_type", `{"posts":{}}`}, {"items_wrong_type", `{"posts":[],"items":{}}`}, {"post_null", `{"posts":[null]}`}, {"partial_bad_time", `{"posts":[` + migrationContentPostJSON("null") + `,{"id":"bad","publishedDatetime":"invalid"}]}`}} {
			add("page/"+op+"_"+body.name, []migrationContentCall{one(op)}, ok(`{"body":`+body.raw+`}`))
		}
	}
	image := `{"id":"image-1","extension":"jpg","originalUrl":"https://downloads.fanbox.cc/image.jpg","thumbnailUrl":"https://i.pximg.net/thumb.jpg","width":101,"height":202,"caption":"ignored"}`
	file := `{"id":"file-1","name":" file 名 ","extension":"zip","url":"https://downloads.fanbox.cc/file.zip","caption":"ignored"}`
	for _, body := range []struct{ name, raw string }{{"omitted", "omit"}, {"null", "null"}, {"empty_object", `{}`}, {"text_only", `{"text":" text\n名 "}`}, {"images", `{"images":[` + image + `]}`}, {"files", `{"files":[` + file + `]}`}, {"images_win", `{"images":[` + image + `],"files":[` + file + `],"blocks":[{"type":"image","imageId":"missing"}]}`}, {"empty_images_win", `{"images":[],"files":[` + file + `],"blocks":[{"type":"image","imageId":"missing"}]}`}, {"null_images_files_win", `{"images":null,"files":[` + file + `],"blocks":[{"type":"image","imageId":"missing"}]}`}, {"empty_files_win", `{"images":null,"files":[],"blocks":[{"type":"image","imageId":"missing"}]}`}, {"empty_blocks", `{"images":null,"files":null,"blocks":[]}`}, {"unused_maps_ignored", `{"imageMap":{"unused":{"id":"unused","originalUrl":"https://evil.example/private"}},"fileMap":{"unused":{"id":"unused","url":"https://evil.example/private"}}}`}, {"all_blocks_maps_repeated", `{"text":"root text","blocks":[{"type":"image","imageId":"i"},{"type":"file","fileId":"f"},{"type":"image","imageId":"i"},{"type":"article","imageId":"i","fileId":"f","text":"ignored"},{"type":"video","provider":"ignored","videoId":"ignored"},{"type":"unknown","text":"ignored"},{"type":""}],"imageMap":{"i":` + image + `},"fileMap":{"f":` + file + `}}`}, {"matching_map_keys", `{"blocks":[{"type":"file","fileId":"file-1"},{"type":"image","imageId":"image-1"}],"imageMap":{"image-1":` + image + `},"fileMap":{"file-1":` + file + `}}`}, {"unknown_dual_assets", `{"blocks":[{"type":"mystery","imageId":"image-1","fileId":"file-1"}],"imageMap":{"image-1":` + image + `},"fileMap":{"file-1":` + file + `}}`}, {"missing_image", `{"blocks":[{"type":"image","imageId":"absent"}]}`}, {"missing_file", `{"blocks":[{"type":"article","fileId":"absent"}]}`}, {"null_reference_ignored", `{"blocks":[{"type":"image","imageId":null},{"type":"file","fileId":null}]}`}, {"empty_reference_missing", `{"blocks":[{"type":"image","imageId":""}]}`}, {"map_key_id_disagreement", `{"blocks":[{"type":"image","imageId":"key"}],"imageMap":{"key":` + image + `}}`}, {"repeated_list_ids", `{"images":[` + image + `,` + strings.ReplaceAll(image, "image.jpg", "rotated.jpg") + `]}`}, {"same_id_cross_kind", `{"blocks":[{"type":"image","imageId":"image-1"},{"type":"file","fileId":"image-1"}],"imageMap":{"image-1":` + image + `},"fileMap":{"image-1":` + strings.ReplaceAll(file, "file-1", "image-1") + `}}`}, {"space_ids_retained", `{"images":[` + strings.ReplaceAll(image, "image-1", " image-1 ") + `]}`}, {"images_wrong_type", `{"images":{}}`}, {"blocks_wrong_type", `{"blocks":{}}`}, {"map_wrong_type", `{"imageMap":[]}`}, {"block_wrong_type", `{"blocks":[1]}`}, {"text_wrong_type", `{"text":1}`}} {
		add("body/"+body.name, []migrationContentCall{one("Post")}, ok(post(body.raw)))
	}
	for _, kind := range []string{"image", "file"} {
		seed := image
		key := "images"
		urlKey := "originalUrl"
		if kind == "file" {
			seed = file
			key = "files"
			urlKey = "url"
		}
		for _, bad := range []struct{ name, old, new string }{{"empty_id", `"id":"` + kind + `-1"`, `"id":" "`}, {"wrong_id", `"id":"` + kind + `-1"`, `"id":1`}, {"missing_url", `"` + urlKey + `":"https://downloads.fanbox.cc/` + kind + map[string]string{"image": ".jpg", "file": ".zip"}[kind] + `"`, `"` + urlKey + `":""`}, {"external_url", "https://downloads.fanbox.cc/", "https://evil.example/"}, {"http_url", "https://downloads.fanbox.cc/", "http://downloads.fanbox.cc/"}, {"userinfo_url", "https://downloads.fanbox.cc/", "https://user:secret@downloads.fanbox.cc/"}, {"empty_path", "https://downloads.fanbox.cc/" + kind + map[string]string{"image": ".jpg", "file": ".zip"}[kind], "https://downloads.fanbox.cc/"}} {
			add("asset/"+kind+"_"+bad.name, []migrationContentCall{one("Post")}, ok(post(`{"`+key+`":[`+strings.ReplaceAll(seed, bad.old, bad.new)+`]}`)))
		}
	}
	add("asset/image_invalid_thumbnail", []migrationContentCall{one("Post")}, ok(post(`{"images":[`+strings.ReplaceAll(image, "https://i.pximg.net/thumb.jpg", "https://evil.example/thumb")+`]}`)))
	for _, change := range []struct{ name, old, new string }{{"empty_id", `"id":"post-1"`, `"id":" "`}, {"null_id", `"id":"post-1"`, `"id":null`}, {"numeric_id", `"id":"post-1"`, `"id":1`}, {"title_wrong_type", `"title":" title "`, `"title":1`}, {"fee_fraction", `"feeRequired":500`, `"feeRequired":1.5`}, {"fee_overflow", `"feeRequired":500`, `"feeRequired":9223372036854775808`}, {"fee_null", `"feeRequired":500`, `"feeRequired":null`}, {"restricted_body_retained", `"isRestricted":false`, `"isRestricted":true`}, {"restricted_wrong_type", `"isRestricted":false`, `"isRestricted":"true"`}, {"missing_creator", `"creatorId":"creator-1"`, `"creatorId":""`}, {"case_alias", `"id":"post-1"`, `"ID":"alias"`}, {"last_duplicate", `"id":"post-1"`, `"id":"first","ID":"last"`}, {"long_s_alias", `"isRestricted":false`, `"iſRestricted":true`}, {"unknown_wrong_type", `"ignored":"synthetic-ignored"`, `"ignored":{"x":[1,2]}`}} {
		add("post_fields/"+change.name, []migrationContentCall{one("Post")}, ok(strings.ReplaceAll(post(`{"text":"body"}`), change.old, change.new)))
	}
	for _, date := range []string{"", "invalid", "2026-01-02T03:04:05Z", "2026-01-02T03:04:05.123456789999-04:30", "2026-01-02T3:04:05,123Z", "2026-01-02T03:04:05+24:60", "2026-01-02t03:04:05z", "2026-01-02T03:04:60Z"} {
		add("time/"+fmt.Sprintf("%02d", len(rows)), []migrationContentCall{one("Post")}, ok(strings.ReplaceAll(post("null"), "2026-01-02T03:04:05.123456789+09:00", date)))
	}
	for _, api := range []struct{ name, step string }{{"trailing_root", post("null") + ` {"later":true}`}, {"trailing_invalid", post("null") + ` garbage`}, {"first_null", `null ` + post("null")}, {"empty", ""}, {"malformed", `{"body":`}, {"duplicate_body", `{"body":{"post":{"id":"bad"}},"BODY":{"post":` + migrationContentPostJSON("null") + `}}`}, {"wrong_body", `{"body":true}`}} {
		add("decode/"+api.name, []migrationContentCall{one("Post")}, ok(api.step))
	}
	for _, spec := range []struct {
		name, body, read, close string
		chunk                   int
		cancelRead, cancelClose bool
	}{{"valid_read_error", post("null"), "external", "", 0, false, false}, {"valid_read_eof", post("null"), "eof", "", 0, false, false}, {"partial_read_error", `{"body":`, "external", "", 0, false, false}, {"valid_close_error", post("null"), "", "external", 0, false, false}, {"decode_close_join", `{`, "external", "external", 0, false, false}, {"decode_cancel_close_join", `{`, "external", "canceled", 0, false, false}, {"valid_cancel_read", post("null"), "", "", 0, true, false}, {"invalid_cancel_read", `{`, "", "", 0, true, false}, {"valid_cancel_close", post("null"), "", "", 0, false, true}, {"close_cancel_read_failure", `{`, "external", "external", 0, false, true}, {"chunk_first_root", post("null") + ` garbage`, "", "", 1, false, false}} {
		s := ok(spec.body)
		s.ReadError = spec.read
		s.CloseError = spec.close
		s.Chunk = spec.chunk
		s.CancelOnRead = spec.cancelRead
		s.CancelOnClose = spec.cancelClose
		add("ownership/"+spec.name, []migrationContentCall{one("Post")}, s)
	}
	for _, spec := range []struct {
		name string
		step migrationFanboxStep
	}{{"transport_external", migrationFanboxStep{TransportError: "external"}}, {"transport_canceled", migrationFanboxStep{TransportError: "canceled"}}, {"transport_deadline", migrationFanboxStep{TransportError: "deadline"}}, {"nil_response", migrationFanboxStep{TransportError: "nil_response"}}, {"nil_body", migrationFanboxStep{Status: 200, NilBody: true}}, {"nil_body_nonzero_length", migrationFanboxStep{Status: 200, NilBody: true, ContentLength: 5}}, {"expired", migrationFanboxStep{Status: 401, Body: "private response"}}, {"forbidden", migrationFanboxStep{Status: 403, Body: `{"challenge":"ordinary"}`, Headers: http.Header{"Content-Type": {"application/json"}}}}, {"challenge", migrationFanboxStep{Status: 403, Body: "cf-chl private response"}}, {"rate_limit", migrationFanboxStep{Status: 429, Body: "private response"}}, {"not_found", migrationFanboxStep{Status: 404, Body: "private response"}}, {"server_error", migrationFanboxStep{Status: 503, Body: "private response"}}} {
		add("errors/"+spec.name, []migrationContentCall{one("Post")}, spec.step)
	}
	for _, state := range []string{"canceled", "deadline"} {
		c := one("Post")
		c.Context = state
		add("context/"+state+"_ignoring_transport_success", []migrationContentCall{c}, ok(post("null")))
		s := migrationFanboxStep{TransportError: "external"}
		add("context/"+state+"_transport_error", []migrationContentCall{c}, s)
	}
	add("redirect/content_drops_cookie_preserves_owned_close", []migrationContentCall{one("Post")}, migrationFanboxStep{Status: 302, Location: "https://api.fanbox.cc/post.info?postId=post-1&redirect=1", Body: "redirect bytes"}, ok(post("null")))
	for _, op := range []string{"Creators", "CreatorPosts", "TaggedPosts", "Home", "Supporting"} {
		scoped := op == "Creators" || op == "Home" || op == "Supporting"
		c := one(op)
		page := func(next string, empty bool, urls []string) string {
			key := "posts"
			item := migrationContentPostJSON("null")
			if op == "Creators" {
				key = "plans"
				item = `{"creatorId":"creator-1"}`
			}
			items := []json.RawMessage{}
			if !empty {
				items = append(items, json.RawMessage(item))
			}
			raw, _ := json.Marshal(map[string]any{"body": map[string]any{key: items, "pageUrls": urls, "nextUrl": next}})
			return string(raw)
		}
		identity := ok(homeMetadataBody(42, "synthetic-user"))
		nextURL := "https://api.fanbox.cc/continuation?z=last&a=%2f&a=+&tag=%E5%90%8D#fragment"
		continuation := c
		continuation.Continue = true
		steps := []migrationFanboxStep{ok(page("https://api.fanbox.cc/ignored", false, []string{nextURL, "https://api.fanbox.cc/ignored-2"}))}
		if scoped {
			steps = append(steps, identity)
		}
		steps = append(steps, ok(page("https://api.fanbox.cc/final?next=3", true, nil)), ok(page("", false, nil)))
		add("pagination/"+op+"_priority_exact_query_empty_page_cached_identity", []migrationContentCall{c, continuation, continuation}, steps...)
		for _, spec := range []struct {
			name, next string
			urls       []string
		}{{"empty_next", "", nil}, {"empty_pageurl_wins", "https://api.fanbox.cc/ignored", []string{""}}, {"unsafe_minted", "https://evil.example/next", nil}, {"relative_minted", "/next", nil}, {"whitespace_next", "   ", nil}, {"subdomain_continuation", "https://other.fanbox.cc:65536/a?raw=+", nil}, {"leading_space_continuation", " https://api.fanbox.cc/next ", nil}, {"cycle_allowed", "https://api.fanbox.cc/cycle", nil}} {
			s := []migrationFanboxStep{ok(page(spec.next, true, spec.urls))}
			active := spec.next != "" && !(len(spec.urls) > 0 && spec.urls[0] == "")
			calls := []migrationContentCall{c}
			if active && scoped {
				s = append(s, identity)
			}
			if active {
				calls = append(calls, continuation)
				switch spec.name {
				case "unsafe_minted", "relative_minted":
				case "leading_space_continuation":
					if op == "Creators" {
						s = append(s, ok(page("", true, nil)))
					}
				case "cycle_allowed":
					s = append(s, ok(page(spec.next, true, nil)))
					calls = append(calls, continuation)
					s = append(s, ok(page("", true, nil)))
				default:
					s = append(s, ok(page("", true, nil)))
				}
			}
			add("pagination/"+op+"_"+spec.name, calls, s...)
		}
		for _, mutation := range []string{"product", "operation", "binding", "query"} {
			next := continuation
			next.Mutation = mutation
			next.FreshClient = true
			s := []migrationFanboxStep{ok(page(nextURL, true, nil))}
			if scoped {
				s = append(s, identity)
			}
			add("cursor_binding/"+op+"_"+mutation, []migrationContentCall{c, next}, s...)
		}
		if op == "Home" || op == "CreatorPosts" {
			for _, mutation := range []string{"format", "identity", "no_identity", "empty_identity", "bad_payload", "empty_url", "null_payload", "wrong_url_type", "relative_url", "unsafe_url", "empty_payload", "unknown_fields"} {
				next := continuation
				next.Mutation = mutation
				next.FreshClient = true
				s := []migrationFanboxStep{ok(page(nextURL, true, nil))}
				if scoped {
					s = append(s, identity)
				}
				if scoped && mutation != "format" && mutation != "no_identity" && mutation != "empty_identity" {
					s = append(s, identity)
				}
				if mutation == "unknown_fields" || (!scoped && (mutation == "identity" || mutation == "no_identity" || mutation == "empty_identity")) {
					s = append(s, ok(page("", true, nil)))
				}
				add("cursor_payload/"+op+"_"+mutation, []migrationContentCall{c, next}, s...)
			}
		}
		if scoped {
			next := continuation
			next.FreshClient = true
			add("cursor_identity/"+op+"_cross_account", []migrationContentCall{c, next}, ok(page(nextURL, true, nil)), identity, ok(homeMetadataBody(99, "another-user")))
			add("cursor_identity/"+op+"_fresh_same_account", []migrationContentCall{c, next}, ok(page(nextURL, true, nil)), identity, identity, ok(page("", true, nil)))
			for _, id := range []int64{0, -1} {
				add("cursor_identity/"+op+fmt.Sprintf("_invalid_%d", id), []migrationContentCall{c}, ok(page(nextURL, true, nil)), ok(homeMetadataBody(id, "synthetic-user")))
			}
			add("cursor_identity/"+op+"_expired_after_list", []migrationContentCall{c}, ok(page(nextURL, false, nil)), migrationFanboxStep{Status: 401, Body: "private identity body"})
			add("cursor_identity/"+op+"_query_precedes_identity", []migrationContentCall{c, {Operation: op, CreatorID: c.CreatorID, Tag: c.Tag, Kind: "following", Continue: true, Mutation: "query", FreshClient: true}}, ok(page(nextURL, false, nil)), identity)
		} else {
			first := c
			first.CreatorID = " creator-1 "
			second := continuation
			second.CreatorID = "creator-1"
			add("cursor_query/"+op+"_raw_input_binding_before_trim", []migrationContentCall{first, second}, ok(page(nextURL, true, nil)))
			if op == "TaggedPosts" {
				second = c
				second.Continue = true
				second.Tag = "changed"
				add("cursor_query/TaggedPosts_tag_bound", []migrationContentCall{c, second}, ok(page(nextURL, true, nil)))
			}
		}
	}
	for _, raw := range []string{"", "not-base64", "e30", "bnVsbA"} {
		c := one("Home")
		c.Cursor = raw
		if raw == "" {
			continue
		}
		add("cursor_parse/"+raw, []migrationContentCall{c})
	}
	for index, raw := range []string{
		"https://fanbox.cc/@creator", "https://www.fanbox.cc/@creator/posts", "https://www.fanbox.cc/@creator/posts/123", "https://www.fanbox.cc/@creator/posts/tag/%E5%90%8D%20%2B", "https://www.fanbox.cc/creators/creator", "https://creator.fanbox.cc/posts/123", "https://api.fanbox.cc/anything", "https://www.fanbox.cc:65536/@Creator", "https://www.fanbox.cc:000443/@creator", " HTTPS://WWW.FANBOX.CC/@creator/ ", "https://www.fanbox.cc/@creator?ignored=1#ignored", "https://www.fanbox.cc///@creator///", "https://www.fanbox.cc/@creator/posts/non-numeric", "https://www.fanbox.cc/@creator/posts/%2F", "https://www.fanbox.cc/@creator/posts/tag/a%2Fb", "https://www.fanbox.cc/@creator/posts/tag/%20", "https://www.fanbox.cc/@creator/posts/../", "https://www.fanbox.cc/@creator/posts//", "https://www.fanbox.cc/@creator/posts/tag", "https://www.fanbox.cc/@creator/unknown", "https://www.fanbox.cc/@creator/posts/tag/a/deeper", "https://www.fanbox.cc/@", "https://www.fanbox.cc/creators/", "https://www.fanbox.cc/creators/id/extra", "https://www.fanbox.cc/", "https://www.fanbox.cc/posts/123", "https://nested.creator.fanbox.cc/", "https://fanbox.pixiv.net/@creator", "https://www.pixiv.net/fanbox/creator", "https://evil.example/@creator", "http://www.fanbox.cc/@creator", "//www.fanbox.cc/@creator", "https://user:secret@www.fanbox.cc/@creator", "https://www.fanbox.cc./@creator", "https://www.fanbox.cc:port/@creator", "https://www.fanbox.cc/%zz", "https://www.fanbox.cc/@creator\n", "123", "creator", "https://www.fanbox.cc/@%E5%90%8D/posts/tag/%E9%9B%AA",
	} {
		add(fmt.Sprintf("resolve/%02d", index), []migrationContentCall{{Operation: "ResolveURL", RawURL: raw}})
	}
	add("resolve/canceled_is_local", []migrationContentCall{{Operation: "ResolveURL", RawURL: "https://www.fanbox.cc/@creator/posts/123", Context: "canceled"}})
	for _, spec := range []struct{ name, body string }{
		{"all_nullable_collections", `{"images":null,"files":null,"blocks":null,"imageMap":null,"fileMap":null}`},
		{"kelvin_blocks_alias", `{"blocKs":[{"type":"article","text":"ignored"}]}`},
		{"case_aliases_image_map", `{"BLOCKS":[{"TYPE":"image","IMAGEID":"image-1"}],"IMAGEMAP":{"image-1":` + image + `}}`},
		{"case_aliases_file_map", `{"blocks":[{"type":"file","fileId":"file-1"}],"FILEMAP":{"file-1":` + file + `}}`},
		{"ignored_dimensions_wrong_types", `{"images":[` + strings.ReplaceAll(strings.ReplaceAll(image, `"width":101`, `"width":[]`), `"height":202`, `"height":{}`) + `]}`},
		{"ignored_branch_still_decoded", `{"images":[],"files":{},"blocks":[]}`},
		{"duplicate_images_null_falls_to_files", `{"images":[` + image + `],"IMAGES":null,"files":[` + file + `]}`},
		{"duplicate_images_empty_wins", `{"images":[` + image + `],"IMAGES":[],"files":[` + file + `]}`},
		{"block_image_wrong_type", `{"blocks":[{"type":"image","imageId":1}]}`},
		{"block_file_wrong_type", `{"blocks":[{"type":"file","fileId":true}]}`},
		{"block_type_null_unknown", `{"blocks":[{"type":null}]}`},
		{"image_id_null", `{"images":[` + strings.ReplaceAll(image, `"id":"image-1"`, `"id":null`) + `]}`},
		{"file_url_wrong_type", `{"files":[` + strings.ReplaceAll(file, `"url":"https://downloads.fanbox.cc/file.zip"`, `"url":1`) + `]}`},
		{"uncredentialed_file", `{"files":[` + strings.ReplaceAll(file, "https://downloads.fanbox.cc/file.zip", "https://i.pximg.net/file.zip?signature=synthetic-locator") + `]}`},
	} {
		add("body/"+spec.name, []migrationContentCall{one("Post")}, ok(post(spec.body)))
	}
	add("post_fields/duplicate_null_scalar_preserves_previous", []migrationContentCall{one("Post")}, ok(strings.ReplaceAll(post("null"), `"feeRequired":500`, `"feeRequired":500,"FEEREQUIRED":null`)))
	add("post_fields/duplicate_null_body_clears_previous", []migrationContentCall{one("Post")}, ok(strings.ReplaceAll(post(`{"text":"first"}`), `"body":{"text":"first"}`, `"body":{"text":"first"},"BODY":null`)))
	add("post_fields/asset_empty_creator_omits_ref_component", []migrationContentCall{one("Post")}, ok(strings.ReplaceAll(post(`{"images":[`+image+`]}`), `"creatorId":"creator-1"`, `"creatorId":""`)))
	add("body/locator_rotation_ref_stable", []migrationContentCall{one("Post"), one("Post")}, ok(post(`{"images":[`+image+`]}`)), ok(post(`{"images":[`+strings.ReplaceAll(image, "image.jpg", "rotated.jpg?signature=synthetic-secret")+`]}`)))
	for _, raw := range []struct{ name, body string }{{"next_url_wrong_type", `{"body":{"posts":[],"nextUrl":1}}`}, {"page_urls_wrong_type", `{"body":{"posts":[],"pageUrls":{}}}`}, {"page_urls_null_entry", `{"body":{"posts":[],"pageUrls":[null],"nextUrl":"https://api.fanbox.cc/ignored"}}`}} {
		add("page/Home_"+raw.name, []migrationContentCall{one("Home")}, ok(raw.body))
	}
	challenge := migrationFanboxStep{Status: 403, Headers: http.Header{"Cf-Mitigated": {"challenge"}}, Body: "synthetic challenge"}
	solved := migrationFanboxStep{Status: 200, Body: migrationPublicSolverDocument}
	for _, op := range []string{"Creator", "Creators", "CreatorTags", "CreatorPosts", "TaggedPosts", "Post", "Home", "Supporting"} {
		response := post("null")
		switch op {
		case "Creator":
			response = profile
		case "Creators":
			response = `{"body":[]}`
		case "CreatorTags":
			response = `{"body":[]}`
		case "CreatorPosts", "TaggedPosts", "Home", "Supporting":
			response = `{"body":{"posts":[]}}`
		}
		rows = append(rows, migrationContentRow{Name: "solver/" + op + "_challenge_solve_native_content_replay", Input: migrationContentInput{Calls: []migrationContentCall{one(op)}, Steps: []migrationFanboxStep{challenge, ok(response)}, SolverSteps: []migrationFanboxStep{solved}}})
	}
	rows = append(rows, migrationContentRow{Name: "solver/Post_cached_clearance_refresh_one_replay", Input: migrationContentInput{Calls: []migrationContentCall{one("Post"), one("Post"), one("Post")}, Steps: []migrationFanboxStep{challenge, ok(post("null")), ok(post("null")), challenge, ok(post("null"))}, SolverSteps: []migrationFanboxStep{solved, {Status: 200, Body: strings.ReplaceAll(migrationPublicSolverDocument, "synthetic-clearance", "replacement-clearance")}}, SolverProxy: "http://proxy.example:8080"}})
	for _, spec := range []struct {
		name    string
		control migrationFanboxStep
		replay  bool
	}{{"failed", migrationFanboxStep{Status: 200, Body: `{"status":"error","message":"private control"}`}, false}, {"malformed", migrationFanboxStep{Status: 200, Body: `{`}, false}, {"unavailable", migrationFanboxStep{Status: 503, Body: "private control"}, false}, {"invalid_solution", migrationFanboxStep{Status: 200, Body: `{"status":"ok","solution":{"userAgent":"agent","cookies":[]}}`}, false}, {"second_challenge", solved, true}} {
		steps := []migrationFanboxStep{challenge}
		if spec.replay {
			steps = append(steps, challenge)
		}
		rows = append(rows, migrationContentRow{Name: "solver/Post_" + spec.name, Input: migrationContentInput{Calls: []migrationContentCall{one("Post")}, Steps: steps, SolverSteps: []migrationFanboxStep{spec.control}}})
	}
	return rows
}

func migrationContentProvenance(t *testing.T, root string) map[string]any {
	t.Helper()
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	frozen := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	sources := map[string]string{}
	for _, path := range []string{"sdk/fanbox/fanbox.go", "sdk/fanbox/ops.go", "sdk/fanbox/models.go", "sdk/fanbox/dto.go", "sdk/fanbox/cursor.go", "sdk/fanbox/reference.go", "sdk/fanbox/request.go", "sdk/fanbox/errors.go", "sdk/fanbox/resource.go", "sdk/cursor.go", "sdk/ref.go", "sdk/resource.go", "sdk/resource_dto.go", "sdk/error.go", "internal/services/fanbox/protocol/protocol.go", "internal/services/fanbox/protocol/solver.go", "internal/services/fanbox/endpoint/creator/creator.go", "internal/services/fanbox/endpoint/creator/creators/creators.go", "internal/services/fanbox/endpoint/creator/tags/tags.go", "internal/services/fanbox/endpoint/post/post.go", "internal/services/fanbox/endpoint/post/wire/wire.go", "internal/services/fanbox/endpoint/post/posts/posts.go", "internal/services/fanbox/endpoint/post/info/info.go", "internal/services/fanbox/endpoint/post/home/home.go", "internal/services/fanbox/endpoint/post/supporting/supporting.go", "internal/services/fanbox/resource/resource.go"} {
		current, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		original, err := exec.Command("git", "-C", root, "show", reference.SourceCommit+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(current, original) {
			t.Fatalf("frozen source changed: %s", path)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	stdlib := map[string]string{}
	paths, err := filepath.Glob(filepath.Join(runtime.GOROOT(), "src", "encoding", "json", "*.go"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{"net/url/url.go", "time/format.go", "time/format_rfc3339.go", "encoding/base64/base64.go", "net/http/client.go", "strconv/number.go"} {
		paths = append(paths, filepath.Join(runtime.GOROOT(), "src", path))
	}
	for _, path := range paths {
		if strings.HasSuffix(path, "_test.go") {
			continue
		}
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		rel, err := filepath.Rel(filepath.Join(runtime.GOROOT(), "src"), path)
		if err != nil {
			t.Fatal(err)
		}
		stdlib[rel] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	const base = "7078b729cc4dd48ee2b28f8eedcb758bacbc3a0f"
	listing, err := exec.Command("git", "-C", root, "ls-tree", "-r", "--name-only", base).Output()
	if err != nil {
		t.Fatal(err)
	}
	protected := map[string]string{}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if !strings.Contains(path, "fanbox") {
			continue
		}
		if !(strings.HasSuffix(path, "_test.go") || strings.HasPrefix(path, "crates/") && (strings.Contains(path, "/tests/") || strings.HasSuffix(path, ".json"))) {
			continue
		}
		original, err := exec.Command("git", "-C", root, "show", base+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		current, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, current) {
			t.Fatalf("published contract changed: %s", path)
		}
		protected[path] = fmt.Sprintf("%x", sha256.Sum256(current))
	}
	return map[string]any{"source_commit": reference.SourceCommit, "published_base": base, "go_version": runtime.Version(), "source_sha256": sources, "go_stdlib_sha256": stdlib, "dependencies": reference.Dependencies, "frozen_go_production_guard": frozen, "protected_published_files_sha256": protected}
}
func TestMigrationFanboxContentReadsFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	contract := migrationContentProvenance(t, root)
	rows := migrationContentCases()
	names := map[string]bool{}
	families := map[string]int{}
	for index := range rows {
		row := &rows[index]
		if names[row.Name] {
			t.Fatalf("duplicate row %s", row.Name)
		}
		names[row.Name] = true
		families[strings.SplitN(row.Name, "/", 2)[0]]++
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationContentObserve(t, row.Input) })
	}
	if t.Failed() {
		t.Fatal("capture has failed rows; no fixture is published")
	}
	contract["public_operations"] = []string{"Client.Creator", "Client.Creators", "Client.CreatorTags", "Client.CreatorPosts", "Client.TaggedPosts", "Client.Post", "Client.Home", "Client.Supporting", "Client.ResolveURL", "output-safe DTO conversions"}
	contract["cases"] = rows
	contract["family_counts"] = families
	contract["evidence"] = "Actual frozen Go public SDK OpenWith and content operations use fallible owned injected net/http transports. Endpoint JSON envelopes, wire normalization, SDK mapping, cursor mint/consume, resource generation, local URL resolution and DTO conversion are observed directly. Fixture expectations are captured from execution rather than hand-reconstructed. Raw continuation URL bytes, query binding and API/identity/control call order remain intact. Public configured solver replay uses the established anonymous ordinary HTTP/1 owned loopback control route, with no business URLs/session credentials sent to control."
	contract["go_only_projections"] = []string{"Concrete error tree types and IsReason matching, injected pointer ownership and Read call/event topology are Go-specific observations; semantic error/source text, context identity, SDK DTOs, exact requests, bytes and Close counts remain actual public contract outcomes.", "Runtime model nil collections are explicitly observed separately from output-safe DTO empty-array conversions.", "Resource runtime URL/Referer/credential requirements and exact ref payloads are recorded for comparison; safe DTOs exclude URLs and headers. No generated resource is opened in this content scope."}
	contract["limitations"] = []string{"Bounded meaningful response/input classes, not complete JSON/HTML/URL/date grammar equivalence or FANBOX/platform completion.", "Owned synthetic API/identity responses and established anonymous ordinary HTTP/1 loopback solver-control POST only; no external networking, live accounts, native TLS/media, browser, trust, HEAD/upload or denied supplemental native probes.", "CLI source numeric classification, logical list pagination/cycle policy and MCP projections belong to sibling entry captures. SDK continuation permits upstream cycles; no loop detector is invented.", "Resource opening/re-resolution/media ownership, saved account facade/lease execution, file saving and download expansion remain separate owners."}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-content-reads.json")
	if *migrationCaptureFanboxContentReads {
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public FANBOX content behavior differs from frozen fixture; review actual Go outcome before recapture")
	}
	t.Logf("frozen Go %s: %d bounded public content rows; fixture bytes=%d sha256=%x; protected Go production/module paths=434", runtime.Version(), len(rows), len(data), sha256.Sum256(data))
	keys := []string{}
	for k := range families {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		t.Logf("%s: %d", k, families[k])
	}
}

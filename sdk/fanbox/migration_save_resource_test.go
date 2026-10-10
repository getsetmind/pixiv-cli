package fanbox_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxSaveResource = flag.Bool("migration-capture-fanbox-save-resource", false, "capture frozen Go public FANBOX saves to owned local destinations")

const migrationSavePublished = "7991c7d6c8e58ad1b4e562ebcf91e8070d0ccfb5"

type migrationSaveInput struct {
	Resource        migrationResourceInput `json:"resource"`
	Path            string                 `json:"path"`
	Setup           string                 `json:"setup"`
	Progress        bool                   `json:"progress"`
	ProgressAction  string                 `json:"progress_action"`
	PanicOnProgress bool                   `json:"panic_on_progress"`
	ZeroReads       int                    `json:"zero_reads"`
	StopOnOpen      string                 `json:"stop_on_open"`
	StopOnRead      string                 `json:"stop_on_read"`
}
type migrationSaveRow struct {
	Name        string             `json:"name"`
	Input       migrationSaveInput `json:"input"`
	Observation map[string]any     `json:"observation"`
}
type migrationSaveContext struct {
	context.Context
	mu   sync.Mutex
	err  error
	done chan struct{}
}

func (c *migrationSaveContext) Done() <-chan struct{} { return c.done }
func (c *migrationSaveContext) Err() error            { c.mu.Lock(); defer c.mu.Unlock(); return c.err }
func (c *migrationSaveContext) stop(kind string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.err != nil {
		return
	}
	if kind == "deadline" {
		c.err = context.DeadlineExceeded
	} else {
		c.err = context.Canceled
	}
	close(c.done)
}

type migrationSaveBody struct {
	source     *migrationResourceBody
	ctx        *migrationSaveContext
	zeroReads  int
	stopOnRead string
}

func (b *migrationSaveBody) Read(p []byte) (int, error) {
	var n int
	var err error
	if b.zeroReads > 0 {
		b.zeroReads--
		b.source.reads = append(b.source.reads, map[string]any{"requested": len(p), "returned": 0, "error_kind": "", "after_close": b.source.closes > 0})
	} else {
		n, err = b.source.Read(p)
	}
	if b.stopOnRead != "" {
		b.ctx.stop(b.stopOnRead)
		b.stopOnRead = ""
	}
	return n, err
}
func (b *migrationSaveBody) Close() error { return b.source.Close() }

type migrationSaveTransport struct {
	inner *migrationResourceTransport
	input migrationSaveInput
	ctx   *migrationSaveContext
}

func (r *migrationSaveTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	response, err := r.inner.RoundTrip(request)
	metadata := request.URL.Hostname() == "api.fanbox.cc" && (request.URL.Path == "/creator.get" || request.URL.Path == "/post.info")
	if !metadata && response != nil {
		if source, ok := response.Body.(*migrationResourceBody); ok {
			response.Body = &migrationSaveBody{source: source, ctx: r.ctx, zeroReads: r.input.ZeroReads, stopOnRead: r.input.StopOnRead}
		}
		if r.input.StopOnOpen != "" {
			r.ctx.stop(r.input.StopOnOpen)
		}
	}
	return response, err
}
func (r *migrationSaveTransport) CloseIdleConnections() { r.inner.CloseIdleConnections() }
func migrationSaveDefault(kind string) migrationSaveInput {
	return migrationSaveInput{Resource: migrationResourceDefault(kind), Path: "nested/output.bin", Progress: true}
}
func migrationSaveFiles(t *testing.T, root string) []map[string]any {
	t.Helper()
	out := []map[string]any{}
	err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if path == root {
			return nil
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		kind := "file"
		if info.IsDir() {
			kind = "directory"
		}
		if info.Mode()&os.ModeSymlink != 0 {
			kind = "symlink"
		}
		if strings.HasPrefix(filepath.Base(relative), ".atomic-write-") {
			relative = filepath.Join(filepath.Dir(relative), ".atomic-write-$TEMP")
		}
		item := map[string]any{"path": filepath.ToSlash(relative), "kind": kind, "mode": fmt.Sprintf("%04o", info.Mode().Perm())}
		if kind == "file" {
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			item["bytes"] = append([]byte{}, data...)
			item["size"] = len(data)
			item["sha256"] = fmt.Sprintf("%x", sha256.Sum256(data))
		}
		if kind == "symlink" {
			target, err := os.Readlink(path)
			if err != nil {
				return err
			}
			item["target"] = filepath.ToSlash(strings.TrimPrefix(target, root+string(filepath.Separator)))
		}
		out = append(out, item)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	sort.Slice(out, func(i, j int) bool { return out[i]["path"].(string) < out[j]["path"].(string) })
	return out
}
func migrationSaveDestination(t *testing.T, root string, input migrationSaveInput) string {
	t.Helper()
	path := input.Path
	if strings.TrimSpace(path) != "" {
		path = filepath.Join(root, path)
	}
	mkdir := func(dir string, mode os.FileMode) {
		if err := os.MkdirAll(dir, mode); err != nil {
			t.Fatal(err)
		}
	}
	write := func(name, value string, mode os.FileMode) {
		if err := os.WriteFile(name, []byte(value), mode); err != nil {
			t.Fatal(err)
		}
	}
	switch input.Setup {
	case "":
	case "parent_file":
		write(filepath.Join(root, "nested"), "parent-old", 0600)
	case "final_directory":
		mkdir(path, 0755)
		write(filepath.Join(path, "child"), "final-old", 0644)
	case "old_final":
		mkdir(filepath.Dir(path), 0755)
		write(path, "final-old", 0644)
	case "existing_parent":
		mkdir(filepath.Dir(path), 0755)
	case "final_symlink":
		mkdir(filepath.Dir(path), 0755)
		target := filepath.Join(root, "link-target")
		write(target, "target-old", 0644)
		if err := os.Symlink(target, path); err != nil {
			t.Fatal(err)
		}
	case "parent_symlink":
		target := filepath.Join(root, "owned-target")
		mkdir(target, 0755)
		if err := os.Symlink(target, filepath.Join(root, "nested")); err != nil {
			t.Fatal(err)
		}
	default:
		t.Fatalf("unknown destination setup %s", input.Setup)
	}
	return path
}
func migrationSaveProgressAction(t *testing.T, root, path, action string, ctx *migrationSaveContext) {
	t.Helper()
	switch action {
	case "":
	case "cancel", "deadline":
		ctx.stop(action)
	case "remove_temp", "temp_directory":
		matches, err := filepath.Glob(filepath.Join(filepath.Dir(path), ".atomic-write-*"))
		if err != nil || len(matches) != 1 {
			t.Fatalf("progress must observe one real atomic destination: %v %v", matches, err)
		}
		if err := os.Remove(matches[0]); err != nil {
			t.Fatal(err)
		}
		if action == "temp_directory" {
			if err := os.Mkdir(matches[0], 0700); err != nil {
				t.Fatal(err)
			}
		}
	case "final_directory":
		if err := os.Mkdir(path, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(path, "child"), []byte("callback-owned"), 0600); err != nil {
			t.Fatal(err)
		}
	default:
		t.Fatalf("unknown progress action %s", action)
	}
}
func migrationSaveObserve(t *testing.T, input migrationSaveInput) map[string]any {
	t.Helper()
	root := t.TempDir()
	ctx := &migrationSaveContext{Context: context.WithValue(context.Background(), migrationResourceContextKey{}, "resource-context"), done: make(chan struct{})}
	defer ctx.stop("canceled")
	producerTransport := &migrationResourceTransport{t: t, role: "producer", document: input.Resource.ProducerDocument, input: input.Resource, requests: []map[string]any{}, bodies: []*migrationResourceBody{}, cancel: func() { ctx.stop("canceled") }}
	consumerTransport := &migrationResourceTransport{t: t, role: "consumer", document: input.Resource.ReopenDocument, input: input.Resource, requests: []map[string]any{}, bodies: []*migrationResourceBody{}, cancel: func() { ctx.stop("canceled") }}
	open := func(source *migrationResourceTransport) *fanbox.Client {
		client, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationResourceSession}, fanbox.Options{HTTPClient: &http.Client{Transport: &migrationSaveTransport{inner: source, input: input, ctx: ctx}}, UserAgent: "resource-injected-agent"})
		if err != nil {
			t.Fatal(err)
		}
		return client
	}
	producer := open(producerTransport)
	client := producer
	var consumer *fanbox.Client
	if input.Resource.Mode == "fresh" {
		consumer = open(consumerTransport)
		client = consumer
	}
	out := map[string]any{"generated_resources": []map[string]any{}, "generation_error": migrationResourceError(nil), "reference_error": migrationResourceError(nil), "outcomes": []map[string]any{}}
	var ref sdk.ResourceRef
	ready := true
	if input.Resource.Kind != "" {
		resource, all, err := migrationResourceGenerate(t, ctx, producer, input.Resource.Kind)
		out["generated_resources"] = all
		out["generation_error"] = migrationResourceError(err)
		ref = resource.Ref
		ready = err == nil
	} else if input.Resource.RefPayload != "" {
		var err error
		ref, err = sdk.NewResourceRef(input.Resource.RefProduct, []byte(input.Resource.RefPayload))
		out["reference_error"] = migrationResourceError(err)
		ready = err == nil
	} else if input.Resource.ParseText != "" || input.Resource.ParseOnly {
		var err error
		ref, err = sdk.ParseResourceRef(input.Resource.ParseText)
		out["reference_error"] = migrationResourceError(err)
		ready = err == nil && !input.Resource.ParseOnly
	}
	path := migrationSaveDestination(t, root, input)
	out["initial_files"] = migrationSaveFiles(t, root)
	if ready {
		out["selected_ref"] = ref.String()
		if input.Resource.Context != "" {
			ctx.stop(input.Resource.Context)
		}
		outcomes := []map[string]any{}
		for index := 0; index < max(1, input.Resource.Repeat); index++ {
			progress := []map[string]any{}
			actionApplied := false
			options := sdk.SaveOptions{Path: path}
			if input.Progress {
				options.Progress = func(value sdk.SaveProgress) {
					progress = append(progress, map[string]any{"done": value.Done, "total": value.Total, "files_before_write": migrationSaveFiles(t, root), "ownership_before_write": migrationResourceOwnership(producerTransport, consumerTransport)})
					if input.PanicOnProgress {
						panic("owned-progress-panic")
					}
					if !actionApplied {
						migrationSaveProgressAction(t, root, path, input.ProgressAction, ctx)
						actionApplied = true
					}
				}
			}
			var saved sdk.SavedResource
			var err error
			panicText := ""
			func() {
				defer func() {
					if value := recover(); value != nil {
						if !input.PanicOnProgress || value != "owned-progress-panic" {
							t.Fatalf("unexpected SaveResource panic: %v", value)
						}
						panicText = value.(string)
					}
				}()
				saved, err = client.SaveResource(ctx, ref, options)
			}()
			savedPath := saved.Path
			if strings.HasPrefix(savedPath, root+string(filepath.Separator)) {
				savedPath = "$OWNED/" + filepath.ToSlash(strings.TrimPrefix(savedPath, root+string(filepath.Separator)))
			}
			outcomes = append(outcomes, map[string]any{"returned": panicText == "", "panic": panicText, "saved": map[string]any{"path": savedPath, "size": saved.Size, "content_type": saved.ContentType}, "error": migrationResourceError(err), "progress": progress, "files": migrationSaveFiles(t, root), "ownership_at_return": migrationResourceOwnership(producerTransport, consumerTransport), "context_at_return": migrationResourceError(ctx.Err())})
		}
		out["outcomes"] = outcomes
	}
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
	for _, file := range migrationSaveFiles(t, root) {
		if strings.Contains(file["path"].(string), ".atomic-write-") {
			t.Fatal("public save left its owned temporary destination behind")
		}
	}
	return out
}
func migrationSaveCases() []migrationSaveRow {
	rows := []migrationSaveRow{}
	add := func(name string, input migrationSaveInput) {
		rows = append(rows, migrationSaveRow{Name: name, Input: input})
	}
	for _, kind := range []string{"creator_icon", "creator_cover", "post_image", "post_file"} {
		for _, mode := range []string{"cached", "fresh"} {
			input := migrationSaveDefault(kind)
			input.Resource.Mode = mode
			add("reopening/"+kind+"/"+mode, input)
			input.Resource.Repeat = 2
			input.Resource.Media = append(input.Resource.Media, input.Resource.Media[0])
			add("reopening/"+kind+"/"+mode+"_repeat_overwrite", input)
		}
	}
	refs := []struct{ name, product, payload string }{{"zero", "", ""}, {"foreign", "pixiv", `{"k":"post_image","p":"resource-post","a":"resource-image"}`}, {"malformed", "fanbox", `{`}, {"array", "fanbox", `[]`}, {"empty_kind", "fanbox", `{}`}, {"unsupported", "fanbox", `{"k":"unknown"}`}, {"missing_post", "fanbox", `{"k":"post_image","a":"resource-image"}`}, {"missing_creator", "fanbox", `{"k":"creator_cover"}`}}
	for _, ref := range refs {
		for _, destination := range []string{"empty", "whitespace", "unicode_whitespace", "nul", "parent_file", "final_directory"} {
			input := migrationSaveDefault("post_image")
			input.Resource.Kind = ""
			input.Resource.RefProduct = ref.product
			input.Resource.RefPayload = ref.payload
			switch destination {
			case "empty":
				input.Path = ""
			case "whitespace":
				input.Path = " \t\r\n "
			case "unicode_whitespace":
				input.Path = "\u3000\u00a0\u2003"
			case "nul":
				input.Path = "nested/bad\x00name"
			default:
				input.Setup = destination
			}
			add("precedence/"+ref.name+"/"+destination, input)
		}
	}
	for _, destination := range []string{"empty", "whitespace", "unicode_whitespace", "nul", "parent_file", "final_directory"} {
		for _, contextKind := range []string{"", "canceled", "deadline"} {
			input := migrationSaveDefault("post_image")
			input.Resource.Context = contextKind
			switch destination {
			case "empty":
				input.Path = ""
			case "whitespace":
				input.Path = " \t "
			case "unicode_whitespace":
				input.Path = "\u3000"
			case "nul":
				input.Path = "nested/bad\x00name"
			default:
				input.Setup = destination
			}
			add("destination/"+destination+"/context_"+map[string]string{"": "active", "canceled": "canceled", "deadline": "deadline"}[contextKind], input)
		}
	}
	for _, setup := range []string{"old_final", "existing_parent", "final_symlink", "parent_symlink"} {
		input := migrationSaveDefault("post_file")
		input.Setup = setup
		add("filesystem/"+setup+"_successful_save", input)
	}
	for _, path := range []string{"nested/ spaced name.bin ", "single-file", "nested/../normal.bin", "nested/" + strings.Repeat("x", 256), "nested/long-parent/" + strings.Repeat("y", 256)} {
		input := migrationSaveDefault("post_file")
		input.Path = path
		add(fmt.Sprintf("filesystem/path_%d", len(rows)), input)
	}
	for _, status := range []int{200, 201, 204, 206, 299, 304, 400, 401, 403, 404, 429, 500} {
		for _, setup := range []string{"", "parent_file"} {
			input := migrationSaveDefault("post_file")
			input.Resource.Media[0].Status = status
			input.Setup = setup
			add(fmt.Sprintf("status/%d/setup_%s", status, map[string]string{"": "new", "parent_file": "parent_file"}[setup]), input)
		}
	}
	for _, kind := range []string{"header", "html", "marker", "business_word"} {
		input := migrationSaveDefault("post_file")
		input.Resource.Media[0].Status = 403
		switch kind {
		case "header":
			input.Resource.Media[0].Header.Set("Cf-Mitigated", "challenge")
		case "html":
			input.Resource.Media[0].Header.Set("Content-Type", "text/html")
		case "marker":
			input.Resource.Media[0].Data = "prefix cf_chl suffix"
		case "business_word":
			input.Resource.Media[0].Data = `{"challenge":"a business word"}`
		}
		add("challenge/"+kind, input)
	}
	for _, header := range []http.Header{{"Content-Type": {"image/png; charset=binary"}, "Content-Length": {"999999"}}, {"Content-Length": {"-1"}}, {"Content-Length": {"nonsense"}}, {"Content-Type": {"text/plain"}, "Content-Length": {"0"}}, {}} {
		input := migrationSaveDefault("post_file")
		input.Resource.Media[0].Header = header
		add(fmt.Sprintf("representation/headers_%d", len(rows)), input)
	}
	input := migrationSaveDefault("post_file")
	input.Progress = false
	add("representation/no_progress_callback", input)
	input = migrationSaveDefault("post_file")
	input.Resource.Media[0].Wire = []byte{0, 255, 10, 128, 65}
	input.Resource.Media[0].Chunk = 2
	add("stream/binary_exact_bytes", input)
	input = migrationSaveDefault("post_file")
	input.Resource.Media[0].Data = strings.Repeat("x", 65537)
	input.Progress = false
	add("stream/buffer_boundary_65537_bytes", input)
	for _, data := range []string{"", "media"} {
		for _, kind := range []string{"", "EOF", "wrapped_EOF", "unexpected_EOF", "raw", "canceled", "deadline", "canceled_then_deadline", "deadline_then_canceled"} {
			for _, withBytes := range []bool{false, true} {
				input := migrationSaveDefault("post_file")
				input.Setup = "old_final"
				input.Resource.Media[0].Data = data
				input.Resource.Media[0].ReadError = kind
				input.Resource.Media[0].ErrorWithLastBytes = withBytes
				add(fmt.Sprintf("stream/data_%d/%s/last_bytes_%t", len(data), map[string]string{"": "automatic_EOF", "EOF": "EOF", "wrapped_EOF": "wrapped_EOF", "unexpected_EOF": "unexpected_EOF", "raw": "raw", "canceled": "canceled", "deadline": "deadline", "canceled_then_deadline": "canceled_then_deadline", "deadline_then_canceled": "deadline_then_canceled"}[kind], withBytes), input)
			}
		}
	}
	for _, chunk := range []int{1, 2, 3} {
		input := migrationSaveDefault("post_file")
		input.Resource.Media[0].Chunk = chunk
		add(fmt.Sprintf("progress/chunk_%d_before_write", chunk), input)
	}
	for _, zero := range []int{1, 3} {
		input := migrationSaveDefault("post_file")
		input.ZeroReads = zero
		input.Resource.Media[0].Chunk = 2
		add(fmt.Sprintf("stream/%d_zero_reads_then_data", zero), input)
	}
	for _, closeKind := range []string{"raw", "canceled", "deadline", "canceled_then_deadline", "deadline_then_canceled"} {
		for _, readKind := range []string{"", "raw"} {
			input := migrationSaveDefault("post_file")
			input.Resource.Media[0].CloseError = closeKind
			input.Resource.Media[0].ReadError = readKind
			input.Setup = "old_final"
			add("close/"+closeKind+"/read_"+map[string]string{"": "success", "raw": "raw"}[readKind], input)
		}
		input := migrationSaveDefault("post_file")
		input.Resource.Media[0].CloseError = closeKind
		input.Resource.Media[0].Status = 304
		add("close/"+closeKind+"/status_304", input)
	}
	for _, ctxKind := range []string{"canceled", "deadline"} {
		for _, fault := range []string{"", "raw", "canceled", "deadline"} {
			input := migrationSaveDefault("post_image")
			input.Resource.Context = ctxKind
			input.Resource.TransportError = fault
			add("cancellation/before_open/"+ctxKind+"/transport_"+map[string]string{"": "success", "raw": "raw", "canceled": "canceled", "deadline": "deadline"}[fault], input)
		}
		for _, at := range []string{"open", "read", "progress"} {
			for _, readKind := range []string{"", "EOF", "raw", "wrapped_EOF", "unexpected_EOF", "canceled", "deadline"} {
				input := migrationSaveDefault("post_file")
				input.Setup = "old_final"
				input.Resource.Media[0].ReadError = readKind
				input.Resource.Media[0].ErrorWithLastBytes = true
				switch at {
				case "open":
					input.StopOnOpen = ctxKind
				case "read":
					input.StopOnRead = ctxKind
				case "progress":
					input.ProgressAction = map[string]string{"canceled": "cancel", "deadline": "deadline"}[ctxKind]
				}
				add("cancellation/during_"+at+"/"+ctxKind+"/read_"+map[string]string{"": "automatic_EOF", "EOF": "EOF", "raw": "raw", "wrapped_EOF": "wrapped_EOF", "unexpected_EOF": "unexpected_EOF", "canceled": "canceled", "deadline": "deadline"}[readKind], input)
			}
		}
	}
	for _, action := range []string{"remove_temp", "temp_directory", "final_directory"} {
		for _, setup := range []string{"", "old_final"} {
			if action == "final_directory" && setup == "old_final" {
				continue
			}
			input := migrationSaveDefault("post_file")
			input.ProgressAction = action
			input.Setup = setup
			input.Resource.Media[0].Chunk = 2
			add("temp_namespace/"+action+"/setup_"+map[string]string{"": "new", "old_final": "old_final"}[setup], input)
		}
	}
	input = migrationSaveDefault("post_file")
	input.Resource.Media[0].NilBody = true
	input.Resource.Media[0].Length = 5
	add("ownership/nil_body_nonzero_transport_length", input)
	input = migrationSaveDefault("post_file")
	input.Resource.Media[0].NilBody = true
	input.Resource.Media[0].Length = 0
	add("ownership/nil_body_zero_transport_length", input)
	input = migrationSaveDefault("post_file")
	input.Resource.Media[0].CancelOnClose = true
	input.Resource.Media[0].CloseError = "deadline"
	add("ownership/cancel_on_close_success_is_preserved", input)
	for _, mode := range []string{"cached", "fresh"} {
		for _, fault := range []string{"raw", "canceled", "deadline"} {
			input := migrationSaveDefault("post_file")
			input.Resource.Mode = mode
			input.Resource.TransportError = fault
			input.Resource.TransportErrorAtMetadata = true
			add("transport/"+mode+"/"+fault, input)
		}
	}
	for _, sample := range []struct{ name, kind, document string }{{"creator_icon_absent", "creator_icon", migrationResourceCreator("", "https://i.pximg.net/cover.png")}, {"creator_cover_absent", "creator_cover", migrationResourceCreator("https://i.pximg.net/icon.png", "")}, {"post_body_absent", "post_file", migrationResourcePostBody(nil)}, {"post_attachment_absent", "post_image", migrationResourcePost("post_file", "https://downloads.fanbox.cc/other.bin")}, {"forbidden_locator", "post_image", migrationResourcePost("post_image", "https://untrusted.invalid/media.bin")}, {"block_only", "post_image", migrationResourcePostBody(map[string]any{"blocks": []map[string]any{{"type": "image", "imageId": "resource-image"}}, "imageMap": map[string]any{"resource-image": map[string]any{"id": "resource-image", "originalUrl": "https://i.pximg.net/block.png"}}})}} {
		input := migrationSaveDefault(sample.kind)
		input.Resource.Mode = "fresh"
		input.Resource.ReopenDocument = sample.document
		add("metadata/"+sample.name, input)
	}
	for _, status := range []int{401, 403, 404, 500} {
		input := migrationSaveDefault("post_file")
		input.Resource.Mode = "fresh"
		input.Resource.Metadata.Status = status
		add(fmt.Sprintf("metadata/status_%d", status), input)
	}
	for _, kind := range []string{"raw", "canceled", "deadline"} {
		input := migrationSaveDefault("post_image")
		input.Resource.Mode = "fresh"
		input.Resource.Metadata.CloseError = kind
		add("metadata/close_"+kind, input)
	}
	for _, sample := range []struct {
		name        string
		status      int
		read, close string
	}{{"close_precedes_status", 401, "", "raw"}, {"close_precedes_403_read", 403, "raw", "raw"}, {"read_precedes_403_classification", 403, "raw", ""}} {
		input := migrationSaveDefault("post_file")
		input.Resource.Media[0].Status = sample.status
		input.Resource.Media[0].ReadError = sample.read
		input.Resource.Media[0].CloseError = sample.close
		add("status_ownership/"+sample.name, input)
	}
	for _, kind := range []string{"canceled", "deadline"} {
		for _, status := range []int{304, 401, 403} {
			input := migrationSaveDefault("post_file")
			input.Resource.Context = kind
			input.Resource.Media[0].Status = status
			input.Setup = "parent_file"
			add(fmt.Sprintf("status_context/%s/status_%d", kind, status), input)
		}
	}
	for _, setup := range []string{"", "old_final"} {
		input := migrationSaveDefault("post_file")
		input.Setup = setup
		input.PanicOnProgress = true
		add("progress_panic/setup_"+map[string]string{"": "new", "old_final": "old_final"}[setup], input)
	}
	for _, sample := range []struct{ name, url string }{
		{"literal_unicode_path_and_fragment", "https://downloads.fanbox.cc/画像.png#猫"},
		{"literal_space_fragment", "https://downloads.fanbox.cc/space-fragment.bin#with space"},
		{"lowercase_escaped_fragment", "https://downloads.fanbox.cc/escaped-fragment.bin#%2f"},
		{"literal_reserved_fragment", "https://downloads.fanbox.cc/reserved-fragment.bin#!()[]?/@"},
		{"percent_encoded_unicode_path_and_fragment", "https://downloads.fanbox.cc/%e7%94%bb%e5%83%8f.png#%e7%8c%ab"},
		{"unicode_raw_query", "https://downloads.fanbox.cc/query.bin?name=猫&label=画像"},
	} {
		input := migrationSaveDefault("post_file")
		input.Resource.Mode = "fresh"
		input.Resource.ReopenDocument = migrationResourcePost("post_file", sample.url)
		add("uri_serialization/"+sample.name, input)
	}
	return rows
}
func migrationSavePublishedGuard(t *testing.T, root string) map[string]string {
	t.Helper()
	listing, err := exec.Command("git", "-C", root, "ls-tree", "-r", "--name-only", migrationSavePublished, "crates").Output()
	if err != nil {
		t.Fatal(err)
	}
	out := map[string]string{}
	for _, path := range strings.Split(strings.TrimSpace(string(listing)), "\n") {
		if !strings.Contains(path, "/tests/fixtures/") {
			continue
		}
		original, err := exec.Command("git", "-C", root, "show", migrationSavePublished+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, data) {
			t.Fatalf("published fixture changed: %s", path)
		}
		out[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	return out
}
func TestMigrationFanboxSaveResourceFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	production := migrationFanboxJSONBytesFrozenProduction(t, root, reference.SourceCommit)
	sources, stdlib, _ := migrationResourceVerifySources(t, root)
	published := migrationSavePublishedGuard(t, root)
	for _, path := range []string{"internal/storage/file/atomic/atomic.go"} {
		original, err := exec.Command("git", "-C", root, "show", reference.SourceCommit+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil || !bytes.Equal(original, data) {
			t.Fatalf("atomic source changed: %s %v", path, err)
		}
		sources[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	for _, path := range []string{"os/file.go", "os/tempfile.go", "os/path.go", "path/filepath/path.go", "io/fs/walk.go"} {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		stdlib[path] = fmt.Sprintf("%x", sha256.Sum256(data))
	}
	rows := migrationSaveCases()
	if len(rows) != 277 || len(published) != 95 {
		t.Fatalf("save inventory differs: cases=%d published=%d", len(rows), len(published))
	}
	seen := map[string]bool{}
	families := map[string]int{}
	for index := range rows {
		row := &rows[index]
		if seen[row.Name] {
			t.Fatalf("duplicate save case %s", row.Name)
		}
		seen[row.Name] = true
		families[strings.Split(row.Name, "/")[0]]++
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationSaveObserve(t, row.Input) })
	}
	if t.Failed() {
		return
	}
	fixture := map[string]any{"source_commit": reference.SourceCommit, "published_base": migrationSavePublished, "go_version": runtime.Version(), "frozen_go_production_guard": production, "source_sha256": sources, "go_stdlib_sha256": stdlib, "dependencies": reference.Dependencies, "protected_published_fixtures_sha256": published, "case_family_counts": families, "cases": rows,
		"public_operations":   []string{"fanbox.OpenWith", "fanbox.Client.Creator", "fanbox.Client.Post", "fanbox.Client.OpenResource (called by SaveResource)", "fanbox.Client.SaveResource", "sdk.NewResourceRef", "sdk.ParseResourceRef", "sdk.ResourceRefPayload"},
		"evidence":            "Actual frozen public Client.SaveResource through genuine metadata-generated opaque references and unchanged OpenResource/media/atomic implementations. Injected owned in-memory HTTP transports and bodies only; every destination and symlink target is in an owned temporary directory. Captures returned SavedResource fields, full safe error trees, requests, per-body read/close ownership, progress observed before destination write, exact local bytes/hash/modes, overwrite/old-final preservation, contextual cancellation and partial-temp cleanup. Every replay compares serialized bytes exactly.",
		"go_only_projections": []string{"Concrete Go error type names and source read-call topology are Go representations; semantic error classifications/matching, safe cause text, exact bytes, requests, result fields, progress and explicit close counts remain contracts.", "Progress-panic rows recover only outside actual SaveResource, observing its deferred body Close and atomic temp removal; they do not assert OS file-handle closure or Rust future-Drop behavior.", "Owned random absolute destination roots are normalized only in the successful Path and relative filesystem snapshots; random temp basenames are normalized to .atomic-write-$TEMP without dropping observed bytes/size/mode/type. No errors are rewritten.", "Injected custom Context deterministically closes Done and returns canceled/deadline errors at declared boundaries; this proves public context/error precedence without asserting wall-clock scheduler timing."},
		"limitations":         []string{"Owned synthetic HTTP/body and Linux local filesystem only: no external media/accounts/browser, native sockets/decoder/HEAD/HTTP2/multiplex/uploads, host trust or HKCU work. The denied supplemental native probe is not retried or reconstructed.", "Temp namespace mutation is performed only through caller progress after real temp creation. It captures actual chmod/rename/cleanup blockers; filesystem-full, fsync/close OS failure and CreateTemp permission failure under this privileged executor are not claimed.", "Creator/post public metadata mapping and all four resource kinds are real. Solver challenge classification is captured without adding a solver control client or external solve. Native clearance/control contracts remain in existing preserved fixtures.", "Bounded saves do not establish CLI source/page expansion, automatic update startup, physical concurrent stream reads/closes, symlink-safe containment or cross-platform parity. All 434 frozen Go production/module bytes and all fixtures tracked at the published base remain guarded unchanged."}}
	data, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-save-resource.json")
	if *migrationCaptureFanboxSaveResource {
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(want, data) {
		t.Fatal("public SaveResource differs from frozen fixture; review actual behavior before explicit recapture")
	}
	if len(rows) == 0 || len(published) == 0 {
		t.Fatal("save and protected-fixture inventories must be nonempty")
	}
	t.Logf("frozen Go %s public SaveResource: %d rows; %d published fixtures unchanged; bytes=%d sha256=%x; production/module paths=434", runtime.Version(), len(rows), len(published), len(data), sha256.Sum256(data))
}

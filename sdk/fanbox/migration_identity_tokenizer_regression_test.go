package fanbox_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"html"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxTokenizerRegressions = flag.Bool("migration-capture-fanbox-identity-tokenizer-regressions", false, "capture bounded public FANBOX source regressions")

func migrationFanboxTokenizerRegressionCases() []migrationFanboxCase {
	content := html.EscapeString(`{"context":{"user":{"userId":42,"name":"tokenizer regression"}}}`)
	metadata := `<meta name='other' name='metadata' content='` + content + `'>`
	var rows []migrationFanboxCase
	for _, sample := range []struct{ name, document string }{
		{"comment/abrupt_empty", `<!-->` + metadata},
		{"comment/abrupt_dash", `<!--->` + metadata},
		{"comment/bang_end", `<!--x--!>` + metadata},
		{"rawtext/script_fake_quote", `<script><meta content="</script>` + metadata},
		{"rawtext/style_fake_quote", `<style><meta content="</style>` + metadata},
		{"rawtext/textarea_fake_quote", `<textarea><meta content="</textarea>` + metadata},
		{"json/long_s_is_creator", migrationFanboxMetadata(`{"userId":42,"name":"long s","iſCreator":true}`)},
		{"json/long_s_user", `<meta name='metadata' content='` + html.EscapeString(`{"context":{"uſer":{"userId":42,"name":"long s"}}}`) + `'>`},
		{"json/long_s_creator_status", migrationFanboxMetadata(`{"userId":42,"name":"long s","creatorſtatus":true}`)},
	} {
		rows = append(rows, migrationFanboxCase{Name: sample.name, Input: migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{{Status: http.StatusOK, Body: sample.document}}}})
	}
	for _, sample := range []struct{ name, proxy string }{
		{"proxy/port_65536", "http://proxy.example:65536"},
		{"proxy/port_many_digits", "http://proxy.example:999999999999999999999999999999"},
		{"proxy/ipv6_port_65536", "http://[::1]:65536"},
	} {
		rows = append(rows, migrationFanboxCase{Name: sample.name, Input: migrationFanboxInput{Operation: "sdk_open_with", Proxy: sample.proxy}})
	}
	rows = append(rows,
		migrationFanboxCase{Name: "solver/service_port_65536", Input: migrationFanboxInput{Operation: "sdk_open_with", Solver: &fanbox.FlareSolverrOptions{URL: "http://solver.example:65536"}}},
		migrationFanboxCase{Name: "solver/upstream_port_65536", Input: migrationFanboxInput{Operation: "sdk_open_with", Solver: &fanbox.FlareSolverrOptions{URL: "http://solver.example", ProxyURL: "http://proxy.example:65536"}}},
	)
	return rows
}

func TestMigrationFanboxIdentityTokenizerRegressionsFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	publicHTML, err := os.ReadFile(filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-identity-public-html.json"))
	if err != nil {
		t.Fatal(err)
	}
	if fmt.Sprintf("%x", sha256.Sum256(publicHTML)) != "fe0eee2fc4f5d08c717a223f0df043c003458c1893fe55e95f66ac4df043e388" {
		t.Fatal("public HTML 67-row fixture changed")
	}
	rows := migrationFanboxTokenizerRegressionCases()
	if len(rows) != 14 {
		t.Fatal("bounded regression supplement must have exactly 14 rows")
	}
	for index := range rows {
		row := &rows[index]
		t.Run(row.Name, func(t *testing.T) {
			if row.Input.Operation == "sdk_current_user" {
				row.Observation = migrationFanboxObservePublicHTML(t, row.Input)
			} else {
				row.Observation = migrationFanboxObserve(t, row.Input)
			}
		})
	}
	contract := map[string]any{
		"source_commit": reference.SourceCommit, "source_sha256": reference.SourceSHA256,
		"go_version": runtime.Version(), "go_stdlib_sha256": reference.GoStdlibSHA256,
		"dependencies": reference.Dependencies, "go_only_projections": reference.GoOnlyProjections,
		"preserved_fixtures": []map[string]any{
			{"path": "fanbox-identity-protocol.json", "sha256": "cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b", "case_count": 309},
			{"path": "fanbox-identity-public-html.json", "sha256": "fe0eee2fc4f5d08c717a223f0df043c003458c1893fe55e95f66ac4df043e388", "case_count": 67},
		},
		"public_operations": []string{"fanbox.Client.CurrentUser", "fanbox.OpenWith"}, "cases": rows,
		"evidence": "Actual frozen Go public SDK outcomes from fourteen bounded synthetic source-review regressions: HTML comment termination and raw-text fake quotes with duplicate attributes; encoding/json Unicode SimpleFold long-s keys; net/url decimal port validation without a 16-bit range restriction. Existing owned RoundTripper capture supplies response bodies; constructors receive an actual explicit injected transport.",
		"limitations": []string{
			"Only these fourteen added source-review inputs are supplemented; original309 and public67 fixture bytes are protected unchanged.",
			"Owned synthetic response bodies and session values only; no external requests, native transport, live accounts, browser, media, HEAD or upload work.",
			"Out-of-range ports are option-validation witnesses with injected transports; no network dial or control request is performed.",
			"Go-only error types, reader call topology and injected-client pointer ownership remain projections; public DTOs, SDK errors, request details, bytes read and close counts remain semantic.",
		},
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-identity-tokenizer-regressions.json")
	if *migrationCaptureFanboxTokenizerRegressions {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public source regressions differ from frozen Go fixture; review actual behavior before recapture")
	}
	t.Logf("frozen Go %s public source regressions: %d cases; fixture bytes=%d sha256=%x", runtime.Version(), len(rows), len(data), sha256.Sum256(data))
}

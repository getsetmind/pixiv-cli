package fanbox_test

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
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
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/fanbox"
)

var migrationCaptureFanboxPublicHTML = flag.Bool("migration-capture-fanbox-identity-public-html", false, "capture frozen Go public FANBOX CurrentUser HTML behavior")

type migrationFanboxPublicHTMLReference struct {
	SourceCommit      string                `json:"source_commit"`
	SourceSHA256      map[string]string     `json:"source_sha256"`
	GoVersion         string                `json:"go_version"`
	GoStdlibSHA256    map[string]string     `json:"go_stdlib_sha256"`
	Dependencies      []map[string]string   `json:"dependencies"`
	GoOnlyProjections []string              `json:"go_only_projections"`
	Cases             []migrationFanboxCase `json:"cases"`
}

func migrationFanboxPublicHTMLError(err error) map[string]any {
	result := migrationFanboxErrorObservation(err)
	result["code"] = ""
	result["source_message"] = ""
	var classified *sdk.Error
	if errors.As(err, &classified) {
		result["code"] = string(classified.Reason)
	}
	if source := errors.Unwrap(err); source != nil {
		result["source_message"] = source.Error()
	}
	return result
}

func migrationFanboxObservePublicHTML(t *testing.T, input migrationFanboxInput) map[string]any {
	t.Helper()
	ctx, cancel := context.WithCancel(context.WithValue(context.Background(), migrationFanboxContextKey{}, "synthetic-context"))
	defer cancel()
	transport := &migrationFanboxTransport{t: t, steps: input.Steps, requests: []map[string]any{}, bodies: []*migrationFanboxBody{}, cancel: cancel}
	jar := &migrationFanboxJar{}
	redirectCalls := 0
	client := &http.Client{Transport: transport, Jar: jar, CheckRedirect: func(*http.Request, []*http.Request) error {
		redirectCalls++
		return errors.New("synthetic caller redirect rejection")
	}}
	sdkClient, err := fanbox.OpenWith(fanbox.SessionCredentials{FANBOXSESSID: migrationFanboxSessionValue}, fanbox.Options{HTTPClient: client})
	result := map[string]any{"constructed": sdkClient != nil, "constructor_error": migrationFanboxPublicHTMLError(err)}
	var outcomes []map[string]any
	if err == nil {
		user, currentErr := sdkClient.CurrentUser(ctx, fanbox.CurrentUserRequest{})
		outcomes = append(outcomes, map[string]any{"dto": fanbox.ToUserDTO(user), "error": migrationFanboxPublicHTMLError(currentErr)})
		sdkClient.CloseIdleConnections()
		sdkClient.CloseIdleConnections()
	}
	result["outcomes"] = outcomes
	result["requests"] = transport.requests
	result["bodies"] = transport.bodies
	result["close_idle_calls"] = transport.idleCalls
	result["jar_calls"] = jar.calls
	result["redirect_callback_calls"] = redirectCalls
	result["go_only_injected_client_unchanged"] = client.Transport == transport && client.Jar == jar && client.CheckRedirect != nil && client.Timeout == 0
	return result
}

func migrationFanboxPublicHTMLVerifyReference(t *testing.T, root string) migrationFanboxPublicHTMLReference {
	t.Helper()
	protected := map[string]string{
		"sdk/fanbox/migration_identity_protocol_test.go":                "65784f3eaf202bc3799e7ee252c6c75e8e6b3a2ab259ef0b273db9f1e56b7015",
		"crates/pixiv-sdk/tests/fixtures/fanbox-identity-protocol.json": "cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b",
	}
	var reference migrationFanboxPublicHTMLReference
	for path, want := range protected {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("original identity contract changed: %s", path)
		}
		if filepath.Ext(path) == ".json" {
			if err := json.Unmarshal(data, &reference); err != nil {
				t.Fatal(err)
			}
		}
	}
	if reference.SourceCommit != "4b4426487ef18bed276706daec385e0d0a6979f9" || reference.GoVersion != "go1.27.1" || runtime.Version() != reference.GoVersion || len(reference.Cases) != 309 {
		t.Fatal("original identity contract provenance differs from frozen Go")
	}
	for path, want := range reference.SourceSHA256 {
		data, err := os.ReadFile(filepath.Join(root, path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("frozen source changed: %s", path)
		}
		original, err := exec.Command("git", "-C", root, "show", reference.SourceCommit+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(original, data) {
			t.Fatalf("source differs from frozen commit: %s", path)
		}
	}
	for path, want := range reference.GoStdlibSHA256 {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != want {
			t.Fatalf("Go standard library changed: %s", path)
		}
	}
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		t.Fatal("GOMODCACHE must identify the verified offline cache")
	}
	goSum, err := os.ReadFile(filepath.Join(root, "go.sum"))
	if err != nil {
		t.Fatal(err)
	}
	for _, dependency := range reference.Dependencies {
		module, version := dependency["module"], dependency["version"]
		data, err := os.ReadFile(filepath.Join(cache, "cache", "download", module, "@v", version+".zip"))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != dependency["zip_sha256"] || !bytes.Contains(goSum, []byte(module+" "+version+" "+dependency["sum"]+"\n")) {
			t.Fatalf("official dependency provenance changed: %s", module)
		}
		archive, err := zip.NewReader(bytes.NewReader(data), int64(len(data)))
		if err != nil {
			t.Fatal(err)
		}
		for _, file := range archive.File {
			if file.FileInfo().IsDir() {
				continue
			}
			reader, err := file.Open()
			if err != nil {
				t.Fatal(err)
			}
			original, readErr := io.ReadAll(reader)
			closeErr := reader.Close()
			if readErr != nil || closeErr != nil {
				t.Fatalf("archive read: %v / %v", readErr, closeErr)
			}
			extracted, err := os.ReadFile(filepath.Join(cache, filepath.FromSlash(file.Name)))
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(original, extracted) {
				t.Fatalf("dependency source differs from official archive: %s", file.Name)
			}
		}
	}
	return reference
}

func TestMigrationFanboxIdentityPublicHTMLFrozenGo(t *testing.T) {
	root := filepath.Join("..", "..")
	reference := migrationFanboxPublicHTMLVerifyReference(t, root)
	generated := map[string]string{}
	for _, row := range migrationFanboxCases() {
		if row.Input.Operation == "parse_identity" {
			generated[row.Name] = row.Input.Document
		}
	}
	rows := []migrationFanboxCase{}
	seen := map[string]bool{}
	for _, original := range reference.Cases {
		if original.Input.Operation != "parse_identity" {
			continue
		}
		if document, exists := generated[original.Name]; !exists || document != original.Input.Document || seen[original.Name] {
			t.Fatalf("original identity HTML input differs: %s", original.Name)
		}
		seen[original.Name] = true
		input := migrationFanboxInput{Operation: "sdk_current_user", Steps: []migrationFanboxStep{{Status: http.StatusOK, Body: original.Input.Document}}}
		row := migrationFanboxCase{Name: original.Name, Input: input}
		t.Run(row.Name, func(t *testing.T) { row.Observation = migrationFanboxObservePublicHTML(t, row.Input) })
		rows = append(rows, row)
	}
	if len(rows) != 67 || len(generated) != len(rows) {
		t.Fatalf("public HTML supplement must reuse exactly 67 existing inputs, got %d", len(rows))
	}
	contract := map[string]any{
		"source_commit": reference.SourceCommit, "source_sha256": reference.SourceSHA256,
		"go_version": runtime.Version(), "go_stdlib_sha256": reference.GoStdlibSHA256,
		"dependencies": reference.Dependencies, "go_only_projections": reference.GoOnlyProjections,
		"source_fixture":   map[string]any{"path": "fanbox-identity-protocol.json", "sha256": "cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b", "case_count": 309, "reused_operation": "parse_identity", "reused_case_count": 67},
		"public_operation": "fanbox.Client.CurrentUser", "cases": rows,
		"evidence": "Actual frozen Go fanbox.OpenWith and Client.CurrentUser with the unchanged 67 private identity HTML documents supplied as owned 200 response bodies through the existing genuine injected http.Client/RoundTripper harness. Error code, ReasonOf, Error, Unwrap source, DTO, request and body ownership observations come from the public SDK call, never from transformed private observations.",
		"limitations": []string{
			"Only the original 67 identity HTML inputs are supplemented; no added input matrix or changed original 309-row fixture/test bytes.",
			"Owned synthetic response bodies and session values only; no external requests, native transport, live accounts, browser, media, HEAD or upload work.",
			"SDK code is the actual sdk.Error.Reason; source_message is the actual errors.Unwrap result, or an empty string when absent.",
			"Go-only error types, reader call topology and injected-client pointer ownership remain projections; SDK classifications, safe source messages, DTOs, request details, bytes read and close counts remain semantic.",
		},
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-identity-public-html.json")
	if *migrationCaptureFanboxPublicHTML {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("public FANBOX CurrentUser HTML differs from frozen Go fixture; recapture only after reviewing actual Go behavior")
	}
	t.Logf("frozen Go %s public CurrentUser HTML: %d reused cases; fixture bytes=%d sha256=%x", runtime.Version(), len(rows), len(data), sha256.Sum256(data))
}

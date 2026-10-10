package download

import (
	"crypto/sha256"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"testing"
)

var updateRandomEncoding = flag.Bool("migration-update-download-random-encoding", false, "capture frozen Go random MCP number/collection diagnostic encoding")

const randomEncodingOptionsSHA = "65967daedaf09c7d48b40a983a0e6bd0f2f584a03b4def3273fb5ab125be10a9"

func TestMigrationMCPDownloadRandomEncodingMatchesFrozenContract(t *testing.T) {
	base := filepath.Join("..", "..", "..", "..", "..")
	hashes := map[string]string{"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c", "go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e"}
	for path, hash := range randomOptionsHashes {
		hashes[path] = hash
	}
	for path, want := range hashes {
		body, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		frozen, err := exec.Command("git", "-C", base, "show", randomOptionsRef+":"+path).Output()
		if err != nil {
			t.Fatal(err)
		}
		for _, source := range [][]byte{body, frozen} {
			if fmt.Sprintf("%x", sha256.Sum256(source)) != want {
				t.Fatalf("frozen/worktree source changed: %s", path)
			}
		}
	}
	original := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_random_options.json")
	checkOriginal := func() {
		t.Helper()
		body, err := os.ReadFile(original)
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(body)) != randomEncodingOptionsSHA {
			t.Fatal("original77 options fixture identity changed")
		}
	}
	checkOriginal()
	defer checkOriginal()
	rows := []randomOptionsCase{}
	for _, v := range []struct{ name, args string }{
		{"count-decimal-below-scientific-threshold", `{"count":1e20}`},
		{"count-scientific-threshold", `{"count":1e21}`},
		{"count-scientific-above-threshold", `{"count":1e22}`},
		{"count-negative-huge-scientific", `{"count":-1e100}`},
		{"count-nonempty-object", `{"count":{"z":2,"a":1}}`},
		{"count-nested-object", `{"count":{"z":{"b":false,"a":"owned"},"a":true}}`},
		{"count-nonempty-array", `{"count":[1,2]}`},
		{"count-nested-array", `{"count":[[1,2],{"z":false,"a":true}]}`},
		{"count-object-array-values", `{"count":{"z":[1,{"x":"owned"}],"a":null}}`},
		{"count-object-number-values", `{"count":{"z":1e100,"a":1.0}}`},
		{"quality-nonempty-object", `{"quality":{"z":2,"a":1}}`},
		{"quality-nested-object", `{"quality":{"z":[1,"owned",null],"a":{"b":true}}}`},
		{"quality-nonempty-array", `{"quality":["regular","small"]}`},
		{"quality-nested-array", `{"quality":[["regular","small"],{"a":true}]}`},
		{"quality-array-number-values", `{"quality":[1.0,1e21,1e100]}`},
	} {
		rows = append(rows, randomOptionsCase{Name: v.name, Request: randomOptionsRequest(v.args)})
	}
	var tool json.RawMessage
	for i := range rows {
		t.Run(rows[i].Name, func(t *testing.T) {
			random, _ := randomOptionsCapture(t, &rows[i], false, i == 0)
			if i == 0 {
				tool = random
			}
			if rows[i].OpenCalls != 0 {
				t.Fatal("encoding validation crossed saved open")
			}
		})
	}
	actual := struct {
		Source        string              `json:"source"`
		Hashes        map[string]string   `json:"source_hashes"`
		Original      string              `json:"original_options_sha256"`
		Configuration string              `json:"configuration"`
		Boundary      string              `json:"observation_boundary"`
		Tool          json.RawMessage     `json:"tool"`
		Cases         []randomOptionsCase `json:"cases"`
	}{randomOptionsRef, hashes, randomEncodingOptionsSHA, randomOptionsConfiguration, "Actual Register JSON-RPC schema/typed binding diagnostics; raw number spelling preserved; no error-message normalization; original77 options fixture unchanged", tool, rows}
	body, err := json.MarshalIndent(actual, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join(base, "crates", "pixiv-mcp", "tests", "fixtures", "download_random_encoding.json")
	if *updateRandomEncoding {
		if err = os.WriteFile(path, body, 0600); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var gotValue, wantValue any
	if err = json.Unmarshal(body, &gotValue); err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(want, &wantValue); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(gotValue, wantValue) {
		out := filepath.Join(t.TempDir(), "actual.json")
		_ = os.WriteFile(out, body, 0600)
		t.Fatalf("frozen random diagnostic encoding changed: %s", out)
	}
}

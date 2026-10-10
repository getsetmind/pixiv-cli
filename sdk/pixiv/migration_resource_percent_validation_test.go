package pixiv

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var captureResourcePercentValidation = flag.Bool("migration-capture-resource-percent-validation", false, "capture frozen resource percent validation")

func TestMigrationResourcePercentValidationFrozenGo(t *testing.T) {
	type row struct {
		URL       string `json:"url"`
		Reason    string `json:"reason"`
		Operation string `json:"operation"`
		Detail    string `json:"detail"`
		Message   string `json:"message"`
	}
	contract := struct {
		SourceCommit string            `json:"source_commit"`
		SourceSHA256 map[string]string `json:"source_sha256"`
		GoVersion    string            `json:"go_version"`
		Cases        []row             `json:"cases"`
	}{
		SourceCommit: "4b4426487ef18bed276706daec385e0d0a6979f9",
		SourceSHA256: map[string]string{
			"sdk/pixiv/resource.go": "e94cdf3b2d7f67e159bd2481a1c419e887d842c903107767c4004e4f6a529ed8",
			"sdk/pixiv/errors.go":   "9a1830393129ca195ef4d57c0bda17f219f0f750c7293abf5520aa888bfe81a8",
			"sdk/error.go":          "d8e48078c464f18a26cdcf32828e423dd948f17061269b222f82e08a8cee0041",
		},
		GoVersion: runtime.Version(),
	}
	for path, expected := range contract.SourceSHA256 {
		source, err := os.ReadFile(filepath.Join("..", "..", path))
		if err != nil {
			t.Fatal(err)
		}
		if actual := fmt.Sprintf("%x", sha256.Sum256(source)); actual != expected {
			t.Fatalf("frozen source changed: %s: %s", path, actual)
		}
	}
	client := &Client{}
	for _, rawURL := range []string{
		"https://i.pximg.net/%zz",
		"https://i.pximg.net/%",
		"https://i.pximg.net/%2",
		"https://i.pximg.net/%2F",
		"https://i.pximg.net/%25zz",
		"https://i.pximg.net/a.jpg#%zz",
		"https://i.pximg.net/a.jpg#%",
		"https://i.pximg.net/a.jpg?sig=%zz",
		"https://i.pximg.net/a.jpg?sig=%#%20",
		"https://i.pximg.net/a%zz?sig=%20",
		"https://i.pximg.net/a.jpg?sig=%zz#%zz",
		"https://u%zz@i.pximg.net/a.jpg",
		"https://i.pximg.net/a.jpg#%00",
		"https://i.pximg.net/a.jpg?sig=%20#%2F",
	} {
		result := row{URL: rawURL}
		if err := client.validateResourceURL(rawURL); err != nil {
			var classified *sdk.Error
			if !errors.As(err, &classified) {
				t.Fatalf("resource classification: %v", err)
			}
			result.Reason = string(classified.Reason)
			result.Operation = classified.Operation
			result.Detail = classified.Detail
			result.Message = err.Error()
		}
		contract.Cases = append(contract.Cases, result)
	}
	data, err := json.MarshalIndent(contract, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "crates", "pixiv-sdk", "tests", "fixtures", "resource_percent_validation.json")
	if *captureResourcePercentValidation {
		if err := os.WriteFile(path, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	expected, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, expected) {
		t.Fatalf("frozen Go resource percent capture differs:\n%s", data)
	}
	t.Logf("frozen Go %s: %d resource percent cases", contract.GoVersion, len(contract.Cases))
}

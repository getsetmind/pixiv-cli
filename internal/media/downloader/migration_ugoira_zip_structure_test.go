package downloader

import (
	"archive/zip"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

type migrationZIPDirectoryResult struct {
	NamesHex    []string           `json:"names_hex"`
	NamesJSON   string             `json:"names_json"`
	ReaderError string             `json:"reader_error"`
	Report      *UgoiraFrameReport `json:"report"`
	Code        string             `json:"code"`
	Error       string             `json:"error"`
}
type migrationZIPDirectoryCase struct {
	Name     string                       `json:"name"`
	ZIPHex   string                       `json:"zip_hex"`
	Declared []string                     `json:"declared"`
	Expected *migrationZIPDirectoryResult `json:"expected"`
}
type migrationZIPDirectoryFixture struct {
	Reference      string                      `json:"reference"`
	SourceSHA      map[string]string           `json:"source_sha256"`
	GoVersion      string                      `json:"go_version"`
	GoZIPSourceSHA map[string]string           `json:"go_zip_source_sha256"`
	Evidence       string                      `json:"evidence"`
	Deferred       []string                    `json:"deferred"`
	Cases          []migrationZIPDirectoryCase `json:"cases"`
}

func TestMigrationUgoiraZIPDirectoryFixture(t *testing.T) {
	const fixturePath = "../../../crates/pixiv-app/tests/fixtures/ugoira_zip_directory.json"
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	var fixture migrationZIPDirectoryFixture
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	if fixture.Reference != "4b4426487ef18bed276706daec385e0d0a6979f9" || fixture.GoVersion != runtime.Version() || len(fixture.Cases) != 73 {
		t.Fatal("reference, Go runtime, or coverage changed")
	}
	for path, want := range fixture.SourceSHA {
		migrationZIPGuard(t, filepath.Join("../../..", path), want)
	}
	for path, want := range fixture.GoZIPSourceSHA {
		migrationZIPGuard(t, filepath.Join(runtime.GOROOT(), "src/archive/zip", path), want)
	}
	capture := os.Getenv("PIXIV_CAPTURE_ZIP_DIRECTORY") == "1"
	for i := range fixture.Cases {
		c := &fixture.Cases[i]
		t.Run(c.Name, func(t *testing.T) {
			payload, err := hex.DecodeString(c.ZIPHex)
			if err != nil {
				t.Fatal(err)
			}
			result := migrationZIPDirectoryResult{NamesHex: []string{}}
			reader, err := zip.NewReader(bytes.NewReader(payload), int64(len(payload)))
			if err != nil {
				result.ReaderError = err.Error()
			} else {
				names := []string{}
				for _, file := range reader.File {
					result.NamesHex = append(result.NamesHex, hex.EncodeToString([]byte(file.Name)))
					names = append(names, file.Name)
				}
				serialized, err := json.Marshal(names)
				if err != nil {
					t.Fatal(err)
				}
				result.NamesJSON = string(serialized)
			}
			path := filepath.Join(t.TempDir(), "archive.zip")
			if err = os.WriteFile(path, payload, 0600); err != nil {
				t.Fatal(err)
			}
			declared := make([]pixiv.UgoiraFrame, len(c.Declared))
			for j, n := range c.Declared {
				declared[j] = pixiv.UgoiraFrame{Filename: n}
			}
			result.Report, result.Code, err = inspectUgoiraArchive(path, declared)
			if err != nil {
				result.Error = err.Error()
			}
			if capture && c.Expected == nil {
				c.Expected = &result
			} else {
				want, _ := json.Marshal(c.Expected)
				got, _ := json.Marshal(result)
				if !bytes.Equal(want, got) {
					t.Fatalf("want %s\ngot %s", want, got)
				}
			}
		})
	}
	if capture {
		raw, err = json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(fixturePath, append(raw, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
	}
}
func migrationZIPGuard(t *testing.T, path, want string) {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(raw)
	if hex.EncodeToString(sum[:]) != want {
		t.Fatalf("source changed: %s", path)
	}
}

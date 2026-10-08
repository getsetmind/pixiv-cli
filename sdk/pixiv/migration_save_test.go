package pixiv_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateSave = flag.Bool("migration-update-save", false, "capture atomic resource saves from the fixed Go reference")

type migrationSaveBody struct {
	data    []byte
	failure string
	closes  *int
}

func (r *migrationSaveBody) Read(p []byte) (int, error) {
	if len(r.data) > 0 {
		n := min(3, len(p), len(r.data))
		copy(p, r.data[:n])
		r.data = r.data[n:]
		return n, nil
	}
	switch r.failure {
	case "read":
		return 0, errors.New("fixture private URL/path")
	case "cancel":
		return 0, context.Canceled
	case "deadline":
		return 0, context.DeadlineExceeded
	default:
		return 0, io.EOF
	}
}
func (r *migrationSaveBody) Close() error { *r.closes++; return errors.New("fixture close failure") }

func TestMigrationResourceSaveMatchesFrozenFilesProgressAndErrors(t *testing.T) {
	type row struct {
		Name           string             `json:"name"`
		ViaRef         bool               `json:"via_ref"`
		Path           string             `json:"path"`
		URL            string             `json:"url"`
		Status         int                `json:"status"`
		Length         string             `json:"length"`
		Body           string             `json:"body"`
		Failure        string             `json:"failure"`
		Setup          string             `json:"setup"`
		Error          json.RawMessage    `json:"error"`
		Cause          string             `json:"cause"`
		Size           int64              `json:"size"`
		ContentType    string             `json:"content_type"`
		PathMatches    bool               `json:"path_matches"`
		Progress       []sdk.SaveProgress `json:"progress"`
		Calls          int                `json:"calls"`
		Closes         int                `json:"closes"`
		Final          string             `json:"final"`
		Exists         bool               `json:"exists"`
		TemporaryFiles int                `json:"temporary_files"`
	}
	rows := []row{}
	for _, viaRef := range []bool{false, true} {
		prefix := "url-"
		if viaRef {
			prefix = "ref-"
		}
		add := func(name, path, url string, status int, length, body, failure, setup string) {
			rows = append(rows, row{Name: prefix + name, ViaRef: viaRef, Path: path, URL: url, Status: status, Length: length, Body: body, Failure: failure, Setup: setup})
		}
		valid := "https://i.pximg.net/asset.bin?fixture=secret"
		add("nested", "nested/asset.bin", valid, 200, "7", "payload", "", "")
		add("overwrite", "asset.bin", valid, 206, "7", "payload", "", "file")
		add("empty", "asset.bin", valid, 204, "", "", "", "")
		add("unknown-size", "asset.bin", valid, 200, "bad", "payload", "", "")
		add("negative-size", "asset.bin", valid, 200, "-1", "payload", "", "")
		add("read-failure", "asset.bin", valid, 200, "7", "payload", "read", "")
		add("failed-overwrite", "asset.bin", valid, 200, "7", "payload", "read", "file")
		add("cancel-cause", "asset.bin", valid, 200, "7", "payload", "cancel", "")
		add("deadline-cause", "asset.bin", valid, 200, "7", "payload", "deadline", "")
		add("directory-destination", "asset.bin", valid, 200, "7", "payload", "", "directory")
		add("parent-file", "blocked/asset.bin", valid, 200, "7", "payload", "", "parent-file")
		add("empty-path", "", "http://invalid/", 404, "7", "payload", "", "")
		add("blank-path", " \t", valid, 200, "7", "payload", "", "")
		add("bad-url", "asset.bin", "http://i.pximg.net/asset.bin", 200, "7", "payload", "", "")
		for _, status := range []int{199, 300, 304, 401, 404, 500} {
			add("status-"+strconv.Itoa(status), "asset.bin", valid, status, "7", "payload", "", "file")
		}
	}
	for index := range rows {
		row := &rows[index]
		row.Progress = []sdk.SaveProgress{}
		root := t.TempDir()
		dest := row.Path
		if strings.TrimSpace(dest) != "" {
			dest = filepath.Join(root, filepath.FromSlash(dest))
		}
		switch row.Setup {
		case "file":
			if err := os.WriteFile(dest, []byte("previous"), 0o600); err != nil {
				t.Fatal(err)
			}
		case "directory":
			if err := os.Mkdir(dest, 0o700); err != nil {
				t.Fatal(err)
			}
		case "parent-file":
			if err := os.WriteFile(filepath.Dir(dest), []byte("previous"), 0o600); err != nil {
				t.Fatal(err)
			}
		}
		client, err := pixiv.NewWith("fixture-access", pixiv.Options{HTTPClient: &http.Client{Transport: migrationArtworkTransport(func(req *http.Request) (*http.Response, error) {
			if req.URL.Host == "app-api.pixiv.net" {
				return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(`{"ugoira_metadata":{"zip_urls":{"original":"` + row.URL + `"},"frames":[{"file":"0.jpg"}]}}`)), Request: req}, nil
			}
			row.Calls++
			if req.Method != "GET" || req.Header.Get("Referer") != "https://app-api.pixiv.net/" || req.Header.Get("Authorization") != "" || req.Header.Get("Cookie") != "" {
				t.Fatal("unsafe save request")
			}
			return &http.Response{StatusCode: row.Status, Header: http.Header{"Content-Type": {"image/png"}, "Content-Length": {row.Length}}, Body: &migrationSaveBody{data: []byte(row.Body), failure: row.Failure, closes: &row.Closes}, Request: req}, nil
		})}})
		if err != nil {
			t.Fatal(err)
		}
		options := sdk.SaveOptions{Path: dest, Progress: func(value sdk.SaveProgress) {
			row.Progress = append(row.Progress, value)
			if row.Setup == "file" {
				content, err := os.ReadFile(dest)
				if err != nil || string(content) != "previous" {
					t.Fatal("published file during transfer")
				}
			}
		}}
		var saved sdk.SavedResource
		if row.ViaRef {
			ref, refErr := sdk.NewResourceRef("pixiv", []byte(`{"k":"ugoira_archive","id":42,"p":-1,"v":"original"}`))
			if refErr != nil {
				t.Fatal(refErr)
			}
			saved, err = client.SaveResource(context.Background(), ref, options)
		} else {
			saved, err = client.SaveResourceURL(context.Background(), row.URL, options)
		}
		if err != nil {
			row.Error, _ = json.Marshal(err)
			if errors.Is(err, context.Canceled) {
				row.Cause = "cancel"
			}
			if errors.Is(err, context.DeadlineExceeded) {
				row.Cause = "deadline"
			}
		} else {
			row.Size = saved.Size
			row.ContentType = saved.ContentType
			row.PathMatches = saved.Path == dest
		}
		if info, err := os.Stat(dest); err == nil {
			row.Exists = true
			if info.IsDir() {
				row.Final = "<directory>"
			} else {
				data, err := os.ReadFile(dest)
				if err != nil {
					t.Fatal(err)
				}
				row.Final = string(data)
			}
		}
		if err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if strings.HasPrefix(entry.Name(), ".atomic-write-") {
				row.TemporaryFiles++
			}
			return nil
		}); err != nil {
			t.Fatal(err)
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "resource-save.json")
	if *migrationUpdateSave {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("resource saves differ from fixed Go reference")
	}
}

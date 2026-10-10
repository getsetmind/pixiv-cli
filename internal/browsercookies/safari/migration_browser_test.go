//go:build linux && amd64

package safari

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/browsercookies"
)

var captureMigrationBrowserSafari = flag.Bool("migration-capture-browser-safari", false, "capture frozen Go Safari browser contracts")

type migrationSafariCase struct {
	Name      string         `json:"name"`
	Operation string         `json:"operation"`
	Input     map[string]any `json:"input"`
	Output    map[string]any `json:"output"`
}

func migrationSafariError(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}
func migrationSafariCookies(cookies []safariCookie) any {
	if cookies == nil {
		return nil
	}
	out := make([]map[string]any, len(cookies))
	for i, c := range cookies {
		out[i] = map[string]any{"domain_hex": hex.EncodeToString([]byte(c.domain)), "name_hex": hex.EncodeToString([]byte(c.name)), "path_hex": hex.EncodeToString([]byte(c.path)), "value_hex": hex.EncodeToString(c.value), "secure": c.secure, "http_only": c.httpOnly}
	}
	return out
}
func migrationSafariParse(data []byte) (out map[string]any) {
	out = map[string]any{"cookies": nil, "error": "", "panic": ""}
	defer func() {
		if p := recover(); p != nil {
			out["panic"] = fmt.Sprint(p)
		}
	}()
	cookies, err := parseBinaryCookies(data)
	out["cookies"] = migrationSafariCookies(cookies)
	out["error"] = migrationSafariError(err)
	return out
}
func migrationSafariWrite(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestMigrationBrowserSafariContracts(t *testing.T) {
	cases := []migrationSafariCase{}
	plain := safariCookie{domain: ".fanbox.cc", name: "FANBOXSESSID", path: "/", value: []byte("synthetic-safari-session"), secure: true, httpOnly: true}
	valid := buildBinaryCookies(t, plain)
	changed := func(offset int, value uint32) []byte {
		v := bytes.Clone(valid)
		binary.BigEndian.PutUint32(v[offset:offset+4], value)
		return v
	}
	recordOffset := 12 + 24
	type parseSpec struct {
		name string
		data []byte
	}
	parses := []parseSpec{
		{"valid-record", valid},
		{"empty-file", nil},
		{"short-header", []byte("cook\x00\x00\x00")},
		{"wrong-magic", append([]byte("nope"), valid[4:]...)},
		{"zero-pages", []byte("cook\x00\x00\x00\x00")},
		{"zero-pages-ignore-trailing-bytes", []byte("cook\x00\x00\x00\x00ignored")},
		{"valid-file-ignore-trailing-bytes", append(bytes.Clone(valid), []byte("ignored-trailer")...)},
		{"declared-page-count-exceeds-directory-bounds", changed(4, 0xffffffff)},
		{"page-size-too-small", changed(8, 15)},
		{"page-size-exceeds-file", changed(8, uint32(len(valid)))},
		{"page-header-size-too-small", changed(12, 15)},
		{"page-start-too-small", changed(20, 15)},
		{"cookie-offset-table-exceeds-page", changed(16, 0xffffffff)},
		{"cookie-offset-at-end-of-page", changed(28, uint32(len(valid)-12-16))},
		{"cookie-offset-beyond-page", changed(28, 0xffffffff)},
		{"cookie-record-size-zero", func() []byte {
			v := bytes.Clone(valid)
			binary.BigEndian.PutUint16(v[recordOffset:recordOffset+2], 0)
			return v
		}()},
		{"cookie-record-size-exceeds-page", func() []byte {
			v := bytes.Clone(valid)
			binary.BigEndian.PutUint16(v[recordOffset:recordOffset+2], 0xffff)
			return v
		}()},
		{"cookie-record-four-bytes-lacks-first-length", func() []byte {
			v := bytes.Clone(valid)
			binary.BigEndian.PutUint16(v[recordOffset:recordOffset+2], 4)
			return v
		}()},
		{"length-prefixed-field-exceeds-record", func() []byte { v := bytes.Clone(valid); v[recordOffset+4] = 255; return v }()},
		{"record-version-is-ignored", func() []byte {
			v := bytes.Clone(valid)
			binary.BigEndian.PutUint16(v[recordOffset+2:recordOffset+4], 0xffff)
			return v
		}()},
		{"timestamps-are-ignored", func() []byte {
			v := bytes.Clone(valid)
			for i := len(v) - 14; i < len(v)-2; i++ {
				v[i] = 0xff
			}
			return v
		}()},
		{"unknown-flags-ignored", func() []byte { v := bytes.Clone(valid); v[len(v)-2] = 0xff; return v }()},
		{"unknown-final-byte-optional", func() []byte {
			v := bytes.Clone(valid)
			v = v[:len(v)-1]
			binary.BigEndian.PutUint32(v[8:12], uint32(len(v)-12))
			binary.BigEndian.PutUint16(v[recordOffset:recordOffset+2], uint16(len(v)-recordOffset))
			return v
		}()},
		{"flags-byte-required", func() []byte {
			v := bytes.Clone(valid)
			v = v[:len(v)-2]
			binary.BigEndian.PutUint32(v[8:12], uint32(len(v)-12))
			binary.BigEndian.PutUint16(v[recordOffset:recordOffset+2], uint16(len(v)-recordOffset))
			return v
		}()},
		{"invalid-utf8-value-parser-preserves-bytes", buildBinaryCookies(t, safariCookie{domain: ".fanbox.cc", name: "FANBOXSESSID", value: []byte{0xff, 0, 0x80}})},
		{"invalid-utf8-domain-name-path-preserved", buildBinaryCookies(t, safariCookie{domain: string([]byte{0xff}), name: string([]byte{0x80}), path: string([]byte{0xfe}), value: []byte("plain")})},
		{"empty-fields", buildBinaryCookies(t, safariCookie{})},
		{"multiple-interleaved-pages-preserve-order", buildBinaryCookiesPages(t, []safariCookie{plain}, nil, []safariCookie{{domain: "fanbox.cc", name: "FANBOXSESSID", value: []byte("second")}})},
		{"1025-pages-no-arbitrary-cap", buildBinaryCookiesPages(t, make([][]safariCookie, 1025)...)},
		{"missing-next-size-after-full-page-panics", func() []byte {
			v := make([]byte, 28)
			copy(v, "cook")
			binary.BigEndian.PutUint32(v[4:8], 2)
			binary.BigEndian.PutUint32(v[8:12], 16)
			binary.BigEndian.PutUint32(v[12:16], 16)
			binary.BigEndian.PutUint32(v[20:24], 16)
			return v
		}()},
	}
	for _, s := range parses {
		t.Run(s.name, func(t *testing.T) {
			cases = append(cases, migrationSafariCase{s.name, "parse_binary_cookies", map[string]any{"data_hex": hex.EncodeToString(s.data)}, migrationSafariParse(s.data)})
		})
	}
	type fileNode struct {
		Path    string `json:"path"`
		Kind    string `json:"kind"`
		Target  string `json:"target,omitempty"`
		DataHex string `json:"data_hex,omitempty"`
	}
	type profileSpec struct {
		name   string
		nodes  []fileNode
		cancel bool
	}
	candidateNames := []string{"Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies", "Library/Cookies/Cookies.binarycookies"}
	profiles := []profileSpec{
		{name: "discover-missing-both"},
		{name: "discover-first-candidate-wins", nodes: []fileNode{{Path: candidateNames[0], Kind: "file", DataHex: hex.EncodeToString(valid)}, {Path: candidateNames[1], Kind: "file", DataHex: hex.EncodeToString(valid)}}},
		{name: "discover-legacy-fallback", nodes: []fileNode{{Path: candidateNames[1], Kind: "file", DataHex: hex.EncodeToString(valid)}}},
		{name: "discover-directory-falls-back", nodes: []fileNode{{Path: candidateNames[0], Kind: "directory"}, {Path: candidateNames[1], Kind: "file", DataHex: hex.EncodeToString(valid)}}},
		{name: "discover-file-symlink-followed", nodes: []fileNode{{Path: "owned-target", Kind: "file", DataHex: hex.EncodeToString(valid)}, {Path: candidateNames[0], Kind: "symlink", Target: "owned-target"}}},
		{name: "discover-dangling-symlink-falls-back", nodes: []fileNode{{Path: candidateNames[0], Kind: "symlink", Target: "owned-missing"}, {Path: candidateNames[1], Kind: "file", DataHex: hex.EncodeToString(valid)}}},
		{name: "discover-pre-cancel-before-existing-file", nodes: []fileNode{{Path: candidateNames[0], Kind: "file", DataHex: hex.EncodeToString(valid)}}, cancel: true},
	}
	for _, s := range profiles {
		t.Run(s.name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			for _, n := range s.nodes {
				path := filepath.Join(home, n.Path)
				if n.Kind == "directory" {
					if err := os.MkdirAll(path, 0o700); err != nil {
						t.Fatal(err)
					}
				} else if n.Kind == "symlink" {
					if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
						t.Fatal(err)
					}
					if err := os.Symlink(filepath.Join(home, n.Target), path); err != nil {
						t.Fatal(err)
					}
				} else {
					data, err := hex.DecodeString(n.DataHex)
					if err != nil {
						t.Fatal(err)
					}
					migrationSafariWrite(t, path, data)
				}
			}
			p, err := newProvider(nil)
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithCancel(context.Background())
			if s.cancel {
				cancel()
			}
			defer cancel()
			found, err := p.DiscoverProfiles(ctx)
			var outputProfiles any
			if found != nil {
				items := make([]map[string]string, len(found))
				for i, p := range found {
					items[i] = map[string]string{"id": p.ID, "name": p.Name, "path": strings.ReplaceAll(p.Path, home, "$home")}
				}
				outputProfiles = items
			}
			paths := make([]string, len(p.paths))
			for i, path := range p.paths {
				paths[i] = strings.ReplaceAll(path, home, "$home")
			}
			cases = append(cases, migrationSafariCase{s.name, "discover", map[string]any{"nodes": s.nodes, "pre_cancel": s.cancel}, map[string]any{"profiles": outputProfiles, "error": migrationSafariError(err), "candidate_paths": paths, "name": p.Name(), "close_error": migrationSafariError(p.Close())}})
		})
	}
	type readSpec struct {
		name                                   string
		cookies                                []safariCookie
		data                                   []byte
		id                                     string
		query                                  browsercookies.CookieQuery
		cancel, missing, directory, emptyPaths bool
	}
	reads := []readSpec{
		{name: "read-matching-plain", cookies: []safariCookie{plain}},
		{name: "read-no-leading-dot-host", cookies: []safariCookie{{domain: "fanbox.cc", name: "FANBOXSESSID", value: []byte("no-dot")}}},
		{name: "read-name-is-exact", cookies: []safariCookie{{domain: ".fanbox.cc", name: "fanboxsessid", value: []byte("wrong-name")}}},
		{name: "read-only-one-leading-dot-removed", cookies: []safariCookie{{domain: "..fanbox.cc", name: "FANBOXSESSID", value: []byte("double-dot")}}},
		{name: "read-query-host-without-leading-dot", cookies: []safariCookie{plain}, query: browsercookies.CookieQuery{Host: "fanbox.cc", Name: "FANBOXSESSID"}},
		{name: "read-duplicates-path-and-flags-unfiltered", cookies: []safariCookie{plain, {domain: ".fanbox.cc", name: "FANBOXSESSID", path: "/other", value: []byte("second")}}},
		{name: "read-utf8-and-nul", cookies: []safariCookie{{domain: ".fanbox.cc", name: "FANBOXSESSID", value: []byte("合成\x00value")}}},
		{name: "read-empty-matched-value", cookies: []safariCookie{{domain: ".fanbox.cc", name: "FANBOXSESSID", value: []byte{}}}},
		{name: "read-matched-invalid-utf8-classified", cookies: []safariCookie{{domain: ".fanbox.cc", name: "FANBOXSESSID", value: []byte{0xff, 0x80}}}},
		{name: "read-unmatched-invalid-utf8-ignored", cookies: []safariCookie{{domain: ".other.cc", name: "FANBOXSESSID", value: []byte{0xff}}, plain}},
		{name: "read-good-rows-discarded-on-invalid-utf8", cookies: []safariCookie{plain, {domain: ".fanbox.cc", name: "FANBOXSESSID", value: []byte{0xff}}}},
		{name: "read-pre-cancel-still-parses-file", cookies: []safariCookie{plain}, cancel: true},
		{name: "read-invalid-query-before-profile-and-cancel", cookies: []safariCookie{plain}, query: browsercookies.CookieQuery{Host: "bad host", Name: "FANBOXSESSID"}, id: "private-invalid", cancel: true},
		{name: "read-profile-case-is-exact", cookies: []safariCookie{plain}, id: "default"},
		{name: "read-unknown-profile-redacted", cookies: []safariCookie{plain}, id: "private-sensitive-id"},
		{name: "read-missing-file-database-not-found", missing: true},
		{name: "read-directory-first-candidate-database-not-found", directory: true},
		{name: "read-no-candidates-database-not-found", emptyPaths: true},
		{name: "read-invalid-format", data: []byte("invalid owned file")},
		{name: "read-zero-pages-is-empty", data: []byte("cook\x00\x00\x00\x00")},
	}
	for _, s := range reads {
		t.Run(s.name, func(t *testing.T) {
			home := t.TempDir()
			t.Setenv("HOME", home)
			path := filepath.Join(home, candidateNames[0])
			data := s.data
			if data == nil {
				data = buildBinaryCookies(t, s.cookies...)
			}
			if s.directory {
				if err := os.MkdirAll(path, 0o700); err != nil {
					t.Fatal(err)
				}
			} else if !s.missing && !s.emptyPaths {
				migrationSafariWrite(t, path, data)
			}
			p, _ := newProvider(nil)
			if s.emptyPaths {
				p.paths = nil
			}
			id := s.id
			if id == "" {
				id = "Default"
			}
			query := s.query
			if query.Host == "" && query.Name == "" {
				query = browsercookies.DefaultQuery
			}
			ctx, cancel := context.WithCancel(context.Background())
			if s.cancel {
				cancel()
			}
			defer cancel()
			secrets, err := p.Read(ctx, query, id)
			var values any
			if secrets != nil {
				items := make([]string, len(secrets))
				for i, v := range secrets {
					items[i] = hex.EncodeToString([]byte(v.Value()))
				}
				values = items
			}
			cases = append(cases, migrationSafariCase{s.name, "read", map[string]any{"data_hex": hex.EncodeToString(data), "profile_id": id, "query_host": query.Host, "query_name": query.Name, "pre_cancel": s.cancel, "missing_file": s.missing, "directory_file": s.directory, "empty_candidates": s.emptyPaths}, map[string]any{"values_hex": values, "error": migrationSafariError(err)}})
		})
	}
	sources := map[string]string{}
	for _, name := range []string{"go.mod", "go.sum", "internal/browsercookies/browsercookies.go", "internal/browsercookies/safari/safari.go"} {
		body, err := os.ReadFile(filepath.Join("../../..", name))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(body)
		sources[name] = hex.EncodeToString(sum[:])
	}
	pinnedSources := map[string]string{
		"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
		"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
		"internal/browsercookies/browsercookies.go": "1b1e373c3a240f54d8b0d255ad5ae5b753b2eae0c8e506df1615e3b9190a3e6b",
		"internal/browsercookies/safari/safari.go":  "9500d203edb3be536a584492d4c7ed05609b2e34fa88d54dc95a0fea0162cdf9",
	}
	for name, sum := range sources {
		if pinnedSources[name] != sum {
			t.Fatalf("frozen source mismatch: %s", name)
		}
	}
	fixture := map[string]any{"reference": "4b4426487ef18bed276706daec385e0d0a6979f9", "environment": runtime.GOOS + "/" + runtime.GOARCH, "go_version": runtime.Version(), "sources": sources, "scope": "unchanged Go Safari binary parser/discovery/read using owned synthetic files; Linux runtime confirms no OS guard, no real/current-format Safari claim", "cases": cases}
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	target := filepath.Join("../../..", "crates/pixiv-cli/tests/fixtures/browser-safari.json")
	if *captureMigrationBrowserSafari {
		if err := os.WriteFile(target, body, 0o600); err != nil {
			t.Fatal(err)
		}
	} else {
		expected, err := os.ReadFile(target)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(body, expected) {
			t.Fatal("frozen Safari fixture changed; inspect source or environment before explicit capture")
		}
	}
	t.Logf("captured/replayed %d distinct Safari cases; %x", len(cases), sha256.Sum256(body))
}

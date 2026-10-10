package installer

import (
	"archive/tar"
	"archive/zip"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"go/build"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"runtime/debug"
	"sort"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/buildinfo"
	filereplace "github.com/FlanChanXwO/pixiv-cli/internal/storage/file/replace"
	"github.com/FlanChanXwO/pixiv-cli/internal/update/release"
	"github.com/FlanChanXwO/pixiv-cli/internal/update/source"
)

var updaterInstallCapture = flag.Bool("migration-updater-signed-install", false, "capture frozen connected signed installer, source detector and physical cache contracts")

const updaterInstallKeyID = "synthetic-installer-fixture-only"
const updaterInstallOld = "owned old target bytes\n"
const updaterInstallNew = "owned synthetic candidate bytes; never executable\n"

var updaterInstallSources = map[string]string{
	"internal/update/installer/release_installer.go":         "22d49ff1816ebf363ee4d01be3aa826a7a7dc955c985b692da64e3e5b935af87",
	"internal/update/installer/install_source.go":            "0e857e9add4f4f6d469e96861e484a4620c17163ee11182a0b1909a8f8f9dff0",
	"internal/update/installer/release_cache.go":             "9d091159d1f32b328f6244f17b2ba23a704dd7cfd487a907172042b7535512be",
	"internal/update/installer/cache_permissions_unix.go":    "481c28f434b7d235f600ab62fd9d63c993ca6830c0ed0a7c871901d75bef004d",
	"internal/update/installer/cache_permissions_windows.go": "cbe2d6b387769683128a2bdf83d1f0f5e15263fa690b475ad7505fd6cf70a30d",
	"internal/storage/file/replace/replace_nonwindows.go":    "06d2793913c2c2e7e4cdccaaccef39b16663ea62c573575d6bde0f85d04eadf9",
	"internal/storage/file/replace/replace_windows.go":       "3b12dfceef7b011d9aa99a9d6a49cfe3e7bd2fa53a33f41cd65ef6f47d06637b",
	"internal/storage/file/replace/replace_recovery.go":      "53e1d52b2cdad910f32d6c065d1841dca9dd33ad42351af68e670b4f24ed9309",
	"internal/storage/file/replace/replacement_error.go":     "e6346582f605366188f60861dad1025133117de02847ccfc844704ef3d39270d",
	"internal/update/release/release_client.go":              "9de30775c343d211ea62a61f29cb9c02f33c59f71c42613620e61001f8eac8d0",
	"internal/update/release/version_policy.go":              "988c885c7c078f0e2c3d748869a55586db0fcd3ce3fcce8db377a860fdbc219a",
	"internal/update/source/release_sources.go":              "5450d686f836da97e264872e80ba4178bcec7a4217fd8a9eaa85abfccf144bc8",
	"internal/update/source/release_source_selector.go":      "cbfb9bedcd32b391a52ce6a038710e45320dfbd2edf70c746ad72d22b8ad21f0",
	"internal/update/source/github.go":                       "1fd8c8abd4e0855298475ce7455ab775bc4eec6c3fda07a59b76fbb8780c617d",
	"internal/shared/buildinfo/buildinfo.go":                 "f1d207133ada61dc518fd2db18ff918cce20d50b26e5667101578ee8f81468ae",
	"go.mod":                                                 "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
	"go.sum":                                                 "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
}

type updaterInstallEntry struct {
	Name string `json:"name"`
	Body string `json:"body"`
	Type byte   `json:"tar_type"`
	Mode uint32 `json:"mode"`
}
type updaterInstallInput struct {
	Name      string          `json:"name"`
	GOOS      string          `json:"goos"`
	GOARCH    string          `json:"goarch"`
	Release   release.Release `json:"release"`
	Archive   []byte          `json:"archive"`
	Checksums []byte          `json:"checksums"`
	Manifest  []byte          `json:"manifest"`
	Trust     string          `json:"trust"`
	Target    string          `json:"target"`
	Checker   string          `json:"checker"`
	Replacer  string          `json:"replacer"`
	Cancel    string          `json:"cancel"`
	Transport string          `json:"transport"`
	Sources   bool            `json:"sources"`
}
type updaterInstallObservation struct {
	Input                updaterInstallInput `json:"input"`
	Trace                []string            `json:"trace"`
	Error                string              `json:"error"`
	Canceled             bool                `json:"canceled"`
	PreserveSource       bool                `json:"preserve_source"`
	CheckerCause         bool                `json:"checker_cause"`
	ReplacementCause     bool                `json:"replacement_cause"`
	TargetBytes          string              `json:"target_bytes"`
	TargetMode           uint32              `json:"target_mode"`
	Symlink              string              `json:"symlink"`
	Material             []string            `json:"material"`
	ExtractError         string              `json:"extract_error"`
	ExtractedBeforeError bool                `json:"direct_candidate_exists"`
}
type updaterInstallDetectorRow struct {
	Name      string   `json:"name"`
	Version   string   `json:"version"`
	GOOS      string   `json:"goos"`
	Actual    string   `json:"actual"`
	Receipt   string   `json:"receipt"`
	BuildMain string   `json:"build_main"`
	GOBIN     string   `json:"gobin"`
	GOPATH    string   `json:"gopath"`
	Expected  string   `json:"expected"`
	Failure   string   `json:"failure"`
	Trace     []string `json:"trace"`
	Source    string   `json:"source"`
	Error     string   `json:"error"`
}
type updaterInstallCacheRow struct {
	Name           string   `json:"name"`
	Trace          []string `json:"trace"`
	Error          string   `json:"error"`
	Exists         bool     `json:"exists"`
	Bytes          string   `json:"bytes"`
	DirectoryMode  uint32   `json:"directory_mode"`
	FileMode       uint32   `json:"file_mode"`
	Material       []string `json:"material"`
	PreserveSource bool     `json:"preserve_source"`
	Canceled       bool     `json:"canceled"`
}
type updaterInstallFixture struct {
	Reference           string                      `json:"reference"`
	Evidence            string                      `json:"evidence"`
	PublicKey           string                      `json:"synthetic_public_key_hex"`
	ProductionKeyID     string                      `json:"production_public_key_id"`
	ProductionPublicKey string                      `json:"production_public_key_hex"`
	Sources             map[string]string           `json:"sources_sha256"`
	Cases               []updaterInstallObservation `json:"cases"`
	Detectors           []updaterInstallDetectorRow `json:"detectors"`
	Caches              []updaterInstallCacheRow    `json:"caches"`
	Boundaries          []string                    `json:"boundaries"`
}

type updaterInstallTransport func(*http.Request) (*http.Response, error)

func (f updaterInstallTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

type updaterInstallChecker func(context.Context, string, string) error

func (f updaterInstallChecker) Check(c context.Context, p, v string) error { return f(c, p, v) }

type updaterInstallReplacer func(string, string) error

func (f updaterInstallReplacer) Replace(s, t string) error { return f(s, t) }

type updaterInstallPreserve struct{ error }

func (updaterInstallPreserve) PreserveReplacementSource() {}
func (e updaterInstallPreserve) Unwrap() error            { return e.error }

type updaterInstallReadError struct {
	body    *bytes.Reader
	cancel  func()
	failure bool
}

func (r *updaterInstallReadError) Read(p []byte) (int, error) {
	n, e := r.body.Read(p)
	if e == io.EOF {
		if r.cancel != nil {
			r.cancel()
		}
		if r.failure {
			return n, errors.New("synthetic body read failure")
		}
	}
	return n, e
}
func (*updaterInstallReadError) Close() error {
	return errors.New("synthetic ignored body close failure")
}

func updaterInstallKey() ed25519.PrivateKey {
	seed := sha256.Sum256([]byte("pixiv updater synthetic fixture key; no production trust"))
	return ed25519.NewKeyFromSeed(seed[:])
}
func updaterInstallSigned(checksums []byte) []byte {
	m := checksumsManifest{KeyID: updaterInstallKeyID, ChecksumsSHA256: fmt.Sprintf("%x", sha256.Sum256(checksums)), Signature: base64.StdEncoding.EncodeToString(ed25519.Sign(updaterInstallKey(), checksums))}
	b, _ := json.Marshal(m)
	return append(b, '\n')
}
func updaterInstallArchive(t *testing.T, goos string, entries []updaterInstallEntry) []byte {
	t.Helper()
	var b bytes.Buffer
	if goos == "windows" {
		w := zip.NewWriter(&b)
		for _, e := range entries {
			h := &zip.FileHeader{Name: e.Name, Method: zip.Store}
			h.SetMode(os.FileMode(e.Mode))
			f, err := w.CreateHeader(h)
			if err != nil {
				t.Fatal(err)
			}
			if _, err = f.Write([]byte(e.Body)); err != nil {
				t.Fatal(err)
			}
		}
		if err := w.Close(); err != nil {
			t.Fatal(err)
		}
	} else {
		g := gzip.NewWriter(&b)
		w := tar.NewWriter(g)
		for _, e := range entries {
			kind := e.Type
			if kind == 0 {
				kind = tar.TypeReg
			}
			h := &tar.Header{Name: e.Name, Mode: 0o644, Typeflag: kind}
			if kind == tar.TypeReg || kind == tar.TypeRegA {
				h.Size = int64(len(e.Body))
			}
			if kind == tar.TypeSymlink || kind == tar.TypeLink {
				h.Linkname = "owned-link-target"
			}
			if err := w.WriteHeader(h); err != nil {
				t.Fatal(err)
			}
			if h.Size > 0 {
				if _, err := io.WriteString(w, e.Body); err != nil {
					t.Fatal(err)
				}
			}
		}
		if err := w.Close(); err != nil {
			t.Fatal(err)
		}
		if err := g.Close(); err != nil {
			t.Fatal(err)
		}
	}
	return b.Bytes()
}
func updaterInstallBase(t *testing.T, name, goos, goarch string) updaterInstallInput {
	archiveName := releaseArchiveName("1.2.3", goos, goarch)
	archive := updaterInstallArchive(t, goos, []updaterInstallEntry{{Name: "nested/" + releaseBinaryName(goos), Body: updaterInstallNew, Mode: 0o644}, {Name: "LICENSE", Body: "owned license", Mode: 0o644}})
	assets := []release.ReleaseAsset{}
	for _, n := range []string{archiveName, checksumsAssetName, manifestAssetName} {
		assets = append(assets, release.ReleaseAsset{Name: n, DownloadURL: "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/" + n})
	}
	checksums := []byte(fmt.Sprintf("%x  %s\n", sha256.Sum256(archive), archiveName))
	return updaterInstallInput{Name: name, GOOS: goos, GOARCH: goarch, Release: release.Release{TagName: "v1.2.3", Version: "1.2.3", Assets: assets}, Archive: archive, Checksums: checksums, Manifest: updaterInstallSigned(checksums), Trust: "valid", Target: "file", Checker: "success", Replacer: "real"}
}
func updaterInstallResign(i *updaterInstallInput) { i.Manifest = updaterInstallSigned(i.Checksums) }
func updaterInstallSetArchive(i *updaterInstallInput, b []byte) {
	i.Archive = b
	i.Checksums = []byte(fmt.Sprintf("%x  %s\n", sha256.Sum256(b), i.Release.Assets[0].Name))
	updaterInstallResign(i)
}
func updaterInstallCases(t *testing.T) []updaterInstallInput {
	var rows []updaterInstallInput
	add := func(name string, f func(*updaterInstallInput)) {
		i := updaterInstallBase(t, name, "linux", "amd64")
		f(&i)
		rows = append(rows, i)
	}
	for _, goos := range []string{"darwin", "linux", "windows"} {
		for _, arch := range []string{"amd64", "arm64"} {
			rows = append(rows, updaterInstallBase(t, "success_"+goos+"_"+arch, goos, arch))
		}
	}
	add("success_prerelease_build_metadata", func(i *updaterInstallInput) {
		i.Release.TagName = "v1.2.3-rc.1+owned.7"
		i.Release.Version = "1.2.3-rc.1+owned.7"
		for a := range i.Release.Assets {
			if a == 0 {
				i.Release.Assets[a].Name = releaseArchiveName(i.Release.Version, i.GOOS, i.GOARCH)
			}
			i.Release.Assets[a].DownloadURL = "https://github.com/FlanChanXwO/pixiv-cli/releases/download/" + i.Release.TagName + "/" + i.Release.Assets[a].Name
		}
		updaterInstallSetArchive(i, i.Archive)
	})
	add("copied_trust_keys", func(i *updaterInstallInput) { i.Trust = "mutate_after_construction" })
	for _, v := range []string{"empty", "unknown_id", "wrong_length", "wrong_public_key"} {
		v := v
		add("trust_"+v, func(i *updaterInstallInput) { i.Trust = v })
	}
	add("canceled_before_trust_empty", func(i *updaterInstallInput) { i.Trust = "empty"; i.Cancel = "before" })
	add("tag_not_prefixed", func(i *updaterInstallInput) { i.Release.TagName = "1.2.3" })
	add("tag_noncanonical", func(i *updaterInstallInput) { i.Release.TagName = "v01.2.3" })
	add("version_empty", func(i *updaterInstallInput) { i.Release.Version = "" })
	add("version_mismatch", func(i *updaterInstallInput) { i.Release.Version = "1.2.4" })
	add("unknown_platform_missing_asset", func(i *updaterInstallInput) { i.GOOS = "plan9" })
	for a := 0; a < 3; a++ {
		a := a
		for _, kind := range []string{"missing", "duplicate", "empty_url"} {
			kind := kind
			add(fmt.Sprintf("asset_%d_%s", a, kind), func(i *updaterInstallInput) {
				switch kind {
				case "missing":
					i.Release.Assets = append(i.Release.Assets[:a], i.Release.Assets[a+1:]...)
				case "duplicate":
					i.Release.Assets = append(i.Release.Assets, i.Release.Assets[a])
				case "empty_url":
					i.Release.Assets[a].DownloadURL = ""
				}
			})
		}
	}
	for _, kind := range []string{"http", "foreign_host", "foreign_repo", "foreign_tag", "foreign_name", "query", "fragment", "port", "userinfo", "encoded_name", "encoded_tag", "host_case", "path_case", "extra_slash"} {
		kind := kind
		for a := 0; a < 3; a++ {
			a := a
			add(fmt.Sprintf("url_%d_%s", a, kind), func(i *updaterInstallInput) {
				s := i.Release.Assets[a].DownloadURL
				switch kind {
				case "http":
					s = strings.Replace(s, "https:", "http:", 1)
				case "foreign_host":
					s = strings.Replace(s, "github.com", "example.invalid", 1)
				case "foreign_repo":
					s = strings.Replace(s, "pixiv-cli/", "foreign/", 1)
				case "foreign_tag":
					s = strings.Replace(s, "/v1.2.3/", "/v1.2.4/", 1)
				case "foreign_name":
					s += ".different"
				case "query":
					s += "?download=1"
				case "fragment":
					s += "#part"
				case "port":
					s = strings.Replace(s, "github.com", "github.com:443", 1)
				case "userinfo":
					s = strings.Replace(s, "github.com", "user@github.com", 1)
				case "encoded_name":
					s = strings.Replace(s, i.Release.Assets[a].Name, "%"+fmt.Sprintf("%02X", i.Release.Assets[a].Name[0])+i.Release.Assets[a].Name[1:], 1)
				case "encoded_tag":
					s = strings.Replace(s, "v1.2.3", "%761.2.3", 1)
				case "host_case":
					s = strings.Replace(s, "github.com", "GitHub.com", 1)
				case "path_case":
					s = strings.Replace(s, "FlanChanXwO", "flanchanxwo", 1)
				case "extra_slash":
					s = strings.Replace(s, "/releases/", "//releases/", 1)
				}
				i.Release.Assets[a].DownloadURL = s
			})
		}
	}
	for _, kind := range []string{"invalid_json", "null", "unknown_field", "two_values", "trailing_invalid", "wrong_hash", "uppercase_hash", "bad_base64", "base64_no_padding", "short_signature", "wrong_signature", "unknown_key", "duplicate_key_last_valid", "duplicate_key_last_invalid", "folded_known_fields", "base64_crlf", "invalid_utf8_key", "invalid_utf8_unknown_field"} {
		kind := kind
		add("manifest_"+kind, func(i *updaterInstallInput) {
			var m checksumsManifest
			_ = json.Unmarshal(i.Manifest, &m)
			switch kind {
			case "invalid_json":
				i.Manifest = []byte("{")
				return
			case "null":
				i.Manifest = []byte("null")
				return
			case "unknown_field":
				i.Manifest = bytes.Replace(i.Manifest, []byte("{"), []byte(`{"extra":1,`), 1)
				return
			case "two_values":
				i.Manifest = append(i.Manifest, []byte("{}")...)
				return
			case "trailing_invalid":
				i.Manifest = append(i.Manifest, '!')
				return
			case "wrong_hash":
				m.ChecksumsSHA256 = strings.Repeat("0", 64)
			case "uppercase_hash":
				m.ChecksumsSHA256 = strings.ToUpper(m.ChecksumsSHA256)
			case "bad_base64":
				m.Signature = "%%%"
			case "base64_no_padding":
				m.Signature = strings.TrimRight(m.Signature, "=")
			case "short_signature":
				m.Signature = base64.StdEncoding.EncodeToString([]byte("short"))
			case "wrong_signature":
				sig, _ := base64.StdEncoding.DecodeString(m.Signature)
				sig[0] ^= 1
				m.Signature = base64.StdEncoding.EncodeToString(sig)
			case "unknown_key":
				m.KeyID = "unknown"
			case "duplicate_key_last_valid":
				i.Manifest = append([]byte(`{"key_id":"unknown",`), i.Manifest[1:]...)
				return
			case "duplicate_key_last_invalid":
				i.Manifest = bytes.Replace(i.Manifest, []byte("}"), []byte(`,"key_id":"unknown"}`), 1)
				return
			case "folded_known_fields":
				i.Manifest = bytes.ReplaceAll(bytes.ReplaceAll(bytes.ReplaceAll(i.Manifest, []byte("key_id"), []byte("KEY_ID")), []byte("checksums_sha256"), []byte("CHECKSUMS_SHA256")), []byte("signature"), []byte("SIGNATURE"))
				return
			case "base64_crlf":
				m.Signature = m.Signature[:10] + "\r\n" + m.Signature[10:]
			case "invalid_utf8_key":
				i.Manifest = bytes.Replace(i.Manifest, []byte(updaterInstallKeyID), []byte{0xff}, 1)
				return
			case "invalid_utf8_unknown_field":
				i.Manifest = append([]byte{'{', '"', 0xff, '"', ':', '1', ','}, i.Manifest[1:]...)
				return
			}
			i.Manifest, _ = json.Marshal(m)
		})
	}
	for _, field := range []string{"key_id", "checksums_sha256", "signature"} {
		for _, kind := range []string{"duplicate_first_invalid", "duplicate_last_invalid", "duplicate_last_null", "wrong_type", "missing"} {
			field, kind := field, kind
			add("manifest_"+field+"_"+kind, func(i *updaterInstallInput) {
				switch kind {
				case "duplicate_first_invalid":
					i.Manifest = append([]byte(`{"`+field+`":"invalid",`), i.Manifest[1:]...)
				case "duplicate_last_invalid":
					i.Manifest = bytes.Replace(i.Manifest, []byte("}"), []byte(`,"`+field+`":"invalid"}`), 1)
				case "duplicate_last_null":
					i.Manifest = bytes.Replace(i.Manifest, []byte("}"), []byte(`,"`+field+`":null}`), 1)
				case "wrong_type", "missing":
					var m map[string]any
					_ = json.Unmarshal(i.Manifest, &m)
					if kind == "missing" {
						delete(m, field)
					} else {
						m[field] = 7
					}
					i.Manifest, _ = json.Marshal(m)
				}
			})
		}
	}
	add("manifest_base64_noncanonical_pad_bits", func(i *updaterInstallInput) {
		var m checksumsManifest
		_ = json.Unmarshal(i.Manifest, &m)
		alphabet := "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
		at := len(m.Signature) - 3
		idx := strings.IndexByte(alphabet, m.Signature[at])
		m.Signature = m.Signature[:at] + string(alphabet[idx+1]) + m.Signature[at+1:]
		i.Manifest, _ = json.Marshal(m)
	})
	add("checksums_raw_bytes_tamper", func(i *updaterInstallInput) { i.Checksums = append(i.Checksums, '\n') })
	for _, kind := range []string{"empty", "missing", "duplicate", "upper_digest", "short_digest", "nonhex", "star", "double_star", "crlf", "tabs", "unicode_space", "whitespace_line", "one_field", "three_fields", "unrelated_invalid_digest", "unrelated_bad_line", "leading_blank", "selected_digest_mismatch"} {
		kind := kind
		add("checksums_"+kind, func(i *updaterInstallInput) {
			d := fmt.Sprintf("%x", sha256.Sum256(i.Archive))
			n := i.Release.Assets[0].Name
			switch kind {
			case "empty":
				i.Checksums = nil
			case "missing":
				i.Checksums = []byte(d + " other\n")
			case "duplicate":
				i.Checksums = append(i.Checksums, i.Checksums...)
			case "upper_digest":
				i.Checksums = []byte(strings.ToUpper(d) + " " + n + "\n")
			case "short_digest":
				i.Checksums = []byte("0 " + n + "\n")
			case "nonhex":
				i.Checksums = []byte(strings.Repeat("g", 64) + " " + n + "\n")
			case "star":
				i.Checksums = []byte(d + " *" + n + "\n")
			case "double_star":
				i.Checksums = []byte(d + " **" + n + "\n")
			case "crlf":
				i.Checksums = []byte(d + " " + n + "\r\n")
			case "tabs":
				i.Checksums = []byte(d + "\t" + n + "\n")
			case "unicode_space":
				i.Checksums = []byte(d + "\u00a0" + n + "\n")
			case "whitespace_line":
				i.Checksums = append(i.Checksums, []byte("  \t\n")...)
			case "one_field":
				i.Checksums = append(i.Checksums, []byte("single\n")...)
			case "three_fields":
				i.Checksums = append(i.Checksums, []byte("one two three\n")...)
			case "unrelated_invalid_digest":
				i.Checksums = append(i.Checksums, []byte("bad unrelated\n")...)
			case "unrelated_bad_line":
				i.Checksums = append([]byte("bad unrelated extra\n"), i.Checksums...)
			case "leading_blank":
				i.Checksums = append([]byte("\n"), i.Checksums...)
			case "selected_digest_mismatch":
				i.Checksums = []byte(strings.Repeat("0", 64) + " " + n + "\n")
			}
			updaterInstallResign(i)
		})
	}
	add("checksums_all_go_fields_whitespace", func(i *updaterInstallInput) {
		i.Checksums = []byte(fmt.Sprintf("%x", sha256.Sum256(i.Archive)) + "\t\v\f\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000" + i.Release.Assets[0].Name + "\n")
		updaterInstallResign(i)
	})
	for _, goos := range []string{"linux", "windows"} {
		for _, kind := range []string{"missing", "duplicate", "symlink_unrelated", "link_binary", "directory_binary", "fifo_binary", "late_traversal", "late_absolute", "late_backslash", "empty_name", "dot_name", "nested_dotdot", "corrupt", "truncated"} {
			i := updaterInstallBase(t, "archive_"+goos+"_"+kind, goos, "amd64")
			bin := releaseBinaryName(goos)
			entries := []updaterInstallEntry{{Name: bin, Body: updaterInstallNew, Mode: 0o644}}
			switch kind {
			case "missing":
				entries[0].Name = "readme"
			case "duplicate":
				entries = append(entries, entries[0])
			case "symlink_unrelated":
				entries = append(entries, updaterInstallEntry{Name: "unrelated", Type: tar.TypeSymlink, Mode: uint32(os.ModeSymlink | 0o777)})
			case "link_binary":
				entries[0].Type = tar.TypeLink
				entries[0].Mode = uint32(os.ModeSymlink | 0o777)
			case "directory_binary":
				entries[0].Type = tar.TypeDir
				entries[0].Mode = uint32(os.ModeDir | 0o755)
			case "fifo_binary":
				entries[0].Type = tar.TypeFifo
				entries[0].Mode = uint32(os.ModeNamedPipe | 0o600)
			case "late_traversal":
				entries = append(entries, updaterInstallEntry{Name: "safe/../escape", Mode: 0o644})
			case "late_absolute":
				entries = append(entries, updaterInstallEntry{Name: "/escape", Mode: 0o644})
			case "late_backslash":
				entries = append(entries, updaterInstallEntry{Name: "back\\slash", Mode: 0o644})
			case "empty_name":
				entries = append(entries, updaterInstallEntry{Name: "", Mode: 0o644})
			case "dot_name":
				entries = append(entries, updaterInstallEntry{Name: ".", Mode: 0o644})
			case "nested_dotdot":
				entries[0].Name = "nested/../" + bin
			}
			b := updaterInstallArchive(t, goos, entries)
			if kind == "corrupt" {
				b = []byte("not an archive")
			}
			if kind == "truncated" {
				b = b[:len(b)/2]
			}
			updaterInstallSetArchive(&i, b)
			rows = append(rows, i)
		}
	}
	for _, goos := range []string{"linux", "windows"} {
		for _, kind := range []string{"safe_dot_segments", "safe_repeated_slash", "drive_colon", "unrelated_nonregular", "payload_corrupt", "footer_corrupt"} {
			i := updaterInstallBase(t, "archive_"+goos+"_"+kind, goos, "amd64")
			name := releaseBinaryName(goos)
			entries := []updaterInstallEntry{{Name: name, Body: updaterInstallNew, Mode: 0o600}}
			switch kind {
			case "safe_dot_segments":
				entries[0].Name = "nested/./" + name
			case "safe_repeated_slash":
				entries[0].Name = "nested//" + name
			case "drive_colon":
				entries[0].Name = "C:/" + name
			case "unrelated_nonregular":
				entries = append([]updaterInstallEntry{{Name: "unrelated", Type: tar.TypeFifo, Mode: uint32(os.ModeNamedPipe | 0o600)}}, entries...)
			}
			b := updaterInstallArchive(t, goos, entries)
			if kind == "payload_corrupt" {
				if goos == "windows" {
					idx := bytes.Index(b, []byte(updaterInstallNew))
					if idx < 0 {
						t.Fatal("stored ZIP payload not found")
					}
					b[idx] ^= 1
				} else {
					b[len(b)/2] ^= 1
				}
			}
			if kind == "footer_corrupt" {
				if goos == "linux" {
					b[len(b)-8] ^= 1
				} else {
					b[len(b)-22] = 'X'
				}
			}
			updaterInstallSetArchive(&i, b)
			rows = append(rows, i)
		}
	}
	for _, kind := range []string{"missing", "symlink", "dangling_symlink", "executable_error", "parent_missing", "directory"} {
		kind := kind
		add("target_"+kind, func(i *updaterInstallInput) { i.Target = kind })
	}
	for _, kind := range []string{"failure", "extra_workdir_file", "parent_swap_failure", "cancel"} {
		kind := kind
		add("checker_"+kind, func(i *updaterInstallInput) { i.Checker = kind })
	}
	for _, kind := range []string{"failure", "preserve", "wrapped_preserve", "joined_preserve", "cleanup_failure", "committed_error"} {
		kind := kind
		add("replacement_"+kind, func(i *updaterInstallInput) { i.Replacer = kind })
	}
	for _, kind := range []string{"before", "after_archive", "after_checker", "after_stage_create", "after_stage_close", "after_staging"} {
		kind := kind
		add("cancel_"+kind, func(i *updaterInstallInput) { i.Cancel = kind })
	}
	for _, kind := range []string{"checksums_request", "checksums_status", "checksums_read", "manifest_request", "archive_request", "ignored_close"} {
		kind := kind
		add("transport_"+kind, func(i *updaterInstallInput) { i.Transport = kind })
	}
	for _, kind := range []string{"fallback_request", "fallback_status", "fallback_read", "all_failure", "verification_no_fallback"} {
		kind := kind
		add("sources_"+kind, func(i *updaterInstallInput) { i.Sources = true; i.Transport = kind })
	}
	add("sources_checksum_grammar_no_fallback", func(i *updaterInstallInput) {
		i.Sources = true
		i.Checksums = append(i.Checksums, []byte("invalid unrelated line\n")...)
		updaterInstallResign(i)
	})
	add("sources_archive_digest_no_fallback", func(i *updaterInstallInput) {
		i.Sources = true
		i.Checksums = []byte(strings.Repeat("0", 64) + " " + i.Release.Assets[0].Name + "\n")
		updaterInstallResign(i)
	})
	add("sources_archive_parse_no_fallback", func(i *updaterInstallInput) {
		i.Sources = true
		updaterInstallSetArchive(i, []byte("owned signed corrupt archive"))
	})
	return rows
}

var updaterInstallTemporary = regexp.MustCompile(`\.pixiv-update-(stage-)?[0-9]+`)

func updaterInstallNormalize(s, root string) string {
	s = strings.ReplaceAll(s, root, "$ROOT")
	return updaterInstallTemporary.ReplaceAllString(s, ".pixiv-update-${1}TEMP")
}
func updaterInstallMaterial(t *testing.T, root string) []string {
	t.Helper()
	out := []string{}
	err := filepath.WalkDir(root, func(p string, d os.DirEntry, e error) error {
		if e != nil {
			return e
		}
		if p == root {
			return nil
		}
		rel, _ := filepath.Rel(root, p)
		kind := "file"
		if d.IsDir() {
			kind = "dir"
		}
		if d.Type()&os.ModeSymlink != 0 {
			v, _ := os.Readlink(p)
			kind = "symlink:" + v
		}
		b := ""
		if kind == "file" {
			v, _ := os.ReadFile(p)
			b = string(v)
		}
		out = append(out, updaterInstallNormalize(rel, root)+"|"+kind+"|"+b)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	sort.Strings(out)
	return out
}
func updaterInstallRun(t *testing.T, in updaterInstallInput) updaterInstallObservation {
	t.Helper()
	o := updaterInstallObservation{Input: in, Trace: []string{}, Material: []string{}}
	root := t.TempDir()
	target := filepath.Join(root, "bin", releaseBinaryName(in.GOOS))
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatal(err)
	}
	switch in.Target {
	case "missing":
	case "symlink":
		real := filepath.Join(root, "real", releaseBinaryName(in.GOOS))
		if err := os.MkdirAll(filepath.Dir(real), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(real, []byte(updaterInstallOld), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink("../real/"+releaseBinaryName(in.GOOS), target); err != nil {
			t.Fatal(err)
		}
	case "dangling_symlink":
		if err := os.Symlink("absent", target); err != nil {
			t.Fatal(err)
		}
	case "parent_missing":
		target = filepath.Join(root, "absent", "pixiv")
	case "directory":
		if err := os.Mkdir(target, 0o755); err != nil {
			t.Fatal(err)
		}
	default:
		if err := os.WriteFile(target, []byte(updaterInstallOld), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	checkerCause := errors.New("synthetic checker rejected version")
	replacementCause := errors.New("synthetic replacement failed")
	norm := func(s string) string { return updaterInstallNormalize(s, root) }
	record := func(s string) { o.Trace = append(o.Trace, norm(s)) }
	client := &http.Client{Transport: updaterInstallTransport(func(r *http.Request) (*http.Response, error) {
		asset := r.URL.Path[strings.LastIndex(r.URL.Path, "/")+1:]
		record("http " + r.Method + " " + r.URL.String() + " user-agent=" + r.Header.Get("User-Agent"))
		side := "archive"
		body := in.Archive
		if asset == checksumsAssetName {
			side = "checksums"
			body = in.Checksums
		}
		if asset == manifestAssetName {
			side = "manifest"
			body = in.Manifest
		}
		fail := in.Transport == side+"_request" || (in.Sources && r.URL.Host == "preferred.invalid" && (in.Transport == "fallback_request" || in.Transport == "all_failure"))
		if in.Sources && in.Transport == "all_failure" {
			fail = true
		}
		if fail {
			return nil, errors.New("synthetic request failure")
		}
		status := 200
		if in.Transport == side+"_status" || (in.Sources && r.URL.Host == "preferred.invalid" && in.Transport == "fallback_status") {
			status = 503
		}
		reader := &updaterInstallReadError{body: bytes.NewReader(body), failure: in.Transport == side+"_read" || (in.Sources && r.URL.Host == "preferred.invalid" && in.Transport == "fallback_read")}
		if in.Cancel == "after_archive" && side == "archive" {
			reader.cancel = cancel
		}
		if in.Sources && in.Transport == "verification_no_fallback" && side == "manifest" {
			reader.body = bytes.NewReader([]byte("{}"))
		}
		return &http.Response{StatusCode: status, Status: fmt.Sprintf("%d %s", status, http.StatusText(status)), Body: reader, Header: make(http.Header), Request: r}, nil
	})}
	key := append(ed25519.PublicKey(nil), updaterInstallKey().Public().(ed25519.PublicKey)...)
	keys := map[string]ed25519.PublicKey{updaterInstallKeyID: key}
	switch in.Trust {
	case "empty":
		keys = nil
	case "unknown_id":
		keys = map[string]ed25519.PublicKey{"another": key}
	case "wrong_length":
		keys[updaterInstallKeyID] = key[:31]
	case "wrong_public_key":
		key[0] ^= 1
	}
	checker := updaterInstallChecker(func(c context.Context, p, v string) error {
		record("checker path=" + p + " tag=" + v)
		if filepath.Base(p) != releaseBinaryName(in.GOOS) || filepath.Dir(filepath.Dir(p)) != filepath.Dir(updaterInstallResolved(target)) {
			t.Fatal("candidate must be in a workdir beside resolved target")
		}
		b, e := os.ReadFile(p)
		if e != nil {
			t.Fatal(e)
		}
		info, e := os.Stat(p)
		if e != nil {
			t.Fatal(e)
		}
		record(fmt.Sprintf("checker bytes=%q mode=%04o", b, info.Mode().Perm()))
		if string(b) != updaterInstallNew || info.Mode().Perm() != 0o755 {
			t.Fatal("checker sees exact verified source bytes and executable mode")
		}
		if in.Checker == "parent_swap_failure" {
			parent := filepath.Dir(filepath.Dir(p))
			if e := os.Rename(parent, filepath.Join(root, "saved-bin")); e != nil {
				t.Fatal(e)
			}
			if e := os.WriteFile(parent, []byte("owned parent replacement"), 0o600); e != nil {
				t.Fatal(e)
			}
			return checkerCause
		}
		if in.Checker == "failure" {
			return checkerCause
		}
		if in.Checker == "extra_workdir_file" {
			if e := os.WriteFile(filepath.Join(filepath.Dir(p), "keep"), []byte("owned extra"), 0o600); e != nil {
				t.Fatal(e)
			}
		}
		if in.Checker == "cancel" || in.Cancel == "after_checker" {
			cancel()
		}
		return nil
	})
	replacer := updaterInstallReplacer(func(s, d string) error {
		record("replace source=" + s + " target=" + d)
		if filepath.Dir(s) != filepath.Dir(d) || !strings.HasPrefix(filepath.Base(s), ".pixiv-update-stage-") {
			t.Fatal("replacement must use closed same-directory staged source")
		}
		b, e := os.ReadFile(s)
		if e != nil || string(b) != updaterInstallNew {
			t.Fatal("replacement source bytes differ")
		}
		record(fmt.Sprintf("replace bytes=%q", b))
		switch in.Replacer {
		case "failure":
			return replacementCause
		case "preserve":
			return updaterInstallPreserve{replacementCause}
		case "wrapped_preserve":
			return fmt.Errorf("wrapped recovery: %w", updaterInstallPreserve{replacementCause})
		case "joined_preserve":
			return errors.Join(errors.New("second recovery cause"), updaterInstallPreserve{replacementCause})
		case "cleanup_failure":
			if e := os.Remove(s); e != nil {
				t.Fatal(e)
			}
			if e := os.Mkdir(s, 0o700); e != nil {
				t.Fatal(e)
			}
			if e := os.WriteFile(filepath.Join(s, "keep"), []byte("owned recovery material"), 0o600); e != nil {
				t.Fatal(e)
			}
			return replacementCause
		case "committed_error":
			if e := filereplace.ReplaceFile(s, d); e != nil {
				t.Fatal(e)
			}
			return replacementCause
		default:
			return filereplace.ReplaceFile(s, d)
		}
	})
	installer := NewReleaseInstaller(ReleaseInstallerOptions{HTTPClient: client, TrustedKeys: keys, ExecutablePath: func() (string, error) {
		record("executable")
		if in.Target == "executable_error" {
			return "", errors.New("synthetic executable lookup failed")
		}
		return target, nil
	}, GOOS: in.GOOS, GOARCH: in.GOARCH, BinaryChecker: checker, Replacer: replacer}).(*releaseInstaller)
	if in.Trust == "mutate_after_construction" {
		key[0] ^= 1
		delete(keys, updaterInstallKeyID)
	}
	if in.Sources {
		sources, e := source.ParseReleaseSources([]byte("preferred|-|https://preferred.invalid/{url}\nfallback|-|https://fallback.invalid/{url}\n"))
		if e != nil {
			t.Fatal(e)
		}
		probe := &http.Client{Transport: updaterInstallTransport(func(r *http.Request) (*http.Response, error) {
			if r.URL.Host != "preferred.invalid" {
				<-r.Context().Done()
				return nil, r.Context().Err()
			}
			return &http.Response{StatusCode: 200, Status: "200 OK", Body: io.NopCloser(strings.NewReader("probe body does not verify checksums")), Header: make(http.Header), Request: r}, nil
		})}
		installer.sourceSelector = source.NewReleaseSourceSelector(sources, probe)
	}
	if in.Cancel == "before" {
		cancel()
	}
	if in.Cancel == "after_stage_create" {
		installer.afterStagedCreate = func() { record("after_stage_create"); cancel() }
	}
	if in.Cancel == "after_stage_close" {
		installer.afterStagedClose = func() { record("after_stage_close"); cancel() }
	}
	if in.Cancel == "after_staging" {
		installer.afterStaging = func() { record("after_staging"); cancel() }
	}
	err := installer.Install(ctx, in.Release)
	if err != nil {
		o.Error = norm(err.Error())
	}
	o.Canceled = errors.Is(err, context.Canceled)
	o.PreserveSource = filereplace.MustPreserveReplacementSource(err)
	o.CheckerCause = errors.Is(err, checkerCause)
	o.ReplacementCause = errors.Is(err, replacementCause)
	b, e := os.ReadFile(target)
	if e == nil {
		o.TargetBytes = string(b)
	}
	if info, e := os.Stat(target); e == nil {
		o.TargetMode = uint32(info.Mode().Perm())
	}
	if link, e := os.Readlink(target); e == nil {
		o.Symlink = link
	}
	o.Material = updaterInstallMaterial(t, root)
	if strings.HasPrefix(in.Name, "archive_") {
		extractRoot := t.TempDir()
		destination := filepath.Join(extractRoot, releaseBinaryName(in.GOOS))
		e := extractReleaseBinary(in.Archive, in.Release.Assets[0].Name, destination, releaseBinaryName(in.GOOS))
		if e != nil {
			o.ExtractError = updaterInstallNormalize(e.Error(), extractRoot)
		}
		_, e = os.Stat(destination)
		o.ExtractedBeforeError = e == nil
	}
	return o
}
func updaterInstallResolved(p string) string {
	r, e := filepath.EvalSymlinks(p)
	if e == nil {
		return r
	}
	return p
}

func updaterInstallDetectorCases() []updaterInstallDetectorRow {
	rows := []updaterInstallDetectorRow{}
	add := func(name string, f func(*updaterInstallDetectorRow)) {
		r := updaterInstallDetectorRow{Name: name, Version: "v1.2.3", GOOS: "linux", Actual: "$ROOT/release/pixiv", Trace: []string{}}
		f(&r)
		rows = append(rows, r)
	}
	add("development_before_all_io", func(r *updaterInstallDetectorRow) { r.Version = "dev"; r.Failure = "all" })
	add("empty_version_is_release", func(r *updaterInstallDetectorRow) { r.Version = "" })
	add("noncanonical_version_detection_not_parsing", func(r *updaterInstallDetectorRow) { r.Version = "garbage" })
	for _, v := range []string{"executable", "resolve_actual"} {
		v := v
		add("error_"+v, func(r *updaterInstallDetectorRow) { r.Failure = v })
	}
	for _, formula := range []string{"pixiv-cli", "pixiv-cli-beta", "unrecognized"} {
		formula := formula
		add("homebrew_"+formula, func(r *updaterInstallDetectorRow) {
			r.Actual = "$ROOT/Cellar/" + formula + "/1.2.3/bin/pixiv"
			r.Receipt = `{"source":{"path":"https://owned.invalid/Formula/` + formula + `.rb"}}`
			r.BuildMain = pixivCLIImportPath
			r.GOBIN = "$ROOT/other/bin"
		})
	}
	for _, kind := range []string{"receipt_missing", "receipt_invalid_json", "receipt_missing_source", "receipt_bad_extension", "receipt_empty_formula", "receipt_identity_mismatch", "receipt_unknown_fields_allowed", "receipt_whitespace_identity", "not_bin", "not_cellar", "wrong_basename", "receipt_duplicate_last"} {
		kind := kind
		add("homebrew_"+kind, func(r *updaterInstallDetectorRow) {
			r.Actual = "$ROOT/Cellar/pixiv-cli/1.2.3/bin/pixiv"
			r.Receipt = `{"source":{"path":"pixiv-cli.rb"}}`
			switch kind {
			case "receipt_missing":
				r.Failure = "receipt"
			case "receipt_invalid_json":
				r.Receipt = "{"
			case "receipt_missing_source":
				r.Receipt = "{}"
			case "receipt_bad_extension":
				r.Receipt = `{"source":{"path":"pixiv-cli.json"}}`
			case "receipt_empty_formula":
				r.Receipt = `{"source":{"path":".rb"}}`
			case "receipt_identity_mismatch":
				r.Receipt = `{"source":{"path":"pixiv-cli-beta.rb"}}`
			case "receipt_unknown_fields_allowed":
				r.Receipt = `{"extra":true,"source":{"path":"pixiv-cli.rb","extra":0}}`
			case "receipt_whitespace_identity":
				r.Receipt = `{"source":{"path":"  /Formula/pixiv-cli.rb \n"}}`
			case "not_bin":
				r.Actual = "$ROOT/Cellar/pixiv-cli/1.2.3/lib/pixiv"
			case "not_cellar":
				r.Actual = "$ROOT/NotCellar/pixiv-cli/1.2.3/bin/pixiv"
			case "wrong_basename":
				r.Actual = "$ROOT/Cellar/pixiv-cli/1.2.3/bin/other"
			case "receipt_duplicate_last":
				r.Receipt = `{"source":{"path":"wrong.rb","path":"pixiv-cli.rb"}}`
			}
		})
	}
	for _, kind := range []string{"gobin", "gopath_first", "gopath_default", "gopath_empty_first", "build_info_absent", "build_info_nil", "wrong_main", "expected_missing", "expected_error", "expected_different", "windows_equal_fold", "linux_case_sensitive", "symlink_identity", "gobin_over_gopath"} {
		kind := kind
		add("go_"+kind, func(r *updaterInstallDetectorRow) {
			r.BuildMain = pixivCLIImportPath
			r.GOBIN = "$ROOT/go/bin"
			r.Expected = r.Actual
			switch kind {
			case "gopath_first":
				r.GOBIN = ""
				r.GOPATH = "$ROOT/first" + string(os.PathListSeparator) + "$ROOT/second"
			case "gopath_default":
				r.GOBIN = ""
			case "gopath_empty_first":
				r.GOBIN = ""
				r.GOPATH = string(os.PathListSeparator) + "$ROOT/second"
			case "build_info_absent":
				r.Failure = "build_absent"
			case "build_info_nil":
				r.Failure = "build_nil"
			case "wrong_main":
				r.BuildMain = "example.invalid/another"
			case "expected_missing":
				r.Failure = "expected_missing"
			case "expected_error":
				r.Failure = "expected_error"
			case "expected_different":
				r.Expected = "$ROOT/another/pixiv"
			case "windows_equal_fold":
				r.GOOS = "windows"
				r.Actual = "$ROOT/Release/PIXIV.exe"
				r.Expected = "$ROOT/release/pixiv.EXE"
			case "linux_case_sensitive":
				r.Actual = "$ROOT/Release/PIXIV"
				r.Expected = "$ROOT/release/pixiv"
			case "symlink_identity":
				r.Expected = r.Actual
			case "gobin_over_gopath":
				r.GOPATH = "$ROOT/ignored"
			}
		})
	}
	return rows
}
func updaterInstallDetectorRun(t *testing.T, r updaterInstallDetectorRow) updaterInstallDetectorRow {
	t.Helper()
	root := t.TempDir()
	expand := func(p string) string { return strings.ReplaceAll(p, "$ROOT", root) }
	norm := func(p string) string {
		p = strings.ReplaceAll(p, build.Default.GOPATH, "$DEFAULT_GOPATH")
		return updaterInstallNormalize(p, root)
	}
	trace := func(s string) { r.Trace = append(r.Trace, norm(s)) }
	resolved := false
	deps := sourceDetector{goos: r.GOOS, executable: func() (string, error) {
		trace("executable")
		if r.Failure == "executable" || r.Failure == "all" {
			return "", errors.New("synthetic executable failure")
		}
		return expand("$ROOT/raw/pixiv"), nil
	}, evalSymlinks: func(p string) (string, error) {
		trace("resolve " + p)
		if !resolved {
			resolved = true
			if r.Failure == "resolve_actual" {
				return "", errors.New("synthetic actual resolver failure")
			}
			return expand(r.Actual), nil
		}
		if r.Failure == "expected_missing" {
			return "", fmt.Errorf("wrapped missing: %w", os.ErrNotExist)
		}
		if r.Failure == "expected_error" {
			return "", errors.New("synthetic expected resolver failure")
		}
		return expand(r.Expected), nil
	}, readFile: func(p string) ([]byte, error) {
		trace("read " + p)
		if r.Failure == "receipt" {
			return nil, os.ErrNotExist
		}
		return []byte(r.Receipt), nil
	}, readBuildInfo: func() (*debug.BuildInfo, bool) {
		trace("buildinfo")
		if r.Failure == "build_absent" {
			return nil, false
		}
		if r.Failure == "build_nil" {
			return nil, true
		}
		return &debug.BuildInfo{Main: debug.Module{Path: r.BuildMain}}, true
	}, getenv: func(k string) string {
		trace("getenv " + k)
		switch k {
		case "GOBIN":
			return expand(r.GOBIN)
		case "GOPATH":
			return expand(r.GOPATH)
		}
		return ""
	}}
	s, e := detectInstallSource(buildinfo.Info{Version: r.Version}, deps)
	r.Source = string(s)
	if e != nil {
		r.Error = norm(e.Error())
	}
	if r.Name == "development_before_all_io" && len(r.Trace) != 0 {
		t.Fatal("development detection touched system ports")
	}
	return r
}
func updaterInstallCacheRun(t *testing.T, name string) updaterInstallCacheRow {
	t.Helper()
	r := updaterInstallCacheRow{Name: name, Trace: []string{}, Material: []string{}}
	root := t.TempDir()
	dir := filepath.Join(root, "cache")
	target := filepath.Join(dir, "github-releases.json")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	old := "{\"schema\":2,\"checked_at\":\"2026-10-10T00:00:00Z\",\"releases\":[]}\n"
	body := "{\"schema\":2,\"checked_at\":\"2026-10-10T12:00:00Z\",\"releases\":[],\"pages\":[]}\n"
	norm := func(s string) string {
		s = updaterInstallNormalize(s, root)
		return regexp.MustCompile(`\.github-releases-[0-9]+`).ReplaceAllString(s, ".github-releases-TEMP")
	}
	record := func(s string) { r.Trace = append(r.Trace, norm(s)) }
	cache := NewFileReleaseCache(dir, target).(*fileReleaseCache)
	switch name {
	case "read_missing":
	case "read_directory":
		if e := os.MkdirAll(target, 0o755); e != nil {
			t.Fatal(e)
		}
	case "read_bytes":
		if e := os.MkdirAll(dir, 0o755); e != nil {
			t.Fatal(e)
		}
		if e := os.WriteFile(target, []byte("not decoded by physical cache\xff"), 0o644); e != nil {
			t.Fatal(e)
		}
	case "directory_is_file":
		if e := os.WriteFile(dir, []byte("owned blocking file"), 0o600); e != nil {
			t.Fatal(e)
		}
	case "first_write":
	default:
		if e := os.MkdirAll(dir, 0o777); e != nil {
			t.Fatal(e)
		}
		if e := os.Chmod(dir, 0o777); e != nil {
			t.Fatal(e)
		}
		if e := os.WriteFile(target, []byte(old), 0o644); e != nil {
			t.Fatal(e)
		}
	}
	if strings.Contains(name, "canceled") {
		cancel()
	}
	if !strings.HasPrefix(name, "read_") {
		real := cache.replaceFile
		cache.replaceFile = func(s, d string) error {
			record("replace source=" + s + " target=" + d)
			info, e := os.Stat(s)
			if e != nil {
				t.Fatal(e)
			}
			b, e := os.ReadFile(s)
			if e != nil {
				t.Fatal(e)
			}
			oldBytes, _ := os.ReadFile(d)
			record(fmt.Sprintf("source mode=%04o bytes=%q old=%q", info.Mode().Perm(), b, oldBytes))
			if info.Mode().Perm() != 0o600 || string(b) != body {
				t.Fatal("cache source must contain complete private bytes before replace")
			}
			switch name {
			case "replace_failure":
				return errors.New("synthetic cache replacement failed")
			case "preserve_source":
				return fmt.Errorf("wrapped: %w", updaterInstallPreserve{errors.New("synthetic unresolved cache recovery")})
			case "committed_error":
				if e := real(s, d); e != nil {
					t.Fatal(e)
				}
				return errors.New("synthetic committed cache failure")
			case "cleanup_error_after_success", "cleanup_ignored_after_failure":
				if e := os.Remove(s); e != nil {
					t.Fatal(e)
				}
				if e := os.Mkdir(s, 0o700); e != nil {
					t.Fatal(e)
				}
				if e := os.WriteFile(filepath.Join(s, "keep"), []byte("owned cache recovery"), 0o600); e != nil {
					t.Fatal(e)
				}
				if name == "cleanup_ignored_after_failure" {
					return errors.New("synthetic original cache failure")
				}
				return nil
			}
			return real(s, d)
		}
	}
	var e error
	if strings.HasPrefix(name, "read_") {
		var b []byte
		b, r.Exists, e = cache.Read(ctx)
		r.Bytes = string(b)
	} else {
		e = cache.Write(ctx, []byte(body))
		b, exists, readErr := cache.Read(context.Background())
		if readErr == nil {
			r.Exists = exists
			r.Bytes = string(b)
		}
	}
	if e != nil {
		r.Error = norm(e.Error())
	}
	r.PreserveSource = filereplace.MustPreserveReplacementSource(e)
	r.Canceled = errors.Is(e, context.Canceled)
	if info, e := os.Stat(dir); e == nil {
		r.DirectoryMode = uint32(info.Mode().Perm())
	}
	if info, e := os.Stat(target); e == nil {
		r.FileMode = uint32(info.Mode().Perm())
	}
	for _, v := range updaterInstallMaterial(t, root) {
		r.Material = append(r.Material, norm(v))
	}
	sort.Strings(r.Material)
	return r
}
func TestMigrationUpdaterSignedInstallFrozenContracts(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("physical fixture uses Unix filesystems; Windows source-driven recovery is a separate fixture")
	}
	root := filepath.Join("..", "..", "..")
	for p, want := range updaterInstallSources {
		b, e := os.ReadFile(filepath.Join(root, p))
		if e != nil {
			t.Fatal(e)
		}
		if fmt.Sprintf("%x", sha256.Sum256(b)) != want {
			t.Fatalf("frozen source or module changed: %s", p)
		}
	}
	fixture := updaterInstallFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", Evidence: "genuine NewReleaseInstaller.Install; deterministic test-only Ed25519 key, canonical official URL in-memory HTTP transport, owned synthetic archives/targets, injected public BinaryChecker; no candidate or external program executed", PublicKey: hex.EncodeToString(updaterInstallKey().Public().(ed25519.PublicKey)), ProductionKeyID: ReleaseSigningKeyID, ProductionPublicKey: hex.EncodeToString(ReleaseSigningPublicKey[:]), Sources: updaterInstallSources, Cases: []updaterInstallObservation{}, Detectors: []updaterInstallDetectorRow{}, Caches: []updaterInstallCacheRow{}, Boundaries: []string{"all six platform names use synthetic ZIP/TAR bytes on a Unix filesystem; not six-platform runtime evidence", "BinaryChecker mock observes exact verified bytes, candidate path,0755 mode and expected tag; native --version process contract is a separate authorized owned-helper gate", "source-detector system ports are synthetic; Windows EqualFold is the genuine string-comparison branch with Unix filepath parsing, not native Windows path/runtime proof", "Go main-module build metadata has no automatic Rust-native equivalent; preserve routing and document native mapping", "cache fsync/close/order proven by frozen source SHA and physical bytes/mode at replace seam; forced syscall sync/close failures not injected", "signed TAR/GZIP footer-CRC corruption is accepted by the frozen Go traversal stopping at TAR EOF; ZIP payload CRC corruption is rejected; only bounded captured bytes claim this behavior", "Windows replacement API recovery and retained .old require separate source-driven fixture; native Windows execution/ACL and cross-platform runtime remain unverified", "release-client cache schema selection/protocol belongs to the connected client fixture; physical cache stores and reads raw bytes"}}
	for _, i := range updaterInstallCases(t) {
		o := updaterInstallRun(t, i)
		updaterInstallAssert(t, o)
		fixture.Cases = append(fixture.Cases, o)
	}
	for _, r := range updaterInstallDetectorCases() {
		fixture.Detectors = append(fixture.Detectors, updaterInstallDetectorRun(t, r))
	}
	for _, n := range []string{"read_missing", "read_bytes", "read_directory", "read_canceled", "first_write", "overwrite_tightens_permissions", "directory_is_file", "write_canceled", "replace_failure", "preserve_source", "committed_error", "cleanup_error_after_success", "cleanup_ignored_after_failure"} {
		fixture.Caches = append(fixture.Caches, updaterInstallCacheRun(t, n))
	}
	data, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	data = append(data, '\n')
	p := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "updater-signed-install.json")
	if *updaterInstallCapture {
		if e := os.WriteFile(p, data, 0o644); e != nil {
			t.Fatal(e)
		}
	} else {
		want, e := os.ReadFile(p)
		if e != nil {
			t.Fatal(e)
		}
		if !bytes.Equal(want, data) {
			actual := filepath.Join(t.TempDir(), "actual.json")
			_ = os.WriteFile(actual, data, 0o600)
			t.Fatalf("connected updater installer fixture differs; actual=%s", actual)
		}
	}
	t.Logf("signed install=%d detector=%d physical cache=%d", len(fixture.Cases), len(fixture.Detectors), len(fixture.Caches))
}

func updaterInstallAssert(t *testing.T, o updaterInstallObservation) {
	t.Helper()
	n := o.Input.Name
	accepted := strings.HasPrefix(n, "success_") || n == "copied_trust_keys" || n == "manifest_duplicate_key_last_valid" || n == "manifest_folded_known_fields" || n == "manifest_base64_crlf" || n == "manifest_base64_noncanonical_pad_bits" || n == "checksums_star" || n == "checksums_crlf" || n == "checksums_tabs" || n == "checksums_unicode_space" || n == "checksums_all_go_fields_whitespace" || n == "checksums_unrelated_invalid_digest" || n == "checksums_leading_blank" || n == "target_missing" || n == "target_symlink" || n == "transport_ignored_close" || strings.HasPrefix(n, "sources_fallback_")
	if strings.HasPrefix(n, "manifest_") && (strings.HasSuffix(n, "_duplicate_first_invalid") || strings.HasSuffix(n, "_duplicate_last_null")) {
		accepted = true
	}
	if strings.HasPrefix(n, "archive_") {
		accepted = strings.HasSuffix(n, "_safe_dot_segments") || strings.HasSuffix(n, "_safe_repeated_slash") || strings.HasSuffix(n, "_drive_colon") || strings.HasSuffix(n, "_unrelated_nonregular") || n == "archive_linux_footer_corrupt"
	}
	if (o.Error == "") != accepted {
		t.Fatalf("%s: unexpected outcome: %s", n, o.Error)
	}
	hasArchive, hasChecker, hasReplace := false, false, false
	requestCount := 0
	for _, v := range o.Trace {
		if strings.HasPrefix(v, "http ") {
			requestCount++
			if strings.Contains(v, o.Input.Release.Assets[0].Name) && strings.HasPrefix(v, "http GET") {
				hasArchive = true
			}
		}
		if strings.HasPrefix(v, "checker path=") {
			hasChecker = true
		}
		if strings.HasPrefix(v, "replace source=") {
			hasReplace = true
		}
	}
	preIO := strings.HasPrefix(n, "url_") || strings.HasPrefix(n, "asset_") || strings.HasPrefix(n, "tag_") || strings.HasPrefix(n, "version_") || n == "unknown_platform_missing_asset" || n == "trust_empty" || n == "canceled_before_trust_empty" || n == "cancel_before"
	if preIO && len(o.Trace) != 0 {
		t.Fatalf("%s: rejected input touched I/O: %v", n, o.Trace)
	}
	if (strings.HasPrefix(n, "manifest_") || strings.HasPrefix(n, "checksums_") || strings.HasPrefix(n, "trust_")) && o.Error != "" && n != "checksums_selected_digest_mismatch" && hasArchive {
		t.Fatalf("%s: archive fetched before sidecar verification", n)
	}
	if o.Error != "" && strings.HasPrefix(n, "archive_") && (hasChecker || hasReplace) {
		t.Fatalf("%s: malformed archive reached checker or replacement", n)
	}
	if accepted {
		if !hasArchive || !hasChecker || !hasReplace || o.TargetBytes != updaterInstallNew || o.TargetMode != 0o755 {
			t.Fatalf("%s: success not connected through verified candidate and replacement", n)
		}
	}
	if n == "target_symlink" && o.Symlink != "../real/pixiv" {
		t.Fatal("executable symlink was not preserved")
	}
	if n == "sources_verification_no_fallback" && (requestCount != 2 || hasArchive) {
		t.Fatal("verification failure retried another source or fetched archive")
	}
	if n == "sources_checksum_grammar_no_fallback" && requestCount != 2 {
		t.Fatal("checksum grammar failure triggered source fallback")
	}
	if (n == "sources_archive_digest_no_fallback" || n == "sources_archive_parse_no_fallback") && requestCount != 3 {
		t.Fatal("archive verification failure triggered source fallback")
	}
	if n == "checker_parent_swap_failure" && (!o.CheckerCause || !strings.Contains(o.Error, "remove update temporary directory") || !strings.Contains(strings.Join(o.Material, "\n"), "saved-bin/pixiv|file|"+updaterInstallOld)) {
		t.Fatal("workdir cleanup failure did not join original error and retain relocated owned material")
	}
	if strings.HasPrefix(n, "sources_fallback_") && requestCount != 6 {
		t.Fatal("every sidecar/archive must reuse preferred-then-fallback order")
	}
	if strings.HasPrefix(n, "cancel_") && n != "canceled_before_trust_empty" {
		if !o.Canceled || hasReplace {
			t.Fatalf("%s: cancellation did not stop before replacement", n)
		}
	}
	if strings.Contains(n, "preserve") {
		if !o.PreserveSource || !o.ReplacementCause || !strings.Contains(strings.Join(o.Material, "\n"), ".pixiv-update-stage-TEMP|file|"+updaterInstallNew) {
			t.Fatalf("%s: unresolved recovery source lost", n)
		}
	}
	if n == "replacement_committed_error" {
		if o.TargetBytes != updaterInstallNew || !o.ReplacementCause {
			t.Fatal("committed error was misrepresented as unchanged")
		}
	} else if o.Error != "" && o.Input.Target == "file" && n != "checker_parent_swap_failure" && o.TargetBytes != updaterInstallOld {
		t.Fatalf("%s: failed installation changed old target", n)
	}
	if n == "replacement_cleanup_failure" && (!o.ReplacementCause || !strings.Contains(o.Error, "remove staged update file")) {
		t.Fatal("cleanup error not joined with replacement cause")
	}
	if strings.HasPrefix(n, "archive_") && (strings.Contains(n, "late_") || strings.HasSuffix(n, "_empty_name") || strings.HasSuffix(n, "_dot_name") || strings.HasSuffix(n, "_nested_dotdot")) && o.ExtractedBeforeError {
		t.Fatalf("%s: entire archive path prescan wrote candidate", n)
	}
}

package installer

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/update/release"
)

var updaterOwnedCapture = flag.Bool("migration-updater-owned-preflight", false, "capture native Linux preflight using only the visible owned helper")

const updaterOwnedModule = "module example.invalid/pixiv-updater-owned-preflight\n\ngo 1.27.1\n"
const updaterOwnedSource = `package main

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

type config struct {
	Mode string
	Stdout string
	Stderr string
	StdoutRepeat int ` + "`json:\"stdout_repeat\"`" + `
	StderrRepeat int ` + "`json:\"stderr_repeat\"`" + `
	Status int
}

type event struct {
	Event string ` + "`json:\"event\"`" + `
	Args []string ` + "`json:\"args,omitempty\"`" + `
	Mode string ` + "`json:\"mode,omitempty\"`" + `
	CandidateSHA256 string ` + "`json:\"candidate_sha256,omitempty\"`" + `
	CandidateMode uint32 ` + "`json:\"candidate_mode,omitempty\"`" + `
	TargetSHA256 string ` + "`json:\"target_sha256,omitempty\"`" + `
}

func main() {
	root := filepath.Dir(filepath.Dir(os.Args[0]))
	control, err := os.ReadFile(filepath.Join(root, "owned-control.json"))
	if err != nil { os.Exit(90) }
	var cfg config
	if json.Unmarshal(control, &cfg) != nil { os.Exit(91) }
	log := func(e event) {
		f, err := os.OpenFile(filepath.Join(root, "owned-effects.jsonl"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
		if err != nil { os.Exit(92) }
		if json.NewEncoder(f).Encode(e) != nil { os.Exit(93) }
		if f.Close() != nil { os.Exit(94) }
	}
	candidate, err := os.ReadFile(os.Args[0])
	if err != nil { os.Exit(95) }
	info, err := os.Stat(os.Args[0])
	if err != nil { os.Exit(96) }
	target, err := os.ReadFile(filepath.Join(root, "target"))
	if err != nil { os.Exit(97) }
	log(event{Event:"started", Args:os.Args[1:], Mode:cfg.Mode,
		CandidateSHA256:fmt.Sprintf("%x", sha256.Sum256(candidate)), CandidateMode:uint32(info.Mode().Perm()),
		TargetSHA256:fmt.Sprintf("%x", sha256.Sum256(target))})
	stdout := cfg.Stdout
	stderr := cfg.Stderr
	if cfg.StdoutRepeat > 0 { stdout = strings.Repeat("q", cfg.StdoutRepeat) }
	if cfg.StderrRepeat > 0 { stderr = strings.Repeat("e", cfg.StderrRepeat) }
	if _, err := io.WriteString(os.Stderr, stderr); err != nil { os.Exit(98) }
	if _, err := io.WriteString(os.Stdout, stdout); err != nil { os.Exit(99) }
	if cfg.Mode == "block" {
		log(event{Event:"blocked"})
		for n := 0; ; n++ {
			if os.WriteFile(filepath.Join(root, "owned-heartbeat"), []byte(strconv.Itoa(n)), 0600) != nil { os.Exit(100) }
			time.Sleep(10*time.Millisecond)
		}
	}
	log(event{Event:"finished"})
	os.Exit(cfg.Status)
}
`

var updaterOwnedSources = map[string]string{
	"internal/update/installer/release_installer.go":      "22d49ff1816ebf363ee4d01be3aa826a7a7dc955c985b692da64e3e5b935af87",
	"internal/update/release/release_client.go":           "9de30775c343d211ea62a61f29cb9c02f33c59f71c42613620e61001f8eac8d0",
	"internal/update/release/version_policy.go":           "988c885c7c078f0e2c3d748869a55586db0fcd3ce3fcce8db377a860fdbc219a",
	"internal/update/source/github.go":                    "1fd8c8abd4e0855298475ce7455ab775bc4eec6c3fda07a59b76fbb8780c617d",
	"internal/update/source/release_source_selector.go":   "cbfb9bedcd32b391a52ce6a038710e45320dfbd2edf70c746ad72d22b8ad21f0",
	"internal/storage/file/replace/replace_nonwindows.go": "06d2793913c2c2e7e4cdccaaccef39b16663ea62c573575d6bde0f85d04eadf9",
	"internal/storage/file/replace/replace_recovery.go":   "53e1d52b2cdad910f32d6c065d1841dca9dd33ad42351af68e670b4f24ed9309",
	"internal/storage/file/replace/replacement_error.go":  "e6346582f605366188f60861dad1025133117de02847ccfc844704ef3d39270d",
	"go.mod": "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
	"go.sum": "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
}

type updaterOwnedConfig struct {
	Mode         string `json:"mode"`
	Stdout       string `json:"stdout"`
	Stderr       string `json:"stderr"`
	StdoutRepeat int    `json:"stdout_repeat"`
	StderrRepeat int    `json:"stderr_repeat"`
	Status       int    `json:"status"`
}
type updaterOwnedCase struct {
	Name     string             `json:"name"`
	Boundary string             `json:"boundary"`
	Control  updaterOwnedConfig `json:"control"`
	Context  string             `json:"context"`
	Barrier  string             `json:"barrier"`
	Symlink  bool               `json:"symlink"`
}
type updaterOwnedBytes struct {
	Length int    `json:"length"`
	SHA256 string `json:"sha256"`
	Text   string `json:"text,omitempty"`
	Prefix string `json:"prefix,omitempty"`
	Suffix string `json:"suffix,omitempty"`
}
type updaterOwnedEvent struct {
	Event           string   `json:"event"`
	Args            []string `json:"args,omitempty"`
	Mode            string   `json:"mode,omitempty"`
	CandidateSHA256 string   `json:"candidate_sha256,omitempty"`
	CandidateMode   uint32   `json:"candidate_mode,omitempty"`
	TargetSHA256    string   `json:"target_sha256,omitempty"`
}
type updaterOwnedError struct {
	Message      updaterOwnedBytes `json:"message"`
	IsCanceled   bool              `json:"is_canceled"`
	IsDeadline   bool              `json:"is_deadline"`
	ExitError    bool              `json:"exit_error"`
	ExitCode     int               `json:"exit_code"`
	ProcessState string            `json:"process_state"`
	Stderr       updaterOwnedBytes `json:"stderr"`
}
type updaterOwnedRow struct {
	Input                         updaterOwnedCase    `json:"input"`
	Requests                      []string            `json:"requests"`
	Effects                       []updaterOwnedEvent `json:"effects"`
	Error                         updaterOwnedError   `json:"error"`
	ContextError                  string              `json:"context_error"`
	Target                        updaterOwnedBytes   `json:"target"`
	TargetMode                    uint32              `json:"target_mode"`
	SymlinkTarget                 string              `json:"symlink_target"`
	TransientMaterial             []string            `json:"transient_material"`
	HeartbeatAdvancedBeforeCancel bool                `json:"heartbeat_advanced_before_cancel"`
	HeartbeatQuietAfterReturn     bool                `json:"heartbeat_quiet_after_return"`
	BoundedReturn                 bool                `json:"bounded_return"`
}
type updaterOwnedHelper struct {
	Source                string   `json:"source"`
	SourceSHA256          string   `json:"source_sha256"`
	Module                string   `json:"module"`
	ModuleSHA256          string   `json:"module_sha256"`
	GoVersion             string   `json:"go_version"`
	GOOS                  string   `json:"goos"`
	GOARCH                string   `json:"goarch"`
	CompilerSHA256        string   `json:"compiler_sha256"`
	ExecStdlibSHA256      string   `json:"exec_stdlib_sha256"`
	BuildArguments        []string `json:"build_arguments"`
	BuildEnvironment      []string `json:"build_environment"`
	BinaryLength          int      `json:"binary_length"`
	BinarySHA256          string   `json:"binary_sha256"`
	IndependentBuildEqual bool     `json:"independent_build_equal"`
}
type updaterOwnedFixture struct {
	Reference          string             `json:"reference"`
	Evidence           string             `json:"evidence"`
	Sources            map[string]string  `json:"sources_sha256"`
	Helper             updaterOwnedHelper `json:"helper"`
	SyntheticPublicKey string             `json:"synthetic_public_key_hex"`
	Rows               []updaterOwnedRow  `json:"rows"`
	Boundaries         []string           `json:"boundaries"`
}
type updaterOwnedTransport func(*http.Request) (*http.Response, error)

func (f updaterOwnedTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func updaterOwnedHash(b []byte) string { return fmt.Sprintf("%x", sha256.Sum256(b)) }
func updaterOwnedSummary(b []byte) updaterOwnedBytes {
	r := updaterOwnedBytes{Length: len(b), SHA256: updaterOwnedHash(b)}
	if len(b) <= 4096 {
		r.Text = string(b)
	} else {
		r.Prefix = string(b[:128])
		r.Suffix = string(b[len(b)-128:])
	}
	return r
}
func updaterOwnedRead(t *testing.T, p string) []byte {
	t.Helper()
	b, e := os.ReadFile(p)
	if e != nil {
		t.Fatal(e)
	}
	return b
}
func updaterOwnedWrite(t *testing.T, p string, b []byte, mode os.FileMode) {
	t.Helper()
	if e := os.WriteFile(p, b, mode); e != nil {
		t.Fatal(e)
	}
}
func updaterOwnedBuild(t *testing.T) ([]byte, updaterOwnedHelper) {
	t.Helper()
	compiler := filepath.Join(runtime.GOROOT(), "bin", "go")
	buildArgs := []string{"build", "-trimpath", "-buildvcs=false", "-ldflags=-buildid=", "-o", "OWNED_MODULE/helper", "."}
	buildEnv := []string{"CGO_ENABLED=0", "GOOS=linux", "GOARCH=amd64", "GOENV=off", "GOWORK=off", "GOPROXY=off", "GOSUMDB=off", "GOTOOLCHAIN=local", "GOAMD64=v1", "GOEXPERIMENT=", "GOFLAGS="}
	overrides := make(map[string]bool)
	for _, env := range buildEnv {
		overrides[strings.SplitN(env, "=", 2)[0]] = true
	}
	build := func() []byte {
		dir := t.TempDir()
		updaterOwnedWrite(t, filepath.Join(dir, "go.mod"), []byte(updaterOwnedModule), 0600)
		updaterOwnedWrite(t, filepath.Join(dir, "main.go"), []byte(updaterOwnedSource), 0600)
		output := filepath.Join(dir, "helper")
		args := append([]string(nil), buildArgs...)
		args[5] = output
		command := exec.Command(compiler, args...)
		command.Dir = dir
		for _, env := range os.Environ() {
			key := strings.SplitN(env, "=", 2)[0]
			if !overrides[key] {
				command.Env = append(command.Env, env)
			}
		}
		command.Env = append(command.Env, buildEnv...)
		if b, e := command.CombinedOutput(); e != nil {
			t.Fatalf("compile visible owned helper: %v\n%s", e, b)
		}
		return updaterOwnedRead(t, output)
	}
	first, second := build(), build()
	if !bytes.Equal(first, second) {
		t.Fatal("independent owned-helper builds differ")
	}
	return first, updaterOwnedHelper{Source: updaterOwnedSource, SourceSHA256: updaterOwnedHash([]byte(updaterOwnedSource)), Module: updaterOwnedModule, ModuleSHA256: updaterOwnedHash([]byte(updaterOwnedModule)), GoVersion: runtime.Version(), GOOS: runtime.GOOS, GOARCH: runtime.GOARCH, CompilerSHA256: updaterOwnedHash(updaterOwnedRead(t, compiler)), ExecStdlibSHA256: updaterOwnedHash(updaterOwnedRead(t, filepath.Join(runtime.GOROOT(), "src", "os", "exec", "exec.go"))), BuildArguments: buildArgs, BuildEnvironment: buildEnv, BinaryLength: len(first), BinarySHA256: updaterOwnedHash(first), IndependentBuildEqual: true}
}

func updaterOwnedCases() []updaterOwnedCase {
	var out []updaterOwnedCase
	add := func(name, boundary, stdout, stderr string, status int) {
		out = append(out, updaterOwnedCase{Name: name, Boundary: boundary, Control: updaterOwnedConfig{Mode: "finite", Stdout: stdout, Stderr: stderr, Status: status}, Context: "background"})
	}
	for _, boundary := range []string{"concrete_checker", "public_default_installer"} {
		add("exact_version", boundary, "pixiv v1.2.3\n", "", 0)
		add("successful_stderr_ignored", boundary, "pixiv v1.2.3\n", "owned successful stderr\n", 0)
		add("large_successful_stderr_ignored", boundary, "pixiv v1.2.3\n", "", 0)
		out[len(out)-1].Control.StderrRepeat = 196608
		add("wrong_prefix", boundary, "other v1.2.3\n", "", 0)
		add("wrong_version", boundary, "pixiv v1.2.4\n", "", 0)
		add("missing_newline", boundary, "pixiv v1.2.3", "", 0)
		add("crlf", boundary, "pixiv v1.2.3\r\n", "", 0)
		add("extra_newline", boundary, "pixiv v1.2.3\n\n", "", 0)
		add("extra_stdout", boundary, "pixiv v1.2.3\nowned extra stdout\n", "", 0)
		add("valid_stdout_exit7", boundary, "pixiv v1.2.3\n", "owned failing stderr\n", 7)
		add("wrong_stdout_exit9", boundary, "wrong\n", "owned wrong-output failure\n", 9)
		add("large_failing_stderr", boundary, "pixiv v1.2.3\n", "", 17)
		out[len(out)-1].Control.StderrRepeat = 196608
		add("large_stdout", boundary, "", "", 0)
		out[len(out)-1].Control.StdoutRepeat = 65536
		add("pre_canceled_no_exec", boundary, "pixiv v1.2.3\n", "", 0)
		out[len(out)-1].Context = "pre_canceled"
		add("cancel_while_blocked_after_valid_output", boundary, "pixiv v1.2.3\n", "", 0)
		out[len(out)-1].Context = "cancel_when_heartbeat_advances"
		out[len(out)-1].Control.Mode = "block"
		add("caller_deadline_while_output_incomplete", boundary, "pixiv v1.2.3", "owned before blocking\n", 0)
		out[len(out)-1].Context = "one_second_deadline"
		out[len(out)-1].Control.Mode = "block"
	}
	add("symlink_target_success", "public_default_installer", "pixiv v1.2.3\n", "", 0)
	out[len(out)-1].Symlink = true
	for _, barrier := range []string{"invalid_signature", "signed_invalid_checksum_entry", "archive_checksum_mismatch", "archive_traversal", "archive_symlink", "archive_duplicate_binary", "archive_missing_binary", "corrupt_archive"} {
		add(barrier+"_no_exec", "public_default_installer", "pixiv v1.2.3\n", "", 0)
		out[len(out)-1].Barrier = barrier
	}
	return out
}

func updaterOwnedArchive(t *testing.T, binary []byte, barrier string) []byte {
	t.Helper()
	if barrier == "corrupt_archive" {
		return []byte("owned non-gzip archive")
	}
	var buffer bytes.Buffer
	g := gzip.NewWriter(&buffer)
	w := tar.NewWriter(g)
	write := func(name string, kind byte, body []byte) {
		h := &tar.Header{Name: name, Mode: 0755, Typeflag: kind}
		if kind == tar.TypeReg {
			h.Size = int64(len(body))
		}
		if kind == tar.TypeSymlink {
			h.Linkname = "owned-target"
		}
		if e := w.WriteHeader(h); e != nil {
			t.Fatal(e)
		}
		if h.Size > 0 {
			if _, e := w.Write(body); e != nil {
				t.Fatal(e)
			}
		}
	}
	if barrier == "archive_traversal" {
		write("../owned-escape", tar.TypeReg, []byte("owned unsafe member"))
	}
	if barrier == "archive_symlink" {
		write("owned-link", tar.TypeSymlink, nil)
	}
	if barrier != "archive_missing_binary" {
		write("nested/pixiv", tar.TypeReg, binary)
	}
	if barrier == "archive_duplicate_binary" {
		write("duplicate/pixiv", tar.TypeReg, binary)
	}
	if e := w.Close(); e != nil {
		t.Fatal(e)
	}
	if e := g.Close(); e != nil {
		t.Fatal(e)
	}
	return buffer.Bytes()
}
func updaterOwnedKey() ed25519.PrivateKey {
	seed := sha256.Sum256([]byte("visible owned preflight synthetic signing fixture; never production trust"))
	return ed25519.NewKeyFromSeed(seed[:])
}
func updaterOwnedInstall(t *testing.T, ctx context.Context, root string, binary []byte, input updaterOwnedCase, requests *[]string) error {
	t.Helper()
	archiveName := releaseArchiveName("1.2.3", "linux", "amd64")
	archive := updaterOwnedArchive(t, binary, input.Barrier)
	checksum := updaterOwnedHash(archive)
	if input.Barrier == "archive_checksum_mismatch" {
		checksum = strings.Repeat("0", 64)
	}
	if input.Barrier == "signed_invalid_checksum_entry" {
		checksum = "invalid-owned-checksum"
	}
	checksums := []byte(checksum + "  " + archiveName + "\n")
	signature := ed25519.Sign(updaterOwnedKey(), checksums)
	if input.Barrier == "invalid_signature" {
		signature[0] ^= 1
	}
	manifest, e := json.Marshal(checksumsManifest{KeyID: "owned-helper-only", ChecksumsSHA256: updaterOwnedHash(checksums), Signature: base64.StdEncoding.EncodeToString(signature)})
	if e != nil {
		t.Fatal(e)
	}
	assets := map[string][]byte{archiveName: archive, checksumsAssetName: checksums, manifestAssetName: manifest}
	var rel release.Release
	rel.TagName = "v1.2.3"
	rel.Version = "1.2.3"
	for _, name := range []string{archiveName, checksumsAssetName, manifestAssetName} {
		rel.Assets = append(rel.Assets, release.ReleaseAsset{Name: name, DownloadURL: "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/" + name})
	}
	transport := updaterOwnedTransport(func(r *http.Request) (*http.Response, error) {
		if r.Method != "GET" || r.Header.Get("User-Agent") != "" {
			t.Fatalf("unexpected owned transport request: %s %s", r.Method, r.URL)
		}
		name := filepath.Base(r.URL.Path)
		body, ok := assets[name]
		if !ok {
			t.Fatalf("unowned transport target: %s", r.URL)
		}
		if r.URL.String() != "https://github.com/FlanChanXwO/pixiv-cli/releases/download/v1.2.3/"+name {
			t.Fatalf("unowned URL: %s", r.URL)
		}
		*requests = append(*requests, fmt.Sprintf("%s %s user-agent=%q", r.Method, r.URL.String(), r.Header.Get("User-Agent")))
		return &http.Response{StatusCode: 200, Status: "200 OK", Header: make(http.Header), Body: io.NopCloser(bytes.NewReader(body)), Request: r}, nil
	})
	target := filepath.Join(root, "target")
	if input.Symlink {
		target = filepath.Join(root, "owned-link")
		if e := os.Symlink("target", target); e != nil {
			t.Fatal(e)
		}
	}
	installer := NewReleaseInstaller(ReleaseInstallerOptions{HTTPClient: &http.Client{Transport: transport}, TrustedKeys: map[string]ed25519.PublicKey{"owned-helper-only": updaterOwnedKey().Public().(ed25519.PublicKey)}, ExecutablePath: func() (string, error) { *requests = append(*requests, "locate-owned-target"); return target, nil }, GOOS: "linux", GOARCH: "amd64", BinaryChecker: nil, Replacer: nil})
	concrete, ok := installer.(*releaseInstaller)
	if !ok {
		t.Fatalf("unexpected installer type: %T", installer)
	}
	if _, ok := concrete.checker.(processReleaseBinaryChecker); !ok {
		t.Fatalf("public constructor did not select the real process checker: %T", concrete.checker)
	}
	if _, ok := concrete.replacer.(releaseFileReplacer); !ok {
		t.Fatalf("public constructor did not select the real replacer: %T", concrete.replacer)
	}
	return installer.Install(ctx, rel)
}

func updaterOwnedEffects(t *testing.T, root string) []updaterOwnedEvent {
	t.Helper()
	b, e := os.ReadFile(filepath.Join(root, "owned-effects.jsonl"))
	if errors.Is(e, os.ErrNotExist) {
		return []updaterOwnedEvent{}
	}
	if e != nil {
		t.Fatal(e)
	}
	var out []updaterOwnedEvent
	decoder := json.NewDecoder(bytes.NewReader(b))
	for {
		var event updaterOwnedEvent
		e := decoder.Decode(&event)
		if errors.Is(e, io.EOF) {
			break
		}
		if e != nil {
			t.Fatal(e)
		}
		out = append(out, event)
	}
	return out
}
func updaterOwnedRun(t *testing.T, binary []byte, input updaterOwnedCase) updaterOwnedRow {
	t.Helper()
	root := t.TempDir()
	target := filepath.Join(root, "target")
	updaterOwnedWrite(t, target, []byte("owned previous target bytes\n"), 0644)
	control, e := json.Marshal(input.Control)
	if e != nil {
		t.Fatal(e)
	}
	updaterOwnedWrite(t, filepath.Join(root, "owned-control.json"), control, 0600)
	row := updaterOwnedRow{Input: input, Requests: []string{}, TransientMaterial: []string{}}
	ctx := context.Background()
	var cancel context.CancelFunc
	defer func() {
		if cancel != nil {
			cancel()
		}
	}()
	var cancelWitness chan bool
	switch input.Context {
	case "pre_canceled":
		ctx, cancel = context.WithCancel(ctx)
		cancel()
	case "one_second_deadline":
		ctx, cancel = context.WithTimeout(ctx, time.Second)
	case "cancel_when_heartbeat_advances":
		var guard context.CancelFunc
		ctx, guard = context.WithTimeout(ctx, 5*time.Second)
		defer guard()
		ctx, cancel = context.WithCancel(ctx)
		cancelWitness = make(chan bool, 1)
		go func() {
			path := filepath.Join(root, "owned-heartbeat")
			var first []byte
			for {
				b, e := os.ReadFile(path)
				if e == nil {
					if first == nil {
						first = b
					} else if !bytes.Equal(first, b) {
						cancelWitness <- true
						cancel()
						return
					}
				}
				select {
				case <-ctx.Done():
					cancelWitness <- false
					return
				case <-time.After(2 * time.Millisecond):
				}
			}
		}()
	}
	start := time.Now()
	var err error
	if input.Boundary == "concrete_checker" {
		dir := filepath.Join(root, "direct")
		if e := os.Mkdir(dir, 0700); e != nil {
			t.Fatal(e)
		}
		path := filepath.Join(dir, "pixiv")
		updaterOwnedWrite(t, path, binary, 0755)
		err = (processReleaseBinaryChecker{}).Check(ctx, path, "v1.2.3")
	} else {
		err = updaterOwnedInstall(t, ctx, root, binary, input, &row.Requests)
	}
	elapsed := time.Since(start)
	row.BoundedReturn = elapsed < 6*time.Second
	if !row.BoundedReturn {
		t.Fatalf("owned helper did not return within caller context bound: %s %v", input.Name, elapsed)
	}
	if cancelWitness != nil {
		row.HeartbeatAdvancedBeforeCancel = <-cancelWitness
		if !row.HeartbeatAdvancedBeforeCancel {
			t.Fatal("context canceled before owned helper heartbeat advanced")
		}
	}
	if e := ctx.Err(); e != nil {
		row.ContextError = e.Error()
	}
	message := ""
	if err != nil {
		message = err.Error()
	}
	message = strings.ReplaceAll(message, root+string(filepath.Separator), "OWNED_ROOT/")
	message = regexp.MustCompile(`\.pixiv-update-[^/" ]+`).ReplaceAllString(message, ".pixiv-update-OWNED_TEMP")
	row.Error = updaterOwnedError{Message: updaterOwnedSummary([]byte(message)), IsCanceled: errors.Is(err, context.Canceled), IsDeadline: errors.Is(err, context.DeadlineExceeded), ExitCode: 0, Stderr: updaterOwnedSummary(nil)}
	var exitError *exec.ExitError
	if errors.As(err, &exitError) {
		row.Error.ExitError = true
		row.Error.ExitCode = exitError.ExitCode()
		row.Error.Stderr = updaterOwnedSummary(exitError.Stderr)
		if exitError.ProcessState != nil {
			row.Error.ProcessState = exitError.ProcessState.String()
		}
	}
	row.Effects = updaterOwnedEffects(t, root)
	if input.Control.Mode == "block" {
		before, e := os.ReadFile(filepath.Join(root, "owned-heartbeat"))
		if e != nil {
			t.Fatal("owned helper did not reach blocked-output heartbeat")
		}
		beforeLog := updaterOwnedRead(t, filepath.Join(root, "owned-effects.jsonl"))
		time.Sleep(60 * time.Millisecond)
		after := updaterOwnedRead(t, filepath.Join(root, "owned-heartbeat"))
		afterLog := updaterOwnedRead(t, filepath.Join(root, "owned-effects.jsonl"))
		row.HeartbeatQuietAfterReturn = bytes.Equal(before, after) && bytes.Equal(beforeLog, afterLog)
		if !row.HeartbeatQuietAfterReturn {
			t.Fatal("owned child continued changing its markers after checker returned")
		}
	}
	targetBytes := updaterOwnedRead(t, target)
	row.Target = updaterOwnedBytes{Length: len(targetBytes), SHA256: updaterOwnedHash(targetBytes)}
	if !bytes.Equal(targetBytes, binary) {
		row.Target.Text = string(targetBytes)
	}
	info, e := os.Stat(target)
	if e != nil {
		t.Fatal(e)
	}
	row.TargetMode = uint32(info.Mode().Perm())
	if input.Symlink {
		link, e := os.Readlink(filepath.Join(root, "owned-link"))
		if e != nil {
			t.Fatal(e)
		}
		row.SymlinkTarget = link
	}
	entries, e := os.ReadDir(root)
	if e != nil {
		t.Fatal(e)
	}
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".pixiv-update-") {
			row.TransientMaterial = append(row.TransientMaterial, entry.Name())
		}
	}
	if _, e := os.Stat(filepath.Join(root, "owned-escape")); e == nil {
		t.Fatal("owned traversal escaped the archive work directory")
	}
	t.Logf("%s/%s caller=%s returned=%s", input.Boundary, input.Name, input.Context, elapsed)
	return row
}

func updaterOwnedAssert(t *testing.T, row updaterOwnedRow, binary []byte) {
	t.Helper()
	input := row.Input
	noExec := input.Context == "pre_canceled" || input.Barrier != ""
	if noExec {
		if len(row.Effects) != 0 {
			t.Fatalf("barrier executed owned helper: %s", input.Name)
		}
	} else {
		if len(row.Effects) != 2 {
			t.Fatalf("unexpected owned process effects: %s %+v", input.Name, row.Effects)
		}
		start := row.Effects[0]
		if start.Event != "started" || len(start.Args) != 1 || start.Args[0] != "--version" || start.CandidateSHA256 != updaterOwnedHash(binary) || start.CandidateMode != 0755 || start.TargetSHA256 != updaterOwnedHash([]byte("owned previous target bytes\n")) {
			t.Fatalf("unowned/misordered candidate execution: %s %+v", input.Name, start)
		}
		want := "finished"
		if input.Control.Mode == "block" {
			want = "blocked"
		}
		if row.Effects[1].Event != want {
			t.Fatalf("unexpected terminal helper marker: %s", input.Name)
		}
	}
	accepted := input.Barrier == "" && input.Context == "background" && input.Control.Mode == "finite" && input.Control.Status == 0 && input.Control.StdoutRepeat == 0 && input.Control.Stdout == "pixiv v1.2.3\n"
	if (row.Error.Message.Length == 0) != accepted {
		t.Fatalf("unexpected checker acceptance: %s %+v", input.Name, row.Error)
	}
	committed := accepted && input.Boundary == "public_default_installer"
	want := updaterOwnedHash([]byte("owned previous target bytes\n"))
	mode := uint32(0644)
	if committed {
		want = updaterOwnedHash(binary)
		mode = 0755
	}
	if row.Target.SHA256 != want || row.TargetMode != mode {
		t.Fatalf("owned target preservation/replacement failed: %s", input.Name)
	}
	if len(row.TransientMaterial) != 0 {
		t.Fatalf("owned updater staging material leaked: %s %+v", input.Name, row.TransientMaterial)
	}
	if input.Context == "pre_canceled" && !row.Error.IsCanceled {
		t.Fatalf("pre-cancel cause lost: %s", input.Name)
	}
	if input.Control.Mode == "block" && (!row.Error.ExitError || row.Error.ExitCode != -1 || row.Error.ProcessState != "signal: killed" || !row.HeartbeatQuietAfterReturn) {
		t.Fatalf("owned blocked process was not killed and waited: %s %+v", input.Name, row.Error)
	}
	if input.Control.Status > 0 && (!row.Error.ExitError || row.Error.ExitCode != input.Control.Status) {
		t.Fatalf("owned exit status lost: %s", input.Name)
	}
	if input.Control.Status > 0 && input.Control.StderrRepeat == 0 && row.Error.Stderr.Text != input.Control.Stderr {
		t.Fatalf("owned failure stderr lost: %s", input.Name)
	}
	if input.Control.StderrRepeat > 0 && input.Control.Status > 0 && row.Error.Stderr.Length != 65536+len("\n... omitting 131072 bytes ...\n") {
		t.Fatalf("unexpected Go stderr prefix/suffix bound: %s length=%d", input.Name, row.Error.Stderr.Length)
	}
	if input.Barrier == "invalid_signature" || input.Barrier == "signed_invalid_checksum_entry" {
		if len(row.Requests) != 2 {
			t.Fatalf("trust/checksum-entry barrier requested the helper archive: %s %+v", input.Name, row.Requests)
		}
	}
	if (input.Barrier == "invalid_signature" || input.Barrier == "signed_invalid_checksum_entry" || input.Barrier == "archive_checksum_mismatch") && strings.Contains(strings.Join(row.Requests, "\n"), "locate-owned-target") {
		t.Fatalf("verification barrier reached target location: %s", input.Name)
	}
}

func TestMigrationUpdaterOwnedPreflightFrozenContracts(t *testing.T) {
	if runtime.GOOS != "linux" || runtime.GOARCH != "amd64" {
		t.Skip("native Linux/amd64 owned-helper fixture; native Windows and Darwin process diagnostics remain unverified")
	}
	if runtime.Version() != "go1.27.1" {
		t.Fatalf("owned helper requires the approved official Go 1.27.1 toolchain, got %s", runtime.Version())
	}
	root := filepath.Join("..", "..", "..")
	for path, want := range updaterOwnedSources {
		if updaterOwnedHash(updaterOwnedRead(t, filepath.Join(root, path))) != want {
			t.Fatalf("frozen source/module changed: %s", path)
		}
	}
	binary, helper := updaterOwnedBuild(t)
	fixture := updaterOwnedFixture{Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", Evidence: "Native Linux/amd64 actual processReleaseBinaryChecker.Check and public NewReleaseInstaller with BinaryChecker=nil, Replacer=nil; execute only a dependency-free helper compiled twice from visible embedded owned source with official Go1.27.1; signed in-memory assets and disposable owned target, no third-party/release/target execution", Sources: updaterOwnedSources, Helper: helper, SyntheticPublicKey: fmt.Sprintf("%x", updaterOwnedKey().Public().(ed25519.PublicKey)), Rows: []updaterOwnedRow{}, Boundaries: []string{
		"No executable/archive bytes are embedded in this fixture. The visible helper source, module, compiler SHA, stdlib exec SHA, flags/environment and native helper hash permit rebuilding. Hash is specific to official Go1.27.1 Linux/amd64 with CGO disabled; other compiler/platform bytes are not claimed identical.",
		"The production checker passes only --version and inherits its process environment. No environment/PATH/settings/registry/browser change or real target/program is executed. The helper locates controls only beside its owned candidate parent directory.",
		"Only owned temporary root paths and random updater workdir basenames are replaced in diagnostics. Process IDs are neither recorded nor normalized. Native Windows/Darwin process diagnostics and six-platform runtime remain unverified.",
		"The Go checker has no intrinsic timeout/output limit. Finite 64KiB stdout and 192KiB stderr witnesses prove these bytes are drained; caller cancellation/deadline bounds owned blocking cases. They do not claim arbitrary output grammar, bounded production allocation or inherited-pipe/grandchild behavior.",
		"Large error/stderr observations store exact byte length and SHA256 plus prefix/suffix; short diagnostics store exact text. Inputs and visible source reproduce omitted large middles; this does not equate different diagnostic bytes.",
		"Volatile duration and heartbeat counters remain in test logs, outside exact fixture. Cancellation waits for a live owned heartbeat; after Output returns, the ExitError ProcessState and quiet owned markers witness child kill/wait. No process-ID equivalence claim is made.",
		"Public successful install uses genuine verified archive extraction, staging and native Unix atomic rename. Only an owned disposable target is replaced; no post-success target launch or automatic restart occurs. Signature/checksum/archive errors and process failures preserve it and leave no updater temporary material.",
		"Synthetic signing seed belongs only to this visible test; no production private key/account/credential is read. Canonical official asset URLs are intercepted in-memory, with no network download or external executable.",
	}}
	for _, input := range updaterOwnedCases() {
		row := updaterOwnedRun(t, binary, input)
		updaterOwnedAssert(t, row, binary)
		fixture.Rows = append(fixture.Rows, row)
	}
	data, e := json.MarshalIndent(fixture, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	data = append(data, '\n')
	path := filepath.Join(root, "crates", "pixiv-cli", "tests", "fixtures", "updater-owned-preflight.json")
	if *updaterOwnedCapture {
		updaterOwnedWrite(t, path, data, 0644)
	} else if !bytes.Equal(updaterOwnedRead(t, path), data) {
		actual := filepath.Join(t.TempDir(), "owned-preflight-actual.json")
		updaterOwnedWrite(t, actual, data, 0600)
		t.Fatalf("owned native preflight fixture differs: %s", actual)
	}
	t.Logf("native owned preflight rows=%d helper_sha256=%s reproducible=%t", len(fixture.Rows), helper.BinarySHA256, helper.IndependentBuildEqual)
}

//go:build cgo && ((darwin && (amd64 || arm64)) || (linux && (amd64 || arm64)) || (windows && (amd64 || arm64)))

package ugoira

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/gif"
	"image/jpeg"
	"image/png"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
	"unsafe"

	"github.com/FlanChanXwO/pixiv-cli/internal/media/ugoira/staticlib"
)

type migrationNativeCase struct {
	Name       string      `json:"name"`
	Format     Format      `json:"format"`
	MaxEdge    uint32      `json:"max_edge"`
	Frames     []Frame     `json:"frames"`
	ZIPHex     string      `json:"zip_hex"`
	OutputName string      `json:"output_name"`
	Error      string      `json:"error"`
	SHA256     string      `json:"output_sha256,omitempty"`
	Width      int         `json:"width,omitempty"`
	Height     int         `json:"height,omitempty"`
	Loop       int         `json:"loop"`
	Delays     []int       `json:"gif_delay_ticks,omitempty"`
	APNGDelays [][2]uint16 `json:"apng_delay_fractions,omitempty"`
	FirstAlpha []uint32    `json:"first_frame_alpha,omitempty"`
}
type migrationNativeFixture struct {
	PinnedGo     string                             `json:"pinned_go"`
	SourceDigest string                             `json:"source_digest"`
	SourceSHA    map[string]string                  `json:"source_sha256"`
	Artifacts    map[string]staticlib.ManifestAsset `json:"artifacts"`
	Platform     string                             `json:"native_execution_platform"`
	Evidence     string                             `json:"evidence"`
	Deferred     []string                           `json:"deferred"`
	Cases        []migrationNativeCase              `json:"cases"`
}

func migrationNativeHash(body []byte) string {
	h := sha256.Sum256(body)
	return hex.EncodeToString(h[:])
}
func migrationNativeImage(t *testing.T, w, h int, jpegFrame bool) []byte {
	t.Helper()
	im := image.NewNRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			im.SetNRGBA(x, y, color.NRGBA{R: uint8(40 + x*35), G: uint8(80 + y*40), B: 190, A: 255})
		}
	}
	im.SetNRGBA(0, 0, color.NRGBA{R: 200, A: 0})
	if w > 1 {
		im.SetNRGBA(1, 0, color.NRGBA{R: 220, G: 40, B: 70, A: 128})
	}
	var b bytes.Buffer
	var err error
	if jpegFrame {
		err = jpeg.Encode(&b, im, &jpeg.Options{Quality: 90})
	} else {
		err = png.Encode(&b, im)
	}
	if err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}
func migrationNativeZIP(t *testing.T, names []string, bodies [][]byte) []byte {
	t.Helper()
	var b bytes.Buffer
	z := zip.NewWriter(&b)
	for i, name := range names {
		w, err := z.CreateHeader(&zip.FileHeader{Name: name, Method: zip.Store})
		if err != nil {
			t.Fatal(err)
		}
		if _, err = w.Write(bodies[i]); err != nil {
			t.Fatal(err)
		}
	}
	if err := z.Close(); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}
func migrationNativeCases(t *testing.T) []migrationNativeCase {
	p := migrationNativeImage(t, 4, 2, false)
	j := migrationNativeImage(t, 4, 2, true)
	zipBody := migrationNativeZIP(t, []string{"first.png", "second.jpg"}, [][]byte{p, j})
	frames := []Frame{{"first.png", 1}, {"second.jpg", 125}}
	base := func(name string, f Format, edge uint32) migrationNativeCase {
		return migrationNativeCase{Name: name, Format: f, MaxEdge: edge, Frames: frames, ZIPHex: hex.EncodeToString(zipBody), OutputName: "out.anim"}
	}
	cases := []migrationNativeCase{base("default_gif", "", 0), base("gif", FormatGIF, 0), base("apng", FormatAPNG, 0), base("gif_max_edge", FormatGIF, 2), base("apng_max_edge", FormatAPNG, 2), base("never_upscale", FormatAPNG, 20), base("invalid_format", "GIF", 0)}
	for _, name := range []string{"empty_frames", "null_frames", "missing_frame", "corrupt_zip", "invalid_image", "zero_delay", "negative_delay", "gif_delay_overflow", "apng_delay_overflow", "size_mismatch", "no_extension", "missing_zip", "apng_size_mismatch", "gif_delay_at_limit", "apng_large_exact_delay"} {
		c := base(name, FormatGIF, 0)
		switch name {
		case "empty_frames":
			c.Frames = []Frame{}
		case "null_frames":
			c.Frames = nil
		case "missing_frame":
			c.Frames = []Frame{{"missing.png", 80}}
		case "corrupt_zip":
			c.ZIPHex = hex.EncodeToString([]byte("not a ZIP"))
		case "invalid_image":
			c.ZIPHex = hex.EncodeToString(migrationNativeZIP(t, []string{"first.png"}, [][]byte{[]byte("not an image")}))
			c.Frames = frames[:1]
		case "zero_delay":
			c.Frames = []Frame{{"first.png", 0}}
		case "negative_delay":
			c.Frames = []Frame{{"first.png", -1}}
		case "gif_delay_overflow":
			c.Frames = []Frame{{"first.png", 655351}}
		case "apng_delay_overflow":
			c.Format = FormatAPNG
			c.Frames = []Frame{{"first.png", 65536000}}
		case "size_mismatch":
			c.ZIPHex = hex.EncodeToString(migrationNativeZIP(t, []string{"first.png", "second.jpg"}, [][]byte{p, migrationNativeImage(t, 2, 2, true)}))
		case "no_extension":
			c.OutputName = "out"
		case "missing_zip":
			c.ZIPHex = ""
		case "apng_size_mismatch":
			c.Format = FormatAPNG
			c.ZIPHex = hex.EncodeToString(migrationNativeZIP(t, []string{"first.png", "second.jpg"}, [][]byte{p, migrationNativeImage(t, 2, 2, true)}))
		case "gif_delay_at_limit":
			c.Frames = []Frame{{"first.png", 655350}}
		case "apng_large_exact_delay":
			c.Format = FormatAPNG
			c.Frames = []Frame{{"first.png", 65535000}}
		}
		cases = append(cases, c)
	}
	for i, name := range []string{"../unsafe.png", "/absolute.png", "back\\slash.png", "..prefix.png"} {
		c := base(fmt.Sprintf("native_exact_unsafe_name_%d", i), FormatAPNG, 0)
		c.Frames = []Frame{{name, 80}}
		c.ZIPHex = hex.EncodeToString(migrationNativeZIP(t, []string{name}, [][]byte{p}))
		cases = append(cases, c)
	}
	return cases
}
func migrationNativeAlpha(im image.Image) []uint32 {
	var out []uint32
	for y := im.Bounds().Min.Y; y < im.Bounds().Max.Y; y++ {
		for x := im.Bounds().Min.X; x < im.Bounds().Max.X; x++ {
			_, _, _, a := im.At(x, y).RGBA()
			out = append(out, a)
		}
	}
	return out
}
func migrationNativeObserve(t *testing.T, c migrationNativeCase) migrationNativeCase {
	t.Helper()
	dir := t.TempDir()
	body, err := hex.DecodeString(c.ZIPHex)
	if err != nil {
		t.Fatal(err)
	}
	zp := filepath.Join(dir, "frames.zip")
	if len(body) > 0 {
		if err = os.WriteFile(zp, body, 0600); err != nil {
			t.Fatal(err)
		}
	}
	out := filepath.Join(dir, c.OutputName)
	old := []byte("existing-output-must-survive-errors")
	if err = os.WriteFile(out, old, 0600); err != nil {
		t.Fatal(err)
	}
	err = NewRustEncoder().Encode(context.Background(), Input{ZipPath: zp, Frames: c.Frames, OutputPath: out, Format: c.Format, MaxEdge: c.MaxEdge, WorkDir: filepath.Join(dir, "unused")})
	if err != nil {
		c.Error = strings.ReplaceAll(err.Error(), out, "${OUTPUT}")
		got, e := os.ReadFile(out)
		if e != nil || !bytes.Equal(got, old) {
			t.Fatalf("%s lost existing output: %v", c.Name, e)
		}
	} else {
		b, e := os.ReadFile(out)
		if e != nil {
			t.Fatal(e)
		}
		c.SHA256 = migrationNativeHash(b)
		if c.Format == FormatAPNG {
			im, e := png.Decode(bytes.NewReader(b))
			if e != nil {
				t.Fatal(e)
			}
			c.Width = im.Bounds().Dx()
			c.Height = im.Bounds().Dy()
			c.FirstAlpha = migrationNativeAlpha(im)
			for pos := 8; pos+12 <= len(b); {
				n := int(binary.BigEndian.Uint32(b[pos : pos+4]))
				if pos+12+n > len(b) {
					t.Fatal("truncated PNG chunk")
				}
				chunk := b[pos+8 : pos+8+n]
				switch string(b[pos+4 : pos+8]) {
				case "acTL":
					c.Loop = int(binary.BigEndian.Uint32(chunk[4:8]))
					if int(binary.BigEndian.Uint32(chunk[:4])) != len(c.Frames) {
						t.Fatal("APNG frame count")
					}
				case "fcTL":
					c.APNGDelays = append(c.APNGDelays, [2]uint16{binary.BigEndian.Uint16(chunk[20:22]), binary.BigEndian.Uint16(chunk[22:24])})
				}
				pos += n + 12
			}
		} else {
			g, e := gif.DecodeAll(bytes.NewReader(b))
			if e != nil {
				t.Fatal(e)
			}
			c.Width = g.Config.Width
			c.Height = g.Config.Height
			c.Loop = g.LoopCount
			c.Delays = g.Delay
			c.FirstAlpha = migrationNativeAlpha(g.Image[0])
			if len(g.Image) != len(c.Frames) {
				t.Fatal("GIF frame count")
			}
		}
	}
	tmp, _ := filepath.Glob(filepath.Join(dir, ".ugoira-*"))
	if len(tmp) != 0 {
		t.Fatalf("%s leaked temporary animation: %v", c.Name, tmp)
	}
	return c
}
func TestMigrationNativeEncoderFixture(t *testing.T) {
	root := filepath.Clean(filepath.Join("..", "..", ".."))
	fp := filepath.Join(root, "crates/pixiv-app/tests/fixtures/ugoira_encoder.json")
	f := migrationNativeFixture{PinnedGo: "4b4426487ef18bed276706daec385e0d0a6979f9", Platform: runtime.GOOS + "/" + runtime.GOARCH, SourceSHA: map[string]string{}, Evidence: "Genuine Go NewRustEncoder and tracked native staticlib on synthetic owned PNG/JPEG ZIPs; full output bytes SHA-256 plus decoded timing/dimensions/loop/alpha. Existing outputs and temporary cleanup checked on every case. Native unsafe names are accepted by exact ZIP lookup; archive-only workflow validation is separate. Injected rustFFI tests establish Go lifecycle joins, not native failure production.", Deferred: []string{"Native run and compile on other five platforms remain unverified; six manifest artifact byte identities are validated, not run.", "Large-image allocation/OOM, GIF dimensions beyond u16, APNG u32 frame/sequence limits and native non-UTF8 filesystem paths are not exercised.", "Encoding byte identity is established for this pinned Linux amd64 artifact only; other target outputs are not inferred."}}
	for _, p := range []string{"internal/media/ugoira/rust.go", "internal/media/ugoira/ugoira.go", "internal/media/ugoira/rust/src/lib.rs", "internal/media/ugoira/rust/Cargo.toml", "internal/media/ugoira/rust/Cargo.lock", "internal/media/ugoira/rust/staticlib/manifest.json"} {
		b, err := os.ReadFile(filepath.Join(root, p))
		if err != nil {
			t.Fatal(err)
		}
		f.SourceSHA[p] = migrationNativeHash(b)
	}
	digest, err := staticlib.CalculateRustSourceDigest(filepath.Join(root, "internal/media/ugoira/rust"), filepath.Join(root, "third_party/rust/quantette-0.6.0"))
	if err != nil {
		t.Fatal(err)
	}
	f.SourceDigest = digest
	manifest, err := os.ReadFile(filepath.Join(root, "internal/media/ugoira/rust/staticlib/manifest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var identity staticlib.Manifest
	if err = json.Unmarshal(manifest, &identity); err != nil {
		t.Fatal(err)
	}
	f.Artifacts = identity.Artifacts
	if err = staticlib.ValidateManifestFiles(filepath.Join(root, "internal/media/ugoira/rust/staticlib"), manifest, digest); err != nil {
		t.Fatal(err)
	}
	if os.Getenv("PIXIV_UPDATE_UGOIRA_ENCODER_FIXTURE") == "1" {
		for _, c := range migrationNativeCases(t) {
			f.Cases = append(f.Cases, migrationNativeObserve(t, c))
		}
		b, err := json.MarshalIndent(f, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(fp, append(b, '\n'), 0644); err != nil {
			t.Fatal(err)
		}
		return
	}
	b, err := os.ReadFile(fp)
	if err != nil {
		t.Fatal(err)
	}
	var want migrationNativeFixture
	if err = json.Unmarshal(b, &want); err != nil {
		t.Fatal(err)
	}
	if f.SourceDigest != want.SourceDigest || !reflect.DeepEqual(f.SourceSHA, want.SourceSHA) || !reflect.DeepEqual(f.Artifacts, want.Artifacts) {
		t.Fatal("native source identity changed")
	}
	for _, c := range want.Cases {
		t.Run(c.Name, func(t *testing.T) {
			if runtime.GOOS+"/"+runtime.GOARCH != want.Platform {
				t.Skip("native byte fixture was executed only on " + want.Platform)
			}
			got := migrationNativeObserve(t, migrationNativeCase{Name: c.Name, Format: c.Format, MaxEdge: c.MaxEdge, Frames: c.Frames, ZIPHex: c.ZIPHex, OutputName: c.OutputName})
			if !reflect.DeepEqual(got, c) {
				t.Fatalf("native observation mismatch\ngot: %+v\nwant: %+v", got, c)
			}
		})
	}
}

type migrationLifecycleFFI struct {
	mu                                          sync.Mutex
	trace                                       []string
	token                                       unsafe.Pointer
	newErr, errorEncode, errorCancel, errorFree error
	entered, canceled, release                  chan struct{}
	newReady                                    chan struct{}
}

func (f *migrationLifecycleFFI) record(s string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.trace = append(f.trace, s)
}
func (f *migrationLifecycleFFI) NewToken() (unsafe.Pointer, error) {
	f.record("new")
	if f.newReady != nil {
		close(f.newReady)
	}
	return f.token, f.newErr
}
func (f *migrationLifecycleFFI) Cancel(p unsafe.Pointer) error {
	if p != f.token {
		panic("wrong token")
	}
	f.record("cancel")
	if f.canceled != nil {
		close(f.canceled)
	}
	return f.errorCancel
}
func (f *migrationLifecycleFFI) Free(p unsafe.Pointer) error {
	if p != f.token {
		panic("wrong token")
	}
	f.record("free")
	return f.errorFree
}
func (f *migrationLifecycleFFI) Encode(_ string, _ []byte, out string, p unsafe.Pointer, _ Format, _ uint32) error {
	if p != f.token {
		panic("wrong token")
	}
	f.record("encode")
	if f.entered != nil {
		close(f.entered)
	}
	if f.release != nil {
		<-f.release
	}
	if err := os.WriteFile(out, []byte("partial-native-output"), 0600); err != nil {
		return err
	}
	return f.errorEncode
}
func migrationWait(t *testing.T, ch <-chan struct{}) {
	t.Helper()
	select {
	case <-ch:
	case <-time.After(5 * time.Second):
		t.Fatal("lifecycle rendezvous timed out")
	}
}
func migrationLifecycleOutput(t *testing.T) (string, func()) {
	t.Helper()
	dir := t.TempDir()
	out := filepath.Join(dir, "existing.gif")
	if err := os.WriteFile(out, []byte("old"), 0600); err != nil {
		t.Fatal(err)
	}
	return out, func() {
		b, err := os.ReadFile(out)
		if err != nil || string(b) != "old" {
			t.Fatalf("old output changed: %q %v", b, err)
		}
		tmp, _ := filepath.Glob(filepath.Join(dir, ".ugoira-*"))
		if len(tmp) > 0 {
			t.Fatalf("temporary files leaked: %v", tmp)
		}
	}
}
func TestMigrationNativeEncoderBeforeCancel(t *testing.T) {
	out, check := migrationLifecycleOutput(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	err := NewRustEncoder().Encode(ctx, Input{OutputPath: out})
	if !errors.Is(err, context.Canceled) || err.Error() != "context canceled" {
		t.Fatalf("before cancel: %v", err)
	}
	check()
}
func TestMigrationNativeEncoderCanceledToken(t *testing.T) {
	f := cgoRustFFI{}
	token, err := f.NewToken()
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := f.Free(token); err != nil {
			t.Fatal(err)
		}
	}()
	if err = f.Cancel(token); err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	zp := filepath.Join(dir, "frames.zip")
	if err = os.WriteFile(zp, migrationNativeZIP(t, []string{"first.png"}, [][]byte{migrationNativeImage(t, 2, 2, false)}), 0600); err != nil {
		t.Fatal(err)
	}
	err = f.Encode(zp, []byte(`[{"file":"first.png","delay":80}]`), filepath.Join(dir, "out.gif"), token, FormatGIF, 0)
	if err == nil || err.Error() != "rust ugoira encoder failed: ugoira encoding canceled" {
		t.Fatalf("actual native canceled token: %v", err)
	}
}
func TestMigrationNativeEncoderGateWaitCancel(t *testing.T) {
	rustEncodeGate <- struct{}{}
	defer func() { <-rustEncodeGate }()
	out, check := migrationLifecycleOutput(t)
	ctx, cancel := context.WithCancel(context.Background())
	result := make(chan error, 1)
	go func() {
		result <- NewRustEncoder().Encode(ctx, Input{OutputPath: out, Frames: []Frame{{"first.png", 80}}})
	}()
	deadline := time.Now().Add(5 * time.Second)
	for {
		tmp, _ := filepath.Glob(filepath.Join(filepath.Dir(out), ".ugoira-*"))
		if len(tmp) == 1 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("native gate wait never created temporary sink")
		}
		runtime.Gosched()
	}
	select {
	case err := <-result:
		t.Fatalf("native encoder bypassed held gate: %v", err)
	default:
	}
	cancel()
	select {
	case err := <-result:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("gate cancel: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("native gate cancellation blocked")
	}
	check()
}
func TestMigrationInjectedFFILifecycle(t *testing.T) {
	t.Run("token_creation_failure", func(t *testing.T) {
		out, check := migrationLifecycleOutput(t)
		sentinel := errors.New("token allocation failed")
		f := &migrationLifecycleFFI{newErr: sentinel}
		err := (rustEncoder{ffi: f}).Encode(context.Background(), Input{OutputPath: out})
		if err != sentinel || !reflect.DeepEqual(f.trace, []string{"new"}) {
			t.Fatalf("allocation lifecycle: %v %v", err, f.trace)
		}
		check()
	})
	t.Run("encode_and_free_join", func(t *testing.T) {
		out, check := migrationLifecycleOutput(t)
		encodeErr := errors.New("rust ugoira encoder failed: injected native failure")
		freeErr := errors.New("free rust ugoira cancellation token: injected free failure")
		f := &migrationLifecycleFFI{token: unsafe.Pointer(new(int)), errorEncode: encodeErr, errorFree: freeErr}
		err := (rustEncoder{ffi: f}).Encode(context.Background(), Input{OutputPath: out})
		if !errors.Is(err, encodeErr) || !errors.Is(err, freeErr) || err.Error() != encodeErr.Error()+"\n"+freeErr.Error() || !reflect.DeepEqual(f.trace, []string{"new", "encode", "free"}) {
			t.Fatalf("encode/free join: %v %v", err, f.trace)
		}
		check()
	})
	t.Run("cancel_encode_free_context_join", func(t *testing.T) {
		out, check := migrationLifecycleOutput(t)
		encodeErr := errors.New("rust ugoira encoder failed: injected canceled encode")
		cancelErr := errors.New("cancel rust ugoira encoder: injected cancel failure")
		freeErr := errors.New("free rust ugoira cancellation token: injected free failure")
		f := &migrationLifecycleFFI{token: unsafe.Pointer(new(int)), errorEncode: encodeErr, errorCancel: cancelErr, errorFree: freeErr, entered: make(chan struct{}), canceled: make(chan struct{}), release: make(chan struct{})}
		ctx, cancel := context.WithCancel(context.Background())
		result := make(chan error, 1)
		go func() { result <- (rustEncoder{ffi: f}).Encode(ctx, Input{OutputPath: out}) }()
		migrationWait(t, f.entered)
		cancel()
		migrationWait(t, f.canceled)
		close(f.release)
		err := <-result
		for _, e := range []error{encodeErr, cancelErr, freeErr, context.Canceled} {
			if !errors.Is(err, e) {
				t.Fatalf("missing joined error %v: %v", e, err)
			}
		}
		if err.Error() != strings.Join([]string{encodeErr.Error(), cancelErr.Error(), freeErr.Error(), context.Canceled.Error()}, "\n") || !reflect.DeepEqual(f.trace, []string{"new", "encode", "cancel", "free"}) {
			t.Fatalf("cancel join: %v %v", err, f.trace)
		}
		check()
	})
	t.Run("gate_wait_owns_token_and_free", func(t *testing.T) {
		rustEncodeGate <- struct{}{}
		defer func() { <-rustEncodeGate }()
		out, check := migrationLifecycleOutput(t)
		freeErr := errors.New("free rust ugoira cancellation token: injected waiting free failure")
		f := &migrationLifecycleFFI{token: unsafe.Pointer(new(int)), errorFree: freeErr, canceled: make(chan struct{})}
		ctx, cancel := context.WithCancel(context.Background())
		result := make(chan error, 1)
		go func() { result <- (rustEncoder{ffi: f}).Encode(ctx, Input{OutputPath: out}) }()
		deadline := time.Now().Add(5 * time.Second)
		for {
			f.mu.Lock()
			ready := len(f.trace) > 0
			f.mu.Unlock()
			if ready {
				break
			}
			if time.Now().After(deadline) {
				t.Fatal("token creation timed out")
			}
			runtime.Gosched()
		}
		cancel()
		err := <-result
		if !errors.Is(err, context.Canceled) || !errors.Is(err, freeErr) || err.Error() != "context canceled\n"+freeErr.Error() {
			t.Fatalf("waiting join: %v", err)
		}
		f.mu.Lock()
		defer f.mu.Unlock()
		if len(f.trace) < 2 || f.trace[0] != "new" || f.trace[len(f.trace)-1] != "free" || strings.Contains(strings.Join(f.trace, ","), "encode") {
			t.Fatalf("waiting lifecycle: %v", f.trace)
		}
		check()
	})
}

func TestMigrationNativeFFINullTokenErrors(t *testing.T) {
	f := cgoRustFFI{}
	for _, c := range []struct {
		name string
		err  error
		want string
	}{
		{"cancel", f.Cancel(nil), "cancel rust ugoira encoder: cancellation_token pointer is null"},
		{"free", f.Free(nil), "free rust ugoira cancellation token: cancellation_token pointer is null"},
		{"encode", f.Encode("unused.zip", []byte("[]"), "unused.gif", nil, FormatGIF, 0), "rust ugoira encoder failed: cancellation_token pointer is null"},
	} {
		t.Run(c.name, func(t *testing.T) {
			if c.err == nil || c.err.Error() != c.want {
				t.Fatalf("native FFI prefix: %v", c.err)
			}
		})
	}
}

type migrationNativeRendezvousFFI struct {
	cgoRustFFI
	encoded, canceled, release chan struct{}
}

func (f *migrationNativeRendezvousFFI) Encode(z string, b []byte, out string, p unsafe.Pointer, format Format, edge uint32) error {
	err := f.cgoRustFFI.Encode(z, b, out, p, format, edge)
	close(f.encoded)
	<-f.release
	return err
}
func (f *migrationNativeRendezvousFFI) Cancel(p unsafe.Pointer) error {
	err := f.cgoRustFFI.Cancel(p)
	close(f.canceled)
	return err
}
func TestMigrationGenuineFFIAfterEncodeCancelPreservesOutput(t *testing.T) {
	out, check := migrationLifecycleOutput(t)
	zp := filepath.Join(filepath.Dir(out), "frames.zip")
	if err := os.WriteFile(zp, migrationNativeZIP(t, []string{"first.png"}, [][]byte{migrationNativeImage(t, 2, 2, false)}), 0600); err != nil {
		t.Fatal(err)
	}
	f := &migrationNativeRendezvousFFI{encoded: make(chan struct{}), canceled: make(chan struct{}), release: make(chan struct{})}
	ctx, cancel := context.WithCancel(context.Background())
	result := make(chan error, 1)
	go func() {
		result <- (rustEncoder{ffi: f}).Encode(ctx, Input{ZipPath: zp, Frames: []Frame{{"first.png", 80}}, OutputPath: out})
	}()
	migrationWait(t, f.encoded)
	cancel()
	migrationWait(t, f.canceled)
	close(f.release)
	err := <-result
	if !errors.Is(err, context.Canceled) || err.Error() != "context canceled" {
		t.Fatalf("native-complete cancellation: %v", err)
	}
	check()
}

func TestMigrationInjectedFFIGlobalEncodeGate(t *testing.T) {
	first := &migrationLifecycleFFI{token: unsafe.Pointer(new(int)), entered: make(chan struct{}), release: make(chan struct{})}
	second := &migrationLifecycleFFI{token: unsafe.Pointer(new(int)), entered: make(chan struct{}), newReady: make(chan struct{})}
	dir := t.TempDir()
	firstResult := make(chan error, 1)
	secondResult := make(chan error, 1)
	go func() {
		firstResult <- (rustEncoder{ffi: first}).Encode(context.Background(), Input{OutputPath: filepath.Join(dir, "first.gif")})
	}()
	migrationWait(t, first.entered)
	go func() {
		secondResult <- (rustEncoder{ffi: second}).Encode(context.Background(), Input{OutputPath: filepath.Join(dir, "second.gif")})
	}()
	migrationWait(t, second.newReady)
	second.mu.Lock()
	trace := append([]string(nil), second.trace...)
	second.mu.Unlock()
	if !reflect.DeepEqual(trace, []string{"new"}) {
		t.Fatalf("second encode bypassed active native gate: %v", trace)
	}
	close(first.release)
	if err := <-firstResult; err != nil {
		t.Fatal(err)
	}
	migrationWait(t, second.entered)
	if err := <-secondResult; err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"first.gif", "second.gif"} {
		b, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil || string(b) != "partial-native-output" {
			t.Fatalf("serialized output: %q %v", b, err)
		}
	}
	tmp, _ := filepath.Glob(filepath.Join(dir, ".ugoira-*"))
	if len(tmp) != 0 {
		t.Fatalf("serialized temporary leak: %v", tmp)
	}
}

func TestMigrationInjectedFFIFreeFailurePreservesOutput(t *testing.T) {
	out, check := migrationLifecycleOutput(t)
	freeErr := errors.New("free rust ugoira cancellation token: injected free failure")
	f := &migrationLifecycleFFI{token: unsafe.Pointer(new(int)), errorFree: freeErr}
	err := (rustEncoder{ffi: f}).Encode(context.Background(), Input{OutputPath: out})
	if !errors.Is(err, freeErr) || err.Error() != freeErr.Error() || !reflect.DeepEqual(f.trace, []string{"new", "encode", "free"}) {
		t.Fatalf("successful encode failed free: %v %v", err, f.trace)
	}
	check()
}
func TestMigrationNativeInvalidFormatPrecedesCanceledContext(t *testing.T) {
	out, check := migrationLifecycleOutput(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	err := NewRustEncoder().Encode(ctx, Input{OutputPath: out, Format: "invalid"})
	if err == nil || err.Error() != `invalid ugoira animation format "invalid"; expected gif or apng` || errors.Is(err, context.Canceled) {
		t.Fatalf("format/context validation order: %v", err)
	}
	check()
}

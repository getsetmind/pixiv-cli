package protocol

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"errors"
	"flag"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"golang.org/x/net/http2"
	"golang.org/x/net/http2/hpack"
)

var migrationUpdateNativeProfile = flag.Bool("migration-update-fanbox-native-profile", false, "capture fixed Go native TLS and single-GET HTTP2 observations")

type migrationNativeSource struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
	Files  int    `json:"files,omitempty"`
}

type migrationNativeExtension struct {
	ID             uint16 `json:"id"`
	Length         int    `json:"length"`
	DataHex        string `json:"data_hex"`
	ComparableID   uint16 `json:"comparable_id"`
	ComparableData string `json:"comparable_data"`
}

type migrationNativeHello struct {
	RecordHex      string                     `json:"record_hex"`
	RecordSHA256   string                     `json:"record_sha256"`
	RecordVersion  uint16                     `json:"record_version"`
	LegacyVersion  uint16                     `json:"legacy_version"`
	RandomHex      string                     `json:"random_hex"`
	SessionIDHex   string                     `json:"session_id_hex"`
	CipherSuites   []uint16                   `json:"cipher_suites"`
	CompressionHex string                     `json:"compression_hex"`
	Extensions     []migrationNativeExtension `json:"extensions"`
}

type migrationNativeFrame struct {
	Type     string               `json:"type"`
	Flags    uint8                `json:"flags"`
	Stream   uint32               `json:"stream"`
	RawHex   string               `json:"raw_hex"`
	Settings [][2]uint32          `json:"settings,omitempty"`
	Window   uint32               `json:"window,omitempty"`
	Reset    uint32               `json:"reset,omitempty"`
	Fields   []hpack.HeaderField  `json:"fields,omitempty"`
	Priority *http2.PriorityParam `json:"priority,omitempty"`
}

type migrationNativeConnection struct {
	Hello          migrationNativeHello   `json:"hello"`
	TLSVersion     uint16                 `json:"tls_version"`
	CipherSuite    uint16                 `json:"cipher_suite"`
	ALPN           string                 `json:"alpn"`
	SNI            string                 `json:"sni"`
	DidResume      bool                   `json:"did_resume"`
	HandshakeError string                 `json:"handshake_error"`
	PrefaceHex     string                 `json:"preface_hex"`
	Frames         []migrationNativeFrame `json:"frames"`
	TerminalError  string                 `json:"terminal_error"`
}

type migrationNativeError struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}

type migrationNativeObservation struct {
	Name                         string                      `json:"name"`
	Connections                  []migrationNativeConnection `json:"connections"`
	ResponseJSON                 []json.RawMessage           `json:"response_json"`
	Errors                       []migrationNativeError      `json:"errors"`
	Canceled                     bool                        `json:"canceled"`
	Deadline                     bool                        `json:"deadline"`
	UnknownAuthority             bool                        `json:"unknown_authority"`
	Hostname                     bool                        `json:"hostname"`
	Expired                      bool                        `json:"expired"`
	AliveAfterIdleClose          bool                        `json:"alive_after_idle_close"`
	AliveAfterThirtySeconds      bool                        `json:"alive_after_thirty_seconds"`
	IdleCloseObserved            bool                        `json:"idle_close_observed"`
	CompletedIdleLivenessChecks  int                         `json:"completed_idle_liveness_checks"`
	ElapsedNS                    int64                       `json:"elapsed_ns"`
	PeerRequestObservedElapsedNS int64                       `json:"peer_request_observed_elapsed_ns"`
	PeerHeaderHoldNS             int64                       `json:"peer_header_hold_ns"`
	Endpoint                     string                      `json:"endpoint"`
	CertificateDERHex            string                      `json:"certificate_der_hex"`
	TrustSubjects                []string                    `json:"trust_subjects"`
}

type migrationNativeCase struct {
	Name              string                     `json:"name"`
	Comparable        json.RawMessage            `json:"comparable"`
	GoOnlyObservation migrationNativeObservation `json:"go_only_observation"`
}

type migrationNativeFixture struct {
	Reference        string                  `json:"reference"`
	GoVersion        string                  `json:"go_version"`
	Platform         string                  `json:"platform"`
	Sources          []migrationNativeSource `json:"sources"`
	Dependencies     []migrationNativeSource `json:"dependencies"`
	ToolchainSources []migrationNativeSource `json:"toolchain_sources"`
	Evidence         string                  `json:"evidence"`
	ReplayPolicy     []string                `json:"replay_policy"`
	Limitations      []string                `json:"limitations"`
	Cases            []migrationNativeCase   `json:"cases"`
}

func TestMigrationFanboxNativeProfile(t *testing.T) {
	if runtime.GOOS != "linux" || runtime.GOARCH != "amd64" || runtime.Version() != "go1.27.1" {
		t.Fatalf("native capture requires pinned linux/amd64 Go1.27.1, got %s/%s %s", runtime.GOOS, runtime.GOARCH, runtime.Version())
	}
	root := filepath.Join("..", "..", "..", "..")
	sources := migrationNativeSourceGuards(t, root)
	dependencies := migrationNativeDependencyGuards(t)
	toolchainSources := migrationNativeToolchainGuards(t)
	fixture := migrationNativeFixture{
		Reference: "4b4426487ef18bed276706daec385e0d0a6979f9", GoVersion: runtime.Version(), Platform: runtime.GOOS + "/" + runtime.GOARCH,
		Sources: sources, Dependencies: dependencies, ToolchainSources: toolchainSources,
		Evidence: "Fresh execution of unchanged newBrowserTransport and its tls-client/fhttp/uTLS boundaries against owned TLS1.3/H2 peers. Session headers use the actual NewSessionWithOptions/GetJSON path with only a declared loopback URL adapter. No copied profile or production TLS configuration changes. Each peer and trust environment exists only inside a bounded owned child.",
		ReplayPolicy: []string{
			"Comparable fields are exact observations, not source labels: complete extension order and fixed extension data, cipher order, key-share group/length, ECH fixed fields and allowed dynamic payload lengths, decrypted HTTP2 SETTINGS order/values, window, HEADERS priority/flags/ordered fields, certificate classifications, sequential reuse and physical idle closure.",
			"Go-only observations retain complete raw ClientHello records, all random/GREASE/key-share/ECH bytes and selected lengths, all decoded fields and decrypted frame bytes, exact raw error chains and elapsed time. They are not expected to equal a later random run.",
			"Only actual GREASE codepoints are canonicalized to 0x0a0a; their positions and fixed extension payload bytes remain exact. Random/session/key-share key bytes are compared by exact required length, not value. ECH config/key/payload randomness and its four source-defined possible lengths are named explicitly.",
			"Direct-factory authority and certificate-failure outer URL replace only this run's owned localhost port in comparable fields; raw fields/errors remain lossless. Expired-certificate comparison replaces only the parsed current-time token, retaining all other diagnostic text and typed x509 reason.",
			"SETTINGS ACK arrival relative to a later sequential request is scheduling-dependent. All raw frames are retained; ACK multiplicity and initial SETTINGS/window/HEADERS remain checked. The peer sends SETTINGS only after receiving the first complete GET HEADERS.",
		},
		Limitations: []string{
			"Linux/amd64 owned synthetic peer proof only; no real FANBOX/media/account, user computer, OS trust change, external network or Rust runtime equivalence.",
			"Session adapter changes only request URL host to owned localhost:port and preserves original api.fanbox.cc authority, path, query, method and headers. TLS SNI/certificate target is localhost, not FANBOX. Session uses an explicit injected client containing the genuine factory transport; default factory TLS/HTTP2 options are unchanged.",
			"TLS1.2 negotiation, resumption/PSK, HRR, HTTP1 fallback, proxy runtime, HTTP3, other platforms/trust stores and concurrency remain untested. Absence of PSK in fresh TLS1.3 captures does not certify resumption.",
			"Stopped supplemental HTTP2 multiplex/unfinished HEAD/upload probe is not executed, reconstructed or used as evidence. This slice uses sequential completed GETs and one stalled response-header GET only.",
			"No body-close cancellation, concurrent Read/Close, compression, legacy deflate, HEADERS END_STREAM/nonzero length or unfinished request-body contract is claimed.",
			"A pending response-header GET is held for at least 31.2 seconds after the peer observes its complete GET HEADERS, proving survival beyond the dependency's default 30-second total timeout in this run; it does not prove an infinite wait. Explicit caller cancellation/deadline and idle-only cleanup are observed separately. Completed idle connections are checked live for 50ms before each explicit cleanup; physical closure must end in peer EOF.",
			"Fixed extension ordering is checked across this bounded sample. GREASE randomness/distribution and every ECH length are not statistically verified; source provenance alone is never runtime equivalence.",
		}, Cases: []migrationNativeCase{},
	}
	for _, name := range []string{"headers_sequential_idle", "response_header_stall", "caller_deadline", "untrusted_certificate", "invalid_hostname", "expired_certificate"} {
		t.Run(name, func(t *testing.T) {
			observation := migrationRunNativeChild(t, name)
			comparable := migrationNativeComparable(t, observation)
			fixture.Cases = append(fixture.Cases, migrationNativeCase{name, comparable, observation})
		})
	}
	if t.Failed() {
		return
	}
	path := filepath.Join(root, "crates", "pixiv-sdk", "tests", "fixtures", "fanbox-native-profile.json")
	if *migrationUpdateNativeProfile {
		data, err := json.MarshalIndent(fixture, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected migrationNativeFixture
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	for i := range expected.Cases {
		migrationNativeValidateWitness(t, expected.Cases[i])
		if i >= len(fixture.Cases) {
			t.Fatal("native case inventory changed")
		}
		if !bytes.Equal(expected.Cases[i].Comparable, fixture.Cases[i].Comparable) {
			var left, right any
			if err := json.Unmarshal(expected.Cases[i].Comparable, &left); err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(fixture.Cases[i].Comparable, &right); err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(left, right) {
				t.Errorf("%s native comparable changed\nwant %s\ngot %s", fixture.Cases[i].Name, expected.Cases[i].Comparable, fixture.Cases[i].Comparable)
			}
		}
		fixture.Cases[i].GoOnlyObservation = expected.Cases[i].GoOnlyObservation
		fixture.Cases[i].Comparable = expected.Cases[i].Comparable
	}
	if !reflect.DeepEqual(expected, fixture) {
		t.Error("native fixture metadata or case inventory changed")
	}
}

func migrationNativeSourceGuards(t *testing.T, root string) []migrationNativeSource {
	t.Helper()
	sources := []migrationNativeSource{
		{Path: "internal/services/fanbox/protocol/protocol.go", SHA256: "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3"},
		{Path: "internal/services/fanbox/protocol/cookie.go", SHA256: "692013694d29e4fe67cee7641c4dcdafe73158bea107666c194e6305d33b45a9"},
		{Path: "internal/services/fanbox/protocol/solver.go", SHA256: "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea"},
		{Path: "go.mod", SHA256: "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c"},
		{Path: "go.sum", SHA256: "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e"},
	}
	for _, source := range sources {
		data, err := os.ReadFile(filepath.Join(root, source.Path))
		if err != nil {
			t.Fatal(err)
		}
		command := exec.Command("git", "show", "4b4426487ef18bed276706daec385e0d0a6979f9:"+source.Path)
		command.Dir = root
		frozen, err := command.Output()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(data, frozen) || fmt.Sprintf("%x", sha256.Sum256(data)) != source.SHA256 {
			t.Fatalf("changed frozen source %s", source.Path)
		}
	}
	return sources
}

func migrationNativeDependencyGuards(t *testing.T) []migrationNativeSource {
	t.Helper()
	sources := []migrationNativeSource{
		{Path: "github.com/bogdanfinn/fhttp@v0.6.8", SHA256: "d430be56baa9727bf6517cd918f3f3081426aaa3e0cfb63a25ac83e39bc406a0", Files: 169},
		{Path: "github.com/bogdanfinn/tls-client@v1.15.1", SHA256: "1e4f80908c65ce4e1b1de0453a9c4d41124a1cebbb53ac4f17227e8cae80887a", Files: 53},
		{Path: "github.com/bogdanfinn/utls@v1.7.7-barnius", SHA256: "d1cc088b2e51ea0985db3b47379d7f243336b9a65f902eb787068807de037986", Files: 311},
		{Path: "golang.org/x/net@v0.48.0", SHA256: "45f7f6d536c2ef2ca8bddc2caa79a84fbd8b011058ed5b33c5f83a8f8ac901a4", Files: 826},
	}
	cache := os.Getenv("GOMODCACHE")
	if cache == "" {
		t.Fatal("canonical GOMODCACHE must be explicit")
	}
	for _, source := range sources {
		root := filepath.Join(cache, source.Path)
		var paths []string
		err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if entry.Type().IsRegular() {
				paths = append(paths, path)
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
		sort.Strings(paths)
		hash := sha256.New()
		for _, path := range paths {
			data, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			relative, err := filepath.Rel(root, path)
			if err != nil {
				t.Fatal(err)
			}
			fmt.Fprintf(hash, "%s\x00%x\n", filepath.ToSlash(relative), sha256.Sum256(data))
		}
		if len(paths) != source.Files || fmt.Sprintf("%x", hash.Sum(nil)) != source.SHA256 {
			t.Fatalf("official dependency source changed: %s files=%d sha256=%x want files=%d sha256=%s", source.Path, len(paths), hash.Sum(nil), source.Files, source.SHA256)
		}
	}
	return sources
}

func migrationNativeToolchainGuards(t *testing.T) []migrationNativeSource {
	t.Helper()
	sources := []migrationNativeSource{
		{Path: "crypto/x509/root.go", SHA256: "bcad6caaa9c0780b87b0509e112600996514184abe1e304bf468e200cacb6223"},
		{Path: "crypto/x509/root_unix.go", SHA256: "7e4b6024e648ca2cc2359049eeca3780388c02debdfd9db66144f7ebe06cf5ac"},
		{Path: "crypto/x509/verify.go", SHA256: "8b3d059f20a75ac8a103d97b473fbe90e01d40654c4548c50d9b0937f54f3c8f"},
		{Path: "crypto/tls/handshake_server_tls13.go", SHA256: "6cb325a7623c3bfe60e066b08fed2cc64e6b6cff85ad8f6467c038968d3a7fe6"},
		{Path: "net/http/client.go", SHA256: "ced3428a85206de8de79c10de38d34951e0b9823c0ccb68ff51329d048a1f7b9"},
	}
	for _, source := range sources {
		data, err := os.ReadFile(filepath.Join(runtime.GOROOT(), "src", source.Path))
		if err != nil {
			t.Fatal(err)
		}
		if fmt.Sprintf("%x", sha256.Sum256(data)) != source.SHA256 {
			t.Fatalf("pinned Go native boundary changed: %s", source.Path)
		}
	}
	return sources
}

func migrationRunNativeChild(t *testing.T, name string) migrationNativeObservation {
	t.Helper()
	dir := t.TempDir()
	if err := os.Mkdir(filepath.Join(dir, "empty-cert-dir"), 0o700); err != nil {
		t.Fatal(err)
	}
	rootCert, certificate := migrationNativeCertificates(t, name)
	for path, data := range map[string][]byte{"root.pem": rootCert, "peer.json": certificate} {
		if err := os.WriteFile(filepath.Join(dir, path), data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 55*time.Second)
	defer cancel()
	command := exec.CommandContext(ctx, executable, "-test.run=^TestMigrationFanboxNativeProfileChild$", "-test.v")
	command.WaitDelay = 2 * time.Second
	command.Env = append(os.Environ(), "PIXIV_NATIVE_PROFILE_CHILD="+name, "PIXIV_NATIVE_PROFILE_DIR="+dir, "SSL_CERT_FILE="+filepath.Join(dir, "root.pem"), "SSL_CERT_DIR="+filepath.Join(dir, "empty-cert-dir"))
	output, err := command.CombinedOutput()
	t.Logf("owned native child %s:\n%s", name, output)
	if err != nil {
		t.Fatalf("owned native child %s: %v", name, err)
	}
	data, err := os.ReadFile(filepath.Join(dir, "observation.json"))
	if err != nil {
		t.Fatal(err)
	}
	var observation migrationNativeObservation
	if err := json.Unmarshal(data, &observation); err != nil {
		t.Fatal(err)
	}
	return observation
}

func migrationNativeCertificates(t *testing.T, name string) ([]byte, []byte) {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	root := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "Owned FANBOX native contract root"}, NotBefore: time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC), NotAfter: time.Date(2035, 1, 1, 0, 0, 0, 0, time.UTC), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	der, err := x509.CreateCertificate(rand.Reader, root, root, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	rootPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
	if name == "untrusted_certificate" {
		key, err = ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		root.Subject.CommonName = "Owned untrusted native root"
	}
	peerKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	peer := &x509.Certificate{SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "Owned native loopback peer"}, DNSNames: []string{"localhost"}, NotBefore: root.NotBefore, NotAfter: root.NotAfter, KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	if name == "invalid_hostname" {
		peer.DNSNames = []string{"wrong-owned.invalid"}
	}
	if name == "expired_certificate" {
		peer.NotAfter = time.Date(2021, 1, 1, 0, 0, 0, 0, time.UTC)
	}
	der, err = x509.CreateCertificate(rand.Reader, peer, root, &peerKey.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	keyDER, err := x509.MarshalECPrivateKey(peerKey)
	if err != nil {
		t.Fatal(err)
	}
	data, err := json.Marshal(struct{ Cert, Key []byte }{der, keyDER})
	if err != nil {
		t.Fatal(err)
	}
	return rootPEM, data
}

type migrationNativeLoopbackAdapter struct {
	transport   *browserTransport
	destination string
}

func (adapter migrationNativeLoopbackAdapter) RoundTrip(request *http.Request) (*http.Response, error) {
	copy := request.Clone(request.Context())
	target := *request.URL
	target.Host = adapter.destination
	copy.URL = &target
	copy.Host = request.URL.Host
	return adapter.transport.RoundTrip(copy)
}

func (adapter migrationNativeLoopbackAdapter) CloseIdleConnections() {
	adapter.transport.CloseIdleConnections()
}

type migrationNativeReplayConn struct {
	net.Conn
	reader io.Reader
}

func (conn migrationNativeReplayConn) Read(data []byte) (int, error) { return conn.reader.Read(data) }

type migrationNativePeer struct {
	listener    net.Listener
	certificate tls.Certificate
	mode        string
	mu          sync.Mutex
	connections []*migrationNativeConnection
	sockets     []net.Conn
	request     chan struct{}
	reset       chan uint32
	closed      chan struct{}
	stopped     chan struct{}
	wg          sync.WaitGroup
}

func migrationStartNativePeer(t *testing.T, name string, certificate tls.Certificate) *migrationNativePeer {
	t.Helper()
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	peer := &migrationNativePeer{listener: listener, certificate: certificate, mode: name, request: make(chan struct{}, 8), reset: make(chan uint32, 8), closed: make(chan struct{}, 8), stopped: make(chan struct{})}
	peer.wg.Add(1)
	go func() {
		defer peer.wg.Done()
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			peer.mu.Lock()
			peer.sockets = append(peer.sockets, conn)
			observation := &migrationNativeConnection{Frames: []migrationNativeFrame{}}
			peer.connections = append(peer.connections, observation)
			peer.mu.Unlock()
			peer.wg.Add(1)
			go func() { defer peer.wg.Done(); peer.serve(conn, observation) }()
		}
	}()
	t.Cleanup(func() {
		close(peer.stopped)
		_ = peer.listener.Close()
		peer.mu.Lock()
		for _, conn := range peer.sockets {
			_ = conn.Close()
		}
		peer.mu.Unlock()
		drained := make(chan struct{})
		go func() { peer.wg.Wait(); close(drained) }()
		select {
		case <-drained:
		case <-time.After(3 * time.Second):
			t.Error("owned native peer did not drain")
		}
	})
	return peer
}

func (peer *migrationNativePeer) serve(raw net.Conn, observation *migrationNativeConnection) {
	defer raw.Close()
	defer func() { peer.closed <- struct{}{} }()
	_ = raw.SetDeadline(time.Now().Add(45 * time.Second))
	recordHeader := make([]byte, 5)
	if _, err := io.ReadFull(raw, recordHeader); err != nil {
		observation.TerminalError = err.Error()
		return
	}
	record := append([]byte(nil), recordHeader...)
	data := make([]byte, binary.BigEndian.Uint16(recordHeader[3:]))
	if _, err := io.ReadFull(raw, data); err != nil {
		observation.TerminalError = err.Error()
		return
	}
	record = append(record, data...)
	hello, err := migrationParseNativeHello(record)
	if err != nil {
		observation.TerminalError = err.Error()
		return
	}
	observation.Hello = hello
	conn := tls.Server(migrationNativeReplayConn{raw, io.MultiReader(bytes.NewReader(record), raw)}, &tls.Config{Certificates: []tls.Certificate{peer.certificate}, MinVersion: tls.VersionTLS13, MaxVersion: tls.VersionTLS13, NextProtos: []string{"h2"}, SessionTicketsDisabled: true})
	if err := conn.Handshake(); err != nil {
		observation.HandshakeError = err.Error()
		return
	}
	state := conn.ConnectionState()
	observation.TLSVersion = state.Version
	observation.CipherSuite = state.CipherSuite
	observation.ALPN = state.NegotiatedProtocol
	observation.SNI = state.ServerName
	observation.DidResume = state.DidResume
	preface := make([]byte, len(http2.ClientPreface))
	if _, err := io.ReadFull(conn, preface); err != nil {
		observation.TerminalError = err.Error()
		return
	}
	observation.PrefaceHex = hex.EncodeToString(preface)
	if string(preface) != http2.ClientPreface {
		observation.TerminalError = "invalid HTTP2 preface"
		return
	}
	var recorded bytes.Buffer
	framer := http2.NewFramer(conn, io.TeeReader(conn, &recorded))
	framer.ReadMetaHeaders = hpack.NewDecoder(65536, nil)
	settingsSent := false
	pendingSettingsACKs := 0
	for {
		start := recorded.Len()
		frame, err := framer.ReadFrame()
		if err != nil {
			observation.TerminalError = err.Error()
			return
		}
		header := frame.Header()
		item := migrationNativeFrame{Type: header.Type.String(), Flags: uint8(header.Flags), Stream: header.StreamID, RawHex: hex.EncodeToString(recorded.Bytes()[start:])}
		switch value := frame.(type) {
		case *http2.SettingsFrame:
			_ = value.ForeachSetting(func(setting http2.Setting) error {
				item.Settings = append(item.Settings, [2]uint32{uint32(setting.ID), setting.Val})
				return nil
			})
			if !value.IsAck() {
				pendingSettingsACKs++
			}
		case *http2.WindowUpdateFrame:
			item.Window = value.Increment
		case *http2.RSTStreamFrame:
			item.Reset = uint32(value.ErrCode)
			peer.reset <- uint32(value.ErrCode)
		case *http2.MetaHeadersFrame:
			item.Fields = append([]hpack.HeaderField(nil), value.Fields...)
			if value.HasPriority() {
				priority := value.Priority
				item.Priority = &priority
			}
		}
		observation.Frames = append(observation.Frames, item)
		if headers, ok := frame.(*http2.MetaHeadersFrame); ok {
			if !headers.StreamEnded() {
				observation.TerminalError = "GET did not end request stream"
				return
			}
			peer.request <- struct{}{}
			if !settingsSent {
				if err := framer.WriteSettings(); err != nil {
					observation.TerminalError = err.Error()
					return
				}
				settingsSent = true
			}
			for pendingSettingsACKs > 0 {
				if err := framer.WriteSettingsAck(); err != nil {
					observation.TerminalError = err.Error()
					return
				}
				pendingSettingsACKs--
			}
			if peer.mode == "response_header_stall" || peer.mode == "caller_deadline" {
				continue
			}
			var block bytes.Buffer
			encoder := hpack.NewEncoder(&block)
			for _, field := range []hpack.HeaderField{{Name: ":status", Value: "200"}, {Name: "content-type", Value: "application/json"}, {Name: "content-length", Value: "11"}} {
				if err := encoder.WriteField(field); err != nil {
					observation.TerminalError = err.Error()
					return
				}
			}
			if err := framer.WriteHeaders(http2.HeadersFrameParam{StreamID: headers.StreamID, BlockFragment: block.Bytes(), EndHeaders: true}); err != nil {
				observation.TerminalError = err.Error()
				return
			}
			if err := framer.WriteData(headers.StreamID, true, []byte(`{"ok":true}`)); err != nil {
				observation.TerminalError = err.Error()
				return
			}
		}
		if pendingSettingsACKs > 0 && settingsSent {
			if err := framer.WriteSettingsAck(); err != nil {
				observation.TerminalError = err.Error()
				return
			}
			pendingSettingsACKs--
		}
	}
}

func TestMigrationFanboxNativeProfileChild(t *testing.T) {
	name := os.Getenv("PIXIV_NATIVE_PROFILE_CHILD")
	if name == "" {
		t.Skip("owned native child requires explicit parent mode")
	}
	migrationNativeVerifyLocalhost(t)
	dir := os.Getenv("PIXIV_NATIVE_PROFILE_DIR")
	if os.Getenv("SSL_CERT_FILE") != filepath.Join(dir, "root.pem") || os.Getenv("SSL_CERT_DIR") != filepath.Join(dir, "empty-cert-dir") {
		t.Fatal("owned child trust environment differs")
	}
	data, err := os.ReadFile(filepath.Join(dir, "peer.json"))
	if err != nil {
		t.Fatal(err)
	}
	var certificateData struct{ Cert, Key []byte }
	if err := json.Unmarshal(data, &certificateData); err != nil {
		t.Fatal(err)
	}
	key, err := x509.ParseECPrivateKey(certificateData.Key)
	if err != nil {
		t.Fatal(err)
	}
	certificate := tls.Certificate{Certificate: [][]byte{certificateData.Cert}, PrivateKey: key}
	peer := migrationStartNativePeer(t, name, certificate)
	transport, err := newBrowserTransport("")
	if err != nil {
		t.Fatal(err)
	}
	defer transport.CloseIdleConnections()
	_, port, err := net.SplitHostPort(peer.listener.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	endpoint := "localhost:" + port
	observation := migrationNativeObservation{Name: name, Connections: []migrationNativeConnection{}, ResponseJSON: []json.RawMessage{}, Errors: []migrationNativeError{}, Endpoint: endpoint, CertificateDERHex: hex.EncodeToString(certificateData.Cert), TrustSubjects: []string{}}
	roots, err := x509.SystemCertPool()
	if err != nil {
		t.Fatal(err)
	}
	for _, subject := range roots.Subjects() {
		observation.TrustSubjects = append(observation.TrustSubjects, hex.EncodeToString(subject))
	}
	if len(observation.TrustSubjects) != 1 {
		t.Fatalf("child trust roots=%d, want exactly owned root", len(observation.TrustSubjects))
	}
	client := &http.Client{Transport: migrationNativeLoopbackAdapter{transport, endpoint}}
	session, err := NewSessionWithOptions("FANBOXSESSID=owned-native-session", SessionOptions{HTTPClient: client})
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	if name == "headers_sequential_idle" {
		for _, suffix := range []string{"first", "second"} {
			var value json.RawMessage
			if err := session.GetJSON(context.Background(), "https://api.fanbox.cc/native/"+suffix+"?owned=1", &value); err != nil {
				t.Fatal(err)
			}
			observation.ResponseJSON = append(observation.ResponseJSON, value)
		}
		select {
		case <-peer.closed:
			t.Fatal("completed connection closed before explicit idle cleanup")
		case <-time.After(50 * time.Millisecond):
			observation.CompletedIdleLivenessChecks++
		}
		session.CloseIdleConnections()
		select {
		case <-peer.closed:
			observation.IdleCloseObserved = true
		case <-time.After(2 * time.Second):
			t.Fatal("completed idle connection not physically closed")
		}
		custom, err := NewSessionWithOptions("FANBOXSESSID=owned-native-session", SessionOptions{HTTPClient: client, UserAgent: "Owned Native UA/1"})
		if err != nil {
			t.Fatal(err)
		}
		var value json.RawMessage
		if err := custom.GetJSON(context.Background(), "https://api.fanbox.cc/native/third?owned=1", &value); err != nil {
			t.Fatal(err)
		}
		observation.ResponseJSON = append(observation.ResponseJSON, value)
		select {
		case <-peer.closed:
			t.Fatal("custom completed connection closed before explicit idle cleanup")
		case <-time.After(50 * time.Millisecond):
			observation.CompletedIdleLivenessChecks++
		}
		custom.CloseIdleConnections()
		select {
		case <-peer.closed:
		case <-time.After(2 * time.Second):
			t.Fatal("second completed idle connection not physically closed")
		}
	} else {
		ctx, cancel := context.WithCancel(context.Background())
		if name == "caller_deadline" {
			cancel()
			ctx, cancel = context.WithTimeout(context.Background(), 300*time.Millisecond)
		}
		result := make(chan error, 1)
		go func() {
			request, buildErr := http.NewRequestWithContext(ctx, http.MethodGet, "https://"+endpoint+"/owned-native", nil)
			if buildErr != nil {
				result <- buildErr
				return
			}
			response, requestErr := transport.RoundTrip(request)
			if response != nil && response.Body != nil {
				_ = response.Body.Close()
			}
			result <- requestErr
		}()
		if name == "response_header_stall" || name == "caller_deadline" {
			var peerObservedAt time.Time
			select {
			case <-peer.request:
				peerObservedAt = time.Now()
				observation.PeerRequestObservedElapsedNS = time.Since(start).Nanoseconds()
			case <-time.After(3 * time.Second):
				t.Fatal("owned single GET did not reach peer")
			}
			transport.CloseIdleConnections()
			select {
			case <-peer.closed:
				t.Fatal("CloseIdleConnections closed active GET")
			case requestErr := <-result:
				t.Fatalf("active GET ended before caller boundary: %v", requestErr)
			case <-time.After(50 * time.Millisecond):
				observation.AliveAfterIdleClose = true
			}
			if name == "response_header_stall" {
				delay := 31200*time.Millisecond - time.Since(peerObservedAt)
				select {
				case requestErr := <-result:
					t.Fatalf("GET ended before 31.2-second boundary: %v", requestErr)
				case <-peer.closed:
					t.Fatal("owned peer closed during stall")
				case <-time.After(delay):
					observation.AliveAfterThirtySeconds = true
					observation.PeerHeaderHoldNS = time.Since(peerObservedAt).Nanoseconds()
				}
				cancel()
			}
		}
		select {
		case err = <-result:
		case <-time.After(3 * time.Second):
			cancel()
			t.Fatal("native request did not drain after caller boundary")
		}
		cancel()
		observation.ElapsedNS = time.Since(start).Nanoseconds()
		if err == nil {
			t.Fatal("negative native case unexpectedly succeeded")
		}
		observation.Canceled = errors.Is(err, context.Canceled)
		observation.Deadline = errors.Is(err, context.DeadlineExceeded)
		var unknown x509.UnknownAuthorityError
		observation.UnknownAuthority = errors.As(err, &unknown)
		var hostname x509.HostnameError
		observation.Hostname = errors.As(err, &hostname)
		var invalid x509.CertificateInvalidError
		observation.Expired = errors.As(err, &invalid) && invalid.Reason == x509.Expired
		for current := err; current != nil; current = errors.Unwrap(current) {
			observation.Errors = append(observation.Errors, migrationNativeError{fmt.Sprintf("%T", current), current.Error()})
		}
		if name == "response_header_stall" || name == "caller_deadline" {
			select {
			case code := <-peer.reset:
				if code != uint32(http2.ErrCodeCancel) {
					t.Fatalf("caller cancellation reset=%d", code)
				}
			case <-time.After(2 * time.Second):
				t.Fatal("caller cancellation emitted no physical CANCEL")
			}
		}
		transport.CloseIdleConnections()
		select {
		case <-peer.closed:
		case <-time.After(2 * time.Second):
			t.Fatal("owned negative peer did not close")
		}
	}
	observation.ElapsedNS = time.Since(start).Nanoseconds()
	peer.mu.Lock()
	sockets := append([]net.Conn(nil), peer.sockets...)
	peer.mu.Unlock()
	for _, conn := range sockets {
		_ = conn.Close()
	}
	_ = peer.listener.Close()
	drained := make(chan struct{})
	go func() { peer.wg.Wait(); close(drained) }()
	select {
	case <-drained:
	case <-time.After(3 * time.Second):
		t.Fatal("owned native child did not drain before snapshot")
	}
	peer.mu.Lock()
	for _, conn := range peer.connections {
		observation.Connections = append(observation.Connections, *conn)
	}
	peer.mu.Unlock()
	for _, conn := range observation.Connections {
		if conn.ALPN == "h2" && conn.TerminalError != "EOF" {
			t.Fatalf("owned native peer closure was not client EOF: %s", conn.TerminalError)
		}
	}
	if name == "headers_sequential_idle" && (len(observation.Connections) != 2 || len(observation.ResponseJSON) != 3) {
		t.Fatalf("sequential native lifecycle: connections=%d responses=%d", len(observation.Connections), len(observation.ResponseJSON))
	}
	if name == "response_header_stall" && (!observation.Canceled || !observation.AliveAfterThirtySeconds || observation.PeerHeaderHoldNS < 31200*time.Millisecond.Nanoseconds()) {
		t.Fatal("31.2s stall/cancellation observation missing")
	}
	if name == "caller_deadline" && !observation.Deadline {
		t.Fatal("caller deadline classification missing")
	}
	if name == "untrusted_certificate" && !observation.UnknownAuthority {
		t.Fatal("unknown authority classification missing")
	}
	if name == "invalid_hostname" && !observation.Hostname {
		t.Fatal("hostname classification missing")
	}
	if name == "expired_certificate" && !observation.Expired {
		t.Fatal("expired certificate classification missing")
	}
	data, err = json.MarshalIndent(observation, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "observation.json"), append(data, '\n'), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Logf("native observation %s: connections=%d responses=%d elapsed=%s canceled=%v deadline=%v trust_roots=%d", name, len(observation.Connections), len(observation.ResponseJSON), time.Duration(observation.ElapsedNS), observation.Canceled, observation.Deadline, len(observation.TrustSubjects))
}

func migrationNativeVerifyLocalhost(t *testing.T) {
	t.Helper()
	data, err := os.ReadFile("/etc/hosts")
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, line := range strings.Split(string(data), "\n") {
		line, _, _ = strings.Cut(line, "#")
		fields := strings.Fields(line)
		if len(fields) < 2 {
			continue
		}
		for _, host := range fields[1:] {
			if host == "localhost" {
				ip := net.ParseIP(fields[0])
				if ip == nil || !ip.IsLoopback() {
					t.Fatal("localhost is not confined to loopback")
				}
				found = true
			}
		}
	}
	if !found {
		t.Fatal("localhost has no owned-loopback hosts entry")
	}
	data, err = os.ReadFile("/etc/nsswitch.conf")
	if err != nil {
		t.Fatal(err)
	}
	for _, line := range strings.Split(string(data), "\n") {
		if strings.HasPrefix(line, "hosts:") {
			if strings.Join(strings.Fields(line), " ") != "hosts: files dns" {
				t.Fatal("localhost resolver must consult hosts before DNS")
			}
			return
		}
	}
	t.Fatal("localhost resolver policy missing")
}

func migrationNativeValidateWitness(t *testing.T, item migrationNativeCase) {
	t.Helper()
	if item.Name == "response_header_stall" && item.GoOnlyObservation.PeerHeaderHoldNS < 31200*time.Millisecond.Nanoseconds() {
		t.Fatal("saved native stall witness has no actual post-HEADERS 31.2s hold")
	}
	for _, connection := range item.GoOnlyObservation.Connections {
		record, err := hex.DecodeString(connection.Hello.RecordHex)
		if err != nil {
			t.Fatal(err)
		}
		parsed, err := migrationParseNativeHello(record)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(parsed, connection.Hello) {
			t.Fatalf("%s raw ClientHello inventory/digest mismatch", item.Name)
		}
		var wire []byte
		for _, frame := range connection.Frames {
			raw, err := hex.DecodeString(frame.RawHex)
			if err != nil {
				t.Fatal(err)
			}
			wire = append(wire, raw...)
		}
		wireReader := bytes.NewReader(wire)
		framer := http2.NewFramer(io.Discard, wireReader)
		framer.ReadMetaHeaders = hpack.NewDecoder(65536, nil)
		for _, expected := range connection.Frames {
			start := len(wire) - wireReader.Len()
			frame, err := framer.ReadFrame()
			if err != nil {
				t.Fatal(err)
			}
			header := frame.Header()
			actual := migrationNativeFrame{Type: header.Type.String(), Flags: uint8(header.Flags), Stream: header.StreamID, RawHex: hex.EncodeToString(wire[start : len(wire)-wireReader.Len()])}
			switch value := frame.(type) {
			case *http2.SettingsFrame:
				_ = value.ForeachSetting(func(setting http2.Setting) error {
					actual.Settings = append(actual.Settings, [2]uint32{uint32(setting.ID), setting.Val})
					return nil
				})
			case *http2.WindowUpdateFrame:
				actual.Window = value.Increment
			case *http2.RSTStreamFrame:
				actual.Reset = uint32(value.ErrCode)
			case *http2.MetaHeadersFrame:
				actual.Fields = append([]hpack.HeaderField(nil), value.Fields...)
				if value.HasPriority() {
					priority := value.Priority
					actual.Priority = &priority
				}
			}
			if !reflect.DeepEqual(actual, expected) {
				t.Fatalf("%s raw HTTP2 frame inventory mismatch", item.Name)
			}
		}
		if wireReader.Len() != 0 {
			t.Fatalf("%s raw HTTP2 witness has trailing bytes", item.Name)
		}
	}
	certificate, err := hex.DecodeString(item.GoOnlyObservation.CertificateDERHex)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := x509.ParseCertificate(certificate); err != nil {
		t.Fatal(err)
	}
	projected := migrationNativeComparable(t, item.GoOnlyObservation)
	var left, right any
	if err := json.Unmarshal(projected, &left); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(item.Comparable, &right); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(left, right) {
		t.Fatalf("%s comparable projection differs from its lossless raw witness", item.Name)
	}
}

func migrationNativeGREASE(value uint16) uint16 {
	if value&0x0f0f == 0x0a0a && byte(value>>8) == byte(value) {
		return 0x0a0a
	}
	return value
}

func migrationParseNativeHello(record []byte) (migrationNativeHello, error) {
	hello := migrationNativeHello{RecordHex: hex.EncodeToString(record), RecordSHA256: fmt.Sprintf("%x", sha256.Sum256(record)), CipherSuites: []uint16{}, Extensions: []migrationNativeExtension{}}
	if len(record) < 9 || record[0] != 22 || record[5] != 1 || int(binary.BigEndian.Uint16(record[3:])) != len(record)-5 || int(record[6])<<16|int(record[7])<<8|int(record[8]) != len(record)-9 {
		return hello, errors.New("unexpected complete ClientHello record envelope")
	}
	hello.RecordVersion = binary.BigEndian.Uint16(record[1:])
	data := record[9:]
	take := func(count int) ([]byte, error) {
		if count > len(data) || count < 0 {
			return nil, io.ErrUnexpectedEOF
		}
		value := data[:count]
		data = data[count:]
		return value, nil
	}
	version, err := take(2)
	if err != nil {
		return hello, err
	}
	hello.LegacyVersion = binary.BigEndian.Uint16(version)
	randomData, err := take(32)
	if err != nil {
		return hello, err
	}
	hello.RandomHex = hex.EncodeToString(randomData)
	length, err := take(1)
	if err != nil {
		return hello, err
	}
	session, err := take(int(length[0]))
	if err != nil {
		return hello, err
	}
	hello.SessionIDHex = hex.EncodeToString(session)
	if len(randomData) != 32 || len(session) != 32 {
		return hello, errors.New("unexpected random/session-id length")
	}
	length, err = take(2)
	if err != nil {
		return hello, err
	}
	ciphers, err := take(int(binary.BigEndian.Uint16(length)))
	if err != nil || len(ciphers)%2 != 0 {
		return hello, errors.New("invalid cipher vector")
	}
	for len(ciphers) > 0 {
		hello.CipherSuites = append(hello.CipherSuites, binary.BigEndian.Uint16(ciphers))
		ciphers = ciphers[2:]
	}
	length, err = take(1)
	if err != nil {
		return hello, err
	}
	compression, err := take(int(length[0]))
	if err != nil {
		return hello, err
	}
	hello.CompressionHex = hex.EncodeToString(compression)
	length, err = take(2)
	if err != nil || int(binary.BigEndian.Uint16(length)) != len(data) {
		return hello, errors.New("invalid extension vector")
	}
	for len(data) > 0 {
		header, err := take(4)
		if err != nil {
			return hello, err
		}
		id := binary.BigEndian.Uint16(header)
		payload, err := take(int(binary.BigEndian.Uint16(header[2:])))
		if err != nil {
			return hello, err
		}
		extension := migrationNativeExtension{ID: id, Length: len(payload), DataHex: hex.EncodeToString(payload), ComparableID: migrationNativeGREASE(id), ComparableData: hex.EncodeToString(payload)}
		switch id {
		case 10, 43:
			copy := append([]byte(nil), payload...)
			offset := 2
			if id == 43 {
				offset = 1
			}
			for i := offset; i+1 < len(copy); i += 2 {
				binary.BigEndian.PutUint16(copy[i:], migrationNativeGREASE(binary.BigEndian.Uint16(copy[i:])))
			}
			extension.ComparableData = hex.EncodeToString(copy)
		case 51:
			if len(payload) < 2 || int(binary.BigEndian.Uint16(payload)) != len(payload)-2 {
				return hello, errors.New("invalid key share vector")
			}
			shares := payload[2:]
			var inventory []string
			for len(shares) > 0 {
				if len(shares) < 4 {
					return hello, io.ErrUnexpectedEOF
				}
				group := binary.BigEndian.Uint16(shares)
				size := int(binary.BigEndian.Uint16(shares[2:]))
				shares = shares[4:]
				if size > len(shares) {
					return hello, io.ErrUnexpectedEOF
				}
				marker := fmt.Sprintf("%04x:random[%d]", migrationNativeGREASE(group), size)
				if migrationNativeGREASE(group) == 0x0a0a {
					marker = fmt.Sprintf("0a0a:fixed[%s]", hex.EncodeToString(shares[:size]))
				}
				inventory = append(inventory, marker)
				shares = shares[size:]
			}
			extension.ComparableData = strings.Join(inventory, ",")
		case 0xfe0d:
			if len(payload) < 10 {
				return hello, io.ErrUnexpectedEOF
			}
			keyLength := int(binary.BigEndian.Uint16(payload[6:]))
			if keyLength+10 > len(payload) {
				return hello, io.ErrUnexpectedEOF
			}
			payloadLength := int(binary.BigEndian.Uint16(payload[8+keyLength:]))
			if keyLength != 32 || payloadLength != len(payload)-10-keyLength || (payloadLength != 144 && payloadLength != 176 && payloadLength != 208 && payloadLength != 240) {
				return hello, errors.New("unexpected GREASE ECH payload length")
			}
			extension.ComparableData = fmt.Sprintf("fixed[%s],config_id=random[1],enc=random[%d],payload=random[144|176|208|240]", hex.EncodeToString(payload[:5]), keyLength)
		}
		hello.Extensions = append(hello.Extensions, extension)
	}
	if len(hello.Extensions) < 2 || migrationNativeGREASE(hello.Extensions[0].ID) != 0x0a0a || migrationNativeGREASE(hello.Extensions[len(hello.Extensions)-1].ID) != 0x0a0a || hello.Extensions[0].ID == hello.Extensions[len(hello.Extensions)-1].ID {
		return hello, errors.New("GREASE extension slots must have distinct actual codepoints")
	}
	var curveGREASE, shareGREASE uint16
	for _, extension := range hello.Extensions {
		payload, _ := hex.DecodeString(extension.DataHex)
		if extension.ID == 10 && len(payload) >= 4 {
			curveGREASE = binary.BigEndian.Uint16(payload[2:])
		}
		if extension.ID == 51 && len(payload) >= 4 {
			shareGREASE = binary.BigEndian.Uint16(payload[2:])
		}
	}
	if curveGREASE != shareGREASE || migrationNativeGREASE(curveGREASE) != 0x0a0a {
		return hello, errors.New("key-share and supported-group GREASE relationship changed")
	}
	return hello, nil
}

func migrationNativeComparable(t *testing.T, observation migrationNativeObservation) json.RawMessage {
	t.Helper()
	data, err := json.Marshal(observation)
	if err != nil {
		t.Fatal(err)
	}
	var comparable map[string]any
	if err := json.Unmarshal(data, &comparable); err != nil {
		t.Fatal(err)
	}
	delete(comparable, "elapsed_ns")
	delete(comparable, "peer_request_observed_elapsed_ns")
	delete(comparable, "peer_header_hold_ns")
	delete(comparable, "endpoint")
	delete(comparable, "certificate_der_hex")
	for _, connValue := range comparable["connections"].([]any) {
		conn := connValue.(map[string]any)
		hello := conn["hello"].(map[string]any)
		delete(hello, "record_hex")
		delete(hello, "record_sha256")
		for _, name := range []string{"random_hex", "session_id_hex"} {
			value := hello[name].(string)
			hello[name] = fmt.Sprintf("random[%d]", len(value)/2)
		}
		for i, value := range hello["cipher_suites"].([]any) {
			hello["cipher_suites"].([]any)[i] = float64(migrationNativeGREASE(uint16(value.(float64))))
		}
		for _, value := range hello["extensions"].([]any) {
			extension := value.(map[string]any)
			extension["id"] = extension["comparable_id"]
			extension["data_hex"] = extension["comparable_data"]
			delete(extension, "comparable_id")
			delete(extension, "comparable_data")
			if extension["id"].(float64) == 0xfe0d {
				extension["length"] = "variable:186|218|250|282"
			}
		}
		var frames []any
		settingsACKs := 0
		for _, value := range conn["frames"].([]any) {
			frame := value.(map[string]any)
			delete(frame, "raw_hex")
			if frame["type"] == "SETTINGS" && frame["flags"].(float64) == 1 {
				settingsACKs++
				continue
			}
			if fields, ok := frame["fields"].([]any); ok {
				for _, fieldValue := range fields {
					field := fieldValue.(map[string]any)
					field["Value"] = strings.ReplaceAll(field["Value"].(string), observation.Endpoint, "localhost:<owned-port>")
				}
			}
			frames = append(frames, frame)
		}
		conn["frames"] = frames
		conn["settings_ack_count"] = settingsACKs
	}
	for _, value := range comparable["errors"].([]any) {
		cause := value.(map[string]any)
		message := strings.ReplaceAll(cause["message"].(string), observation.Endpoint, "localhost:<owned-port>")
		if observation.Expired {
			start := strings.Index(message, "current time ")
			if start < 0 {
				t.Fatal("expired certificate current-time token missing")
			}
			start += len("current time ")
			end := strings.Index(message[start:], " is after ")
			if end < 0 {
				t.Fatal("expired certificate ordering token missing")
			}
			if _, err := time.Parse(time.RFC3339, message[start:start+end]); err != nil {
				t.Fatal(err)
			}
			message = message[:start] + "<current-time>" + message[start+end:]
		}
		cause["message"] = message
	}
	data, err = json.MarshalIndent(comparable, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	return data
}

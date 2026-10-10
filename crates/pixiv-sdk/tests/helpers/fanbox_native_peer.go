package main

import (
	"bufio"
	"bytes"
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
	"fmt"
	"io"
	"math/big"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/net/http2"
	"golang.org/x/net/http2/hpack"
)

var outputMu sync.Mutex

func emit(event any) {
	outputMu.Lock()
	defer outputMu.Unlock()
	if err := json.NewEncoder(os.Stdout).Encode(event); err != nil {
		panic(err)
	}
}

func main() {
	if len(os.Args) != 3 {
		panic("expected owned case and directory")
	}
	name, dir := os.Args[1], os.Args[2]
	switch name {
	case "headers_sequential_idle", "response_header_stall", "caller_deadline", "untrusted_certificate", "invalid_hostname", "expired_certificate":
	default:
		panic("unapproved native case")
	}
	root, encoded := migrationNativeCertificates(name)
	if err := os.WriteFile(filepath.Join(dir, "root.pem"), root, 0600); err != nil {
		panic(err)
	}
	var data struct{ Cert, Key []byte }
	if err := json.Unmarshal(encoded, &data); err != nil {
		panic(err)
	}
	key, err := x509.ParseECPrivateKey(data.Key)
	if err != nil {
		panic(err)
	}
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	peer := &migrationNativePeer{listener: listener, certificate: tls.Certificate{Certificate: [][]byte{data.Cert}, PrivateKey: key}, mode: name, stopped: make(chan struct{})}
	peer.wg.Add(1)
	go func() {
		defer peer.wg.Done()
		for {
			raw, err := listener.Accept()
			if err != nil {
				return
			}
			peer.mu.Lock()
			observation := &migrationNativeConnection{Frames: []migrationNativeFrame{}}
			peer.connections = append(peer.connections, observation)
			peer.sockets = append(peer.sockets, raw)
			peer.mu.Unlock()
			peer.wg.Add(1)
			go func() { defer peer.wg.Done(); peer.serve(raw, observation) }()
		}
	}()
	block, _ := pem.Decode(root)
	if block == nil {
		panic("missing owned CA")
	}
	ca, err := x509.ParseCertificate(block.Bytes)
	if err != nil {
		panic(err)
	}
	_, port, err := net.SplitHostPort(listener.Addr().String())
	if err != nil {
		panic(err)
	}
	emit(map[string]any{"event": "ready", "endpoint": "localhost:" + port, "certificate_der_hex": hex.EncodeToString(data.Cert), "trust_subjects": []string{hex.EncodeToString(ca.RawSubject)}})
	scanner := bufio.NewScanner(os.Stdin)
	if !scanner.Scan() || scanner.Text() != "stop" {
		panic("missing bounded stop")
	}
	_ = listener.Close()
	peer.mu.Lock()
	for _, raw := range peer.sockets {
		_ = raw.Close()
	}
	peer.mu.Unlock()
	done := make(chan struct{})
	go func() { peer.wg.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(3 * time.Second):
		panic("owned peer failed to drain")
	}
	emit(map[string]any{"event": "report", "connections": peer.connections})
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

func migrationNativeCertificates(name string) ([]byte, []byte) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		panic(err)
	}
	root := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "Owned FANBOX native contract root"}, NotBefore: time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC), NotAfter: time.Date(2035, 1, 1, 0, 0, 0, 0, time.UTC), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	der, err := x509.CreateCertificate(rand.Reader, root, root, &key.PublicKey, key)
	if err != nil {
		panic(err)
	}
	rootPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
	if name == "untrusted_certificate" {
		key, err = ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		if err != nil {
			panic(err)
		}
		root.Subject.CommonName = "Owned untrusted native root"
	}
	peerKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		panic(err)
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
		panic(err)
	}
	keyDER, err := x509.MarshalECPrivateKey(peerKey)
	if err != nil {
		panic(err)
	}
	data, err := json.Marshal(struct{ Cert, Key []byte }{der, keyDER})
	if err != nil {
		panic(err)
	}
	return rootPEM, data
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
	stopped     chan struct{}
	wg          sync.WaitGroup
}

func (peer *migrationNativePeer) serve(raw net.Conn, observation *migrationNativeConnection) {
	defer raw.Close()
	defer func() { emit(map[string]any{"event": "closed"}) }()
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
			emit(map[string]any{"event": "reset", "code": uint32(value.ErrCode)})
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
			emit(map[string]any{"event": "request", "stream": headers.StreamID})
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

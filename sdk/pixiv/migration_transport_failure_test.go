package pixiv

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateTransportFailure = flag.Bool("migration-update-transport-failure", false, "capture SDK transport failure classification")

type migrationFailedBody struct{}

func (migrationFailedBody) Read([]byte) (int, error) { return 0, io.ErrUnexpectedEOF }
func (migrationFailedBody) Close() error             { return nil }

func TestMigrationSDKTransportFailureClassificationAndBodyConsumption(t *testing.T) {
	type row struct {
		Name      string        `json:"name"`
		Operation string        `json:"operation"`
		Reason    sdk.Reason    `json:"reason"`
		Transport sdk.Transport `json:"transport"`
		Detail    string        `json:"detail"`
		Message   string        `json:"message"`
		Status    int           `json:"status"`
		Safe      bool          `json:"safe"`
		Calls     int           `json:"calls"`
	}
	var rows []row
	for _, operation := range []string{"Artwork", "Open", "AddArtworkBookmark"} {
		for _, name := range []string{"connection-refused", "connection-reset", "unexpected-eof", "tls-record", "truncated-success", "truncated-error", "malformed-head"} {
			r := row{Name: name, Operation: operation}
			transport := migrationOAuthTransport(func(request *http.Request) (*http.Response, error) {
				r.Calls++
				switch name {
				case "connection-refused":
					return nil, &net.OpError{Op: "dial", Net: "tcp", Err: syscall.ECONNREFUSED}
				case "connection-reset":
					return nil, &net.OpError{Op: "read", Net: "tcp", Err: syscall.ECONNRESET}
				case "unexpected-eof":
					return nil, io.ErrUnexpectedEOF
				case "tls-record":
					return nil, tls.RecordHeaderError{Msg: "fixture-private-host-and-credential"}
				case "malformed-head":
					return nil, errors.New("fixture-private-host-and-credential")
				default:
					status := 200
					if name == "truncated-error" {
						status = 503
					}
					return &http.Response{StatusCode: status, Header: http.Header{}, Body: migrationFailedBody{}, Request: request}, nil
				}
			})
			client, err := NewWith("fixture-access", Options{HTTPClient: &http.Client{Transport: transport}})
			if err != nil {
				t.Fatal(err)
			}
			switch operation {
			case "Artwork":
				_, err = client.Artwork(context.Background(), ArtworkRequest{ArtworkID: 42})
			case "Open":
				_, _, err = OpenWith(context.Background(), "fixture-refresh", Options{HTTPClient: &http.Client{Transport: transport}})
			case "AddArtworkBookmark":
				err = client.AddArtworkBookmark(context.Background(), AddArtworkBookmarkRequest{ArtworkID: 42})
			}
			client.CloseIdleConnections()
			var classified *sdk.Error
			if !errors.As(err, &classified) {
				t.Fatalf("%s/%s: unclassified failure: %v", operation, name, err)
			}
			r.Reason = classified.Reason
			r.Transport = classified.Transport
			r.Detail = classified.Detail
			r.Message = err.Error()
			r.Status = classified.HTTPStatus
			r.Safe = classified.Retry.Safe
			if strings.Contains(r.Message, "fixture-private") {
				t.Fatal("transport failure leaked raw cause")
			}
			rows = append(rows, r)
		}
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "transport-failure.json")
	if *migrationUpdateTransportFailure {
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
		t.Fatal("SDK transport failure differs from fixed Go reference")
	}
}

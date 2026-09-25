package dic_test

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

func TestHTTPTransportSendsBrowserUserAgentAndReturnsStatus(t *testing.T) {
	var userAgent, accept, requestURI string
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		userAgent = request.Header.Get("User-Agent")
		accept = request.Header.Get("Accept")
		requestURI = request.URL.RequestURI()
		writer.WriteHeader(http.StatusNotFound)
		_, _ = writer.Write([]byte("missing"))
	}))
	defer server.Close()

	body, statusCode, err := dic.NewHTTPTransport(dic.HTTPTransportOptions{}).Get(context.Background(), server.URL+"/_api/get_article/x?lang=ja", "application/json")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if statusCode != http.StatusNotFound || string(body) != "missing" {
		t.Fatalf("Get = %q, %d", body, statusCode)
	}
	if requestURI != "/_api/get_article/x?lang=ja" {
		t.Fatalf("request uri = %q", requestURI)
	}
	if accept != "application/json" {
		t.Fatalf("accept = %q", accept)
	}
	if !strings.HasPrefix(userAgent, "Mozilla/5.0") || strings.Contains(userAgent, "curl") || strings.Contains(userAgent, "Go-http-client") {
		t.Fatalf("user-agent = %q, want a browser-like value", userAgent)
	}
}

func TestHTTPTransportUsesConfiguredUserAgent(t *testing.T) {
	var userAgent string
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		userAgent = request.Header.Get("User-Agent")
		_, _ = writer.Write([]byte("ok"))
	}))
	defer server.Close()

	if _, _, err := dic.NewHTTPTransport(dic.HTTPTransportOptions{UserAgent: "fixture-agent"}).Get(context.Background(), server.URL, "text/html"); err != nil {
		t.Fatalf("Get: %v", err)
	}
	if userAgent != "fixture-agent" {
		t.Fatalf("user-agent = %q, want fixture-agent", userAgent)
	}
}

func TestHTTPTransportFailsWithoutClient(t *testing.T) {
	for _, transport := range []*dic.HTTPTransport{nil, {}} {
		if _, _, err := transport.Get(context.Background(), "https://dic.pixiv.net/", "text/html"); err == nil {
			t.Fatalf("transport %#v returned no error", transport)
		}
	}
}

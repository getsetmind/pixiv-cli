package dic

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
)

// DefaultUserAgent is the browser-like User-Agent HTTPTransport sends. The
// encyclopedia rejects requests whose User-Agent looks like a bare HTTP client.
const DefaultUserAgent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"

// HTTPTransportOptions configures HTTPTransport.
type HTTPTransportOptions struct {
	// HTTPClient performs the requests. It belongs to the caller and is never
	// closed by the transport. When nil, a plain net/http client is used.
	HTTPClient *http.Client
	// UserAgent overrides DefaultUserAgent. Nothing else about the request
	// headers was measured as necessary for the encyclopedia.
	UserAgent string
}

// HTTPTransport is the production Transport. It performs a plain net/http GET
// and returns the response body together with its status code.
type HTTPTransport struct {
	client    *http.Client
	userAgent string
}

// NewHTTPTransport returns a Transport backed by a plain HTTP client.
func NewHTTPTransport(options HTTPTransportOptions) *HTTPTransport {
	client := options.HTTPClient
	if client == nil {
		client = &http.Client{}
	}
	userAgent := options.UserAgent
	if userAgent == "" {
		userAgent = DefaultUserAgent
	}
	return &HTTPTransport{client: client, userAgent: userAgent}
}

// Get performs one GET. A non-2xx status is returned as a status code with a
// nil error; only a transport-level failure returns an error.
func (t *HTTPTransport) Get(ctx context.Context, rawURL string, accept string) ([]byte, int, error) {
	if t == nil || t.client == nil {
		return nil, 0, errors.New("dic HTTP transport is not configured")
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, 0, fmt.Errorf("create dic request: %w", err)
	}
	request.Header.Set("User-Agent", t.userAgent)
	if accept != "" {
		request.Header.Set("Accept", accept)
	}
	response, err := t.client.Do(request)
	if err != nil {
		return nil, 0, err
	}
	defer response.Body.Close()
	body, err := io.ReadAll(response.Body)
	if err != nil {
		return nil, response.StatusCode, err
	}
	return body, response.StatusCode, nil
}

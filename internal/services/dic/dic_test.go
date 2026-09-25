package dic_test

import (
	"context"
	"errors"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

// stubTransport is a deterministic Transport for client tests. It records every
// request URL and Accept value and answers the queued responses in order.
type stubTransport struct {
	responses []stubResponse
	requests  []string
	accepts   []string
}

type stubResponse struct {
	body       string
	statusCode int
	err        error
}

func ok(body string) stubResponse { return stubResponse{body: body, statusCode: 200} }

func (s *stubTransport) Get(_ context.Context, rawURL string, accept string) ([]byte, int, error) {
	s.requests = append(s.requests, rawURL)
	s.accepts = append(s.accepts, accept)
	if len(s.responses) == 0 {
		return nil, 0, errors.New("stub transport has no queued response")
	}
	response := s.responses[0]
	s.responses = s.responses[1:]
	if response.err != nil {
		return nil, 0, response.err
	}
	return []byte(response.body), response.statusCode, nil
}

func TestClientWithoutTransportFailsWithTransportCode(t *testing.T) {
	client := dic.New(nil)
	if results, err := client.Search(context.Background(), dic.SearchRequest{Query: "miku"}); dic.CodeOf(err) != dic.CodeTransport || results != nil {
		t.Fatalf("Search = %v, %v; want nil, code %v", results, err, dic.CodeTransport)
	}
	if _, err := client.Article(context.Background(), dic.ArticleRequest{Ref: "初音ミク"}); dic.CodeOf(err) != dic.CodeTransport {
		t.Fatalf("Article error code = %v, want %v", dic.CodeOf(err), dic.CodeTransport)
	}
}

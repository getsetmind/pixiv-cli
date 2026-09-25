package dic_test

import (
	"context"
	"errors"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/internal/services/dic"
)

func TestCodeOfUnknownAndNilErrors(t *testing.T) {
	if dic.CodeOf(errors.New("plain")) != dic.CodeUnknown {
		t.Fatalf("plain error code = %v, want %v", dic.CodeOf(errors.New("plain")), dic.CodeUnknown)
	}
	if dic.CodeOf(nil) != dic.CodeUnknown {
		t.Fatalf("nil error code = %v, want %v", dic.CodeOf(nil), dic.CodeUnknown)
	}
}

func TestErrorPreservesContextCause(t *testing.T) {
	transport := &stubTransport{responses: []stubResponse{{err: context.Canceled}}}
	_, err := dic.New(transport).Search(context.Background(), dic.SearchRequest{Query: "miku"})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context.Canceled in the chain", err)
	}
	var classified *dic.Error
	if !errors.As(err, &classified) || classified.Code() != dic.CodeTransport {
		t.Fatalf("error = %v, want a classified transport error", err)
	}
	if classified.Error() == "" {
		t.Fatal("classified error has an empty message")
	}
}

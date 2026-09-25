package dic

import (
	"errors"
	"strconv"
)

// ErrorCode is a stable classification of a Pixiv encyclopedia failure. It is
// safe to branch on and never carries upstream response content.
type ErrorCode string

const (
	// CodeUnknown classifies an error produced outside this package.
	CodeUnknown ErrorCode = "unknown"
	// CodeInvalidRequest reports a caller-side request that violates the contract.
	CodeInvalidRequest ErrorCode = "invalid_request"
	// CodeInvalidReference reports a reference that is neither a bare article
	// title nor a dic.pixiv.net article URL.
	CodeInvalidReference ErrorCode = "invalid_reference"
	// CodeNotFound reports that the requested article does not exist.
	CodeNotFound ErrorCode = "not_found"
	// CodeUpstreamStatus reports an unexpected non-success HTTP status.
	CodeUpstreamStatus ErrorCode = "upstream_status"
	// CodeMalformedResponse reports a response that could not be parsed into the
	// expected shape.
	CodeMalformedResponse ErrorCode = "malformed_response"
	// CodeTransport reports a failure before a usable HTTP response was received.
	CodeTransport ErrorCode = "transport"
)

// Error is a classified Pixiv encyclopedia failure. message is a controlled,
// redacted description; cause preserves errors.Is and errors.As for callers.
type Error struct {
	code       ErrorCode
	statusCode int
	message    string
	cause      error
}

// NewError builds a classified failure. message must not contain response
// bodies, URLs carrying credentials, or other upstream content.
func NewError(code ErrorCode, message string, cause error) *Error {
	return &Error{code: code, message: message, cause: cause}
}

// newHTTPStatusError builds a classified failure carrying an upstream status.
func newHTTPStatusError(code ErrorCode, message string, statusCode int) *Error {
	return &Error{code: code, statusCode: statusCode, message: message + " (HTTP " + strconv.Itoa(statusCode) + ")"}
}

// Error returns the controlled failure message.
func (e *Error) Error() string { return e.message }

// Unwrap returns the underlying cause, allowing errors.Is and errors.As to walk
// the chain.
func (e *Error) Unwrap() error { return e.cause }

// Code returns the stable classification.
func (e *Error) Code() ErrorCode { return e.code }

// StatusCode returns the upstream HTTP status code, or zero when unavailable.
func (e *Error) StatusCode() int { return e.statusCode }

// CodeOf returns the stable classification of err, or CodeUnknown when err is
// not and does not wrap an *Error.
func CodeOf(err error) ErrorCode {
	var classified interface{ Code() ErrorCode }
	if errors.As(err, &classified) {
		return classified.Code()
	}
	return CodeUnknown
}

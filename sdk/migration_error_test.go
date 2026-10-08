package sdk_test

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"math"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
)

var migrationUpdateErrors = flag.Bool("migration-update-errors", false, "capture shared error contracts from the fixed Go reference")

func TestMigrationErrorsMatchFrozenClassificationAndMetadata(t *testing.T) {
	now := time.Date(2026, 10, 8, 0, 0, 0, 0, time.UTC)
	type errorCase struct {
		Name         string        `json:"name"`
		Product      string        `json:"product"`
		Operation    string        `json:"operation"`
		Reason       sdk.Reason    `json:"reason"`
		Detail       string        `json:"detail"`
		Cause        string        `json:"cause"`
		HTTPStatus   int           `json:"http_status"`
		Transport    sdk.Transport `json:"transport"`
		RetrySafe    bool          `json:"retry_safe"`
		RetryAfter   *time.Time    `json:"retry_after"`
		Message      string        `json:"message"`
		Canceled     bool          `json:"canceled"`
		Deadline     bool          `json:"deadline"`
		Matches      []sdk.Reason  `json:"matches"`
		RetrySeconds *int64        `json:"retry_seconds"`
	}
	reasons := []sdk.Reason{sdk.InvalidArgument, sdk.InvalidCursor, sdk.Unauthorized, sdk.CredentialsExpired, sdk.Forbidden, sdk.NotFound, sdk.ContentUnavailable, sdk.ChallengeRequired, sdk.RateLimited, sdk.UpstreamError, sdk.UpstreamUnavailable, sdk.MalformedUpstreamResponse, sdk.ResourceForbidden, sdk.NotUgoira, sdk.UgoiraArchiveMissing, sdk.UgoiraFrameMismatch, sdk.LocalStateError, sdk.RemovedSetting}
	var cases []errorCase
	add := func(name, cause string, classified *sdk.Error) {
		entry := errorCase{Name: name, Product: classified.Product, Operation: classified.Operation, Reason: classified.Reason, Detail: classified.Detail, Cause: cause, HTTPStatus: classified.HTTPStatus, Transport: classified.Transport, RetrySafe: classified.Retry.Safe, Message: classified.Error(), Canceled: errors.Is(classified, context.Canceled), Deadline: errors.Is(classified, context.DeadlineExceeded), Matches: []sdk.Reason{}}
		for _, reason := range reasons {
			if errors.Is(classified, &sdk.Error{Reason: reason}) {
				entry.Matches = append(entry.Matches, reason)
			}
		}
		if classified.Retry.HasAfter {
			after := classified.Retry.After
			entry.RetryAfter = &after
			seconds := int64(math.Ceil(after.Sub(now).Seconds()))
			if seconds < 0 {
				seconds = 0
			}
			entry.RetrySeconds = &seconds
		}
		cases = append(cases, entry)
	}
	for _, reason := range reasons {
		add(string(reason), "", sdk.NewError("pixiv", "Artwork", reason))
	}
	add("shared", "", sdk.NewError("", "ParseResourceRef", sdk.InvalidArgument))
	add("fanbox", "", sdk.NewError("fanbox", "Post", sdk.ContentUnavailable))
	add("detail-and-cause", "redacted", sdk.NewError("pixiv", "Artwork", sdk.UpstreamError, sdk.WithDetail("status 502"), sdk.WithCause(errors.New("redacted local classification")), sdk.WithHTTPStatus(502), sdk.WithTransport(sdk.TransportHTTP)))
	add("empty-cause", "empty", sdk.NewError("pixiv", "Artwork", sdk.UpstreamError, sdk.WithCause(errors.New(""))))
	add("canceled", "canceled", sdk.NewError("pixiv", "Artwork", sdk.UpstreamError, sdk.WithCause(context.Canceled)))
	add("deadline", "deadline", sdk.NewError("fanbox", "Post", sdk.UpstreamError, sdk.WithCause(context.DeadlineExceeded)))
	add("nested", "nested", sdk.NewError("fanbox", "Post", sdk.UpstreamError, sdk.WithCause(sdk.NewError("fanbox", "Post", sdk.Forbidden))))
	for _, transport := range []sdk.Transport{sdk.TransportHTTP, sdk.TransportTLS, sdk.TransportDNS, sdk.TransportLocal} {
		add("transport-"+string(transport), "", sdk.NewError("pixiv", "Artwork", sdk.UpstreamUnavailable, sdk.WithTransport(transport)))
	}
	add("safe-without-after", "", sdk.NewError("pixiv", "Artwork", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{Safe: true})))
	for _, interval := range []struct {
		name  string
		delay time.Duration
	}{{"past", -time.Second}, {"now", 0}, {"fraction", time.Nanosecond}, {"whole", 30 * time.Second}, {"round-up", 30*time.Second + time.Nanosecond}, {"large-fraction", 1_000_000_000*time.Second + time.Nanosecond}, {"maximum-duration", time.Duration(1<<63 - 1)}} {
		add("retry-"+interval.name, "", sdk.NewError("pixiv", "Artwork", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{Safe: true, HasAfter: true, After: now.Add(interval.delay)})))
	}
	data, err := json.MarshalIndent(struct {
		Now   time.Time   `json:"now"`
		Cases []errorCase `json:"cases"`
	}{now, cases}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "docs", "migration", "contracts", "errors.json")
	if *migrationUpdateErrors {
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
		t.Fatal("shared errors differ from the frozen Go contract")
	}
}

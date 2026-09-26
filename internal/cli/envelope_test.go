package cli

import (
	"bytes"
	"encoding/json"
	"errors"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/sdk"
	"github.com/spf13/cobra"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

type decodedEnvelope struct {
	Error struct {
		Code              string `json:"code"`
		RetryAfterSeconds int64  `json:"retry_after_seconds"`
	} `json:"error"`
}

func TestWriteErrorEnvelopeUsesReasonAndRoundsRetryAfterUp(t *testing.T) {
	classified := sdk.NewError("pixiv", "Download", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{Safe: true, HasAfter: true, After: time.Now().Add(30 * time.Second)}))
	var out bytes.Buffer
	require.NoError(t, writeErrorEnvelope(&out, classified))
	var envelope decodedEnvelope
	require.NoError(t, json.Unmarshal(out.Bytes(), &envelope))
	assert.Equal(t, "rate_limited", envelope.Error.Code)
	assert.Equal(t, int64(30), envelope.Error.RetryAfterSeconds)
}

func TestWriteErrorEnvelopeClampsPastRetryAfter(t *testing.T) {
	classified := sdk.NewError("pixiv", "Download", sdk.RateLimited, sdk.WithRetry(sdk.RetryAdvice{HasAfter: true, After: time.Now().Add(-time.Minute)}))
	var out bytes.Buffer
	require.NoError(t, writeErrorEnvelope(&out, classified))
	var envelope decodedEnvelope
	require.NoError(t, json.Unmarshal(out.Bytes(), &envelope))
	assert.Equal(t, int64(0), envelope.Error.RetryAfterSeconds)
}

func TestWriteErrorEnvelopeFallsBackToLocalCode(t *testing.T) {
	var out bytes.Buffer
	require.NoError(t, writeErrorEnvelope(&out, errors.New("boom")))
	var envelope decodedEnvelope
	require.NoError(t, json.Unmarshal(out.Bytes(), &envelope))
	assert.Equal(t, "command_failed", envelope.Error.Code)
}

func TestCommandExplicitJSONRequiresTheFlag(t *testing.T) {
	cmd := &cobra.Command{Use: "x"}
	cmd.Flags().Bool("json", false, "")
	assert.False(t, commandExplicitJSON(cmd))
	require.NoError(t, cmd.ParseFlags([]string{"--json"}))
	assert.True(t, commandExplicitJSON(cmd))
}

func TestCommandExplicitJSONIgnoresCommandsWithoutTheFlag(t *testing.T) {
	assert.False(t, commandExplicitJSON(&cobra.Command{Use: "y"}))
}

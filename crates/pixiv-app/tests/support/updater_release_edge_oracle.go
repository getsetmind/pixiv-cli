package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/update/release"
)

type cache struct{ writes []string }

func (*cache) Read(context.Context) ([]byte, bool, error) { return nil, false, nil }
func (c *cache) Write(_ context.Context, data []byte) error {
	c.writes = append(c.writes, string(data))
	return nil
}

type transport struct{}

func (transport) RoundTrip(request *http.Request) (*http.Response, error) {
	return &http.Response{StatusCode: 200, Status: "200 OK", Body: io.NopCloser(strings.NewReader("[]")), Header: http.Header{}, Request: request}, nil
}

type row struct {
	Base   string   `json:"base"`
	Now    string   `json:"now"`
	Error  string   `json:"error"`
	Chain  []string `json:"chain"`
	Writes []string `json:"writes"`
}

func main() {
	var rows []row
	for _, base := range []string{"https://api.github.com/%あ", "https://api.github.com/%é", "https://api.github.com/%0あ", "https://api.github.com/%😀", "https://api.github.com/%", "https://api.github.com/%a"} {
		c := &cache{writes: []string{}}
		_, err := release.NewGitHubReleaseClient(release.ReleaseClientOptions{APIBaseURL: base, Cache: c})
		observation := row{Base: base, Chain: []string{}, Writes: c.writes}
		if err != nil {
			observation.Error = err.Error()
			for cause := err; cause != nil; cause = errors.Unwrap(cause) {
				observation.Chain = append(observation.Chain, cause.Error())
			}
		}
		rows = append(rows, observation)
	}
	for _, text := range []string{"2026-10-10T12:00:00.12Z", "2026-10-10T12:00:00.1234Z", "2026-10-10T12:00:00.00000001Z", "2026-10-10T12:00:00.120000000+05:30", "2026-10-10T12:00:00Z", "0000-01-01T00:00:00.120000000Z", "9999-12-31T23:59:59.999999999-03:30"} {
		now, err := time.Parse(time.RFC3339Nano, text)
		if err != nil {
			panic(err)
		}
		c := &cache{writes: []string{}}
		client, err := release.NewGitHubReleaseClient(release.ReleaseClientOptions{Cache: c, HTTPClient: &http.Client{Transport: transport{}}, Now: func() time.Time { return now }})
		if err == nil {
			_, err = client.Check(context.Background(), release.ReleaseCheckOptions{})
		}
		observation := row{Now: text, Chain: []string{}, Writes: c.writes}
		if err != nil {
			observation.Error = err.Error()
			for cause := err; cause != nil; cause = errors.Unwrap(cause) {
				observation.Chain = append(observation.Chain, cause.Error())
			}
		}
		rows = append(rows, observation)
	}
	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(rows); err != nil {
		panic(err)
	}
}

package loginhelper

import (
	"encoding/json"
	"os"
	"reflect"
	"testing"
)

type migrationProtocolCase struct {
	Name      string `json:"name"`
	Operation string `json:"operation"`
	Input     string `json:"input"`
	Base      string `json:"base,omitempty"`
	Suffix    string `json:"suffix,omitempty"`
	Session   string `json:"session,omitempty"`
	Output    any    `json:"output"`
	Error     string `json:"error"`
}

func TestMigrationHandoffProtocol(t *testing.T) {
	fixture := "../../../../../../crates/pixiv-app/tests/fixtures/handoff_protocol.json"
	body, err := os.ReadFile(fixture)
	if err != nil {
		t.Fatal(err)
	}
	var cases []migrationProtocolCase
	if err := json.Unmarshal(body, &cases); err != nil {
		t.Fatal(err)
	}
	update := os.Getenv("PIXIV_UPDATE_HANDOFF_PROTOCOL") == "1"
	for i := range cases {
		c := &cases[i]
		var output any
		var err error
		switch c.Operation {
		case "origin":
			output, err = canonicalRelayOrigin(c.Input)
		case "endpoint":
			output, err = relayEndpointURL(c.Input, c.Suffix, c.Session)
		case "result":
			err = validateRelayResultURL(c.Base, c.Input)
		case "authorization":
			err = ValidateAuthorizationURL(c.Input)
		case "callback":
			output = IsAllowedPixivCallbackURL(c.Input)
		case "link":
			var start RemoteLoginStart
			start, err = ParseRemoteLoginLink(c.Input)
			if err == nil {
				output = map[string]any{"origin": start.Origin, "session_id": start.SessionID, "proof": start.Proof}
			}
		default:
			t.Fatalf("unknown operation %q", c.Operation)
		}
		errorText := ""
		if err != nil {
			errorText = err.Error()
			output = nil
		}
		if update {
			c.Output = output
			c.Error = errorText
			continue
		}
		t.Run(c.Name, func(t *testing.T) {
			if !reflect.DeepEqual(c.Output, output) || c.Error != errorText {
				t.Errorf("got output %#v error %q; want %#v error %q", output, errorText, c.Output, c.Error)
			}
		})
	}
	if update {
		body, err := json.MarshalIndent(cases, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(fixture, append(body, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
	}
}

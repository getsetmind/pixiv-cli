package auth

import (
	"encoding/json"
	config "github.com/FlanChanXwO/pixiv-cli/internal/config/settings"
	"os"
	"testing"
)

func TestMigrationRelayServerDeepLinkWireEncoding(t *testing.T) {
	bytes, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/relay_server.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		DeepLinks []struct{ Origin, Session, Proof, Expected string } `json:"deep_links"`
	}
	if err = json.Unmarshal(bytes, &fixture); err != nil {
		t.Fatal(err)
	}
	for _, tc := range fixture.DeepLinks {
		if actual := handoffRelayDeepLink(tc.Origin, tc.Session, tc.Proof); actual != tc.Expected {
			t.Fatalf("deep-link bytes differ:\nwant %s\ngot  %s", tc.Expected, actual)
		}
	}
}

type migrationRelayChangedFlags struct{}

func (migrationRelayChangedFlags) Changed(string) bool { return true }
func TestMigrationRelayServerExplicitFlagsOverrideConfiguration(t *testing.T) {
	bytes, err := os.ReadFile("../../../../../crates/pixiv-app/tests/fixtures/relay_server.json")
	if err != nil {
		t.Fatal(err)
	}
	type values struct{ Public, Listen, Cert, Key string }
	var fixture struct {
		Priority struct{ Config, Flags values } `json:"flag_priority"`
	}
	if err = json.Unmarshal(bytes, &fixture); err != nil {
		t.Fatal(err)
	}
	cfg := fixture.Priority.Config
	flags := fixture.Priority.Flags
	options, enabled, err := ConfiguredRelayServerOptions(migrationRelayChangedFlags{}, AccountLoginOptions{relayPublicURL: flags.Public, relayListenAddr: flags.Listen, relayTLSCertFile: flags.Cert, relayTLSKeyFile: flags.Key}, config.RuntimeConfig{LoginRelayPublicURL: cfg.Public, LoginRelayListenAddr: cfg.Listen, LoginRelayTLSCertFile: cfg.Cert, LoginRelayTLSKeyFile: cfg.Key})
	if err != nil || !enabled {
		t.Fatalf("flag priority: enabled=%v err=%v", enabled, err)
	}
	if options.PublicURL != flags.Public || options.ListenAddr != flags.Listen || options.TLSCertFile != flags.Cert || options.TLSKeyFile != flags.Key {
		t.Fatalf("flags did not replace config: %+v", options)
	}
}

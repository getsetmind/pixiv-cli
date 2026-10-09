package cli

import (
	"flag"
	"testing"
)

var updateUserRelationshipsStartup = flag.Bool("migration-update-user-relationships-startup", false, "capture relationship startup contracts")

func TestMigrationUserRelationshipsStartupPreservesValidationConfigurationAndAuthOrder(t *testing.T) {
	text := func(value string) *string { return &value }
	var rows []userWorksStartupCase
	for _, kind := range []string{"following", "followers", "related", "blocked"} {
		variants := [][]string{{}, {"42"}, {"https://www.pixiv.net/users/42"}, {"0"}, {"-1"}, {"bad"}, {"https://www.pixiv.net/artworks/42"}, {"42", "43"}, {"42", "--page=0"}, {"0", "--page=0"}, {"42", "--limit=-1"}, {"42", "--limit=0", "--page=2"}, {"42", "--proxy=", "--no-proxy"}, {"42", "--proxy=invalid"}, {"42", "--no-proxy=false"}, {"42", "--ndjson", "--json=false"}, {"42", "--ndjson=false"}}
		if kind == "following" || kind == "followers" {
			for _, value := range []string{"public", "private", "", "PUBLIC", " public ", "invalid"} {
				variants = append(variants, []string{"42", "--restrict=" + value}, []string{"0", "--restrict=" + value, "--page=0"})
			}
		}
		for _, args := range variants {
			for _, mode := range []string{"", "--json", "--ndjson", "--json=false"} {
				current := userWorksStartupCase{Args: append([]string{kind}, args...)}
				if mode != "" {
					current.Args = append(current.Args, mode)
				}
				rows = append(rows, userWorksIsolatedStartup(t, current))
			}
		}
		for _, before := range []string{"[unfinished\n", "# mine\r\n[unknown]\r\nkeep = true\r\n", "[output]\njson = true\n", "[pixiv.network]\nproxy_url = 'invalid'\n"} {
			for _, args := range [][]string{{}, {"42"}, {"0"}, {"42", "--no-proxy"}, {"42", "--json=false"}} {
				rows = append(rows, userWorksIsolatedStartup(t, userWorksStartupCase{Args: append([]string{kind}, args...), Before: text(before)}))
			}
		}
		for _, input := range []string{"", "\n", "\r\n", "42\n", "42\r\n", " 42 \n", "42\n\n", "42\r", "42\n43\n", "https://www.pixiv.net/users/42\n", "{\"id\":42}\n"} {
			for _, args := range [][]string{{}, {"42"}, {"42", "43"}} {
				rows = append(rows, userWorksIsolatedStartup(t, userWorksStartupCase{Args: append([]string{kind}, args...), Input: input}))
			}
		}
		for _, args := range [][]string{{}, {"42"}} {
			rows = append(rows, userWorksIsolatedStartup(t, userWorksStartupCase{Args: append([]string{kind}, args...), ReadError: true}))
		}
	}
	recommendedFixture(t, "cli-user-relationships-startup.json", rows, *updateUserRelationshipsStartup)
}

package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	authcommands "github.com/FlanChanXwO/pixiv-cli/internal/cli/commands/pixiv/auth"
	account "github.com/FlanChanXwO/pixiv-cli/internal/services/pixiv/account"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
)

var updateAuthSelection = flag.Bool("migration-update-auth-selection", false, "capture isolated auth selection commands")

func TestMigrationAuthSelection(t *testing.T) {
	var rows []authAccountCase
	add := func(name string, args ...string) {
		rows = append(rows, authAccountCase{userWorksStartupCase: userWorksStartupCase{Name: name, Args: args}, Environment: map[string]string{}})
	}
	text := func(s string) *string { return &s }
	for _, op := range []string{"use", "remove"} {
		for _, args := range [][]string{{}, {"--help"}, {"1", "2"}, {"--json"}, {"--json=false"}, {"--json=bad"}, {"--unknown"}, {"--", "-1"}, {"--yes"}} {
			add(op+"-"+strings.Join(args, "-"), append([]string{op}, args...)...)
		}
		for _, value := range []string{"1", "+1", "2", "99", "0", "9223372036854775807", "9223372036854775808", "0x10", "1_0", "  ", "bad"} {
			add(op+"-seed-"+value, op, "--", value)
			rows[len(rows)-1].Seed = true
			rows[len(rows)-1].Before = text("# preserved\n[pixiv.auth]\ndefault_user_id=1\n")
		}
		for _, input := range []string{"", "1\n", "2\r\n", "1\n2\n", " \n"} {
			add(op+"-stdin-"+input, op, "--json")
			rows[len(rows)-1].Seed = true
			rows[len(rows)-1].Input = input
		}
		add(op+"-read-error", op)
		rows[len(rows)-1].ReadError = true
		add(op+"-bad-config", op, "bad")
		rows[len(rows)-1].Before = text("[unfinished\n")
		add(op+"-bad-config-empty", op)
		rows[len(rows)-1].Before = text("[unfinished\n")
		add(op+"-json-writer", op, "2", "--json")
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].WriteError = true
		add(op+"-writer", op, "2")
		rows[len(rows)-1].Seed = true
		rows[len(rows)-1].WriteError = true
		for _, before := range []string{"[pixiv.auth]\ndefault_user_id=2\n", "[pixiv.auth]\ndefault_user_id=99\n", "[output]\njson=true\n", ""} {
			add(op+"-default-"+before, op, "2", "--json")
			rows[len(rows)-1].Seed = true
			rows[len(rows)-1].Before = text(before)
		}
	}
	for index, row := range rows {
		home := t.TempDir()
		data, err := json.Marshal(row)
		if err != nil {
			t.Fatal(err)
		}
		child := exec.Command(os.Args[0], "-test.run=^TestMigrationAuthAccountsChild$")
		for _, entry := range os.Environ() {
			key, _, _ := strings.Cut(entry, "=")
			switch key {
			case "HOME", "USERPROFILE", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "DOWNLOAD_PATH", "FILENAME_TEMPLATE", "DIRECTORY_TEMPLATE", "PIXIV_REQUEST_INTERVAL", "PIXIV_LOG_LEVEL", "PIXIV_LOG_FORMAT", "SAUCENAO_API_KEY":
				continue
			}
			child.Env = append(child.Env, entry)
		}
		child.Env = append(child.Env, "HOME="+home, "USERPROFILE="+home, "MIGRATION_AUTH_ACCOUNT_CHILD="+string(data))
		for key, value := range row.Environment {
			child.Env = append(child.Env, key+"="+value)
		}
		if out, err := child.CombinedOutput(); err != nil {
			t.Fatalf("%s: %v %s", row.Name, err, out)
		}
		data, err = os.ReadFile(filepath.Join(home, "result.json"))
		if err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(data, &rows[index]); err != nil {
			t.Fatal(err)
		}
	}
	recommendedFixture(t, "cli-auth-selection.json", rows, *updateAuthSelection)
}

type migrationSelectionPort struct {
	authcommands.AccountPort
	calls    []string
	failList int
}

func (p *migrationSelectionPort) ListAccounts(context.Context) ([]account.AccountSummary, error) {
	p.calls = append(p.calls, "list")
	n := 0
	for _, call := range p.calls {
		if call == "list" {
			n++
		}
	}
	if p.failList == n {
		return nil, fmt.Errorf("fixture list %d failed", n)
	}
	return []account.AccountSummary{{UserID: 2, Username: "合成-user"}, {UserID: 1, Default: true}}, nil
}
func (p *migrationSelectionPort) UseAccount(_ context.Context, id int64) error {
	p.calls = append(p.calls, fmt.Sprintf("use:%d", id))
	return nil
}
func (p *migrationSelectionPort) RemoveAccount(_ context.Context, id int64) error {
	p.calls = append(p.calls, fmt.Sprintf("remove:%d", id))
	return nil
}
func TestMigrationAuthSelectionPromptPorts(t *testing.T) {
	for _, row := range []struct {
		op, selected                  string
		yes, confirmed                bool
		wantError, wantCalls, wantOut string
	}{
		{"use", "2 合成-user", false, false, "", "list,use:2", "default uid: 2\n"},
		{"use", "", false, false, "uid cannot be empty", "list", ""},
		{"remove", "2 合成-user", false, false, "account removal canceled", "list", ""},
		{"remove", "2 合成-user", false, true, "", "list,list,remove:2,list", "account uid:2 removed\ndefault uid: 1\n"},
		{"remove", "1", true, false, "", "list,list,remove:1,list", "account uid:1 removed\ndefault uid: 1\n"},
	} {
		t.Run(row.op+row.selected+row.wantError+strconv.FormatBool(row.yes), func(t *testing.T) {
			port := &migrationSelectionPort{}
			var out bytes.Buffer
			var prompts []string
			cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(""), Output: &out, ErrorOutput: io.Discard,
				Account:   func() (authcommands.AccountService, error) { return authcommands.AccountService{Pixiv: port}, nil },
				CanPrompt: func() bool { return true }, PromptSelect: func(label string, options []string) (string, error) {
					want := "Select default account"
					if row.op == "remove" {
						want = "Select account to remove"
					}
					if label != want {
						t.Fatalf("label %q", label)
					}
					if !reflect.DeepEqual(options, []string{"2 合成-user", "1"}) {
						t.Fatalf("options %q", options)
					}
					prompts = append(prompts, "select")
					return row.selected, nil
				},
				PromptConfirm: func(label string, def bool) (bool, error) {
					if label != "Remove uid 2?" || def {
						t.Fatalf("confirm %q %t", label, def)
					}
					prompts = append(prompts, "confirm")
					return row.confirmed, nil
				}})
			cmd.SilenceErrors = true
			cmd.SilenceUsage = true
			args := []string{row.op}
			if row.yes {
				args = append(args, "--yes")
			}
			cmd.SetArgs(args)
			err := cmd.Execute()
			got := ""
			if err != nil {
				got = err.Error()
			}
			if got != row.wantError || strings.Join(port.calls, ",") != row.wantCalls || out.String() != row.wantOut {
				t.Fatalf("error=%q calls=%q out=%q", got, port.calls, out.String())
			}
			if row.op == "remove" && !row.yes && len(prompts) != 2 {
				t.Fatalf("prompts %q", prompts)
			}
		})
	}
}

func TestMigrationAuthSelectionListFailureOrdering(t *testing.T) {
	for _, n := range []int{1, 2, 3} {
		t.Run(strconv.Itoa(n), func(t *testing.T) {
			port := &migrationSelectionPort{failList: n}
			var out bytes.Buffer
			cmd := authcommands.New(authcommands.Deps{Input: strings.NewReader(""), Output: &out, ErrorOutput: io.Discard, Account: func() (authcommands.AccountService, error) { return authcommands.AccountService{Pixiv: port}, nil }, CanPrompt: func() bool { return false }})
			cmd.SilenceErrors = true
			cmd.SilenceUsage = true
			cmd.SetArgs([]string{"remove", "2", "--json"})
			err := cmd.Execute()
			if err == nil || err.Error() != fmt.Sprintf("fixture list %d failed", n) || out.Len() != 0 {
				t.Fatalf("error=%v out=%q", err, out.String())
			}
			want := []string{"list", "list,list", "list,list,remove:2,list"}[n-1]
			if strings.Join(port.calls, ",") != want {
				t.Fatalf("calls %q", port.calls)
			}
		})
	}
}

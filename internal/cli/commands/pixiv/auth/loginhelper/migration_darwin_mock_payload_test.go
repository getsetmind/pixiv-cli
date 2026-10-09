//go:build darwin_contract_payload

package mockexec

import "context"

var Find func(string) (string, error)
var RunCommand func(string, []string, string) ([]byte, error)

type Cmd struct {
	Name string
	Args []string
}

func LookPath(name string) (string, error)                               { return Find(name) }
func CommandContext(_ context.Context, name string, args ...string) *Cmd { return &Cmd{name, args} }
func (c *Cmd) Run() error                                                { _, e := RunCommand(c.Name, c.Args, "run"); return e }
func (c *Cmd) Output() ([]byte, error)                                   { return RunCommand(c.Name, c.Args, "output") }
func (c *Cmd) CombinedOutput() ([]byte, error)                           { return RunCommand(c.Name, c.Args, "combined") }

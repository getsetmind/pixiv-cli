//go:build windows_contract_payload

package mockexec

import "context"

var RunCommand func(context.Context, string, []string, string) ([]byte, error)

type ExitError struct{ Message string }

func (e *ExitError) Error() string { return e.Message }

type Cmd struct {
	Context context.Context
	Name    string
	Args    []string
}

func CommandContext(ctx context.Context, name string, args ...string) *Cmd {
	return &Cmd{ctx, name, args}
}
func (c *Cmd) Run() error {
	_, err := RunCommand(c.Context, c.Name, c.Args, "run")
	return err
}
func (c *Cmd) Output() ([]byte, error) {
	return RunCommand(c.Context, c.Name, c.Args, "output")
}

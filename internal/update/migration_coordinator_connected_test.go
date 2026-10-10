package update_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"testing"
	"time"

	"github.com/FlanChanXwO/pixiv-cli/internal/shared/buildinfo"
	"github.com/FlanChanXwO/pixiv-cli/internal/update"
)

var migrationCaptureCoordinator = flag.Bool("migration-capture-updater-coordinator", false, "capture frozen public update coordinator and automatic checker contracts")

const migrationCoordinatorReference = "4b4426487ef18bed276706daec385e0d0a6979f9"

const migrationCoordinatorBaselinePath = "docs/migration/provenance/updater-contracts-baseline.json"
const migrationCoordinatorBaselineSHA = "d36a8ee61c519b74211b7def61a2425e6126278d8ea13d8b24e40392b3f03edd"

type migrationCoordinatorInput struct {
	Operation         string               `json:"operation"`
	CurrentVersion    string               `json:"current_version"`
	Source            update.InstallSource `json:"source"`
	Check             bool                 `json:"check"`
	IncludePrerelease bool                 `json:"include_prerelease"`
	Candidate         *update.Release      `json:"candidate"`
	Throttled         bool                 `json:"throttled"`
	DetectorError     string               `json:"detector_error"`
	CheckerError      string               `json:"checker_error"`
	CommandErrors     []string             `json:"command_errors"`
	InstallerError    string               `json:"installer_error"`
	MissingDetector   bool                 `json:"missing_detector"`
	MissingChecker    bool                 `json:"missing_checker"`
	MissingRunner     bool                 `json:"missing_runner"`
	DefaultInstaller  bool                 `json:"default_installer"`
	Context           string               `json:"context"`
}

type migrationCoordinatorErrorLink struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}

type migrationCoordinatorError struct {
	Message string                          `json:"message"`
	Chain   []migrationCoordinatorErrorLink `json:"unwrap_chain"`
	Is      map[string]bool                 `json:"is"`
}

type migrationCoordinatorContext struct {
	Port         string `json:"port"`
	SameIdentity bool   `json:"same_identity"`
	HasDeadline  bool   `json:"has_deadline"`
	Error        string `json:"error"`
}

type migrationCoordinatorOutcome struct {
	Constructed   bool                          `json:"constructed"`
	Result        *update.UpdateResult          `json:"result"`
	Notice        *update.AutomaticUpdateNotice `json:"notice"`
	Trace         []string                      `json:"trace"`
	Detected      []buildinfo.Info              `json:"detected_build_info"`
	Checks        []update.ReleaseCheckOptions  `json:"release_check_options"`
	Commands      []update.Command              `json:"commands"`
	Installations []update.Release              `json:"installed_releases"`
	Contexts      []migrationCoordinatorContext `json:"contexts"`
	Error         *migrationCoordinatorError    `json:"error"`
}

type migrationCoordinatorCase struct {
	Name                     string                      `json:"name"`
	EvidenceScope            string                      `json:"evidence_scope"`
	RepresentationOnlyReason string                      `json:"representation_only_reason,omitempty"`
	Input                    migrationCoordinatorInput   `json:"input"`
	Outcome                  migrationCoordinatorOutcome `json:"outcome"`
}

type migrationCoordinatorBaselineMetadata struct {
	Path            string `json:"path"`
	SHA256          string `json:"sha256"`
	ProductionCount int    `json:"production_count"`
	FixtureCount    int    `json:"fixture_count"`
}

type migrationCoordinatorFixture struct {
	SchemaVersion        int                                  `json:"schema_version"`
	Reference            string                               `json:"reference"`
	Evidence             string                               `json:"evidence"`
	MetadataScope        map[string]string                    `json:"metadata_scope"`
	PreservationBaseline migrationCoordinatorBaselineMetadata `json:"preservation_baseline"`
	SourceSHA256         map[string]string                    `json:"source_sha256"`
	Cases                []migrationCoordinatorCase           `json:"cases"`
}

type migrationCoordinatorPorts struct {
	input     migrationCoordinatorInput
	out       *migrationCoordinatorOutcome
	ctx       context.Context
	sentinels map[string]error
}

type migrationCoordinatorChecker struct{ ports *migrationCoordinatorPorts }
type migrationCoordinatorRunner struct{ ports *migrationCoordinatorPorts }
type migrationCoordinatorInstaller struct{ ports *migrationCoordinatorPorts }

func (p *migrationCoordinatorPorts) recordContext(port string, ctx context.Context) {
	_, deadline := ctx.Deadline()
	errText := ""
	if ctx.Err() != nil {
		errText = ctx.Err().Error()
	}
	p.out.Contexts = append(p.out.Contexts, migrationCoordinatorContext{
		Port: port, SameIdentity: ctx == p.ctx, HasDeadline: deadline, Error: errText,
	})
}

func (p *migrationCoordinatorPorts) Detect(info buildinfo.Info) (update.InstallSource, error) {
	p.out.Trace = append(p.out.Trace, "detector")
	p.out.Detected = append(p.out.Detected, info)
	return p.input.Source, p.sentinels[p.input.DetectorError]
}

func (c migrationCoordinatorChecker) Check(ctx context.Context, options update.ReleaseCheckOptions) (update.ReleaseCheckResult, error) {
	p := c.ports
	p.out.Trace = append(p.out.Trace, "checker")
	p.out.Checks = append(p.out.Checks, options)
	p.recordContext("checker", ctx)
	return update.ReleaseCheckResult{Release: p.input.Candidate, Throttled: p.input.Throttled}, p.sentinels[p.input.CheckerError]
}

func (r migrationCoordinatorRunner) Run(ctx context.Context, command update.Command) error {
	p := r.ports
	p.out.Trace = append(p.out.Trace, "command:"+command.Name)
	p.recordContext("command", ctx)
	index := len(p.out.Commands)
	command.Args = append([]string{}, command.Args...)
	p.out.Commands = append(p.out.Commands, command)
	if index < len(p.input.CommandErrors) {
		return p.sentinels[p.input.CommandErrors[index]]
	}
	return nil
}

func (i migrationCoordinatorInstaller) Install(ctx context.Context, selected update.Release) error {
	p := i.ports
	p.out.Trace = append(p.out.Trace, "installer")
	p.recordContext("installer", ctx)
	if selected.Assets != nil {
		selected.Assets = append([]update.ReleaseAsset{}, selected.Assets...)
	}
	p.out.Installations = append(p.out.Installations, selected)
	return p.sentinels[p.input.InstallerError]
}

func migrationCoordinatorCaptureError(err error, sentinels map[string]error) *migrationCoordinatorError {
	if err == nil {
		return nil
	}
	result := &migrationCoordinatorError{Message: err.Error(), Chain: []migrationCoordinatorErrorLink{}, Is: map[string]bool{}}
	for current := err; current != nil; current = errors.Unwrap(current) {
		result.Chain = append(result.Chain, migrationCoordinatorErrorLink{Type: fmt.Sprintf("%T", current), Message: current.Error()})
	}
	for name, cause := range sentinels {
		result.Is[name] = errors.Is(err, cause)
	}
	return result
}

func migrationCoordinatorObserve(t *testing.T, input migrationCoordinatorInput) migrationCoordinatorOutcome {
	t.Helper()
	out := migrationCoordinatorOutcome{
		Trace: []string{}, Detected: []buildinfo.Info{}, Checks: []update.ReleaseCheckOptions{},
		Commands: []update.Command{}, Installations: []update.Release{}, Contexts: []migrationCoordinatorContext{},
	}
	ctx := context.WithValue(context.Background(), struct{ witness string }{"coordinator"}, "owned synthetic context")
	switch input.Context {
	case "", "plain":
	case "canceled":
		canceled, cancel := context.WithCancel(ctx)
		cancel()
		ctx = canceled
	case "deadline":
		deadline, cancel := context.WithDeadline(ctx, time.Date(2030, 1, 1, 0, 0, 0, 0, time.UTC))
		t.Cleanup(cancel)
		ctx = deadline
	default:
		t.Fatalf("unknown context %q", input.Context)
	}
	sentinels := map[string]error{
		"detector": errors.New("detector failure"), "checker": errors.New("checker failure"),
		"command_first": errors.New("first command failure"), "command_install": errors.New("target install failure"),
		"command_rollback": errors.New("rollback install failure"), "installer": errors.New("release installer failure"),
		"context_canceled": context.Canceled, "context_deadline": context.DeadlineExceeded,
	}
	p := &migrationCoordinatorPorts{input: input, out: &out, ctx: ctx, sentinels: sentinels}
	var detector update.SourceDetector = p
	var checker update.ReleaseChecker = migrationCoordinatorChecker{ports: p}
	var runner update.CommandRunner = migrationCoordinatorRunner{ports: p}
	var installer update.ReleaseInstaller = migrationCoordinatorInstaller{ports: p}
	if input.MissingDetector {
		detector = nil
	}
	if input.MissingChecker {
		checker = nil
	}
	if input.MissingRunner {
		runner = nil
	}
	if input.DefaultInstaller {
		installer = nil
	}
	var err error
	switch input.Operation {
	case "coordinator", "coordinator_constructor":
		var coordinator *update.UpdateCoordinator
		coordinator, err = update.NewUpdateCoordinator(update.UpdateCoordinatorOptions{
			SourceDetector: detector, ReleaseChecker: checker, CommandRunner: runner, ReleaseInstaller: installer,
		})
		out.Constructed = coordinator != nil
		if err == nil && input.Operation == "coordinator" {
			var result update.UpdateResult
			result, err = coordinator.Execute(ctx, update.UpdateRequest{
				BuildInfo: buildinfo.Info{Version: input.CurrentVersion}, Check: input.Check, IncludePrerelease: input.IncludePrerelease,
			})
			out.Result = &result
		}
	case "automatic", "automatic_constructor":
		var automatic *update.AutomaticUpdateChecker
		automatic, err = update.NewAutomaticUpdateChecker(update.AutomaticUpdateCheckerOptions{SourceDetector: detector, ReleaseChecker: checker})
		out.Constructed = automatic != nil
		if err == nil && input.Operation == "automatic" {
			out.Notice, err = automatic.Check(ctx, update.AutomaticUpdateRequest{BuildInfo: buildinfo.Info{Version: input.CurrentVersion}})
		}
	case "coordinator_nil":
		var coordinator *update.UpdateCoordinator
		result, executeErr := coordinator.Execute(ctx, update.UpdateRequest{BuildInfo: buildinfo.Info{Version: input.CurrentVersion}})
		out.Result, err = &result, executeErr
	case "automatic_nil":
		var automatic *update.AutomaticUpdateChecker
		out.Notice, err = automatic.Check(ctx, update.AutomaticUpdateRequest{BuildInfo: buildinfo.Info{Version: input.CurrentVersion}})
	default:
		t.Fatalf("unknown operation %q", input.Operation)
	}
	out.Error = migrationCoordinatorCaptureError(err, sentinels)
	for _, observed := range out.Contexts {
		if !observed.SameIdentity {
			t.Errorf("%s did not receive the caller context", observed.Port)
		}
	}
	for _, options := range out.Checks {
		if input.Operation == "automatic" {
			if options != (update.ReleaseCheckOptions{Automatic: true, IncludePrerelease: false}) {
				t.Errorf("automatic stable-only options changed: %+v", options)
			}
		} else if options != (update.ReleaseCheckOptions{Automatic: false, IncludePrerelease: input.IncludePrerelease}) {
			t.Errorf("explicit checker options changed: %+v", options)
		}
	}
	if input.Operation == "automatic" || input.Check {
		if len(out.Commands) != 0 || len(out.Installations) != 0 {
			t.Error("read-only check crossed an installation barrier")
		}
	}
	return out
}

func migrationCoordinatorRelease(tag string, prerelease bool) *update.Release {
	return &update.Release{
		TagName: tag, Version: "opaque checker version", Prerelease: prerelease,
		Assets: []update.ReleaseAsset{{Name: "synthetic-asset", DownloadURL: "https://example.invalid/never-requested"}},
	}
}

func migrationCoordinatorInputs() []migrationCoordinatorCase {
	cases := []migrationCoordinatorCase{}
	add := func(name string, input migrationCoordinatorInput) {
		if input.Operation == "" {
			input.Operation = "coordinator"
		}
		if input.CurrentVersion == "" {
			input.CurrentVersion = "v1.0.0"
		}
		if input.Context == "" {
			input.Context = "plain"
		}
		if input.CommandErrors == nil {
			input.CommandErrors = []string{}
		}
		captured := migrationCoordinatorCase{Name: name, EvidenceScope: "behavioral_contract", Input: input}
		switch input.Operation {
		case "coordinator_nil", "automatic_nil":
			captured.EvidenceScope = "go_representation_only"
			captured.RepresentationOnlyReason = "Nil pointer receivers are Go API representation behavior; these rows do not define a Rust receiver contract."
		case "coordinator_constructor", "automatic_constructor":
			if input.MissingDetector || input.MissingChecker {
				captured.EvidenceScope = "go_representation_only"
				captured.RepresentationOnlyReason = "Nil required interface dependency ports are Go constructor representation behavior; these rows do not require a Rust nil-port API."
			} else if input.Operation == "coordinator_constructor" && (input.MissingRunner || input.DefaultInstaller) {
				captured.EvidenceScope = "go_representation_only"
				captured.RepresentationOnlyReason = "Nil optional Go constructor ports preserve the captured default-installer and absent-runner semantics; this constructor representation does not require a Rust nil-port API."
			}
		}
		cases = append(cases, captured)
	}
	sources := []update.InstallSource{
		update.InstallSourceDevelopment, update.InstallSourceHomebrewStable, update.InstallSourceHomebrewBeta,
		update.InstallSourceGoInstall, update.InstallSourceRelease,
	}
	candidates := []struct {
		name    string
		current string
		release *update.Release
	}{
		{"absent", "v1.0.0", nil},
		{"equal", "v1.0.0", migrationCoordinatorRelease("v1.0.0", false)},
		{"older", "v1.0.0", migrationCoordinatorRelease("v0.9.0", false)},
		{"newer", "v1.0.0", migrationCoordinatorRelease("v2.0.0", false)},
		{"prerelease", "v1.0.0", migrationCoordinatorRelease("v2.0.0-beta.2+asset.3", true)},
		{"build_metadata_equal", "v1.0.0+local.7", migrationCoordinatorRelease("v1.0.0+remote.9", false)},
	}
	for _, source := range sources {
		for _, candidate := range candidates {
			for _, check := range []bool{false, true} {
				for _, include := range []bool{false, true} {
					add(fmt.Sprintf("explicit/%s/%s/check_%t/prerelease_%t", source, candidate.name, check, include), migrationCoordinatorInput{
						Source: source, CurrentVersion: candidate.current, Candidate: candidate.release, Check: check, IncludePrerelease: include,
					})
				}
			}
		}
	}
	for _, source := range []update.InstallSource{update.InstallSourceHomebrewStable, update.InstallSourceHomebrewBeta} {
		include := source == update.InstallSourceHomebrewStable
		for _, candidate := range []*update.Release{nil, migrationCoordinatorRelease("v0.9.0-beta.1", true)} {
			label := "absent"
			if candidate != nil {
				label = "downgrade"
			}
			for _, failure := range []struct {
				name string
				errs []string
			}{
				{"uninstall_failure", []string{"command_first"}},
				{"rollback_success", []string{"", "command_install", ""}},
				{"rollback_failure", []string{"", "command_install", "command_rollback"}},
			} {
				add(fmt.Sprintf("homebrew_switch/%s/%s/%s", source, label, failure.name), migrationCoordinatorInput{
					Source: source, Candidate: candidate, IncludePrerelease: include, CommandErrors: failure.errs,
				})
			}
		}
		add("homebrew_upgrade_failure/"+string(source), migrationCoordinatorInput{
			Source: source, IncludePrerelease: source == update.InstallSourceHomebrewBeta, CommandErrors: []string{"command_first"},
		})
	}
	for _, source := range []update.InstallSource{update.InstallSourceHomebrewStable, update.InstallSourceHomebrewBeta, update.InstallSourceGoInstall} {
		for _, check := range []bool{false, true} {
			for _, include := range []bool{false, true} {
				for _, candidate := range candidates[:4] {
					add(fmt.Sprintf("missing_runner/%s/%s/check_%t/prerelease_%t", source, candidate.name, check, include), migrationCoordinatorInput{
						Source: source, Candidate: candidate.release, Check: check, IncludePrerelease: include, MissingRunner: true,
					})
				}
			}
		}
	}
	for _, operation := range []string{"coordinator", "automatic"} {
		for _, missing := range []struct {
			name              string
			detector, checker bool
		}{
			{"both", true, true}, {"detector", true, false}, {"checker", false, true}, {"none", false, false},
		} {
			add(operation+"_constructor/"+missing.name, migrationCoordinatorInput{
				Operation: operation + "_constructor", Source: update.InstallSourceRelease,
				MissingDetector: missing.detector, MissingChecker: missing.checker, DefaultInstaller: true, MissingRunner: true,
			})
		}
		add(operation+"_nil", migrationCoordinatorInput{Operation: operation + "_nil"})
		for _, invalid := range []string{"not-a-version", "v01.0.0", "v1.0.0-01"} {
			add(operation+"/current_invalid/"+invalid, migrationCoordinatorInput{Operation: operation, Source: update.InstallSourceRelease, CurrentVersion: invalid})
			add(operation+"/selected_invalid/"+invalid, migrationCoordinatorInput{Operation: operation, Source: update.InstallSourceRelease, Candidate: migrationCoordinatorRelease(invalid, false)})
		}
		add(operation+"/checker_failure", migrationCoordinatorInput{Operation: operation, Source: update.InstallSourceRelease, CheckerError: "checker"})
		add(operation+"/detector_failure", migrationCoordinatorInput{Operation: operation, Source: update.InstallSourceRelease, DetectorError: "detector", Candidate: migrationCoordinatorRelease("v2.0.0", false)})
		add(operation+"/detector_failure_invalid_current", migrationCoordinatorInput{Operation: operation, CurrentVersion: "not-a-version", DetectorError: "detector"})
		for _, pair := range []struct{ name, current, candidate string }{
			{"prerelease_progress", "v2.0.0-beta.1", "v2.0.0-beta.2"},
			{"stable_after_prerelease", "v2.0.0-beta.2", "v2.0.0"},
			{"prerelease_below_stable", "v2.0.0", "v2.0.0-beta.9"},
			{"unbounded_numeric", "v999999999999999999999999.0.0", "v1000000000000000000000000.0.0"},
		} {
			add(operation+"/"+pair.name, migrationCoordinatorInput{
				Operation: operation, Source: update.InstallSourceRelease, CurrentVersion: pair.current,
				Candidate: migrationCoordinatorRelease(pair.candidate, pair.name != "stable_after_prerelease" && pair.name != "unbounded_numeric"),
			})
		}
		for _, source := range append(append([]update.InstallSource{}, sources...), update.InstallSource("unknown-source")) {
			for _, contextKind := range []string{"canceled", "deadline"} {
				add(fmt.Sprintf("%s/context_%s/%s", operation, contextKind, source), migrationCoordinatorInput{
					Operation: operation, Source: source, Candidate: migrationCoordinatorRelease("v2.0.0", false), Context: contextKind,
				})
			}
		}
	}
	for _, source := range []update.InstallSource{update.InstallSourceRelease, update.InstallSourceGoInstall} {
		for _, cause := range []string{"installer", "context_canceled", "context_deadline"} {
			input := migrationCoordinatorInput{Source: source, Candidate: migrationCoordinatorRelease("v2.0.0+exact.4", false)}
			if source == update.InstallSourceRelease {
				input.InstallerError = cause
			} else {
				input.CommandErrors = []string{cause}
			}
			add("installation_failure/"+string(source)+"/"+cause, input)
		}
	}
	add("default_installer/no_trusted_key", migrationCoordinatorInput{Source: update.InstallSourceRelease, Candidate: migrationCoordinatorRelease("v2.0.0", false), DefaultInstaller: true})
	add("default_installer/check_barrier", migrationCoordinatorInput{Source: update.InstallSourceRelease, Candidate: migrationCoordinatorRelease("v2.0.0", false), DefaultInstaller: true, Check: true})
	add("explicit/development_before_version_parse", migrationCoordinatorInput{Source: update.InstallSourceDevelopment, CurrentVersion: "invalid", DetectorError: ""})
	add("explicit/dev_build_injected_release_source", migrationCoordinatorInput{Source: update.InstallSourceRelease, CurrentVersion: "dev"})
	for _, source := range []update.InstallSource{update.InstallSourceHomebrewStable, update.InstallSourceGoInstall, update.InstallSourceRelease, "unknown-source"} {
		add("explicit/throttled_with_candidate/"+string(source), migrationCoordinatorInput{Source: source, Throttled: true, Candidate: migrationCoordinatorRelease("v2.0.0", false)})
		add("explicit/throttled_absent/"+string(source), migrationCoordinatorInput{Source: source, Throttled: true})
	}
	for _, source := range append(append([]update.InstallSource{}, sources...), update.InstallSource("unknown-source")) {
		for _, candidate := range candidates {
			add(fmt.Sprintf("automatic/%s/%s", source, candidate.name), migrationCoordinatorInput{
				Operation: "automatic", Source: source, CurrentVersion: candidate.current, Candidate: candidate.release,
			})
		}
	}
	add("automatic/dev_before_all_ports", migrationCoordinatorInput{
		Operation: "automatic", CurrentVersion: "dev", Source: update.InstallSourceRelease,
		DetectorError: "detector", CheckerError: "checker", Candidate: migrationCoordinatorRelease("bad-tag", true),
	})
	for _, selected := range []*update.Release{nil, migrationCoordinatorRelease("v2.0.0", false), migrationCoordinatorRelease("bad-tag", false)} {
		label := "absent"
		if selected != nil {
			label = selected.TagName
		}
		add("automatic/throttled/"+label, migrationCoordinatorInput{
			Operation: "automatic", Source: update.InstallSourceRelease, Throttled: true, Candidate: selected, DetectorError: "detector",
		})
	}
	for _, candidate := range []struct{ name, tag string }{{"absent", ""}, {"equal", "v1.0.0"}, {"older", "v0.9.0"}, {"build_metadata_equal", "v1.0.0+remote"}} {
		input := migrationCoordinatorInput{Operation: "automatic", Source: update.InstallSourceRelease, DetectorError: "detector"}
		if candidate.tag != "" {
			input.Candidate = migrationCoordinatorRelease(candidate.tag, false)
		}
		add("automatic/skip_detector/"+candidate.name, input)
	}
	add("automatic/throttle_does_not_hide_checker_failure", migrationCoordinatorInput{Operation: "automatic", Source: update.InstallSourceRelease, Throttled: true, CheckerError: "checker"})
	return cases
}

func migrationCoordinatorWitnesses(t *testing.T, cases []migrationCoordinatorCase) {
	t.Helper()
	byName := map[string]migrationCoordinatorOutcome{}
	representationOnlyCount := 0
	for _, captured := range cases {
		if _, exists := byName[captured.Name]; exists {
			t.Fatalf("duplicate case %s", captured.Name)
		}
		byName[captured.Name] = captured.Outcome
		if captured.EvidenceScope == "go_representation_only" {
			representationOnlyCount++
			if captured.RepresentationOnlyReason == "" {
				t.Errorf("%s is missing its representation-only scope reason", captured.Name)
			}
		} else if captured.EvidenceScope != "behavioral_contract" || captured.RepresentationOnlyReason != "" {
			t.Errorf("%s has an unexpected evidence classification", captured.Name)
		}
	}
	if representationOnlyCount != 9 {
		t.Errorf("representation-only rows: got %d, want six nil required-port constructors, one nil optional-port constructor and two nil receivers", representationOnlyCount)
	}
	outcome := func(name string) migrationCoordinatorOutcome {
		t.Helper()
		got, exists := byName[name]
		if !exists {
			t.Fatalf("required witness %s is missing", name)
		}
		return got
	}
	requireTrace := func(name string, want ...string) {
		t.Helper()
		got := outcome(name)
		if !reflect.DeepEqual(got.Trace, append([]string{}, want...)) {
			t.Errorf("%s trace: got %v, want %v", name, got.Trace, want)
		}
	}
	requireTrace("automatic/dev_before_all_ports")
	requireTrace("automatic/current_invalid/not-a-version")
	requireTrace("coordinator/current_invalid/not-a-version", "detector")
	requireTrace("automatic/detector_failure_invalid_current")
	requireTrace("coordinator/detector_failure_invalid_current", "detector")
	requireTrace("explicit/development_before_version_parse", "detector")
	for _, label := range []string{"absent", "equal", "older", "build_metadata_equal"} {
		requireTrace("automatic/skip_detector/"+label, "checker")
	}
	for _, label := range []string{"absent", "v2.0.0", "bad-tag"} {
		requireTrace("automatic/throttled/"+label, "checker")
	}
	for _, source := range []update.InstallSource{update.InstallSourceHomebrewStable, update.InstallSourceHomebrewBeta} {
		include := source == update.InstallSourceHomebrewBeta
		formula := "FlanChanXwO/tap/pixiv-cli"
		if include {
			formula += "-beta"
		}
		for _, candidate := range []string{"absent", "equal", "older", "build_metadata_equal"} {
			name := fmt.Sprintf("explicit/%s/%s/check_false/prerelease_%t", source, candidate, include)
			got := outcome(name)
			want := []update.Command{{Name: "brew", Args: []string{"upgrade", formula}}}
			if !reflect.DeepEqual(got.Commands, want) || got.Error != nil || got.Result == nil || got.Result.UpdateAvailable {
				t.Errorf("%s must execute exact same-channel upgrade regardless of candidate availability", name)
			}
		}
		name := fmt.Sprintf("explicit/%s/absent/check_false/prerelease_%t", source, !include)
		got := outcome(name)
		if got.Result == nil || !got.Result.UpdateAvailable || got.Result.LatestVersion != nil || len(got.Commands) != 2 {
			t.Errorf("%s must switch channels even without a candidate", name)
		}
		for _, label := range []string{"absent", "downgrade"} {
			prefix := "homebrew_switch/" + string(source) + "/" + label + "/"
			requireTrace(prefix+"uninstall_failure", "detector", "checker", "command:brew")
			requireTrace(prefix+"rollback_success", "detector", "checker", "command:brew", "command:brew", "command:brew")
			requireTrace(prefix+"rollback_failure", "detector", "checker", "command:brew", "command:brew", "command:brew")
			for _, suffix := range []string{"rollback_success", "rollback_failure"} {
				got := outcome(prefix + suffix)
				if got.Error == nil || !got.Error.Is["command_install"] || got.Error.Is["command_rollback"] || got.Result == nil || !got.Result.UpdateAvailable {
					t.Errorf("%s lost the primary cause/result or wrapped the non-wrapped rollback cause", prefix+suffix)
				}
			}
		}
	}
	for _, noticeCase := range []struct {
		source  update.InstallSource
		command string
	}{
		{update.InstallSourceHomebrewStable, "brew upgrade FlanChanXwO/tap/pixiv-cli"},
		{update.InstallSourceHomebrewBeta, "pixiv update"},
		{update.InstallSourceGoInstall, "go install github.com/FlanChanXwO/pixiv-cli/cmd/pixiv@v2.0.0"},
		{update.InstallSourceRelease, "pixiv update"},
	} {
		name := "automatic/" + string(noticeCase.source) + "/newer"
		got := outcome(name)
		want := &update.AutomaticUpdateNotice{
			Source: noticeCase.source, CurrentVersion: "v1.0.0", LatestVersion: "v2.0.0", UpdateCommand: noticeCase.command,
		}
		if !reflect.DeepEqual(got.Notice, want) || got.Error != nil {
			t.Errorf("%s notice changed: %+v", name, got.Notice)
		}
		requireTrace(name, "checker", "detector")
	}
	unknown := outcome("automatic/unknown-source/newer")
	if unknown.Notice != nil || unknown.Error == nil || unknown.Error.Message != `unsupported installation source "unknown-source" for automatic update` {
		t.Error("automatic unknown-source error changed")
	}
	development := outcome("automatic/development/newer")
	if development.Notice != nil || development.Error != nil {
		t.Error("automatic detected development source must return no notice")
	}
	for _, operation := range []string{"coordinator", "automatic"} {
		for _, cause := range []string{"detector", "checker"} {
			got := outcome(operation + "/" + cause + "_failure")
			if got.Error == nil || !got.Error.Is[cause] {
				t.Errorf("%s lost %s sentinel identity", operation, cause)
			}
		}
	}
	for _, source := range []update.InstallSource{update.InstallSourceRelease, update.InstallSourceGoInstall} {
		got := byName["installation_failure/"+string(source)+"/installer"]
		if got.Result == nil || *got.Result != (update.UpdateResult{}) || got.Error == nil || !got.Error.Is["installer"] {
			t.Errorf("%s installation failure must return zero result and retain its cause", source)
		}
	}
	goNewer := byName["explicit/go-install/prerelease/check_false/prerelease_true"]
	wantGo := []update.Command{{Name: "go", Args: []string{"install", "github.com/FlanChanXwO/pixiv-cli/cmd/pixiv@v2.0.0-beta.2+asset.3"}}}
	if !reflect.DeepEqual(goNewer.Commands, wantGo) {
		t.Errorf("go install did not preserve exact selected tag: %v", goNewer.Commands)
	}
	for _, label := range []string{"absent", "equal", "older", "build_metadata_equal"} {
		got := byName["explicit/release/"+label+"/check_false/prerelease_false"]
		if len(got.Installations) != 0 || got.Result == nil || got.Result.UpdateAvailable {
			t.Errorf("%s release incorrectly installed or advertised a non-newer candidate", label)
		}
	}
	newer := byName["explicit/release/newer/check_false/prerelease_false"]
	if len(newer.Installations) != 1 || !reflect.DeepEqual(newer.Installations[0], *migrationCoordinatorRelease("v2.0.0", false)) {
		t.Error("release installer did not receive the complete selected release")
	}
}

var migrationCoordinatorSources = map[string]string{
	"go.mod":                                 "81990f7489f40c325163dc9614fe482b60aec6be2460fddfcb6b09b2c666e13c",
	"go.sum":                                 "22b07d0a3de3d9b37e71cc72baebfcd281fe7c95166821f715c215121bbdf64e",
	"internal/shared/buildinfo/buildinfo.go": "f1d207133ada61dc518fd2db18ff918cce20d50b26e5667101578ee8f81468ae",
	"internal/update/automatic_check.go":     "6e3ed70bd9c82213a42a4272912d212f907bad23e2eda718864a6b42e63c2f1c",
	"internal/update/coordinator.go":         "7ddc8b681e9c27fe088236d6d5fe6973b2cdce6bfb8b86a0213194647f320968",
	"internal/update/installer/cache_permissions_unix.go":    "481c28f434b7d235f600ab62fd9d63c993ca6830c0ed0a7c871901d75bef004d",
	"internal/update/installer/cache_permissions_windows.go": "cbe2d6b387769683128a2bdf83d1f0f5e15263fa690b475ad7505fd6cf70a30d",
	"internal/update/installer/install_source.go":            "0e857e9add4f4f6d469e96861e484a4620c17163ee11182a0b1909a8f8f9dff0",
	"internal/update/installer/installer.go":                 "07d5ecde71aa2d13ef87c0da9ee7f722e8b8dd23242775f5048882ab5e16cdf2",
	"internal/update/installer/pending_cleanup.go":           "bfc9fd4ef560745c0cdbe5219320bcf76f82bd2f79f4adee6cb94923763c1d45",
	"internal/update/installer/release_cache.go":             "9d091159d1f32b328f6244f17b2ba23a704dd7cfd487a907172042b7535512be",
	"internal/update/installer/release_installer.go":         "22d49ff1816ebf363ee4d01be3aa826a7a7dc955c985b692da64e3e5b935af87",
	"internal/update/process/process.go":                     "efd9439a9a3417bc9d5812808b9522f6ce9ba8e3207d55a3c79ed79ef9970258",
	"internal/update/process/process_runner.go":              "e73c4687ee16ea1f18901915908c83e2ce3d663e26501226bd61754e73c87245",
	"internal/update/reexports.go":                           "1ec99e116a0a3322486290ed02b089ce870108de6163c818ecf210da9ea12c07",
	"internal/update/release/cache.go":                       "493d46de83ad0ccae219c19a5e2abbe3a7feb2e68783e64afce912d504ca20f2",
	"internal/update/release/release.go":                     "f3bc607d3b5ae74243d91cea06531f8fcee91b0ee5916d337148e17c58a1bb6e",
	"internal/update/release/release_client.go":              "9de30775c343d211ea62a61f29cb9c02f33c59f71c42613620e61001f8eac8d0",
	"internal/update/release/version_policy.go":              "988c885c7c078f0e2c3d748869a55586db0fcd3ce3fcce8db377a860fdbc219a",
	"internal/update/source/github.go":                       "1fd8c8abd4e0855298475ce7455ab775bc4eec6c3fda07a59b76fbb8780c617d",
	"internal/update/source/release_source_selector.go":      "cbfb9bedcd32b391a52ce6a038710e45320dfbd2edf70c746ad72d22b8ad21f0",
	"internal/update/source/release_sources.go":              "5450d686f836da97e264872e80ba4178bcec7a4217fd8a9eaa85abfccf144bc8",
	"internal/update/source/source.go":                       "3d0912a3937d6cfa6f871c380b60922e60898ca23f88e3b722179c4f1ba64aa5",
	"internal/update/update.go":                              "1c9c57ce92f5f30f593f8007764ed2300ea38fb048a7a615820e1f71011964ce",
}

func migrationCoordinatorGuard(t *testing.T) {
	t.Helper()
	root := filepath.Join("..", "..")
	verify := func(files map[string]string) {
		t.Helper()
		paths := make([]string, 0, len(files))
		for path := range files {
			paths = append(paths, path)
		}
		sort.Strings(paths)
		for _, path := range paths {
			body, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(path)))
			if err != nil {
				t.Fatal(err)
			}
			if got := fmt.Sprintf("%x", sha256.Sum256(body)); got != files[path] {
				t.Fatalf("preservation guard failed for %s: got %s, want %s", path, got, files[path])
			}
		}
	}
	verify(migrationCoordinatorSources)
	body, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(migrationCoordinatorBaselinePath)))
	if err != nil {
		t.Fatal(err)
	}
	if got := fmt.Sprintf("%x", sha256.Sum256(body)); got != migrationCoordinatorBaselineSHA {
		t.Fatalf("durable preservation baseline changed: got %s, want %s", got, migrationCoordinatorBaselineSHA)
	}
	var baseline struct {
		BaseSHA         string            `json:"base_sha"`
		FrozenGo        string            `json:"frozen_go"`
		ProductionCount int               `json:"production_count"`
		Production      map[string]string `json:"production"`
		FixtureCount    int               `json:"fixture_count"`
		Fixtures        map[string]string `json:"fixtures"`
		JSONCount       int               `json:"json_count"`
		PEMCount        int               `json:"pem_count"`
	}
	if err := json.Unmarshal(body, &baseline); err != nil {
		t.Fatal(err)
	}
	if baseline.BaseSHA != "0e5f42273f69067bf2e06a130c926c4d58557fb4" || baseline.FrozenGo != migrationCoordinatorReference || baseline.ProductionCount != 434 || len(baseline.Production) != baseline.ProductionCount || baseline.FixtureCount != 110 || len(baseline.Fixtures) != baseline.FixtureCount {
		t.Fatal("unexpected preservation baseline identity or counts")
	}
	jsonCount, pemCount := 0, 0
	for path := range baseline.Fixtures {
		switch filepath.Ext(path) {
		case ".json":
			jsonCount++
		case ".pem":
			pemCount++
		default:
			t.Fatalf("unexpected published fixture extension: %s", path)
		}
	}
	if jsonCount != baseline.JSONCount || pemCount != baseline.PEMCount || jsonCount+pemCount != baseline.FixtureCount {
		t.Fatal("declared published fixture type counts do not match the durable inventory")
	}
	verify(baseline.Production)
	verify(baseline.Fixtures)
}

func TestMigrationUpdaterCoordinatorConnectedContracts(t *testing.T) {
	migrationCoordinatorGuard(t)
	fixture := migrationCoordinatorFixture{
		SchemaVersion: 1, Reference: migrationCoordinatorReference, SourceSHA256: migrationCoordinatorSources,
		Evidence: "Actual public UpdateCoordinator.Execute and AutomaticUpdateChecker.Check with synthetic read-only dependency ports. Trace, full result/notice, checker flags, exact command argv, complete selected release, original context identity, error unwrap types/messages and sentinel cause identity are captured. No command, asset request, downloaded execution, executable replacement or native installer is performed. Nil release installer invokes the actual default constructor and its missing-trusted-key preflight only. Source/module bytes and the supplied 434-production/110-published-fixture preservation snapshot are guarded. Native installation-source detection, release protocol/cache, asset verification, OS replacement and root CLI lifecycle are separate evidence boundaries.",
		MetadataScope: map[string]string{
			"cases[].outcome.error.unwrap_chain":        "go_representation_only: concrete Go wrapping graph and intermediate wrapper messages; not a required Rust error layout",
			"cases[].outcome.error.unwrap_chain[].type": "go_representation_only: dynamic concrete Go error type names; not required Rust error types",
			"cases[].outcome.error.message":             "behavioral_contract: complete returned diagnostic remains captured verbatim",
			"cases[].outcome.error.is":                  "behavioral_contract: primary and rollback cause preservation remains required independently of language-specific wrapper layout",
		},
		PreservationBaseline: migrationCoordinatorBaselineMetadata{
			Path: migrationCoordinatorBaselinePath, SHA256: migrationCoordinatorBaselineSHA, ProductionCount: 434, FixtureCount: 110,
		},
		Cases: migrationCoordinatorInputs(),
	}
	for index := range fixture.Cases {
		captured := &fixture.Cases[index]
		t.Run(captured.Name, func(t *testing.T) {
			captured.Outcome = migrationCoordinatorObserve(t, captured.Input)
		})
	}
	migrationCoordinatorWitnesses(t, fixture.Cases)
	body, err := json.MarshalIndent(fixture, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	body = append(body, '\n')
	path := filepath.Join("..", "..", "crates", "pixiv-cli", "tests", "fixtures", "updater-coordinator.json")
	if *migrationCaptureCoordinator {
		if t.Failed() {
			t.Fatal("refusing to capture failed contract witnesses")
		}
		if err := os.WriteFile(path, body, 0644); err != nil {
			t.Fatal(err)
		}
		t.Logf("captured %d public coordinator/automatic cases", len(fixture.Cases))
	} else {
		frozen, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(frozen, body) {
			var expected migrationCoordinatorFixture
			if err := json.Unmarshal(frozen, &expected); err != nil {
				t.Fatal(err)
			}
			if len(expected.Cases) != len(fixture.Cases) {
				t.Fatalf("case count changed: got %d, frozen %d", len(fixture.Cases), len(expected.Cases))
			}
			for index, actual := range fixture.Cases {
				if !reflect.DeepEqual(actual, expected.Cases[index]) {
					got, _ := json.MarshalIndent(actual, "", "  ")
					want, _ := json.MarshalIndent(expected.Cases[index], "", "  ")
					t.Errorf("%s changed\ngot:\n%s\nfrozen:\n%s", actual.Name, got, want)
				}
			}
			t.Fatal("frozen coordinator fixture bytes changed; inspect the pinned reference before any explicit recapture")
		}
		t.Logf("replayed %d public coordinator/automatic cases", len(fixture.Cases))
	}
	migrationCoordinatorGuard(t)
}

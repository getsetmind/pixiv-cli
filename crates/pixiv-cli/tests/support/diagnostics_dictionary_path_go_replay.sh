#!/usr/bin/env bash
set -euo pipefail

support=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$support/../../../.." && pwd)
cd "$repo"
: "${GOROOT:?Source the authorized official Go toolchain environment first}"
export GOPROXY=off GOWORK=off GOTOOLCHAIN=local CGO_ENABLED=1
sha256sum -c "$support/diagnostics-root-evidence/artifacts.sha256"
sha256sum -c "$support/diagnostics-root-joined-evidence/artifacts.sha256"
sha256sum -c "$support/diagnostics-dictionary-path-evidence/artifacts.sha256"
"$GOROOT/bin/go" version
formatted=$("$GOROOT/bin/gofmt" -l internal/cli/migration_diagnostics_connected_test.go internal/cli/migration_diagnostics_joined_test.go internal/cli/migration_diagnostics_dictionary_path_test.go)
test -z "$formatted"
"$GOROOT/bin/go" test -mod=readonly -run '^TestMigrationDiagnostics(Root|Joined|Dictionary)' -count=3 -v ./internal/cli
"$GOROOT/bin/go" vet -mod=readonly ./internal/cli ./internal/cli/diagnostics ./internal/shared/diagnostics
printf 'Dictionary ordered scalar8 actual root replays passed count3 with zero HTTP; primary19/prior4/joined6 unchanged; gofmt/vet passed\n'

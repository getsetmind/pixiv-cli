#!/usr/bin/env bash
set -euo pipefail

support=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$support/../../../.." && pwd)
cd "$repo"

: "${GOROOT:?Source the authorized official Go toolchain environment first}"
export GOPROXY=off GOWORK=off GOTOOLCHAIN=local CGO_ENABLED=0
if [[ ${1:-} == --race ]]; then
  export CGO_ENABLED=1
  run_flags=(-race -mod=readonly)
else
  run_flags=(-mod=readonly)
fi

probe="$support/diagnostics_presenter_go_probe.go.txt"
tmp=$(mktemp -d "$support/diagnostics-presenter-replay.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
cp "$probe" "$tmp/main.go"

frozen=4b4426487ef18bed276706daec385e0d0a6979f9
for path in internal/cli/diagnostics/diagnostics.go internal/cli/diagnostics/diagnostics_test.go internal/shared/diagnostics/diagnostics.go internal/shared/diagnostics/diagnostics_test.go; do
  live=$(sha256sum "$path" | cut -d' ' -f1)
  reference=$(git show "$frozen:$path" | sha256sum | cut -d' ' -f1)
  test "$live" = "$reference"
done

"$GOROOT/bin/go" version
"$GOROOT/bin/gofmt" -l "$probe" internal/cli/diagnostics/*.go internal/shared/diagnostics/*.go > "$tmp/gofmt.log"
test ! -s "$tmp/gofmt.log"
"$GOROOT/bin/go" run "${run_flags[@]}" "$tmp/main.go" > "$tmp/primary.json"
"$GOROOT/bin/go" run "${run_flags[@]}" "$tmp/main.go" --extra > "$tmp/extra.json"
cmp crates/pixiv-cli/tests/fixtures/diagnostics-presenter.json "$tmp/primary.json"
cmp crates/pixiv-cli/tests/fixtures/diagnostics-presenter-extra.json "$tmp/extra.json"
"$GOROOT/bin/go" vet -mod=readonly "$tmp/main.go"
"$GOROOT/bin/go" vet -mod=readonly ./internal/cli/diagnostics ./internal/shared/diagnostics
"$GOROOT/bin/go" test -mod=readonly -count=1 -v ./internal/cli/diagnostics ./internal/shared/diagnostics
printf 'Presenter primary188 and additive22 Go observations match unchanged fixtures; vet/gofmt passed\n'

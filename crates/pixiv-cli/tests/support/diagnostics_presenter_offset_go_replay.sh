#!/usr/bin/env bash
set -euo pipefail

support=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$support/../../../.." && pwd)
cd "$repo"
: "${GOROOT:?Source the authorized official Go toolchain environment first}"
export GOPROXY=off GOWORK=off GOTOOLCHAIN=local CGO_ENABLED=0
flags=(-mod=readonly)
if [[ ${1:-} == --race ]]; then
  export CGO_ENABLED=1
  flags=(-race -mod=readonly)
fi

probe="$support/diagnostics_presenter_offset_go_probe.go.txt"
tmp=$(mktemp -d "$support/diagnostics-presenter-offset-replay.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
cp "$probe" "$tmp/main.go"
for path in internal/cli/diagnostics/diagnostics.go internal/shared/diagnostics/diagnostics.go; do
  git show "4b4426487ef18bed276706daec385e0d0a6979f9:$path" > "$tmp/reference.go"
  cmp "$path" "$tmp/reference.go"
done
"$GOROOT/bin/go" version
"$GOROOT/bin/gofmt" -l "$probe" > "$tmp/gofmt.log"
test ! -s "$tmp/gofmt.log"
"$GOROOT/bin/go" run "${flags[@]}" "$tmp/main.go" > "$tmp/capture.json"
cmp crates/pixiv-cli/tests/fixtures/diagnostics-presenter-offset.json "$tmp/capture.json"
"$GOROOT/bin/go" vet -mod=readonly "$tmp/main.go"
printf 'All14 additive frozen Presenter clock-offset observations match unchanged fixture; vet/gofmt passed\n'

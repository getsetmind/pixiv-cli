#!/usr/bin/env bash
set -euo pipefail

support=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$support/../../../.." && pwd)
cd "$repo"
: "${GOROOT:?Source the authorized official Go toolchain environment first}"
export GOPROXY=off GOWORK=off GOTOOLCHAIN=local
flags=(-mod=readonly)
if [[ ${1:-} == --race ]]; then
  export CGO_ENABLED=1
  flags=(-race -mod=readonly)
fi
"$GOROOT/bin/go" version
"$GOROOT/bin/go" test "${flags[@]}" ./sdk/pixiv -run '^TestMigrationRequestContextMatchesFrozenGo$' -count=1 -timeout=2m
"$GOROOT/bin/go" vet -mod=readonly ./sdk/pixiv
formatted=$("$GOROOT/bin/gofmt" -l sdk/pixiv/migration_request_context_test.go)
test -z "$formatted"
printf 'Public SDK per-request context matches exact frozen Go fixture; vet/gofmt passed\n'

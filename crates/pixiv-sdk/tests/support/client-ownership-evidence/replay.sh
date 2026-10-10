#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../../../.." && pwd)
cd "$root"
source /tmp/quantus-toolchain-env.txt
source /tmp/pixiv-fanbox-native-build-env.sh
export GOPROXY=off GOWORK=off GOTOOLCHAIN=local
[[ "$(go version)" == 'go version go1.27.1 linux/amd64' ]]
go test -mod=readonly ./sdk/pixiv -run '^TestMigrationClientOwnership(FrozenGo|EntropyChild)$' -count="${CLIENT_OWNERSHIP_REPEAT:-3}" -v

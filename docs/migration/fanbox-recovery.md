# FANBOX recovery after filesystem replacement

## Current verified baseline

The cloud checkout was restored at `33c6866ae7b87c195c20263f1a7074c091b08227` on 2026-10-10. Frozen Go reference: `4b4426487ef18bed276706daec385e0d0a6979f9`. Uncommitted FANBOX source, fixtures, toolchains, caches and execution logs became unavailable. Previous prototype successes are historical observations, not verification of reconstructed source. No FANBOX implementation checkpoint had passed the final full script or been published.

Recovery must freeze actual Go behavior again, reconstruct the Rust implementation against those fixtures, and run fresh related Go tests/vet/gofmt, the unchanged `scripts/check-rust.ps1`, independent review and a tested SHA256 manifest before signed publication. Contracts-only checkpoints may be published at reviewed, completely tested boundaries without representing them as completed implementation.

## Lost prototype scope

- SDK context plus FANBOX models/protocol/identity/solver/native transport
- SQLite saved account repository, selected default, verification-before-save service and once-owned lease
- CLI auth import/list/use/remove/status, FANBOX MCP owner and actual startup wiring
- MCP current_user, all eleven schemas, cancellation/drain and Go-compatible stdio wire serialization

Other FANBOX content tools, browser cookie extraction, content CLI, resource reopening and download remained unimplemented. Existing SDK HTTP2/Accept/context/client ownership/idle-close, pacing, native platform and repeated Windows cleanup debts remain open.

## Historical Go-first contract inventory to reconstruct

Identity/protocol236; solver113; saved accounts100; auth owner182; root startup133; MCP current_user45; CLI MCP owner105; real MCP process6; native TLS/H2/idle; media20; zstd5; buffered Brotli2. These counts describe the lost prototype, not currently available fixtures or current test passes. Preserve explicit Go-only fields and genuine native/platform gaps.

## Genuine defects found before loss

1. Disabled tower-http decompression hid HTTP2 EOS. The prototype fixed this with exact final HEADERS END_STREAM metadata, not Content-Length or eventual DATA EOS inference
2. Brotli tiny reads needed decoder-owned output drained before source refill. A separate Go-first2-row fixture and genuine Rust RED established this. Output correction passed; completed-stream repeated Close still lacked the expected physical CANCEL
3. Root group help must use parsed routing: proxy value `auth` is not the auth command
4. Injected solver control malformed redirect Location must close its response body once before returning solver-unavailable

Concurrent Go Read/Close remains a distinct ownership/API comparison. An unrelated supplemental HTTP2 multiplex/upload probe stopped after a tool safety flag and must not be retried through a different route. It is not available evidence.

## Native source recovery identities

Official source-pinned family: wreq6.0.0-rc.31, btls0.5.6, btls-sys0.5.6, wreq-proto0.2.5, http2 0.5.20. BoringSSL gitlink `91a66a59b6c1435120ff83e245d7719411294386`; all1,435 packaged native blobs were unchanged in the lost prototype. Imported files must be reverified against official registry checksums and Git blobs.

Seven source patches had been applied: safe requested-trust-anchor API; wreq trust-anchor configuration; fixed extension-order suffix shuffle; actual idle-only shutdown; default-off legacy deflate frame-reader gate; default-off client HEADERS EOS/nonzero Content-Length compatibility; exact initial HEADERS EOS metadata. The body-close capability was still design-stage. Preserve CRLF byte-for-byte when generating ordered patches, and independently reconstruct the final source tree from official originals.

Fresh pristine source recovery is recorded in [native source provenance](provenance/fanbox-recovery-native-sources.json). All five exact archives agree with independent official sparse-index and crates.io API checksums; safe extraction preserves every byte and records per-file inventories. The fresh file counts are wreq 88, btls 108, btls-sys 1,452, wreq-proto 39 and http2 71. All 1,435 packaged BoringSSL blobs match the official commit's Git blob IDs and lengths; the upstream native tree was rehashed against Google Gitiles root `258beb93b3d6101c44e172b16e105d2547152dac` (410 trees, 8,140 upstream blobs). This verifies pristine source identity only. No patched dependency has been imported into the repository, and no recovered native build, transport/runtime or platform verification is implied.

Fresh Chrome146 non-PSK TLS profile, requested trust anchor0xca34/0000, X25519MLKEM768+X25519, new ALPS codepoint, exact HTTP2 settings/window/pseudo order/weight, actual31-second stall, certificate rejection and active/idle closure require real owned synthetic peers. Ordinary HTTP transport is not fingerprint equivalence. HTTP1/resumption/HRR/other-platform trust and unfinished upload/HEAD boundaries remain separately unverified.

## Toolchain recovery

Required prior versions: Rust1.93.0, Go1.27.1 and official PowerShell/CMake. Host compiler/Perl/libclang19/GCC14 headers survived. Toolchain directories and caches must be restored from official sources with fresh provenance; PowerShell telemetry opt-out must precede every launch. No user computer or local Codex is used.

## Fresh recovery progress

Official toolchains were restored with checksums recorded in [toolchain provenance](provenance/fanbox-recovery-toolchains.json): Rust/cargo1.93.0, Go1.27.1, PowerShell7.6.6 and CMake3.31.6. Locked Cargo fetch and pinned Go dependency download completed successfully without repository module/lock changes. `go mod verify` passed using the canonical `go-path/pkg/mod` cache; an earlier command against an empty default cache failed with missing ziphash files and is not verification evidence.

New Go-only identity/account/solver-redirect contracts are being captured. The fresh unchanged full Rust script finished successfully on 2026-10-10 at 05:34 UTC: formatter, strict workspace/all-target Clippy, workspace tests and optimized release. The log is `/tmp/pixiv-recovery-contracts-full-gates.log`, with `.exit` equal to zero. It contains 271 test summaries, 655 raw passes, zero failures and the existing 11 ignored tests. Meaningful passes are 654 after excluding the unchanged no-environment terminal child scaffold. The cold optimized release stage took 1 minute 43 seconds.

This verifies the restored published Rust baseline, not new FANBOX implementation parity. Rust production, native/vendor, module/lock metadata and the full script have not changed. Current Go contract checks, independent review and the final frozen manifest remain separate requirements; their completion is not inferred from the baseline full gate.

### Fresh identity and malformed-redirect contracts

`sdk/fanbox/migration_identity_protocol_test.go` captures 309 observations across 20 families into `crates/pixiv-sdk/tests/fixtures/fanbox-identity-protocol.json`. Of these, 116 rows reach the injected transport and record 140 requests. Constructors, credential normalization/redaction, options, request forwarding, redirects, identity decoding, DTOs, session validation, body ownership and safe errors use the actual frozen SDK/protocol. Explicit Go-only formatting/containers and synthetic-cookie projection remain named. Media URL validation does not verify resource delivery. An injected transport that ignores an already-canceled context can still return success; a transport failure preserves cancellation classification. This distinction is not replaced by unconditional cancellation.

Actual URL options distinguish decoded paths: solver/upstream roots containing `%2F` reject because the decoded path is `//`; native proxy validation accepts encoded paths because it only constrains scheme/host/userinfo. A trailing empty fragment accepts where an empty query rejects. These observations are contracts, not a reason to silently tighten Rust validation.

Capture, unchanged replay, both complete related packages, race, vet and gofmt passed with Go 1.27.1. Original logs and commands are `/tmp/fanbox-identity-recovery/manifest.json` and its named `.log` files. The fixture SHA256 is `cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b`; test SHA256 is `65784f3eaf202bc3799e7ee252c6c75e8e6b3a2ab259ef0b273db9f1e56b7015`. Sixteen SDK ledger entries become `in_progress` for Go-first evidence only, with no Rust source or verified platform added.

`internal/services/fanbox/protocol/migration_solver_redirect_test.go` adds three observations/six genuine standard `http.Client.Do` executions in `crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json`. A malformed Location closes the response once before policy, performs no reads and maps to solver-unavailable even when Close fails. A valid no-follow control invokes policy while open, then the actual solver closes once and maps its 302 response to solver-failed. Full private solver state, typed unwrap chain, request metadata, body counts and callback trace are preserved. Capture/replay/race/vet/gofmt passed; original logs and final hashes are in the sibling `fanbox-solver-redirect-logs` directory. This supplement does not verify solver singleflight/cache/challenge replay or physical native cancellation.

### Saved-account source observations retained during recovery

Actual isolated SQLite repository/service and injected owned SDK leases capture persistence, verification-before-save, selected defaults/fallback, protocol options/proxy/context forwarding and cleanup. The public account service is distinct from browser cookie extraction and authenticated remote verification. All sessions are synthetic and each database/config path is owned by the tests.

Three source-observed private failure boundaries remain explicit rather than repaired in Go or hidden from the fixture:

- A nil caller context can panic inside `database/sql` with its mutex locked. Controlled bounded subprocesses preserve the panic/deadlock observation without reusing or closing that poisoned database in the test parent
- Unsupported `fmt.Sprintf("%p", AccountValue)` bypasses the Account formatter and exposes synthetic session bytes in Go's invalid-format diagnostic. Supported formatting verbs remain redacted. This is a documented Go-only formatting/security debt, not a Rust redaction expectation
- Deferred foreign-key commit failure can leave uncommitted changes visible through the same driver connection and prevent another transaction until database Close rolls them back. A durable reopen retains the pre-failure state. Insert/update results and transient/durable observations are separately captured; a failed commit is not silently normalized into atomic success

The sealed saved-account fixture contains 321 fresh observation rows: repository 75, service 162 and leases 84. Fixture SHA256 is `77dc6f24a44d27220e69987698b1e562382fbece23d895abcd31b13b906de221`. Capture and unchanged three-package replay passed. The full five-package related sweep and race sweep each passed 142 top-level tests, with three explicitly reported skips: two existing opt-in Rust interoperability tests and the no-mode nil-context subprocess helper. The main service contract actually executes that helper in five bounded owned children; its no-mode skip is not evidence for those cases. Vet and gofmt passed. Original commands, logs, source/dependency guards and limitations are `/tmp/fanbox-saved-accounts-recovery-manifest.json`, preserved unchanged in [saved-account provenance](provenance/fanbox-recovery-saved-accounts.json).

An initial service capture was interrupted after a nil-context panic poisoned the database mutex. Its original log and observed exit 130 are preserved in that manifest; it is not counted as a pass. The harness now isolates the genuine Go-only boundary in owned children. A separate initial assembled replay found a test-owned canonical key-order bug, corrected with lossless `json.Number` handling rather than changed contract observations. Frozen replay compares all values exactly. RowsAffected errors and arbitrary OS commit failures are not separately injected, and this source-only slice does not certify external authentication, browser extraction, native transport or Rust parity.

### CLI help/parser routing before implementation

`internal/cli/migration_fanbox_help_routing_test.go` captures 22 actual `Run` invocations in owned Linux children into `crates/pixiv-cli/tests/fixtures/fanbox-help-routing.json`. Owned HOME/XDG, tracked stdin and dependency boundaries plus socket/exec denial keep help/parse observations separate from authentication, browser/MCP/native work. Proxy values equal to `auth` or `mcp` remain values: `fanbox --proxy auth --help` displays the full FANBOX group, not auth help. The real auth token routes to auth. Actual Cobra scanning also distinguishes `fanbox --help auth` (FANBOX help) from `fanbox --help -- auth` (auth help); this surprising observation is preserved, not rewritten into an assumed terminator rule. Error/help ordering and malformed-config bypass retain exact stdout/stderr/status and absence of boundary/stdin calls.

Capture, exact replay, race, package vet and gofmt passed. The fixture SHA256 is `5301528abea777cb93ba1c5153714a90dbb79393d253d0c7aeb6793bfcf06b74`; test SHA256 is `0317b2b85bfc20f1e94a6069a734406ab42152592424332725375c04ebb13142`. Original tee logs are `/tmp/pixiv-fanbox-help-routing-recovery/{capture,replay,race,vet,gofmt}.log`. Five CLI ledger entries become `in_progress` for help/parser evidence only. No Rust implementation, full auth/session/MCP workflow, raw Go/Clap parser equivalence or non-Linux behavior is claimed.

The CLI supplemental also passed ten existing top-level root help/surface/parse-startup/explicit-JSON/auth-surface/auth-conflict/MCP-conflict tests plus its 22 owned Run children (1.447 seconds, exit zero). The no-mode helper was not selected; the entire CLI package was not executed. Exact commands/log hashes/scope and setup failure are preserved unchanged in [help-routing provenance](provenance/fanbox-recovery-help-routing.json). Identity, saved accounts, malformed redirect, native pristine source, toolchains and baseline-gate provenance are preserved alongside it.

## Checkpoint boundary

This recovery checkpoint changes only six new Go test harnesses, four new crate test fixtures and migration documentation/provenance. The ledger retains all 673 entries and changes only 16 SDK plus five CLI entries to `in_progress`, with tests and explicit incomplete-scope differences; it adds no Rust implementation/source or verified platform. All 434 production Go/module paths match frozen Go. Existing tracked sources/tests/native/vendor/Cargo/full-script bytes match the restored base, except the documented `.gitattributes` CRLF checkout representation. Migration surface/ledger validator tests and vet pass after all 21 entry updates. Independent final review and the SHA256 freeze establish this source-only boundary, not migration completion.

### Second recovered boundary: core contracts and shared context implementation

The post-recovery published contracts base is `b3f01c9944d3317d10d48543f529af6bb1dea2f2`. Solver124/context41/native6 are now freshly captured from frozen Go and sealed in provenance. The native preliminary proof was withdrawn after independent review and strengthened in five concrete harness areas; repaired capture/replay/race and full related gates replace it. No denied supplemental operation was retried. [Solver](provenance/fanbox-solver-contracts.json), [context](provenance/fanbox-context-contracts.json), [native](provenance/fanbox-native-profile-contracts.json) and [final Go gates](provenance/fanbox-transport-context-go-gates.json) retain exact source/fixture hashes and log provenance.

The first new Rust production slice is shared SDK context/diagnostics, with compatibility app reexports. Tests preceded the two production modules and genuine missing-module compile RED; focused final validation passes eight SDK and 19 existing app tests, with the one existing host child fixture ignored. Twenty mapped context/diagnostic rows are covered; SDK forwarding and FANBOX lifecycle are still pending. Existing Pixiv Facade remains unchanged. Constructors have no independent Go Done timer, so Rust monotonic/lazy-or-awaited timing is explicitly distinct. This foundation is not a reconstructed FANBOX client or native transport.

Independent Rust review found a real paused-Tokio-clock regression: completed timers returned Canceled without recording deadline or notifying descendants. Four deterministic tests were added before correction; three failed and prior-cancellation precedence passed in `/tmp/pixiv-context-paused-clock-red.log`. After recording delivered timer deadlines and propagating inherited owning deadlines, all 12 context tests passed in `/tmp/pixiv-context-paused-clock-green.log`. JoinHandle observations are bounded by two-second timeouts. The original full gate was explicitly interrupted (130) when the finding arrived; its incomplete log remains `/tmp/pixiv-context-bridge-full-gates.log`, never a full-pass claim. The final unchanged script is rerun from its first stage against corrected source.

Final unchanged full Rust script passes all stages: `/tmp/pixiv-context-bridge-final-full-gates.log`, exit0, 311 seconds, optimized release47.56s. Actual272 summaries comprise266 Running, five doc suites and one unchanged child summary; raw667/meaningful666 pass, zero fail and existing11 ignored. No assertion/ignored or Go production/module/native/vendor/Cargo/full-script changes were used. Final migration validator/vet pass; independent Go-first and Rust foundation reviews remain scoped to the recorded evidence, not full FANBOX/migration completion. [Gate provenance](provenance/fanbox-context-foundation-gates.json) preserves genuine compile/runtime RED, format failure, owned interrupted gate and final rerun separately.

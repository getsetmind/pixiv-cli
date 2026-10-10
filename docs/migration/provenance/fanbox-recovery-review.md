# Independent FANBOX recovery contracts review

Decision: APPROVED for the frozen Go-first contracts and pristine source/toolchain provenance checkpoint only. No blocking findings. This is not approval of FANBOX Rust implementation, native transport behavior, platform parity, or migration completion.

Reviewed on 2026-10-10. Restored base: `33c6866ae7b87c195c20263f1a7074c091b08227`. Frozen Go reference: `4b4426487ef18bed276706daec385e0d0a6979f9`.

## Frozen boundary

All owners handed off final test/fixture bytes before final review. All 21 files in `/tmp/pixiv-fanbox-recovery-pre-review-sha256.json` match their listed SHA256 values. This review artifact may be copied unchanged as the 22nd file; any subsequent change to the reviewed source, fixtures, documentation, or provenance requires a new comparison and appropriate checks.

The checkpoint consists of six new Go test harnesses, four new JSON fixtures, migration documentation, and seven provenance records. Independent protected-tree comparison inspected every one of the base's 6,800 tracked blobs: 6,796 untouched files match exactly, three migration metadata files intentionally differ, and `scripts/install.cmd` is the existing lossless 239-line CRLF checkout representation of its LF Git blob. All 434 Go production/module paths are byte-identical to the frozen Go commit. Existing tests, Rust production source, native/vendor source, Cargo/module metadata, and `scripts/check-rust.ps1` are unchanged. The review made no repository edits and ran no Cargo, external network, live account, browser, native peer, or native platform operation.

## Actual Go contract execution

### Identity/options/protocol

The 309 observations across 20 bounded families execute actual frozen SDK/protocol constructors and operations; 116 rows reach the injected transport and record 140 requests. The harness guards 13 frozen repository/module files, Go 1.27.1, three `net/http` source files, and four pinned dependency archives plus their extracted bytes. Independent inspection confirmed all 1,359 archive file comparisons in those four dependencies.

The fake boundary supplies only owned response bodies and failures to a genuine standard `*http.Client`/RoundTripper. Production code performs normalization, option validation, header forwarding, redirect handling, identity parsing, DTO conversion, error classification, and cleanup. Complete JSON bytes are compared without changing observed semantics. Go formatting/error topology/reader scheduling/pointer checks remain explicitly Go-only; the sole credential projection replaces a fixed synthetic session canary in observed Cookie headers.

The review confirmed the source-observed full-cookie normalization, ignored caller Jar/CheckRedirect, repeated delegated idle-close, redirects beyond ten hops, non-cached identity calls, option URL quirks, safe external error mapping, and the distinction between an injected transport ignoring cancellation and a failure preserving cancellation. Direct credential String/GoString calls are observed, and nil-response-body observer counters are explicitly marked `injected_body=false`.

### Injected solver malformed redirect supplement

Three inputs execute six genuine standard Client.Do calls: direct control and actual `solveFlareSolverr`, using byte-identical guarded solver/session/cookie/diagnostics source and pinned HTTP client/response source. The harness observes, rather than imitates, the standard client's malformed-Location path.

Malformed Location closes once before policy, reads no bytes, returns the exact URL error at the control boundary, and maps to solver-unavailable in the actual solver, including when Close fails. The valid no-follow control invokes policy while the response is open; caller cleanup and solver deferred cleanup are recorded separately, with solver-failed for status 302. Full private solver state, error classification/chain, request metadata, callback trace, and body counts are retained. No Close error is invented or retained where frozen Go discards it.

### Saved repository/service/owned leases

The fixture contains 321 fresh observations: repository 75, service 162, and leases 84. Thirty-two production/module anchors match frozen source and Git bytes. Actual isolated SQLite, production account/settings services, SDK identity parsing, and Facade/lifecycle ownership code execute. Failure ports use existing dependency boundaries; no production API or test-only branch was added.

Canonicalization is lossless object-order handling with `json.Number`, not value relaxation. Only wall-clock-generated timestamps are represented as `current-time`, after checking they fall within the observed operation's time range. Fixed timestamps, IDs, revisions, credentials, config content, errors, transient/durable rows, call order, context, and release results remain exact.

Once-owned close behavior is tested under sequential and concurrent calls; concurrent closer panic is represented without assigning it to a scheduling-dependent goroutine. Partial-open error cleanup, error identity/joining, cancellation, idempotent Attempt.Commit, and absence of rate-limit replay are captured. Default SDK idle-close calls are observed without inferring physical connection cleanup.

Three real Go-only risks are retained openly, not repaired or softened in capture:

- Nil caller context may panic inside database/sql while its mutex remains locked. Five bounded owned subprocess probes isolate the poisoned handle, never reuse or close it, and inspect persistence with a separate handle
- Unsupported `%p` formatting of an Account value bypasses its formatter and exposes the synthetic session as byte integers in Go's invalid-format diagnostic. Supported formats are redacted; this defect is not a Rust secrecy expectation
- Owned deferred foreign-key failures genuinely reach Commit, leave transient changes visible on the same connection, prevent the next BeginTx, and are rolled back on Close before durable reopen. Insert and update rows preserve all of these outcomes

### CLI help/parser supplement

Twenty-two actual `cli.Run` invocations execute in disposable Linux/amd64 Go test children. Each uses owned HOME/USERPROFILE/XDG paths, process-wide TSYNC socket/socketpair/connect/exec denial, forbidden dependency canaries, tracked stdin, and exact owned-home content/mode snapshots. The isolation probes confirm EPERM before executing Run; no boundary calls, stdin reads, or home mutations are observed. A separate preliminary root.Find observation never supplies execution results.

Nineteen repository/module source anchors match frozen Go; two parser source hashes match the pinned module cache and the corresponding cached registry archive bytes. Output and status comparisons remain exact. Proxy values named auth or mcp remain values. The surprising distinction between `fanbox --help auth` and `fanbox --help -- auth` is preserved. Go-only parser diagnostic spellings and preliminary Find fields are named explicitly. These are actual Run children, not separately built cmd/pixiv executables, full CLI workflow tests, or portable raw parser equivalence.

## Fresh verification and provenance

Original owner capture/replay/related/race/vet/gofmt logs and hashes were inspected and matched their copied provenance records. All final checks report exit zero. Identity covered both full related packages; saved accounts covered five full related packages, each related/race sweep with 142 top-level passes and three explicit skips (two existing opt-in interoperability tests and the no-mode nil-context helper). The contract itself executes the five owned nil-context children. CLI covered its 22 children plus ten existing relevant top-level root/help/surface/parser/JSON/auth/MCP conflict tests; it does not claim a full CLI-package run. Original failed development/setup attempts are preserved and excluded from passing evidence.

Four independent offline focused replays passed against unchanged frozen test/fixture hashes. Their total is 655 Go fixture observations (309 + 321 + 3 + 22), not a count of implemented Rust contracts or a migration percentage.

The parent-owned unchanged full Rust gate's original log and exit marker were inspected, without running Cargo. It independently parses as 265 Running suites, five doc suites, 271 result summaries, 655 raw passes, zero failures, and the existing 11 ignored tests; the documented meaningful count is 654 after the existing no-environment child scaffold. Optimized release ends successfully in 1m43s. These coincidentally matching Go/Rust totals are different quantities. This gate verifies the restored published Rust baseline only. The final migration validator test/vet log hash and zero exit marker also match their provenance.

### Pristine native source proof

A separate independent read-only local audit verified all five pristine official crate archives against the saved crates.io API and sparse-index checksums, and compared all 1,758 extracted files (30,508,856 bytes) to archive members and file inventories. It checked complete path/type inventories, absence of symlinks/special entries, non-executable extracted files, licenses, and VCS metadata.

Using an independently constructed Git tree traversal, the audit rehashed 25 btls tree objects and 410 BoringSSL tree objects to the recorded official roots. The btls source commit's Gitlink identifies BoringSSL `91a66a59b6c1435120ff83e245d7719411294386`; saved Google Gitiles and google/boringssl mirror metadata agree on root `258beb93b3d6101c44e172b16e105d2547152dac`. The complete mirror tree contains 8,549 entries and 8,140 blobs. All independently counted 1,435 packaged native files match official Git blob SHA1 and byte length with zero mismatches. This verifies the packaged subset and pristine saved-source provenance, not reconstructed patches, native execution, or TLS/HTTP2 equivalence.

### Restored toolchain proof

All nine recorded archive/manifest/installer/version-log hashes match local files. Go and CMake archive digests match the saved official vendor/PyPI release manifests; PowerShell matches its saved official UTF-16 SHA256 manifest; rustup installer and Rust channel manifest match their saved vendor checksum files. The recorded installed versions are Rust/cargo 1.93.0, Go 1.27.1, PowerShell 7.6.6, and CMake 3.31.6. No new downloads, installations, or toolchain launches were needed for this audit.

## Documentation and remaining limits

Final recovery/contracts/remaining-features documentation accurately separates unavailable historical prototype results from fresh evidence, and the pristine source recovery from native behavior. Copied owner manifests are byte-identical to their sealed originals. The ledger retains all 673 entries and changes exactly the owner's 16 SDK plus five CLI IDs to `in_progress`; only status/tests/differences change. No Rust source or verified platform is added.

The review does not establish Rust FANBOX consumers or parity; solver challenge/cache/singleflight/replay; auth/MCP/process/startup workflows beyond the bounded help cases; browser extraction; content/resource/download/media delivery; native TLS/HTTP2/Accept/decompression, trust, physical idle/body-close cancellation, concurrent Read/Close, HTTP1/resumption/HRR, pacing/timeouts, Windows ACL, or other-platform behavior. Historical seven native patches remain unavailable and must be reconstructed separately. The stopped supplemental multiplex/upload probe was neither retried nor counted. RowsAffected failures and arbitrary OS commit failures remain separately unobserved.

## Reviewer evidence artifacts

- Solver supplement replay: 3 observations / 6 actual client executions; exit 0; 0.545 seconds
  - `/tmp/pixiv-fanbox-solver-redirect-review-replay-final.log`
  - SHA256 `6fdb9f617501fa2102e450cee326c221dad5de9ea606a69f26ec6eb3183784a7`
- Identity/protocol replay: 309 observations; exit 0; 0.694 seconds
  - `/tmp/pixiv-fanbox-identity-review-replay.log`
  - SHA256 `44b2a7fc8f0a8e5fefe929287fd0390e997aac2910af4cc6c7627dc2fecd0c68`
- Repository/service/lease replay: 321 observations; exit 0; 0.958 seconds
  - `/tmp/pixiv-fanbox-saved-accounts-review-replay.log`
  - SHA256 `bf5e43d611e03768dd93b04cb2ebf4bd3260d29f643d3e601e20e4de7f66dc1b`
- CLI help/parser replay: 22 owned actual Run children; exit 0; 1.480 seconds
  - `/tmp/pixiv-fanbox-help-routing-review-replay.log`
  - SHA256 `0e130fb7914f3187b5a45e0b4dcb5bd7b93791b3ef440d11a00ea692852f4099`
- Pristine native archive/extraction/Git proof audit: exit 0
  - `/tmp/pixiv-fanbox-native-provenance-review-audit.log`
  - SHA256 `bebc59fe6057a4dea03faf1657eae7e311cee1499c25121a8f556ace99eb8cd8`
- Frozen checkpoint and protected baseline byte audit: exit 0
  - `/tmp/pixiv-fanbox-recovery-review-snapshot-audit.log`
  - SHA256 `01644e7c59a81736b812f4e6694ae7145ed6c4f1707bd25877c0972b497d3a88`
- Saved official toolchain manifest/archive/version provenance audit: exit 0
  - `/tmp/pixiv-fanbox-toolchain-provenance-review-audit-final.log`
  - SHA256 `60d9a511e31adf7968618418996a0f8955fa5ae5a92ebf204e3c7ce0651f20a8`

Reviewer-only unsuccessful attempts remain separately available and do not count as passed checks: `/tmp/pixiv-fanbox-solver-redirect-review-replay.log` (missing /usr/bin/time, Go never launched), and `/tmp/pixiv-fanbox-toolchain-provenance-review-audit.log` (initial UTF-8 decoding assumption for the unchanged official UTF-16 manifest). Both were corrected without changing reviewed inputs.

## Reviewed checkpoint SHA256 values

- `crates/pixiv-app/tests/fixtures/fanbox-saved-accounts.json`: `77dc6f24a44d27220e69987698b1e562382fbece23d895abcd31b13b906de221`
- `crates/pixiv-cli/tests/fixtures/fanbox-help-routing.json`: `5301528abea777cb93ba1c5153714a90dbb79393d253d0c7aeb6793bfcf06b74`
- `crates/pixiv-sdk/tests/fixtures/fanbox-identity-protocol.json`: `cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b`
- `crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json`: `6a8db01180325dc9074984b4b763250d0029118889ed7e5141a8ae1554709651`
- `docs/migration/contracts.md`: `19cca9b8aabc658d84b64042bf5b61c9e01f1259a099f24859eed3a323e06736`
- `docs/migration/fanbox-recovery.md`: `03992e0b262986d4c784210d74d7b8c5be18b184b837f1eab174580189137211`
- `docs/migration/ledger.json`: `a7fcbe1d2ddfa53081c90c813964002a58fa10c04885178ec9dcca8c388b5991`
- `docs/migration/provenance/fanbox-recovery-baseline-gates.json`: `e1fd911c7b5745b891896fbe34f42ffc45f243d13408aa9f981dadf032716843`
- `docs/migration/provenance/fanbox-recovery-help-routing.json`: `48aaaccb76774f103e52c986fbe250ea3e65722e474726fa42bef5edd60530d8`
- `docs/migration/provenance/fanbox-recovery-identity.json`: `cc3212a9e913573c79671fec44caaef2e4f6b38957dd9e61e7c9d332e0e8a7ed`
- `docs/migration/provenance/fanbox-recovery-native-sources.json`: `72bba375d414b1554f001ee2b2abb5e7dec7f79ba8853f9d17d7b594ee4a56a2`
- `docs/migration/provenance/fanbox-recovery-saved-accounts.json`: `acb55ddf57fd402af3b09448d22a0422decebfca82636f7b84dde2466695e6f0`
- `docs/migration/provenance/fanbox-recovery-solver-redirect.txt`: `39ca9522bb666e25a7abd8e77c047f873fbd7b1e19106d72010d0e3a89ca090d`
- `docs/migration/provenance/fanbox-recovery-toolchains.json`: `d0268a570fd0b8778f72d2a2cc791765841b32df184b31ab796e7b3bee00f2d0`
- `docs/migration/remaining-features.md`: `70f46554df19ef0988d326efc3d2b44ab300c0e747e85d03a3f5e140531957aa`
- `internal/cli/migration_fanbox_help_routing_test.go`: `0317b2b85bfc20f1e94a6069a734406ab42152592424332725375c04ebb13142`
- `internal/services/fanbox/account/migration_saved_accounts_test.go`: `4bf13041a18ef7ab168d4546b1407ca1f7f4dbba6450b7a52e452a51866e5606`
- `internal/services/fanbox/migration_saved_accounts_test.go`: `162d96b5bffb3ae01a913e594ad350721654bf74f42e321bbbc6d8009c360d1e`
- `internal/services/fanbox/protocol/migration_solver_redirect_test.go`: `40a96c8b674c9f5f9185f9b9be49616b6ccd108c9375c76531d54de7f4896da2`
- `internal/storage/database/migration_fanbox_saved_accounts_test.go`: `6850bea6daa67e7c359c36807ed1ef7637e01ccbdcdc89dcb6789949a0b4ab8a`
- `sdk/fanbox/migration_identity_protocol_test.go`: `65784f3eaf202bc3799e7ee252c6c75e8e6b3a2ab259ef0b273db9f1e56b7015`

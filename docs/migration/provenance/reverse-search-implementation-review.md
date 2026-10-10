# Reverse-search connected implementation: independent final review

**Decision: approved bounded connected checkpoint.** No unresolved blocking finding remains within the documented owned-synthetic/Linux comparison scope. This is not approval of complete reverse-search parity, a platform promotion, or the whole migration.

Reviewed UTC: `2026-10-10T15:56:55.910330+00:00`  
Published base: `71d798690ac0e80942803641d9d7e5129d0e749b`  
Frozen Go: `4b4426487ef18bed276706daec385e0d0a6979f9`  
Final candidate manifest SHA256: `02f0003e31b0e4c88394e7326cf895d6d25852eea474beb1b4bd0c952c75bf30`

## Binding and preservation

The final seal binds every changed/new tracked or nonignored candidate file: **58 files, 2,426,425 bytes**. Independent hashing found zero drift and the exact changed-file set matches the seal. The complete file map is retained in the accompanying JSON. These two review artifacts are the only self-hash exclusions; later candidate changes require a renewed review.

All **434 Go production/module paths** match both the protected baseline and frozen Go git blobs. All **109 previously published fixtures** (107 JSON and two PEM) match both the protected baseline and published base git blobs. The additive Go large-multipart test and new fixture do not change these protected files. No Go production change, production Rust test attribute, test-only source API, gate-script weakening or removed old assertion was found. The MCP catalog corrections append the newly registered descriptor/name while preserving the old exact-array and frozen-catalog assertions.

The passing gate's 8,933-input manifest is byte-exact for production, tests, fixtures, dependencies and scripts. Exactly five pre-gate inputs changed afterward, all documentation/ledger: `contracts.md`, `ledger.json`, `ledger.md`, `remaining-features.md` and `reverse-search-contracts.md`. Two new baseline/gate provenance files and these review files are document-only additions. All 673 ledger identities and previous verified-platform arrays are preserved. Only MCP `reverse_search` advances pending → in_progress; totals are 612 in_progress, 61 pending and zero verified.

## Reviewed connected behavior

The review read the actual frozen Go source, current application Loader/Facade/Aggregator/provider/assembly modules, SDK native transport extension, record identity, CLI library/root/main routing, MCP executor/stdio and their exact comparison tests. Image routing precedes saved-account acquisition. Source summaries omit private source paths/URLs. Facade preflight precedes source I/O; one immutable snapshot feeds the provider branches; outcomes retain a response beside failure, partial results and cleanup errors. Aggregation preserves provider order, one ASCII2D upload with color/BOVW branches, canonical identities and evidence accumulation. Owned cleanup is shared and once-only in the compared schedules; caller-supplied MCP Searcher ownership remains external, while binary assembly owns its facade.

CLI safe diagnostic codes and MCP safe structured error mapping retain their different Go boundaries. Exact int64-derived record strings/NDJSON remain distinct from Go's structured numeric float projection and wire rounding. Frozen nil/empty/evidence observations are checked only within the representable scopes below; the excluded Go representations are not presented as Rust API equivalence.

Source and SauceNAO requests retain streaming reader boundaries without an aggregate image cap. ASCII2D alone limits image bytes to 10 MiB. Its CSRF token and multipart framing are not subject to an invented total cap. The native raw adapter still buffers the complete body into a Vec, and maps Go reader length zero to SDK unknown length -1. The new ASCII2D HeaderPolicy uses the existing Chrome146 SDK infrastructure, preserves FANBOX's default policy/profile/certificate and lifecycle source, and does not equate an ordinary mock with native wire behavior.

### Exact comparison scope

- Core: **91/99** rows (source29, redirect5, aggregate42, facade13, snapshot2); eight Go nil/context/facade or request/final-URL mutation seams remain unrepresented
- SauceNAO: **277 harness rows**, comprising **268 non-nil runtime rows** and **nine Go nil-receiver representational rows**. Eight use an empty-key Rust client invalid-configuration correspondence; nil idempotent Close is Go-only and does not execute Rust Close
- ASCII2D: **176/192 public rows**; 16 Go nil/zero rows remain unrepresented. Twelve separate private Go solver-cache rows remain Go-only; supplementary Rust solver tests do not establish that private correspondence
- HTTP bridge: **14 tests**; SDK native: **five source/construction/no-network tests**. These are not physical native TLS/H2/upload/proxy evidence
- CLI: **147/148 scoped observations**, with 126 module rows (101 genuine Loader/Facade/Aggregator image calls and 25 validation rows) plus 21/22 distinct root remainder rows. Private Go constructor-replacement row44 is unrepresented. Four real Linux subprocess tests cover the missing-key image route and owned MCP binary startup. Successful provider searches in the shipped binary remain unverified
- Records: two exact NDJSON rows are accepted unchanged by the public artwork/user read-command parsers; this does not establish the full Go generic multi-record consumer schedules
- MCP: **52 scalar raw-wire rows** and **35 eligible direct-result rows** from the original 60-call Go capture; separately scoped two-call cancellation/reuse, three-call out-of-order/reuse, owned-Go-child initialization/three tool frames (two Searcher calls), inherited parent-context and genuine local-source cancellation/reuse regressions. The original Go60-call count is not an independent Rust native capture

## Resolved review findings

1. **Native framing cap:** frozen Go caps image bytes, not aggregate multipart framing. A genuine Rust RED and additive three-case public Go capture precede removal of the invented 10 MiB +64 KiB cap. A 10,485,760-byte image plus 131,072-byte token yields a 10,617,185-byte body. Actual Rust ASCII2D Client → NativeHttpTransport → mocked SDK RawTransport checks exact parts, declared-boundary-only canonicalization, unknown length, context and once-owned cleanup. Physical native upload remains outside this proof
2. **Copy cancellation:** local and ready URL-body loops previously monopolized the same task. Genuine local, buffered-URL and real MCP cancellation REDs precede per-32-KiB cooperative yields. Partial snapshots are removed before providers run, request cancellation permits reuse and the captured final URL bytes-plus-EOF cancellation quirk is preserved. Blocking OS/Read calls remain noninterruptible while in progress
3. **Catalog and accounting:** the first full gate's test-helper acronym Clippy failure and second gate's stale exact MCP catalog failure are preserved. The catalog fix is additive. Failed-test summaries are counted as failures, and target versus extra child pass counts are explicit
4. **SauceNAO documentation:** the final seal corrects the initial all277-runtime wording to the actual 268-runtime plus nine representational boundary, including the Go-only nil Close row

## Terminal gate evidence

The third **unchanged `scripts/check-rust.ps1`** completes with exit0 in **349 seconds**: formatter, strict workspace/all-target Clippy, workspace tests and optimized release (**65 seconds**) pass. Independent log recount finds **315 target sections** (310 Running and five doc-test suites), **919 target passes, zero failures and 11 unchanged ignored**. Seven extra owned-child summaries produce **926 raw reported passes** across 322 summaries. The new reverse test targets total **80 tests**. Existing vendored zip release lifetime warnings remain nonfatal; the script and lint policy are unchanged.

Full gate log SHA256: `791caa4fab95cb47e3292897a273b7b7c6b025ea5315da73c79226c621b1282b`  
Pre-gate input map SHA256: `e233551c226f64071f04cc3bcd686b57d2571aaa7bafdb5600962dd0e7c76496`  
Gate script SHA256: `a5cc6a8c5265061d8e25fa67f364b5f9253ba93fcbcd904e5ca768c3e77fb7f6`

Focused related Go succeeds with **767 named passes** (46 top-level +721 nested), six packages, no failures/skips. Additive large-multipart Go separately succeeds with **four named passes** (one top-level +three nested), one package, no failures/skips. Vet and empty gofmt succeed. The final document validator succeeds in **1.020 seconds**, and diff check is empty/successful. All cited log hashes were independently verified; the reviewer did not rerun Cargo, Go or repository tests.

The two earlier full-script exits1 remain recorded: strict Clippy stopped the first before tests/release; the second reported **one test failure**, 732 raw passes and ten ignored, with release not reached. Both broader Go attempts also remain **failed**, not folded into focused success:

- First: 2,533 named passes, four package passes, 22 skips; missing owned SQLite shell plus the existing MCP UserWorks overlapping-schema diagnostic failure
- Corrected environment: 2,616 named passes, five package passes, 22 skips; MCP BookmarkReads selected `user_id -1` before frozen `page 0`

No optional normalization or frozen expectation change was used. The gate provenance retains exact failed logs, ten initial test-first REDs (MCP classified as dependency-blocked rather than isolated missing-API proof), and four genuine runtime REDs.

## Approval limits and obligations

Normal pending EOF is a known behavior difference: Go suppresses a pending result after readErr is visible; Rust drains/emits. Sequential owned-child frames do not resolve it. Generic NDJSON consumer/error/cleanup scheduling, arbitrary future-drop or concurrent cleanup, Go private cache/nil representations, arbitrary blocking readers, filesystem races/ACLs/faults and arbitrary JSON/HTML/numeric/URL/error-metadata grammar remain open.

Native ASCII2D physical upload/cancellation/TLS/H2/proxy/solver lifecycle, public SDK Context/client ownership/idle-close/pacing, ordinary reqwest's extra `Accept: */*`, relay HTTP/2 capability, signal restoration, repeated Windows cleanup and all native OS/arch runtime/ABI evidence remain incomplete. No real user credential/account/media, third-party upload, host association or security-setting change was used. The denied supplemental native multiplex/unfinished-HEAD/upload probe was not retried and is not evidence. Standalone ugoira metadata and anonymous dictionary retain their earlier bounded gaps; automatic updater and signed install/distribution/license packaging remain unimplemented.

This decision applies to the sealed connected checkpoint and its declared comparisons. It does not mark a feature/platform verified or authorize publication. Production/test/fixture/dependency/script edits invalidate this gate-bound decision; changed documentation claims also require renewed hash-bound review.

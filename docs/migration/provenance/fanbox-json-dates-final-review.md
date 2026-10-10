# Independent FANBOX JSON bytes / depth / date review

Reviewed 2026-10-10 UTC against published base `3dee232bf2571abbb9835fcd5ba9d5042aa0ec03` and frozen Go `4b4426487ef18bed276706daec385e0d0a6979f9`.

## Result

No must-fix defect found in the reviewed production changes or the 47-row identity and 79-row solver public replay additions. This is a source and existing-evidence review, not a fresh runtime gate. The separately sealed two-case ordinary owned HTTP/1 control streaming supplement and its three final Rust tests are now included in this source review. The final test-only constructor-cleanup refinement is reviewed below. Fresh final formatter/strict Clippy/focused16+15, the entire unchanged full Rust script, related Go/vet/gofmt/validator, and post-result-document validation now have genuine successful terminal records. Independent log/count/hash checks agree. The earlier interrupted exit130 run remains inconclusive and is not counted as completion. Final approval remains bounded to this checkpoint and its preserved regressions; it is not general FANBOX/platform parity or permission for new scenarios.

Only this report and `/tmp/pixiv-fanbox-json-dates-final-review-sourcehash.json` were written. No repository file was edited. No Cargo, Go, network request, native executable, or native probe was run. The denied supplemental multiplex/unfinished HEAD/upload probe was neither reconstructed nor retried.

## Codec and identity

- `codec.rs:159-180` extracts the exact previous per-invalid-byte UTF-8 loop into `go_utf8`. Mechanically removing the new function boundary and call recreates the base `codec.rs` byte-for-byte. Consequently Pixiv's existing `normalize_json` behavior and internal error contract are unchanged, rather than merely similar.
- `identity.rs:434-449` applies that bytewise conversion before HTML tokenization. It does not run a JSON quote/depth pass on raw HTML. This preserves the number of replacements for truncated prefixes before metadata extraction.
- `identity.rs:161-172` normalizes extracted, entity-decoded metadata before typed/RawValue member decoding and maps internal normalization failures to the established safe metadata-JSON error. It retains complete-input validation through `serde_json::from_str` and the ordered member merge.
- Literal NUL detection is scoped to the actual emitted meta tag's consumed source span, after comment/doctype/structural boundaries (`:372-394`). The source reader records value ranges, not the whole tag. Selecting the last exact `content` attribute matches the existing ordered duplicate-attribute extraction (`:395-415`), so an overwritten earlier content NUL or an unrelated attribute NUL is not by itself a rejection. Quoted `>` and `<meta` text remain inside their source value; the source candidate must finish at the actual span end. Entity-encoded NUL/surrogates are not mistaken for literal NULs. This agrees with x/net/html `TagAttr` preserving raw attribute NULs while unescaping entities; html5ever's replacement of literal NUL is the difference being repaired.
- Read/close ordering is unchanged (`identity.rs:26-64`): all bytes are read, then the owned body is closed before parse. Read failure remains first when joined with close failure; close failure alone precedes JSON/depth errors. The five public precedence rows exercise those safe SDK results.
- Unicode escape repair does not modify numeric lexemes, null handling, field-fold aliases, or duplicate member ordering. The existing global normalizer counts all enclosing and ignored containers and ignores quoted/escaped delimiters, preventing acceptance through overwritten/ignored RawValue storage.

## First-value framing, resource ownership, and cancellation

`fanbox/json.rs` is a private lexical boundary/depth finder; `serde_json::RawValue` remains the real syntax validator. It is not substituted for a JSON grammar parser. Quoted escapes do not terminate the frame; a top-level object/array terminates at its first structural close. The normalizer receives only that frame, so unparsed malformed, deeply nested, or invalid-UTF8 trailing input cannot introduce a semantic error after a valid solution object. An incomplete first object is rescanned from the original raw byte buffer after each chunk; provisional replacement of an incomplete UTF-8 prefix or surrogate escape is not persisted.

At the 10001st opener, `first_value_end` returns the prefix through that opener and the existing normalizer rejects it immediately. It need not await the closing suffix or response EOF. Syntax errors within the frame remain subject to RawValue validation and the public malformed response category. Mismatched bracket kinds cannot become successful JSON merely because the lexical depth reaches zero.

The only solver loop change is the normalization/framing step before RawValue deserialization (`solver.rs:331-351`). Request construction, chunk acquisition, cancellation select, and typed solution validation are retained. Early `?` failures drop the owned response through Rust scope unwinding; successful decode explicitly drops it before typed validation. No newly exposed physical Close-error identity is claimed. `Inner::run` still caches only an `Ok(State)` for the current active call with surviving waiters, emits solver completion only for success, and suppresses failure diagnostics once the shared context is canceled. Existing waiter cancellation and cache replay tests remain intact.

Nonblocking source-derived limitation: scalar-root streaming timing is not generally Go Decode timing. For example, serde RawValue can finish a complete `null`/string at a chunk boundary before Go's scanner obtains a delimiter or EOF. That timing difference exists in the base streaming loop too. The new lexical helper also leaves top-level number framing to serde. All useful successful solver documents are objects; the 79 captured rows establish public root-type classification and object-first trailing-input behavior, not all scalar completion timing. Do not upgrade this checkpoint to full streaming `encoding/json` parity.

## Private expiry grammar

`expiry_date.rs` follows the exact two layouts actually called by frozen `parseSolverExpiry`: RFC3339 and `http.TimeFormat`; it does not call a broader HTTP-date parser.

Source review against official Go 1.27.1 `time/format.go` and `format_rfc3339.go` found:

- Four-digit year, two-digit month/day/minute/second, one- or two-digit hour, uppercase RFC3339 `T` and `Z`, exact numeric offset punctuation, and rejection of extra bytes align with Go's optimized parser plus layout fallback
- Offset hour 24 and minute 60 are accepted; larger values are rejected, matching Go's inclusive fallback bounds
- Dot/comma fractions require digits, consume the maximal digit run, and truncate rather than round to nine digits; accumulation is bounded below one billion nanoseconds
- Clock second 60 is rejected before Chrono can construct its leap-second representation, on both layouts
- HTTP dates require a syntactically valid ASCII case-insensitive short weekday and comma, a short month, runs of ASCII spaces, a four-digit year, and the literal case-sensitive GMT suffix; weekday/calendar consistency is intentionally ignored
- Gregorian date validity comes from Chrono's proleptic date constructor over the same 0000..9999 year domain; timestamp subtraction is safely within i64 for that bounded domain and the offset bounds

The 28 public date rows directly observe acceptance/rejection and future cache/header retention. Exact parsed fraction nanoseconds and UTC offset timestamps are source-derived, not exposed by that workflow. The fixture explicitly states this limitation. Numeric expiry paths and int64 wrapping are unchanged. One-/two-digit hour, token case, space-run, lowercase t/z, and other lexical reasoning above is source review, not newly captured evidence.

## Tests-first and provenance

Existing raw logs show the required genuine public RED and subsequent GREEN sequence:

- Solver RED: 7 passed / 5 failed
- Identity RED: 12 passed / 4 failed
- Identity after normalizer, before NUL correction: 15 passed / 1 failed, specifically the captured raw-NUL row
- First combined GREEN: identity 16 passed / 0 failed; solver 12 passed / 0 failed

These are top-level Rust test counts. They are not the 47/79 fixture-row counts. Each family replay compares actual SDK DTO/error/source, requests, headers/cache, body byte/close ownership where observable, and diagnostics against genuine public Go observations. Go-only concrete error identity, call topology, and projections remain excluded as documented. Existing test diff review found added public replays and raw-byte support, without changing original expected rows/assertions or adding test-only production APIs. New modules are private production helpers.

Independent read-only verification found no mismatches in:

- Both fixture family inventories and all canonical raw-hex inputs; all identity body lengths and SHA256 values
- Both fixture protected/reference/stdlib SHA256 maps
- All 434 current Go production/module paths against the full source guard
- All ten capture/replay/race/vet/gofmt log byte counts and hashes against the two manifests
- Native family, Cargo manifests/lockfile, and `scripts/check-rust.ps1` against the published base (empty diffs)

The identity evidence prose correction changed only top-level evidence wording; its manifest preserves the unchanged raw case-array hash and bytes. Date/depth accounting fields are input/source descriptors rather than synthesized expected SDK results. Protected originals and their published row inventories remain preserved.

## Final held-stream and documentation review (08:24 UTC update)

The new sealed fixture has exactly two independent rows, a 99,131-byte whole-fixture SHA256 pin, canonical prefix/remainder descriptors with individual lengths and SHA256, and a test-only 128 KiB bound. Independent checks match its owned source/fixture hashes and every Go gate log hash. Raw action records show capture 2, replay3 6, and race 2 nested passes, with no failure/skip. Both public observations record return before release and before handler completion. The first reaches global depth10001 and returns the existing safe malformed result without native replay/completion; the second returns the actual identity success and replay from the complete first object before a held deep second value/invalid-UTF8 tail.

Go handler and cleanup source review confirms the withheld remainder/EOF gate is independent of request cancellation. Flush errors and missing completion are failures, not persisted expectations. The single five-second timer is an outer failure guard. The original private solver124 forced-UTC failure log remains hash-verified separately; the manifest records the unchanged-fixture successful original Local/JST replay rather than normalizing its timezone-sensitive expectations.

Final Rust additions (`fanbox_solver.rs:520-733`, `fanbox_solver_support/mod.rs:508-648`) map exactly those two sealed rows through the real public client. The dedicated owned HTTP/1.1 POST helper records the unchanged anonymous request fields, emits a chunked response containing only the canonical prefix, flushes successfully before signaling it, and waits exclusively on its explicit release. Disconnect cannot release the remainder or EOF. Completion state is sampled before release. On ordinary failure paths, release/cancel and bounded caller plus handler draining occur before assertions; timeout aborts are awaited and remain test failures. The Notify gate safely retains a permit if release races ahead of the handler wait. The completion record is gated by the real flushed prefix signal and successful public caller termination; it cannot be fabricated from a timeout. Native DTO/error/source, replay headers/body ownership, exact caller context and diagnostic comparisons remain against the genuine fixture. The shared `control_request` extraction preserves the previous test helper's recorded fields for old rows and adds only protocol validation for the new stream helper.

The existing `/tmp/pixiv-fanbox-json-dates-final-focused.log` now records identity16 and solver15 passes, including the new integrity test and both public stream tests. These are read observations of the parent's run; this reviewer did not execute Cargo. All production source hashes remain unchanged from the initial source review. Only the two intended Rust test files changed before this update.

Documentation scope is consistent after one wording correction: identity has per-body length/SHA256; solver79 uses canonical response hex plus whole-fixture pin. The three migration documents explicitly retain scalar timing and unrestricted grammar, exact private timestamp exclusions, saved app/CLI/MCP/browser/resource/native media debts, and no verified-platform claim. The `sdk/fanbox:Client` ledger correction adds the already-published public implementation and SDK test mapping missing from its stale foundation-only view. All 673 entry IDs, historical differences, statuses and platform fields remain preserved. The other bounded FANBOX additions add sources/tests/differences without deleting old evidence. Provisional documents do not turn the running full gate into a pass. Final result-only edits should be checked against actual terminal evidence when they are ready.

## Final test-only cleanup refinement and interrupted gate (08:28 UTC)

One further test-only edit landed after the first full script started. `fanbox_solver.rs:19-37` factors construction into `client_result` returning the existing `Client::open_with` result; the old `client` wrapper still calls `.unwrap()` with identical credentials, options and old-call behavior. The new stream test constructs `Native` before starting `StreamControl`, then handles a constructor error by awaiting the helper's bounded release/finish cleanup before asserting and reporting setup failure (`:642-651`). If no request was created, the existing five-second handler guard may abort and await the task blocked in accept; this remains a failure rather than a synthesized public observation. Success-path assertions and captured outcomes are unchanged. This refinement introduces no production/public-test API, fixture or expected-result change.

Independent current-hash comparison confirms the only change to the previously reviewed 51-file snapshot is `fanbox_solver.rs`, now SHA256 `7175950faac8882b7611af430472c5699eaecb6517db63f5d963767a965a6606`. Support remains `39a5889ed76edfd82b03e1defb4485fea907624185b1e0b2478dbda373f17948`; production and sealed inputs/expectations remain unchanged. Source approval applies to this reviewed final delta too.

The parent owned-interrupted the in-flight full script. Its preserved record is `/tmp/pixiv-fanbox-json-dates-interrupted-full-gates.log`, with exit130 observed at `2026-10-10T08:27:05Z`, start-marker `2026-10-10T08:24:15Z` and finish-marker written at `2026-10-10T08:27:33Z`; the original 8,702-path source snapshot remains separate. Although the raw log contains many passing suites, it is an inconclusive interrupted run and is not a final full-script pass. Likewise the prior focused16/15 pass belongs to the pre-refinement test snapshot. A fresh final focused run and complete unchanged full-script rerun are required before any terminal completion claim. At that point the parent stopped worker editing and began the serialized reruns, now completed as recorded below. This reviewer has not run Cargo or any native probe.

## Terminal evidence and bounded final approval (08:39 UTC)

Independent read-only verification of `docs/migration/provenance/fanbox-json-dates-final-gates.json` and its raw artifacts found no hash/count mismatch. Eighteen referenced artifact descriptors were checked directly. The final full-script log SHA256 is `85083ed64f2b77ad9ca6b248665a7903a02b3bdb1fd987b135e6a50cf4db577e`; its exit marker is0. The unchanged required script runs formatter, strict workspace/all-target Clippy, all workspace tests, and optimized release with locked dependencies. Native source/helper, Cargo manifests/lockfile and the script still equal the published base.

Actual terminal records:

- Fresh focused log: identity16 / solver15 pass, zero failures/ignored, on the final constructor-cleanup test bytes; formatter/strict lint terminal success
- Full unchanged script: exit0, recorded294 seconds, optimized release48.78 seconds
- Raw full log: 282 successful summaries, 707 passes, zero failures and11 existing ignored; 270 Running suites and5 doc suites leave7 child summaries
- Meaningful full count:705, subtracting only the two unchanged no-mode terminal/native child scaffolds; no assertion, expected row or ignored case was removed
- Related Go raw action records:39 top-level /719 nested named passes, zero failure/skip, with three package passes; related vet, empty gofmt and migration validator succeed
- Post-result-document check: exit0; raw validator records4 top-level /2 nested passes with no failure/skip, plus declared vet/empty new-Go-test gofmt/tracked diff-check success

The six approved native children all execute inside the unchanged existing full gate. There are exactly six current witness directories and six persisted `rust-observation.json` files, matching the unchanged CASES array: headers_sequential_idle, response_header_stall, caller_deadline, untrusted_certificate, invalid_hostname and expired_certificate. The full log contains their six successful child summaries plus the unchanged no-mode native scaffold and successful parent comparison. Witness contents retain three sequential responses/two idle-liveness checks, header stall and caller deadline outcomes, and the expected UnknownAuthority/HostnameMismatch/Expired certificate classes. This is review of existing approved-gate evidence; this reviewer did not execute or expand native scenarios. No denied multiplex/unfinished HEAD/upload reconstruction or retry is present.

Current independent hashing of all8,702 pre-run snapshot paths finds no missing file and no source/test/fixture/dependency changes. Only `contracts.md` and `remaining-features.md` differ from the pre-run snapshot after the terminal gate. Their current bytes retain the complete published-base documents as exact prefixes; git diff shows18 and8 added lines, zero deletions. The parent also made spacing-only readability replacements within this checkpoint's newly added sections (for example, identity47 to identity 47 and hour24/minute60 to hour 24/minute 60), explaining why the provisional pre-append prefix no longer reproduces its snapshot hash. All historical text preceding the new headings is byte-preserved. The additions accurately distinguish first focused/source approval, inconclusive interrupted run, final successful rerun, Go timezone replay limitation, and bounded future scope. Ledger status/platform/history fields remain unchanged.

The interrupted finish-marker precision is now explicit: exit130 was observed at08:27:05;08:27:33 is the artifact finish-marker write time, not an exact process duration. Its raw log and original snapshot remain separately retained. No earlier or interrupted suite count substitutes for the final rerun.

No must-fix finding remains. Final source/tests/evidence and result documentation are approved for the bounded identity47 + solver79 + stream2 checkpoint. Scalar-root streaming timing, unrestricted JSON/HTML/URL/date grammar, exact private expiry timestamp fields, native media integration, saved app/CLI/MCP/resource/download/browser work, and unverified platforms remain excluded or open exactly as documented.

The hash manifest pins the precise files and raw evidence reviewed. If any production helper or replay assertion changes after this snapshot, re-review the changed portion before treating this report as current.

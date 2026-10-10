# Connected updater implementation scope

This checkpoint activates a bounded Rust updater path against frozen Go `4b4426487ef18bed276706daec385e0d0a6979f9`. Its published base is `9c30c587def68f6291cb666d2d38ded618f7648f`. The [original contracts](updater-contracts.md) remain the authority. It is not whole migration, native-platform, distribution, or every-root-row verification.

## Connected production path

`pixiv update` uses the real startup/config boundary, ordinary HTTP factory, file release cache, install-source detector, release/source selection, coordinator and signed installer. The compile-time `PIXIV_BUILD_VERSION` is the build identity, defaulting to exactly `dev`; runtime environment changes cannot replace it. Explicit output keeps the frozen text/indented JSON/null-version and Changed-JSON error-envelope policy. The real development path constructs the configured factory before rejecting self-update; proxy constructor errors precede that rejection.

The app's normal ports preserve caller Context, source/channel strategy, canonical unlimited-decimal v-SemVer, explicit versus automatic deadline/cache policy, five public release sources, URL transformations, ordered fallback and first-winner cancellation. Native source detection uses executable/receipt paths; Rust does not contain Go `debug.ReadBuildInfo`, so automatic Go-install provenance remains a disclosed gap. The ordinary HTTP client adapter adds Go redirect/error/body ownership semantics to the existing low-level transport. Its native URL/HTTP/TLS/proxy grammar is not asserted equivalent merely because synthetic adapters match.

Production assembly uses the frozen public Ed25519 key and exact all-three release URL policy. Manifest signature and raw checksums.txt hash precede archive download/checksum. Whole-archive paths/types/duplicate binary validation precedes candidate creation. Candidate copy/sync/chmod/close, owned version preflight, target-sibling staging/replacement and preserve-on-danger cleanup remain separate effects. No release candidate is executed by the synthetic signed replay. The concrete checker tests execute only the reproducibly built visible owned Go helper. The no-shell command runner covers exact argv, concurrent unlimited output drainage, copy/exit precedence, and cancellation kill/wait.

The archive adapter retains mature TAR/ZIP decompression while correcting demonstrated Go reader differences: duplicate/repeated PAX/GNU metadata, strict PAX records, sparse 0.x/1.0 and old-GNU maps, historical header numeric/path rules, original ZIP duplicate entries, Unix/DOS type mapping, and complete central-header enumeration/count validation. This is no claim about arbitrary historical headers or every ZIP64/wrapped-count combination. ZIP paths are inspected, never extracted as arbitrary filesystem destinations.

## Automatic hook and ownership

Eligible successful ordinary commands invoke the warning-only checker once. Failure, development build, help Changed, groups, MCP/update/hidden exclusions and actual resolved bundle import/export policy retain their distinctions. Configuration mutations feed the subsequently loaded runtime.

The root hook follows the frozen owner order rather than broadcasting cleanup: detail/search/download per-call SDK leases close before the hook; FANBOX per-call lease closes before it; registered database closes afterward; reverse-search's registered closer runs afterward. Auth owner entrypoints retain the actual database across the awaited callback. Token/check per-call transport release remains before it. Existing entrypoints delegate with no callback, preserving their API. Native idle-close effects and auth database Close error reporting remain wider lifecycle debts; an Arc drop does not prove Go Close error joining.

Actual-root `start_update_diagnostics` currently loads/validates the real runtime but does not implement debug started/completed/failed events or diagnostic writer-fault joining. The corresponding original root observations are retained and are not claimed covered by injectable library ports. Root parser/global-debug, all 158 behavioral root rows, formal-build native network behavior and dynamic diagnostic ownership remain next compatibility work.

## Evidence boundaries

The original 1,148 sealed Go observations are unchanged: coordinator305 (296 behavioral/nine Go representations), release/cache160 (153 public/six configured private/one nil receiver), source200 (167/33), Install199, detector34, physical cache13, Windows source-driven26, concrete owned Linux preflight41, and root170 (158/12). Counts are evidence inventory, not completion percentages.

New Go-first evidence adds 58 observations before the corresponding Rust changes: release URL/timestamp13, absolute-zero cache instant8, standard HTTP nil/redirect7, archive metadata13, PAX sparse7, old-GNU sparse2, historical headers4, ZIP creator modes3 and EOCD count1. Source/probe/results/provenance are retained under crate tests/support and the implementation evidence directory. The original baseline Go/module434 paths, prior fixtures110 paths and seven updater fixtures remain hash-protected.

Focused Rust evidence includes coordinator3 tests, source7, release7, HTTP5, signed10, cache1, owned preflight1, assembly8; CLI library10 replays exactly83 explicit/parser and26 automatic rows. Actual binary6 tests compare18 frozen parser/help observations plus real production dev/version/proxy/config effects. Auth5 and download2 tests exercise actual synthetic database ownership across an awaited callback, and distinguish per-call close from registered cleanup. Neither these process checks nor library109 replay establish all root158 observations.

The final unchanged full script, Go/vet/gofmt/ledger validation, input preservation and independent reviews are recorded separately in `provenance/updater-implementation-final-validation.json` when terminal. A focused green is not a replacement for those gates.

## Failures and corrections retained

Real semantic REDs included cache ParseError cause layout, UTF-8 percent-escape panic, nanosecond cache-byte spelling, duplicate ZIP binary loss, PAX duplicate unsafe path, sparse hole expansion/old-GNU alignment, historical numeric headers, ZIP creator modes and underreported central count. They were corrected without changing published oracle values. Nil-response comparison maps only the one concrete Go transport type and compares the complete remaining diagnostic/cause flags.

Owned harness mistakes are separate: mixed-duration parsing, private test prompt import, invalid auth command name, record missing its required URL, executable-copy ETXTBSY race, and archive preparation consuming a synthetic deadline before invocation. Initial missing APIs and Rust compilation/lint errors are not semantic behavior REDs.

A new unpublished auth transport Drop-after-hook witness incorrectly treated per-call clients as registered root resources. Frozen account methods actually defer CloseIdleConnections before returning. That premise and unnecessary temporary retention were corrected; their earlier RED/green logs remain invalid-premise history, not a production bug or a weakened published expectation. The same review prevented inappropriate data/download/FANBOX transport retention. Full Go native idle-close behavior remains unverified.

## Remaining required work and safety boundary

No feature, expectation or ignored-test requirement is removed. The ledger entry advances only to in_progress, with no verified platform. All prior SDK Context/client ownership/idle-close, Accept: */*, relay HTTP/2, pacing, signal restoration, destructive repeated Windows URL-association cleanup, parser/raw-byte/IO/cache/concurrent/future-drop and native OS/architecture debts remain explicit. In particular successful source selection drops pending Rust loser futures; delayed Go goroutine/body-close timing is not broadly equivalent.

Windows replacement tests are source-driven normal API mocks on owned Unix files, not Windows ABI/ACL/runtime evidence. Native Windows loader/procedure availability, Darwin/Windows execution, physical transport/source/syscall schedules and six-platform builds remain unverified. Real registry/browser/association, account authentication, external copyrighted media, third-party image uploads and real release installation are not exercised. The previously denied supplemental multiplex/unfinished-HEAD/upload probe is not retried.

Official crates.io tar0.4.46/filetime0.2.29 are checksum-verified new dependencies; ring0.17.14, flate21.1.10 and zip2.4.2 reuse locked known sources. The registry API403 was an ordinary HTTP response; official sparse index/Cargo fetch succeeded without a policy-denial workaround. Complete release packaging, native dependency licenses, signed publication/build metadata, install.sh/install.cmd's distinct checksums.txt-only trust, handler/PATH installation and distribution remain distinct required work after this checkpoint.

## Current gate recovery

The first unchanged full-script attempt passed formatter and strict workspace Clippy, then failed during workspace-test compilation with actual ENOSPC and follow-on linker failures. No failing test assertion was reached. The failed log, exit and timestamps are retained. Only canonical owned `target/debug` was removed under the existing rebuildable-output authorization; source, release artifacts and logs were preserved. The clean retry runs the entire unchanged script without skipped assertions or altered profiles. All8,486 sealed non-documentation gate inputs remained byte-identical across this recovery.

Supplemental captured Go text formatting is reported separately from actual repository Go owners. The original13-row archive `.go.txt` remains the exact compact source compiled and vetted for its historical Go-first capture; it is not formatter-identical and no formatter-pass claim is made for those bytes. The other eight supplemental captured sources are gofmt-identical. Actual updater/CLI Go owner files have an empty gofmt listing. Compressed retained logs include original byte counts and SHA256 hashes, so successful focused evidence, semantic REDs, harness mistakes, invalid auth-order premise and ENOSPC can be inspected independently.

Independent review additionally found a concrete cache zero-instant defect outside the earlier160+13 rows: the candidate's zero-time predicate ignored nanoseconds and unnecessarily checked local year. Exact `time.Time.IsZero` semantics require the absolute zero second plus zero nanoseconds, independent of offset representation. Eight additive actual frozen-Go public Check observations confirmed the issue. The new replay failed against the original guard, then all seven release tests passed after the minimal absolute-second plus zero-nanosecond predicate. The original finding and intermediate gate remain preserved; the corrected complete gate is required before publication.

## Final corrected validation

The unchanged full script passed at 18:24:58 UTC after the zero-instant correction: formatter, strict workspace/all-target Clippy, 984 workspace passes plus seven subprocess children (six FANBOX native and one handoff), 991 raw passes, zero failures and 11 existing ignored; release build 58.30 seconds. All 8,489 sealed non-documentation inputs were unchanged. Fresh focused Go 7+710, related Go 47+732, ledger 5+2 and vet/actual-owner gofmt passed; additive zero-instant Go replay/vet also passed. Six source-hashed independent bounded reviews are retained. The final validation JSON records exact logs, recovery history, scope and unfinished compatibility/native/distribution work.

# Independent final review: connected FANBOX saved-session reads

## Decision

Approved as a bounded connected-read implementation checkpoint against published base `f56f27e178dbbbcc1020fabb0a7f60745aae8dfd` and frozen Go `4b4426487ef18bed276706daec385e0d0a6979f9`. No unresolved blocker remains for the documented candidate scope. This does not approve whole FANBOX parity, native compressed-wire or platform completion, or replacement of the Go binary.

The review read the actual source and observed terminal records. It did not execute Cargo, Go, native peers, network requests, or any denied supplemental probe. The two final review artifacts were written only after explicit parent assignment; implementation and expectations were not edited by the reviewer.

## Source fidelity and public boundaries

- Read AGENTS, migration strategy and saved-session read contracts, and compared actual frozen Go SDK/endpoint/protocol/listing/resource/account sources plus the pinned fhttp native dispatch/decoder source
- Reviewed SDK content/DTO/cursor/reference/resource implementations; all API families use real Session requests and typed mapping. First-root JSON decoding, account-bound continuation rejection, resource identity generation, cache reuse, and fresh image-before-file reopening retain the source contracts
- Reviewed API/media host and redirect validation, first-target cookie scope, conditional request fields, solver challenge/replay/cache invalidation, and safe error projection. API cookies remain limited to the approved www/api targets and media cookies to first-target downloads; redirected credentials are removed
- Reviewed raw bytes with simultaneous failure, exact EOF versus wrapped failure, repeated body-close forwarding, and native-only decoder wiring. Ordinary injected transports remain undecoded. HEAD/nonempty Range omit automatic Accept-Encoding; HTTP2 EOS/HEAD bypass and HTTP1 header deletion follow the pinned source. No arbitrary body-size cap is introduced
- Reviewed saved SQLite/config lazy account selection, FANBOX-only facade/lease ownership and proxy precedence, six CLI leaves, eleven MCP tools/schema, pagination/partial discard, stdio cancellation/reuse/drainage, and actual main dispatch. Recognized FANBOX routes are intercepted before Pixiv execution and compose only FANBOX services; auth/download remain explicitly pending
- Reviewed Linux binary owned-child tests. HOME/USERPROFILE/TMP/XDG/PATH/current directory are isolated, child lifetimes are bounded and kill/reap owned, Linux automatic-handler and pending-update paths are inert, and all exercised binary reads stop before native open at help/config/argument/missing-account boundaries. Real FANBOX stdio schema/error/EOF is exercised

## Findings resolved during review

1. Native default encoding was initially inserted on HEAD/range requests. The corrected source omits it for HEAD/nonempty Range and keeps caller-supplied gzip detection. This is a source-level correction, not a new native-wire probe
2. An active MCP future initially held an unguarded lease, while fatal stdio I/O could drop pending work. OwnedLease now releases on drop. Fatal read/parse failures drain; fatal write failure cancels owned requests then drains. Late frames are suppressed. Three new genuine Go-backed failure schedules and a separately labelled Rust lease-drop test exercise the correction
3. CLI driver/path diagnostic exceptions initially skipped entire stderr/error fields. The final harness asserts exact Rust and Go payloads, error arity/order and invariant JSON envelope, then narrowly projects only the documented physical diagnostic message

The source-only formatter/Clippy adjustments retain single-write/nil-short-write behavior, private list-output grouping, and test-only type aliases. No public test API, test-only production flag or source test body was added. The FANBOX spool writer has its own short-write commit path; existing Pixiv commit behavior remains unchanged.

## Terminal verification and exact count labels

The unchanged `scripts/check-rust.ps1` terminal log and exit file confirm exit 0: formatter, strict workspace/all-target Clippy, workspace tests, and release build. Release compilation reports 1m03s. All gate manifest log byte counts and SHA-256 values were independently checked.

- Rust: 747 top-level passes plus six genuine native child passes, giving 753 raw result-line passes; zero failures and 11 unchanged ignored tests
- Three top-level no-mode helper scaffolds are included in 747: native child, terminal-prompt child and owned saved-FANBOX child. Thus there are 744 substantive top-level passes plus six genuine native child passes, 750 substantive passes including those children
- The raw log has 294 result blocks. These counts are test-execution labels, not feature completion percentages
- The six native scenarios remain headers/sequential-idle, response-header stall, caller deadline, untrusted certificate, expired certificate and invalid hostname. No supplemental multiplex/unfinished HEAD/upload action was retried
- Fresh relevant Go: 23 top-level passes, 1608 nested passes, zero failures, eleven no-test MCP packages and two no-mode CLI child skips. Relevant vet and empty gofmt output are verified, and the ledger validator passed after ledger edits
- The four media-solver and three fatal-stdio supplemental oracles have genuine Go capture, three replays, race, vet and empty gofmt evidence; their final producer/fixture/log hashes were checked

The focused and final workspace records retain SDK content416, resource243 plus standard-injected31 and new media-solver4, private decoder42/compiled resource safety43/four same-read failure checks/two Rust-only future-drop tests, selected saved-account110, CLI167 plus five root-stop comparisons/help22, MCP126/159calls/11schemas/three original stdio schedules, and seven actual binary tests. These are separate comparison surfaces, not additive feature counts.

## Preservation and final input seal

- Independently compared all 434 frozen Go production/module paths to actual frozen commit blobs: zero drift
- Independently compared all 91 published baseline fixture paths to actual base commit blobs: zero drift
- All 673 ledger IDs and references are retained. Exactly 100 entries change, with 96 pending-to-in-progress transitions; no verified-platform field changes or promotion to verified
- Only official registry zstd 0.13.3, zstd-safe 7.3.0 and zstd-sys 2.1.1+zstd.1.5.7 are newly locked. Archive bytes match the official lock checksums; existing non-owner lock entries and third-party production bytes remain exact
- The 8763-path full-gate snapshot has zero executable input drift. Its only changed existing path is the final narrative `docs/migration/fanbox-connected-reads.md`, updated after the terminal result. The newly added gate provenance and final narrative were separately reviewed
- The companion sourcehash JSON seals final candidate files and excludes these two review artifacts to avoid circular self-hashing

## Explicit remaining scope

Parser-layer fatal-stdio diagnostic wording, physical SQLite/OS diagnostic payloads and source buffer-size differences remain explicitly identified. Synchronous lease Drop is not asynchronous RawBody.Close. Arbitrary mid-body abort, duplicate IDs and unrestricted disconnect/concurrency remain unproved. Private decoder replay and the existing six native scenarios do not prove compressed native wire, native HEAD/Range, resumption/HRR, physical concurrent Read/Close, successful authenticated binary HTTPS, native OS/bootstrap or non-Linux runtime. FANBOX auth/browser extraction, download/save/replay, the wider saved-account workflow and pre-existing relay/Accept/lifecycle/pacing/signal/Windows cleanup/distribution debts remain open.

These limits are retained in the candidate narrative, provenance and in-progress ledger rather than hidden by fixture changes or empty success. Final review is therefore approval of this candidate checkpoint only.

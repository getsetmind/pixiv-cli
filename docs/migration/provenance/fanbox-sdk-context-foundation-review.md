# SDK context foundation review

## Decision

APPROVED for the bounded shared Rust context/diagnostics foundation after correction of the awaited-timer regression and a successful complete final Rust gate. No unresolved blocker remains within this slice. The interrupted first full gate is not a pass.

This review is limited to the shared Rust context/diagnostics foundation. It does not approve FANBOX SDK forwarding, FANBOX lifecycle integration, native transport parity, another platform, or final migration replacement.

## Scope and constraints

- Base: `b3f01c9944d3317d10d48543f529af6bb1dea2f2`
- Reviewed `AGENTS.md`, current migration strategy/contracts, `crates/pixiv-sdk/src/context.rs`, `diagnostics.rs`, `lib.rs`, `tests/context_ownership.rs`, the frozen `fanbox-context-ownership.json`, and app lifecycle/diagnostics compatibility reexports
- Checked the unchanged Pixiv `crates/pixiv-app/src/facade.rs` against the base
- Reviewer performed only source/log inspection and wrote this report. No Cargo, Go, native program, network probe, production Go change, or additional denied probe was performed by the reviewer

## Finding resolved during review

### Awaited Tokio deadline could return a false cancellation without completing state

The initial `cancelled()` implementation selected an awaited Tokio sleep but then relied solely on `std::time::Instant` in `error()`. On a supported paused/advanced Tokio clock, the sleep finished while the standard monotonic deadline remained in the future. Consequently the future returned `Canceled`, `error()` remained `None`, and descendants retained live cancellation tokens. The original app timer arm explicitly recorded a deadline, so the move regressed that behavior.

The parent preserved a runtime RED: three deadline regressions failed with `Canceled` instead of `DeadlineExceeded`, while the prior-cancellation regression passed. The final implementation calls `ContextState::deadline_elapsed` from the selected sleep arm. It first honors existing self/parent completion, advances only ancestors owning an effective deadline no later than the delivered deadline, and records the deadline through the existing immutable-completion path. Earlier child deadlines leave later/no-deadline parents and siblings live. An inherited deadline expires its owning ancestor and affected siblings. Prior recorded cancellation remains unchanged.

Four deterministic Rust-specific tests cover these outcomes and bound post-advance task joins with a two-second timeout. This correction does not alter any frozen Go observation or broaden the claimed Go-to-Rust API correspondence.

Evidence inspected:

- `/tmp/pixiv-context-paused-clock-red.log`: 1 passed, 3 failed; SHA256 `6c4ec9cb3c8c2877a16ee5d1fb62a7c83cc1120f08cd603e75bcc511c7e6dcf8`
- `/tmp/pixiv-context-paused-clock-green.log`: 12 passed, 0 failed, 0 ignored; SHA256 `6f34d5e64b661dc5a1110208100511a42b3c2a72628c2706b4c888662fcc1c42`

## Ownership and cancellation assessment

- Cancellation state is shared by `Context` clones and metadata/scope wrappers. Existing `new()` and `Default` remain cancelable owners; `background()` and `todo()` are uncancelable roots
- Children hold their parent strongly, while the parent registry holds only weak child references. There is no internal parent/child strong-reference cycle. Completion unlinks a child and drains descendants; dead weak registrations are pruned on subsequent registration/completion
- `without_cancel()` creates independent uncancelable state with no cancellation parent or deadline, while retaining the metadata chain and diagnostic sink/scope. Children created from this detached state own independent cancellation. Canceling the detached context cannot cancel its source or child
- Metadata lookup preserves key-type namespaces and owned equality, newest-value/explicit-empty shadowing, a separate extension namespace, and shared `Arc` payload identity. Context debug formatting does not print stored values
- Completion is immutable after the first recorded cause. Effective child deadlines are the minimum of inherited/requested deadlines. A deadline elapsed before a later cancellation wins. A child constructed from an already-completed parent inherits that cause even if its newly requested deadline is already past, matching the frozen constructor ordering
- Registration and completion synchronize through the parent child registry plus a completion check, closing the lost-cancellation registration window. Completion releases its reason and child-list locks before upward unlinking or recursive descendant completion. The new ancestor deadline traversal also holds no lock across recursive calls. No opposite held-lock order or callback-under-context-lock deadlock was found
- Diagnostics emit outside context locks. Scope copies the sink, fills only an empty event module, always binds its request ID, preserves explicit modules and all other payload fields, and remains silent without an installed sink/scope
- Constructors are synchronous and runtime-free. The only timer is stored in an awaited notification future, with no spawned context task or independently running timer. Dropping that future drops its timer; independent automatic Go `Done` timing is explicitly not claimed

## App compatibility and scope protection

`pixiv_app::lifecycle::{Context, ContextError}` and `pixiv_app::diagnostics::{Event, Scope, Sink}` remain available through reexports of the SDK types. The diagnostics implementation is an unchanged move from the base. The old Context methods and signatures remain available, and `Attempt`/`Lease` are unchanged. Moving the concrete Rust type changes its defining path/debug representation; a stable runtime type-name or debug-text identity is not claimed.

The Pixiv Facade has no diff from the base and continues forwarding caller context. A FANBOX `lifecycle.Run` child must not be added to Pixiv by inference. No FANBOX client/facade is implemented in this slice.

## Test strength and coverage boundaries

The fixture SHA256 is `6c0d721eeb35314058c8f928114a9ea2c6bf76e92d468502fc543fd7651cc705`. The Rust test checks its full hash, fixed reference, Go version, and 41-row inventory. The Go harness records actual standard context, diagnostics, actual CurrentUser transport execution, and lifecycle ownership, guards frozen production/standard-library sources, and compares capture/replay bytes exactly.

Rust covers representable observations from 12 standard and all 8 diagnostic rows in the original 8 tests, plus the 4 new paused-clock regressions: 12 Rust tests total. The mapped assertions check cancellation/deadline messages and classification, inherited values, scope presence, awaitable readiness, min deadline identity, both cancellation orders, exact typed diagnostic event sequences, detached scope/value retention, and runtime-free construction. Extensions also test shared payload identity and type separation.

This is not byte-for-byte Rust replay of all fields in those 20 rows. Go channel nil-ness, concrete/interface identities, exact Go error-tree types, pointer-key equality/identity observations, and absolute wall-clock timestamps are not mapped. The typed-value row includes `distinct_pointer_key_value` without a `go_only_` prefix; it is nevertheless outside the explicitly documented owned-`Eq` Rust key mapping. The six complete nil/uncomparable-key panic rows remain Go-only. The 9 SDK-forwarding and 6 FANBOX lifecycle rows are frozen evidence awaiting actual Rust production integration.

Other honest gaps remain:

- No independent Go-style Done-channel timer; lazy `error()` or an awaited timer is the declared Rust topology
- No custom `RequestContext` forwarding integration, caller-source/interface identity proof, or arbitrary Go cause/custom-context mapping
- No unrestricted concurrent schedule, cancellation-vs-registration stress, deep-tree/stack-depth, large child-registry performance, or exhaustive deadlock/race proof
- No dedicated dropped-notification-future/resource-retention regression, despite the absence of an independently owned task/timer in source
- No native socket/cancellation, live account, FANBOX lease/body integration, or other-platform parity established by this foundation

These limits are not converted into completion or platform-verification claims.

## Verification

The parent owns serialized execution. Initial focused checks passed 8 SDK tests and 19 app tests with the existing one ignored app helper. The corrected final focused run passed all 12 SDK tests. The first full gate was explicitly interrupted at `bookmark_reads` with exit 130 after the review finding, and remains superseded evidence rather than a full pass.

Final unchanged `scripts/check-rust.ps1` run passed with exit 0. The parent ran the serialized aggregate script; the reviewer inspected the final log, terminal exit/timing files, source seals, and unchanged script/Cargo metadata. Required metadata/source-test separation, formatter, strict workspace/all-target Clippy, workspace tests, and optimized release build completed. Existing vendor dependency warnings remain visible; no expectation, ignored test, source separation rule, or script stage was relaxed.

- Started `2026-10-10T06:25:33Z`; finished `2026-10-10T06:30:44Z`; elapsed 311 seconds
- Log: `/tmp/pixiv-context-bridge-final-full-gates.log`
- Log SHA256: `6f78dd4bdb724d02b9de3fb5c031f932a808d856b75d174033339cc2732e1f98`
- Terminal exit file: `/tmp/pixiv-context-bridge-final-full-gates.exit` = `0`
- 272 test summaries: 266 Running suites, 5 doc suites, and 1 existing child summary
- 667 raw passes / 666 meaningful passes, 0 failed, and the existing 11 ignored tests
- Corrected context suite: all 12 tests passed in the final full gate
- Strict Clippy completed in 15.44 seconds; test-profile compilation 54.86 seconds; optimized release build 47.56 seconds
- `/tmp/pixiv-context-foundation-source-before-final.sha256` recheck: all six reviewed production/test source entries OK
- Fixture SHA remains unchanged; Pixiv Facade and `scripts/check-rust.ps1`/Cargo metadata have no diff from the review base

## Reviewed source seals

- `crates/pixiv-sdk/src/context.rs`: `ccdaba94a61bcd150221a0fa78d8ef6427ffd502f52e3129d762ed63e258348d`
- `crates/pixiv-sdk/src/diagnostics.rs`: `e94201065bf1c0fb880c110b8b1c6e0129ca13d728a6db4078eb47cc015fd886`
- `crates/pixiv-sdk/src/lib.rs`: `c1efe963d332370046a9f3663e64a35e2fcdee519d5b8639d2c82494a0760bc8`
- `crates/pixiv-sdk/tests/context_ownership.rs`: `861eb159686ac50f7d05e6b284df5337a8d7c23ff031f723e645d4fa5443f6b6`
- `crates/pixiv-app/src/lifecycle.rs`: `3627de176480c46b30642a4952f809be26271536a420c09cf45cdc93c4c95a4e`
- `crates/pixiv-app/src/diagnostics.rs`: `3f90e94d2f59a8ea648563478c882a1a9d009137a8741d59f0ce90e88aa2a59f`

Final approval recorded `2026-10-10` after the final full-gate evidence and unchanged source seals were checked. Approval is limited to the reviewed foundation and stated mappings; all integration, source-only, timer-topology, concurrency, drop/depth, native, and platform gaps above remain open.

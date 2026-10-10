# Reverse image search: Go-first connected contracts

Reference: `4b4426487ef18bed276706daec385e0d0a6979f9`; published implementation baseline: `99246d8bfb8712caa0903426953423850836cefe`.

This checkpoint captures actual unchanged Go behavior before Rust implementation. It adds Go test producers, synthetic fixtures and provenance only. It does not implement reverse search, promote platform verification or establish complete migration parity. CLI image-source selection is account-independent; source loading, normalization, provider protocols and entry-point output are distinct connected boundaries.

## Production boundaries

- `Loader` creates a private immutable file snapshot of an owned regular file or mocked HTTP body. Independent readers, safe source summaries, cancellation/error precedence and cleanup are observed through the actual public implementation. Linux held-reader unlink behavior is platform-specific. Filesystem-cause observations retain operation, concrete type, errno text and platform; only the unstable owned temporary pathname is omitted.
- `Facade` preflights before source I/O and owns one per-search snapshot. The genuine `Aggregator` runs SauceNAO alongside one ASCII2D upload followed by concurrent color/BOVW searches; provider errors/results retain fixed order. Canonical Pixiv identity deduplication and evidence accumulation run through the original aggregator, including int64 limits without a float projection.
- SauceNAO observations come from the original public client with a genuine multipart reader and owned HTTP response dependencies. Typed JSON errors, case-insensitive aliases, duplicate fields, Go numeric-string syntax, quota, safe errors and cancellation timing remain distinct.
- ASCII2D observations use the original public constructor, Preflight, Upload, Session.Search and Close. HTML/CSRF/forms, redirects, cookies, challenge/control payloads and recovery ownership use owned synthetic dependencies. Private solver-cache rows are explicitly separate from public-client observations.
- CLI observations execute the actual Go root with real source/facade/aggregator composition and synthetic providers, complete writer/error/output behavior, source-first routing and configuration precedence. Record interoperability uses the actual record pipeline rather than a guessed command route.
- MCP observations use actual tool registration, SDK wire validation, cancellation notification, concurrent requests and reuse. An owned Go test-binary child exercises genuine stdio transport. Its synthetic Searcher boundary does not establish live-provider composition; session Close does not own the caller's Searcher.

## Compatibility and safety limits

Owned synthetic files, images, HTTP responses, child processes and solver responses are the entire execution scope. No external provider traffic, third-party upload, real credential/profile read, real account mutation, OS trust change or supplemental native probe is performed. Keyword-search control rows may rotate only owned synthetic credentials and update their owned SQLite account state. The earlier denied multiplex/unfinished-HEAD/upload probe remains denied and is not retried.

ASCII2D requires the original Chrome146 TLS/HTTP2 fingerprint family. Mocked HTTP and ordinary reqwest transport do not establish that parity. Shared native-source reuse must preserve distinct provider header, cookie, redirect and solver lifecycle behavior; FANBOX's target-specific solver is not an ASCII2D substitute. Physical native upload/cancellation/proxy/platform behavior remains unverified.

Source URL grammar, filesystem race/open behavior and unrestricted JSON/HTML/numeric grammar outside the recorded cases remain open. Nil Go contexts, typed-nil interfaces, private cache state and native Windows/Darwin behavior must not be silently represented as an equivalent Rust API. Go allows some cancellation after successful final read/HTML decoding and nil-error short writes to remain successful; captured quirks must not be repaired by changing fixture expectations.

The existing SDK HTTP2/Accept/public Context/client ownership/idle-close, pacing, signal restoration, destructive Windows repeated-cleanup and distribution/update debts remain explicit. Automatic cleanup must be once-only by normal ownership. Source-capture counts describe evidence, not completed Rust feature progress.

## Implementation handoff

The shared service belongs in the application layer, with source/ordinary HTTP, aggregation/normalization and both provider adapters. Its search outcome must retain a response beside an optional error; a conventional `Result<Response, Error>` would discard Go's required all-provider-failure and cleanup-error envelopes. Native fingerprint infrastructure can be shared by normal provider header policy while preserving FANBOX's public interfaces; sessionful ASCII2D solver ownership remains separate.

Source copies and SauceNAO multipart uploads must remain streaming: the original source/upload has no aggregate image-size cap. The current FANBOX raw request's in-memory `Vec` alone is insufficient. ASCII2D's 10MiB limit is a provider-specific limit, not a source/Sauce limit. The runtime already carries reverse settings; source/Sauce use the standard proxy while ASCII2D uses its separately configured override, and solver control is direct. CLI routing precedes saved-account execution. MCP cancellation needs graceful owned cleanup and reverse-specific structured error/results, rather than blindly aborting its future.

## Sealed evidence counts

| Boundary | Captured observations | Final validation |
| --- | --- | --- |
| Source / facade / aggregate | 99 rows: source33, redirect5, aggregate43, facade16, snapshot2 | Capture, three exact replays, full core package and race28 top-level, vet/gofmt |
| SauceNAO | 277 actual public-provider rows | Capture, three exact replays, race, related package297 named pass records, vet/gofmt |
| ASCII2D | 192 public-port rows and12 separate private-cache rows | Capture, three exact fresh recaptures/replays, package race42 top-level/226 subtests, vet/gofmt |
| Actual root CLI | 148 guarded rows;101 reverse invocations,91 source HTTP cases,12 SDK constructors,3 completed synthetic OAuth/search controls,2 record consumers,66 exit0 | Capture, three exact replays, selected race16 top-level, vet/gofmt |
| Registered MCP | 52 scalar cases plus cancellation/reuse, concurrent out-of-order reuse and owned stdio;60 actual tool calls per replay | Capture, three exact replays, race10 MCP and7 existing CLI ownership tests, vet/gofmt |

ASCII2D observes101 multipart requests:98 complete bodies have exact byte-equality framing assertions and complete parameter/header capture. Three intentionally unread CloseUpload bodies explicitly retain unknown framing. Only the random boundary token is masked at declared positions; its actual60 lowercase-hex characters/30 bytes remain observed. Prior192 provider outcomes and body ownership are unchanged by this capture correction.

Initial core float64 rounding, CLI nonexistent command/environment proxy/startup-only/keyword input harness failures, ASCII multipart omissions and owned launch failures are preserved in their provenance. The rejected CLI startup-only full JSON was overwritten before a standalone copy was saved; its fingerprint, gate logs and actual row-audit samples remain. None of these preliminary captures is final Rust evidence.

Per-boundary manifests are in `provenance/reverse-{core,saucenao,ascii2d,cli,mcp}-go.json`; the unchanged published Rust baseline and prior full gate are bound separately in `provenance/reverse-unchanged-rust.json`. Protected maps preserve434 Go production/module and104 published fixtures. All673 ledger IDs, statuses and verified-platform values remain unchanged.

The final combined related Go gate passes46 top-level plus721 nested named tests (767), six packages, zero failures/skips, in5 seconds. Vet, empty gofmt, diff check and migration validator pass. [Final gate provenance](provenance/reverse-search-contracts-gates.json) records exact commands/log hashes and the document-only evidence update after stable source/test/fixture inputs. No Rust source, test, dependency or gate script changes are present; the just-published complete unchanged Rust gate is reused with independent5,536-input byte comparison, rather than presented as a fresh reverse runtime result.

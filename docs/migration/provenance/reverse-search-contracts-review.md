# Reverse-search contracts checkpoint: independent review

Status: **Approved, contracts-only**. No blocking findings remain.

Candidate manifest SHA-256: `cad30094fcb8a7631103cfe6edfc637641f69b31472d2084aa09959dd74ef81e`

Published base: `99246d8bfb8712caa0903426953423850836cefe`  
Frozen Go reference: `4b4426487ef18bed276706daec385e0d0a6979f9`  
Candidate: **24 files, 7,167,508 bytes**. The accompanying JSON retains every exact candidate file hash. Approval applies only to these bytes plus the two review additions explicitly listed in the candidate.

## Independent checks

- Candidate manifest hash, all 24 current file hashes, total bytes and exact changed-path set match
- All 434 frozen Go/module paths and 104 published fixtures remain byte-preserved
- All 5,536 published Rust/dependency/gate inputs match current files and the published git blobs
- All 673 ledger IDs, statuses and verified-platform values remain unchanged
- All five boundary seals match their producer/fixture bytes and recorded validation log hashes; three fresh ASCII2D captures are byte-identical to the final fixture
- The related Go JSON log independently recounts 46 top-level and 721 nested named passes, 767 total, six packages, zero failures/skips
- Final vet, empty gofmt, diff-check and validator evidence pass. The separate sealed-document validator log reports success in 0.914 seconds
- The only pre-gate-to-final byte change is the explicitly documented contract-evidence document update

## Evidence boundaries

- Core: 99 rows (33 source, five redirects, 43 aggregation, 16 facade, two snapshot lifecycle)
- SauceNAO: 277 public-provider rows
- ASCII2D: 192 public-port rows and 12 separately labeled private-cache rows. Of 101 multipart requests, 98 have exact complete framing assertions and three explicitly retain unknown unread framing
- CLI: 148 independently guarded rows; 101 reverse calls, 91 source HTTP cases, 12 SDK constructors, three completed synthetic OAuth/search controls, two record consumers and 66 successful exits
- MCP: 52 isolated cases plus cancellation/reuse, concurrent out-of-order reuse and owned test-binary stdio; 60 actual tool calls per replay

## Findings resolved before approval

Core exact int64 preservation, immutable-snapshot rereads after source mutation, redirect/caller-hook traces and separate branch/mode concurrency barriers are now genuine Go observations. ASCII2D preserves and validates the previously omitted multipart parameters, MIME headers and framing; only the random boundary token is masked at declared positions. CLI stage assertions prevent startup-only captures from being counted as provider/output evidence. Documentation accurately states evidence accumulation, synthetic-only account state changes and narrowly scoped temporary-path omission.

## Limitations

- Approval is limited to this Go-first contracts-only checkpoint. No reverse-search Rust implementation, usable Rust feature, feature parity or verified-platform promotion is approved or claimed.
- Evidence uses finite owned Linux/amd64 Go cases. Core and CLI connect actual SourceLoader/Facade/Aggregator to synthetic terminal providers; MCP connects actual registration/SDK/stdio to a synthetic Searcher and an owned Go test-binary child, not the shipped CLI composition.
- ASCII2D Chrome146 TLS/HTTP2 fingerprint, physical uploads/cancellation/proxy behavior, live-provider integration and Windows/macOS/other architecture parity remain unexecuted. Denied supplemental native multiplex/unfinished HEAD/upload probes were not retried.
- Private solver-cache and private session-client-close observations remain separately labeled. Solver session randomness is controlled after construction; multipart normalization masks only generated boundary tokens at validated declared positions. Three deliberately unread multipart bodies retain unknown complete framing.
- Filesystem-cause normalization excludes only unstable owned temporary paths; operation, concrete type, errno text and platform remain. Linux held-reader unlink behavior is not a portable caller-lifecycle guarantee.
- Unrestricted URL/filesystem race/JSON/HTML/numeric grammar outside recorded cases and all existing SDK/lifecycle/pacing/signal/update/distribution debts remain open.
- The published Rust full gate is reused for unchanged inputs. No fresh reverse Rust gate or new Rust fixture consumer was run. The independent reviewer performed read-only source, hash, JSON and log inspections, without executing Go/Cargo/native/network operations.
- The rejected startup-only CLI full JSON was overwritten before a standalone copy was saved; its exact fingerprint, logs and observed audit samples are retained and it is not final behavioral evidence.

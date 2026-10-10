# Updater Go-contracts checkpoint review

Verdict: approved for the contracts-only checkpoint. No blocking findings remain.

Reviewed candidate SHA-256: `4179167726bd7ef3e2a45481ea7192af2eca9ada74062e9e91185a7b7937e194` (180 files, 15,030,959 bytes), based on `0e5f42273f69067bf2e06a130c926c4d58557fb4`; frozen Go `4b4426487ef18bed276706daec385e0d0a6979f9`. The JSON companion records every reviewed file hash.

## Review and preservation

Independent production-source-first review covered the actual root, coordinator, release/cache, source selector, signed installer, platform replacement/recovery and process checker before their new Go harnesses/fixtures. The reviewer ran only static reads and local hash/inventory comparisons, not Cargo, Go tests/compiler, network probes or native programs. Only these review artifacts were written.

All 180 candidate hashes and 150 durable evidence entries match. The 434 protected Go/module files and 110 prior fixtures remain byte-identical to the baseline. All 8,596 complete Rust-gate inputs match the exact tracked non-document inventory plus ignored Cargo.lock with no drift. All 673 ledger identities/statuses/Rust-source/platform values are preserved; only `cli:pixiv update` tests and differences changed. It remains pending with no Rust source or verified platform.

## Genuine boundaries and corrected findings

- CLI170 (158 behavioral/12 Go-only), coordinator305 (296/nine), release/cache160, source200 (167/33), signed Install199/detector34/physical-cache13, Windows26 and owned preflight41 retain their separate boundaries
- Signature over exact raw checksums, manifest digest, selected checksum entry and all-three official URL validation precede archive request; verified archive precedes execution. Invalid verification bytes never trigger alternate-source retry
- Duplicate/null manifest fields, case folding, base64 CRLF/noncanonical pad bits, query-first-value versus strict root-array probe, and bounded TAR EOF/footer-CRC versus ZIP CRC behavior match frozen Go without semantic normalization
- All-failure source ordering is controlled by actual probe goroutine exit after buffered enqueue. No sorting or completion-order normalization replaces the prior failed race witness
- Windows capture now rejects failed subtests. Its intentional exit1 proof preserves the fixture; exact replay3 and final27 named pass events succeed. Adapted function bodies remain exact and do not establish native Windows ABI/runtime
- CLI/coordinator preservation guards are durable and SHA-pinned. Nil/private/identity rows and language-specific error metadata are explicitly classified; their original behavior is unchanged
- Owned native Linux preflight uses the actual default process checker/replacer and only a visible dependency-free helper built twice identically. Exact stdout, stderr/status, caller cancellation/kill/wait and disposable target replacement are bounded evidence. The replaced target is never launched
- Final documentation records terminal gate success and retains failed captures1–6, provisional7–8, unestablished capture3 cause, discarded public DNS/network attempts, classification proofs, source race3, expected failed guard proof and zero-match nonvalidation command

## Validation and limits

Sealed logs show capture/replays, relevant Go/vet/gofmt and source/release race checks succeeded. The fresh unchanged `scripts/check-rust.ps1` passed in268 seconds with926 raw passes (919 workspace plus seven native child runs), zero failures and11 existing ignored tests;322 summaries and release completion were independently checked. This is Rust baseline preservation, not Rust updater implementation success.

No feature usability, platform/status promotion, native Windows/Darwin equivalence or complete distribution is approved. Go-install native metadata mapping, live transport/proxy/source timing, arbitrary grammars/process/syscall/concurrency schedules, six-platform builds/runtime, signing/publication/install scripts/PATH/handler/licensing and all prior debts remain open. Earlier discarded public probes are honestly retained as ordinary unsuccessful DNS/network attempts, not an all-offline success or safety-denial bypass. The denied supplemental native multiplex/unfinished-HEAD/upload probe remains unattempted.

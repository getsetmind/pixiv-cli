# Independent browser-contract foundation review

APPROVED_FOUNDATION_ONLY on base `0da302cdebcbcbbeeee558d9b93b9533e4d69662`, against frozen Go `4b4426487ef18bed276706daec385e0d0a6979f9`. No remaining blocker was found in this bounded scope.

This approves durable Go extraction contracts and backwards-compatible Rust byte API/auth forwarding. `SystemBrowserProvider::system` still explicitly reports native extraction as unimplemented for all four browsers. Native saved-browser import and Rust comparison of the six new browser fixtures are not approved as complete.

## Source and evidence review

The six fixtures preserve 454 distinct Go rows: Chromium crypto 159, Firefox 60, Safari 57, sqliteio 42, source-driven native secret 54, and connected import 82. Import contains 84 actual root/RunContext observations and 84 recorded commands. Case IDs are unique. Native evidence has 21 entries from 15 unique frozen sources and exactly 13 independently reconstructed substitutions: 9 build tags, 3 filename constraints and 1 native import.

The producers call genuine unchanged Go functions. Owned synthetic executables and native allocations are separated from actual OS-secret integration. Connected Go import uses the genuine registry/provider path, official readonly SQLite shell, real crypto, SDK validation and owned saved DB/config/output; it uses no registry, read, password or key override. Process-helper CSV/error rows remain separate from official-shell SQL rows. Parameter-map semantics are compared without inventing a Go iteration order.

The official SQLite archive SHA256, SHA3-256, size, all four source hashes and binary hash match provenance. Raw Chromium bytes, provider-specific UTF-8 rejection, Safari's malformed-input panic and pre-canceled Read, static errors, cleanup, DPAPI input lifetime/copy/free and early-return allocation edges are preserved as actual source observations. They are not native Darwin/Windows verification or completed Rust behavior.

Rust's byte defaults remain compatible with existing String implementations. The byte adapter preserves opaque values and cardinality errors, and auth calls the normal byte API then the existing byte import. Shared generic close/join/drop ownership is retained. No production test-only API, flag, branch or test module was added. The three new tests do not independently cover every byte-specific panic/drop/read/close-error or AuthCommand override schedule; those remain part of the connected backend comparison.

## Gates and preservation

The unchanged `scripts/check-rust.ps1` passes all stages in 338 seconds, release in 46.12 seconds. The log reports 798 passed, 0 failed, 11 existing ignored and 12 filtered results across 303 summaries. These raw results include existing child summaries and are not unique compatibility cases. Related Go records 295 named passes (63 top-level, 232 nested), no failures or skips; vet, empty gofmt and the post-document validator pass.

The missing-byte-API compile RED is exit 101. Focused GREEN is exit 0 with 3 new byte tests and 5 existing auth tests. The initial grouped Go failure from missing `MIGRATION_BROWSER_SQLITE_SOURCE` remains in historical evidence; the corrected same-command run passes. No failure or skip was rewritten as success.

All 434 recorded Go production/module paths equal both the frozen reference and the published base; all 97 existing fixtures (95 JSON, 2 PEM) remain byte-identical. All 673 ledger IDs, statuses and verified-platform entries are unchanged. All 8,831 protected inputs have zero hash drift. Persisted provenance retains every original manifest key/value, and final artifact hashes match current bytes.

## Limits and binding

The remaining native backend, real shell/CSV/crypto/secret/parser implementation, blocking lifecycle, native platforms, raw path/ACL/permission/cancellation, current browser formats and distribution debts remain required. Helper-only Go inputs are not cross-language production-interface comparisons. The denied supplemental native probe is excluded. This is not whole-feature, native-platform, FANBOX or migration completion approval.

The reviewer ran no Cargo, Go, network, credential operation or native probe and changed no production, test or fixture file. Only the two expressly authorized review artifacts are written. [Machine-readable approval](browser-cookie-foundation-review.json) binds every final changed/new candidate file except these two artifacts and every protected input. Later candidate content changes require review reconciliation.

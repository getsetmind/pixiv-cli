# FANBOX SDK final integration source review

Reviewed 2026-10-10 UTC. Implementation base: `51dcb2c4f6e196372f0f022170d8b53f1f5eff3a`. Frozen Go: `4b4426487ef18bed276706daec385e0d0a6979f9`.

## Decision

**Scoped source approval after the bounded tokenizer, field-fold and decimal-port corrections.** No remaining critical resource, credential/error-boundary, or shared-solver ownership blocker was found in this SDK slice. The approval covers the public identity/options/DTO/context/solver implementation and the observed rows below, with the explicit remaining compatibility debts preserved. It is not complete FANBOX, arbitrary parser/URL/date grammar, native-platform, distribution, or replacement approval.

The parent-owned final focused run is terminal `0`: formatter, strict workspace/all-target Clippy, identity 9/9, solver 4/4, source attributes 1/1. The required unchanged full script is **pending at report creation**. Its terminal result must be appended or separately linked before claiming the complete required gate passed.

I read AGENTS.md, migration strategy/contracts, all eight FANBOX production modules, both public contract tests and their helpers, original and supplemental fixtures, actual frozen Go Session/client/identity/solver behavior, and the separate native source reviews. I ran only read-only source/log/hash inspection. I ran no Cargo, Go tests/native program, network request, trust modification or supplemental native probe; no repository source was edited. The only created artifact is this report. The denied multiplex/unfinished HEAD/upload probe was never retried, reconstructed or counted as evidence.

## Findings corrected in this checkpoint

### 1. Duplicate metadata recovery incorrectly skipped abrupt comments and misparsed raw text

The original document-wide `raw_tags` scanner searched only literal `-->`, although frozen x/net/html `token.go:598-645` terminates `<!-->`, `<!--->`, and `<!--x--!>`. It also treated fake tags and open quotes inside script/style/RCDATA as real markup. The actual tokenizer could emit a valid duplicate-attribute metadata tag while the auxiliary scan had no matching tag or supplied the wrong fake source.

The new fourteen-row actual public Go supplement freezes six comment/raw-text cases, three identity Unicode-fold cases and five injected-constructor decimal-port cases. Genuine Rust runtime RED preserved six existing passes and three failing new tests. The fix removes the document-wide scan. `identity.rs:342-381` records consumed source boundaries at genuine html5ever structural-token callbacks; only the emitted duplicate meta span is located and re-tokenized with temporary unique attribute names. The boundary is based on raw input bytes. Markup5ever `BufferQueue` derives Clone over an owned `RefCell<VecDeque<StrTendril>>`, so inspection of its clone does not drain the original queue. Comment/end-tag callbacks exclude the fake raw-text spans that caused the finding. The ordered recovered attributes preserve Go's last duplicate name/content choice.

### 2. ASCII-only JSON lookup omitted actual Go Unicode SimpleFold aliases

Frozen encoding/json `fold.go:12-48` uses Unicode SimpleFold, including long-s U+017F and Kelvin sign U+212A. The original ASCII-only lookup silently ignored `iſCreator`, `ſtatus`, etc.

The private `identity::json_name_eq` now handles the only non-ASCII fold classes reaching the ASCII schema, with exact character count and ordered duplicate processing. Identity uses it for all known envelope/user fields; solver uses the same private helper for all eight known fields. No exported test API was added. Three new actual public identity rows and seven separate actual public solver rows cover the relevant aliases, including an observable past `expireſ` alias and repeated alias ordering. The solver supplement has its own genuine runtime RED before production correction.

### 3. WHATWG port range rejection changed Go constructor validation

Go `net/url/url.go:762-775` checks optional decimal-port syntax without a u16 range bound; Url 2.5.8 `parser.rs:1127-1131` rejects values above 65535. With genuine injected transport, public Go OpenWith accepted the affected native-proxy, solver-service and solver-upstream options, while Rust returned InvalidArgument.

`options.rs:74-85` now validates decimal syntax and general bracketed/nonbracketed host structure in a fallback parse, retaining the original authority/wire bytes. It does not silently dial a different port: the parsed host is used for policy, while downstream request/factory input retains the supplied port. Five actual public constructor rows prove this bounded validation change; no out-of-range port was dialed. The fallback does not establish complete Go URL grammar equivalence.

## SDK ownership, ordering and safety assessment

- Session and Solver remain private. RawTransport/RawBody are genuine production dependency boundaries; there is no private Session accessor, fake nil-Transport wrapper, public control-client seam, fixture branch, localhost hardcode, test function or cfg(test) module in FANBOX src
- Open preserves user-agent, native proxy, solver option, transport construction, then cookie normalization order. Credentials Display/Debug/Serialize are redacted. Public errors use product `fanbox`, actual operation names and the captured distinct challenge/forbidden/expired/unavailable/malformed classifications
- Ordinary solver control is separate from browser-fingerprint native transport, is direct/no environment proxy, has no native proxy/cookie/origin/referer, sends only the fixed anonymous homepage request and optional solver upstream proxy, and follows no redirects. No ordinary-HTTP fallback replaces the native factory
- Native caller Arc context is forwarded unchanged. Failure boundaries discard arbitrary external error chains and preserve recognizable cancellation/deadline causes. Identity combines read and close failures in read-first order; close alone precedes parsing. 403 scanning reads the full body and close failure takes precedence over scan failure/status classification. 401 and other error statuses do not scan challenge markers
- Actual Go Client.Do malformed Location semantics are reproduced before manual no-follow processing: malformed Location closes once, ignores close failure, suppresses the ordinary network_request event and returns safe request failure. Valid/absent redirect Location retains explicit response-close precedence. Redirect cookies are removed after the first authenticated hop, host/scheme/userinfo checks remain per-hop, loop detection has no arbitrary ten-hop cap, and replay starts from the original homepage
- Shared solver ownership retains first caller values/extensions/scope but removes its individual deadline/cancellation. Each waiter owns independent cancellation. Last-waiter Drop cancels the active solve; cache/active changes are mutex-protected; stale completion checks Arc identity; successful completion clears active before notification. Successful guards therefore do not decrement a replacement call. No strong-reference cycle or held cache lock across diagnostics/network await was found
- Expiry is checked per native-state access; invalidation before solve and after a second challenge matches Go. Integer expiry lexemes, quoted positive integers, signed-int64 extrema and Go internal-seconds wrapping are retained in captured cases. First complete solver JSON is accepted without requiring stream EOF; trailing JSON/garbage is not promoted to an error. Physical control Close failures and cancellation-ignoring/stale completion remain separate private/source evidence, not loopback public proof
- The native raw factory/profile interface retains genuine fallible construction, system trust, certificate/hostname verification, no automatic redirects, independent physical URL/logical Host and shared client pool. Detailed certificate and idle/admission source assessment belongs to the separate reviews. Their approvals and the six synthetic raw-boundary witnesses are not complete public native Session equivalence

## Evidence and expectation preservation

The public tests exercise actual Rust Client methods and compare captured Go DTOs, SDK reason/code/text/safe source, method/URL/headers, byte/Close/idle totals, exact caller context identity and per-caller event order. The current inventories are 156 representable original public identity rows, 67 actual public HTML rows, eight context-forwarding rows, fourteen new identity/options rows, 93 actual public solver rows and seven new solver alias rows. These are observations, not top-level test totals or completion percentages.

Two valid-option Go nil-Transport constructors remain unrepresentable. The two invalid-option implicit-client cases still compare precedence through the representable Rust boundary. Nil caller context, concrete Go error/sentinel/interface identity, read-call topology and unrelated private rows are explicitly excluded. Private parser observations are not rewritten into guessed public SDK error expectations.

Original 309 identity, context 41, solver 124, redirect 3 and native 6 fixtures match the base bytes. Public HTML 67 remains `fe0eee…e388`; public solver 93 remains `a908d4…ba80`. New identity 14 Go capture/replay/race/vet logs are terminal0 and durable in `docs/migration/provenance/fanbox-identity-tokenizer-go-gates.json`.

The solver 7 metadata clarification after RED changed **only the top-level evidence prose**. Independently compared the preserved pre-clarification fixture (`73030dcc…a0a1b3`) to final fixture (`81e88cc9…e745cf`): all seven cases, inputs and observations are unchanged, including exact raw case-array bytes (68,404 bytes; SHA256 `9814abe1748a7260cd3810c0bb2e55af6a6eb846d74c9b7ee1387298609b031f`). The clarification correctly says the expires alias moves the existing expiry/one value under interchangeable expires without a competing expiry. It does not change any expected outcome.

Parent-owned logs inspected:

- Identity runtime RED: `/tmp/pixiv-fanbox-identity-all-regressions-red.log`, exit101, six passed/three failed; SHA256 `b2dfba244b9127f1c860421bea2e9eea274e7b0068053c8e5284448e9a9f7109`
- Solver alias runtime RED: `/tmp/pixiv-fanbox-solver-aliases-red.log`, exit101; SHA256 `29ceb189999604ff7d348b44fdd0655ea8877fa9c4c1b53720b3ee9f64ba9d65`
- Final focused: `/tmp/pixiv-fanbox-sdk-final-focused.log`, exit0, identity9/solver4/attributes1; SHA256 `257ff4d170f9899582f85cd45f7ce80b7f849f892e6d720540c9b4b105c21e41`
- Earlier combined identity GREEN log had identity9/9 but aggregate exit101 due a nonexistent attribute representative path. It is not a successful aggregate gate. Corrected actual-path/existence-guard rerun `/tmp/pixiv-fanbox-source-attributes-final-focused.log` is exit0; SHA256 `f39fdea2c5773d4ed7e1eb677528f27ff01ad7e119564022ee0812df38bdca18`
- Approved native six-case focused run `/tmp/pixiv-fanbox-native-third-focused.log` is terminal0. It reports two top-level passes, one being the excluded no-mode child-entry scaffold; the driver ran six real owned children. Detailed source/runtime evidence remains scoped to native reviewers and its durable focused report

## Explicit follow-on compatibility debts

The parent chose the bounded observed rows plus the specific reviewed regressions for this checkpoint. The following source-confirmed differences remain real, need Go-first public evidence and production correction in the next parser slice, and prevent a broad parity claim. No probes for these candidates were performed by this reviewer.

1. **Lone surrogate replacement.** Identity `identity.rs:133,155,187` and solver `solver.rs:430-433` use serde_json string decoding. Go `encoding/json/decode.go:1267-1280` substitutes U+FFFD for unpaired UTF-16 surrogate escapes; serde_json 1.0.151 `read.rs:908-963` rejects them. Candidate public metadata `name:"n\uD800"` or solver userAgent `"agent\uDEAD"`: Go can accept replacement characters, while Rust reports invalid/malformed JSON. JSON member-name and malformed UTF8 replacement require their own exact-byte mapping too
2. **Go total nesting limit.** Solver `solver.rs:339-340` and identity Entries/RawValue parsing use serde_json's iterative `de.rs:1102-1210,1285-1292` raw-value scanner without Go's global limit. Frozen Go `encoding/json/scanner.go:148,179-185` caps total object/array depth at 10000. Candidate otherwise-valid identity/solution with an ignored field containing enough nested arrays to make total depth 10001: Go rejects; current Rust can accept. Count enclosing envelopes, not merely the array count. This is a compatibility/resource-boundary difference, not a found stack-safety failure of the iterative Rust parser
3. **RFC3339 leap seconds.** `solver.rs:550-551` uses Chrono 0.4.45. Chrono `format/parse.rs:230-232` accepts seconds 60; Go `time/format_rfc3339.go:112` and `time/format.go:1165-1168` reject it. Candidate expiry `"2100-01-01T00:00:60Z"`: Go malformed, current Rust can accept
4. **HTTP-date weekday.** `solver.rs:553-554` parses `%a` with Chrono, whose `format/parsed.rs:670` checks weekday agreement. Go `time/format.go:1125-1127` validates the weekday token then ignores its consistency. Candidate `"Mon, 01 Jan 2100 00:00:00 GMT"` (actual Friday): Go accepts, current Rust rejects

General HTML/JSON/URL/date grammar, ready-result/cancellation schedules, arbitrary custom Go contexts/error identities, deep/dropped waiter stress, canceled transport ignoring completion, concurrent RawBody Read/Close, physical solver body Close, native media/decompression and resource/content/download are not established. Current options still are not a complete RFC-style Go net/url implementation (including encoded IPv6-zone authority and general redirect escaping/canonicalization). Existing dependency mappings do not establish HTTP1/proxy/TLS1.2/resumption/HRR, native Windows/macOS/other architectures/system trust, saved-account facade/CLI/MCP or browser extraction. Root Rust CLI/native release license packaging remains open: the unchanged licensebundle check covers its actual ugoira manifest, not the entire new root dependency graph. The denied supplemental probe remains unavailable evidence.

## Source ownership and final seal

Independent byte comparison of all 434 frozen Go production/module paths found zero differences. Existing Pixiv Facade, shared SDK Context/diagnostics/Error and unchanged check-rust.ps1 match the base. Cargo.lock removes no previous package/version/source or checksum and adds 37 packages; changed existing dependency edges are SDK additions, duplicate-version qualification, and the tokio-util libc feature edge.

All 1,758 current files in the five imported native families match the published patched-family manifest by size/SHA256, with no missing/extra files. Native inventory SHA256: `78a081d82a021f10f80ee8ae00c36374d9cf512fbc4b425ca2d129182269fef7`. All ten idle-review file hashes remain current. Certificate callback/per-SSL-storage/service hashes remain current; that review's tls.rs/native.rs/profile.rs snapshot is superseded by declared formatting/Default-derive corrections and the current factory semantics were independently re-read here. This report does not reinterpret the native reviews as runtime or full-platform approval.

Final source/test/fixture hashes below match corresponding entries in the parent's pre-full-gate snapshot `/tmp/pixiv-fanbox-sdk-full-gate-source-snapshot.json`, whose SHA256 is `2eb870f09a96ca796771ba3692cd463c87ac310ab25f5becc216c4484d7a4d52`. Full-gate result is pending. The manifest records the exact reviewed implementation and contract files; it does not claim exhaustive semantic review of the imported native family.

```json
{
  ".gitattributes": "1d13402b0bde4c07b654cd26294b5dc470f6bd64f445473c756ad7cd3c64f4aa",
  "Cargo.toml": "bef5a4a8ab25d0700fa3261d4a771b1a79b0195dc884e6b2feb8a9a6e67bd663",
  "Cargo.lock": "bae87120a261b20f615cb4b961037221ac7875e384d95903d13fff20f5f84b54",
  "crates/pixiv-sdk/Cargo.toml": "77cb5983e586204fce04ed54e8ce0096efa9dfd48bf49d7fec0c1bc688198924",
  "crates/pixiv-sdk/src/lib.rs": "5e74aa9bf85a0b105273af1c434627c535d3f88b01f0fb3012e6f31406cccc12",
  "crates/pixiv-sdk/src/fanbox/identity.rs": "e9ca72e5402610e316a84cbb2c53d422cdfb3799212bc58fa006e584cb88fcfa",
  "crates/pixiv-sdk/src/fanbox/mod.rs": "50ee0853f2c13a422ef65f4f0f401febcb2fb553b35a8073661946bf39ecd9ec",
  "crates/pixiv-sdk/src/fanbox/models.rs": "bedb48f5ff4a318864c69784a670a5885e40a1455e1b8ae55a702e58542ed606",
  "crates/pixiv-sdk/src/fanbox/native.rs": "22bac9a4d1e7b2988228da5c2f3f7969a75407f3aa28657668341a9d34df58b2",
  "crates/pixiv-sdk/src/fanbox/options.rs": "ef1760135ed14da3b28eadf222d2c30c7e3e8151d2ed2288b6e7a5a3d4d35b0d",
  "crates/pixiv-sdk/src/fanbox/profile.rs": "ba4fad8edaa90f2dfc5d0c1e04679f8218c6570ec797ba0cae7f3ead58724295",
  "crates/pixiv-sdk/src/fanbox/solver.rs": "fc70fe4fb27c46e6881e8ef5a20f0a7e51e5a72dae7b941ce52d646d08c83612",
  "crates/pixiv-sdk/src/fanbox/transport.rs": "af3d80e6ffbc017a96fcbf326206d7b4ad5d6a47333b92d36b5b756d92b51f89",
  "crates/pixiv-sdk/tests/fanbox_identity.rs": "7e1c3fc00d6c2d2532bd49a3bea4f72bf102d15a7e6dd36fc70a049018adef1e",
  "crates/pixiv-sdk/tests/fanbox_solver.rs": "2dc06094b7baaeac3741517e76c3f9581284add91a08a06650f6e5b699cadb9c",
  "crates/pixiv-sdk/tests/fanbox_support/mod.rs": "563edd98980b73db7904a8edbaf80cbbaed6d22ecdca8f41c6ccca76f1f3636d",
  "crates/pixiv-sdk/tests/fanbox_solver_support/mod.rs": "dce7f2fdf8cdb3e3496c10ec83c52a169eae8ba8b7dd298e48a5eadb5b70e008",
  "crates/pixiv-sdk/tests/fanbox_source_attributes.rs": "8ec97d937c31a6457c6b4d3a9cde589151a5a77079aa612d488ac4f5af0193f6",
  "sdk/fanbox/migration_identity_public_html_test.go": "b71cd78abd9d2e98b26cef97a8cbcbcd648b8b53a506389c669525c28c3b09c9",
  "sdk/fanbox/migration_identity_tokenizer_regression_test.go": "f27059d6a92cd3f16a8067929a254a53f058ce68eca19eef04ec46666745ae90",
  "sdk/fanbox/migration_solver_public_test.go": "a21a5a150c8a644d7874f5e94cbcf78716742279048a41f680d1ede0a5d6fa34",
  "sdk/fanbox/migration_solver_public_aliases_test.go": "bbca2490257049b75b4883b97428b44d794bc0d290e668040d30151e040f9a80",
  "crates/pixiv-sdk/tests/fixtures/fanbox-context-ownership.json": "6c0d721eeb35314058c8f928114a9ea2c6bf76e92d468502fc543fd7651cc705",
  "crates/pixiv-sdk/tests/fixtures/fanbox-identity-protocol.json": "cab8573fbc51a49be2eadb540c25d5386739edc2329eaa80e0ca4d9f6e2c611b",
  "crates/pixiv-sdk/tests/fixtures/fanbox-identity-public-html.json": "fe0eee2fc4f5d08c717a223f0df043c003458c1893fe55e95f66ac4df043e388",
  "crates/pixiv-sdk/tests/fixtures/fanbox-identity-tokenizer-regressions.json": "732d71a38a98407dbf874586c3f77412d4b34006ac916590e5593321da79129a",
  "crates/pixiv-sdk/tests/fixtures/fanbox-media-body.json": "a3b48317ef6f4dba2c5ca6d6f5d935467eee273147b88ebeff776b286f60bce2",
  "crates/pixiv-sdk/tests/fixtures/fanbox-native-profile.json": "df753a715efd28c016a2532fe544e992b00a1fe5ce58b756d807c13b9522dd62",
  "crates/pixiv-sdk/tests/fixtures/fanbox-solver-public-aliases.json": "81e88cc966ae90cd837a8bde8ea7ad9fdeb83c4d3419f2509d55a64c19e745cf",
  "crates/pixiv-sdk/tests/fixtures/fanbox-solver-public.json": "a908d47882462e698b67d80f2f48cfab578964e675029afe973a8a024711ba80",
  "crates/pixiv-sdk/tests/fixtures/fanbox-solver-redirect.json": "6a8db01180325dc9074984b4b763250d0029118889ed7e5141a8ae1554709651",
  "crates/pixiv-sdk/tests/fixtures/fanbox-solver.json": "45ed3d463ce1159290dbb6686490e754e30d4540a108754571a926b8bba6f382",
  "internal/services/fanbox/protocol/protocol.go": "c153337aa61756f5d5ea36ec32ca272e68da8a1604c1d4a4e4d6bcb2c957fdd3",
  "internal/services/fanbox/protocol/identity.go": "10ba481b43e3ec7d0bdaa2defbbd629c4b5bb2aa169c5bff99c8e756de8143d9",
  "internal/services/fanbox/protocol/solver.go": "e55464b091fa6720b7134a9487684c6c0969f9b4921384ea0e091d634782fcea",
  "sdk/fanbox/fanbox.go": "2208576144b94b89efd57b6f054ee018268812d52d556fd0758e2ee322577542",
  "sdk/fanbox/errors.go": "523577a066e3d53ce9eced8c7afbe29f422a75ce6aca58bc3b5b6fbf8da59909",
  "sdk/fanbox/ops.go": "868b3b68de07d7638af5f9dd3ab1be8f0c9751f222c05c7776cde092052a7c8d",
  "docs/migration/provenance/fanbox-native-source-records/patched-family-manifest.json": "4e07d11a577ec39fa1f8f4489ed02f962a6d23cd2831c3c307e45c8553344fd6"
}
```

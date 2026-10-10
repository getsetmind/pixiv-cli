# Independent Go-first FANBOX transport/context contract review

## Decision and boundary

PASS for the six-file, bounded Go-first contract slice after the native harness proof repairs listed below. No unresolved review blocker remains within this scope. This is not Rust parity, native Rust dependency approval, full FANBOX compatibility, platform certification, or final migration approval.

Repository base: `b3f01c9944d3317d10d48543f529af6bb1dea2f2`. Frozen Go: `4b4426487ef18bed276706daec385e0d0a6979f9`. Review read `AGENTS.md`, migration strategy, recovery history and provenance, all three new harnesses and fixtures, relevant unchanged production paths, dependency profile source, original execution logs and final manifests. Repository edits and additional native executions were not performed by this reviewer. Concurrent Rust work is explicitly outside this review.

## Sealed six-file inventory

- `sdk/fanbox/migration_context_ownership_test.go`
  SHA256 `c885b2e415d1595bd4b9c363578b29d8bcfe9d2c5d5ccd1dae9acb4da7da0f72`, 29,018 bytes
- `crates/pixiv-sdk/tests/fixtures/fanbox-context-ownership.json`
  SHA256 `6c0d721eeb35314058c8f928114a9ea2c6bf76e92d468502fc543fd7651cc705`, 183,768 bytes
- `internal/services/fanbox/protocol/migration_solver_test.go`
  SHA256 `c9a27c01aafbf271dc7a6b33cab8959a7b8e721b9b3d0edc6d41941d7aae1f56`, 46,504 bytes
- `crates/pixiv-sdk/tests/fixtures/fanbox-solver.json`
  SHA256 `45ed3d463ce1159290dbb6686490e754e30d4540a108754571a926b8bba6f382`, 771,775 bytes
- `internal/services/fanbox/protocol/migration_native_profile_test.go`
  SHA256 `50e22883ecdcde2fc673acc4d8d7ab5633d06056e348c41c50b716e6b685dd7e`, 46,819 bytes
- `crates/pixiv-sdk/tests/fixtures/fanbox-native-profile.json`
  SHA256 `df753a715efd28c016a2532fe544e992b00a1fe5ce58b756d807c13b9522dd62`, 164,861 bytes

## Review findings resolved before approval

The initial native pair passed its original executions but had five proof defects. Those successes do not certify the repaired pair. The owner retained copies of the prior source/fixture and original logs, marked that evidence superseded, then recaptured and reran focused and complete shared gates on the new seal.

1. The 31.2-second stall interval previously began before launching the request goroutine. The repaired interval starts after receiving the peer's complete GET HEADERS notification. The raw witness records a post-HEADERS hold of 31,227,619,861 ns and an earlier launch-to-notification interval of 3,086,236 ns. Replay enforces the saved hold lower bound; each new execution independently waits beyond 31.2 seconds before caller cancellation. It does not infer an infinite timeout.
2. Initial client SETTINGS was not acknowledged because the peer delayed its own SETTINGS until first GET HEADERS. The peer now records the pending ACK obligation, sends server SETTINGS as its first outbound frame, and flushes that ACK. No transport or production profile setting changed.
3. Completed idle closure could consume a previously queued closure without establishing preceding liveness or the terminal reason. Both completed connections now survive a 50 ms pre-cleanup observation, then explicit idle cleanup must physically close them. The witness records two liveness checks, and all accepted H2 peers must terminate with exact EOF. EOF remains a comparable field.
4. The sequential scenario previously left elapsed time at zero. It now records actual elapsed time, 106,992,851 ns in the capture. This runtime timing remains a named raw observation.
5. The raw-frame validator previously populated actual RawHex from the expected slot, rather than proving the consumed span. It now derives each byte span from the reader's position and rejects trailing bytes. An independent reviewer parser also checked that each of this sample's 19 recorded raw slots is exactly one complete frame.

## Solver: actual execution and preserved ownership

The fixture contains 124 unique observations in nine families: solution validation 31, expiry 22, control request 27, challenge replay 24, shared waiters 6, cache lifecycle 5, cache wait 4, nil receiver 1 and waiter identity 4. Fifty-eight rows reach owned transports, recording 41 native and 52 solver-control requests. These are observation counts, not top-level test counts or completion percentages.

The harness calls actual frozen validation/expiry functions, standard `http.Client.Do`, `Session.doWithRequest`, `waitForSolver`, `runSolver`, cache lookup/invalidation and waiter unregister paths. Injected RoundTrippers implement only the actual dependency boundary and track requests, reads and closes. They do not return the final business result directly. Solver-control request assertions enforce anonymous homepage POST, no native Cookie/User-Agent/Origin/Referer, no leaked session/business route/native proxy, and exactly one Close for every owned control response.

Important source outcomes remain visible:

- Standard Client.Do supplies an empty zero-length nil body, but rejects nil response or positive-length nil body. Those rejected transport outcomes map to solver-unavailable
- The decoder accepts the first complete JSON despite trailing text/another document or a same-read error; deferred control Close errors are ignored
- Numeric string `"1"` and the maximum signed-int64 expiry are accepted; overflow is rejected. Exact null permits fallback to expires, whitespace-surrounded null does not. Preserve numeric-domain acceptance independently of a Rust date library's range
- Direct control cancellation/deadline transport errors map to unavailable unless the caller context has itself failed. A cancellation-ignoring transport can succeed
- Challenge scanning crosses chunk boundaries and 32 KiB; read/close failures retain their actual precedence. Recovery restarts the original URL once, preserves method/allowlisted headers, and maintains host/redirect cookie restrictions
- Shared solves retain the first caller's values and diagnostic scope while detaching its deadline and individual cancellation. One canceled waiter does not cancel another; zero waiters cancel the shared operation. Late completion cannot replace the active newer call or cache a result with no waiters. An ignoring transport may still emit completion without caching
- Successful waiters remain counted in the detached completed Go call. Returned state is a value copy. Private state/call fields and pointer equality relations are explicit observations

Five-second channel/barrier bounds and solverMu-protected observations establish the selected schedules without fixed-sleep concurrency claims. Read-call boundaries, concrete Go context/error types, nil receiver behavior and time representation remain named Go-only. The fixture explicitly records the execution timezone: local JST with empty TZ. No timestamp/timezone rewriting was used. Default uninjected control transport Proxy:nil remains source-guarded rather than network-executed. Nil caller panic, physical native stream/body ownership and unrestricted concurrent schedules are not inferred from injected transport proof.

## Context: standard API, diagnostic scope and actual SDK/lifecycle paths

The fixture contains 41 unique observations: standard context 18, diagnostics 8, SDK CurrentUser 9 and lifecycle 6. Go uses the standard context.Context API; there is no invented Go sdk/context.go bridge API.

Actual context construction/cancellation/deadline/WithoutCancel/value lookup, typed diagnostic scope operations, `fanbox.Client.CurrentUser` and `lifecycle.Run` execute. The transport observes the actual forwarded context, Done channel and scope. Controlled during-request cancellation waits for actual transport entry and actual context Done, then returns that context's actual Err. A precanceled context can still succeed when the injected transport ignores cancellation; Rust must preserve this observed dependency-boundary outcome rather than add an unconditional precheck.

The scope tests retain typed event fields, parent sink, explicit-module behavior, scope RequestID overwrite, child scope, nil sink/function, absent scope and post-cancellation emission. Lifecycle paths show close-before-final-child-cancel, parent independence, once-owned repeated Close, joined use/close failures and cleanup during panic. Nil boundaries and typed SDK cancellation/deadline error causes are preserved.

Fixed deadline inputs are explicitly UTC and outputs format the actual source values. The one real timer waits for its actual Done channel and compares the declared construction-origin-plus-interval expression exactly; runtime absolute clock and scheduling latency are openly excluded. Each wait is bounded at two seconds. Arbitrary comparable Go keys, interface/value identity, concrete types, panic text and fixed timestamp details are Go-only. Custom contexts, nonstandard causes, all concurrent schedules and a Rust bridge's API identity remain outside this slice.

## Native: genuine peer evidence, projection and limits

This is fresh runtime evidence from the unchanged production `newBrowserTransport` and genuine tls-client/fhttp/uTLS dependencies. The Session path uses actual NewSessionWithOptions/GetJSON with an adapter changing only URL host to the owned localhost endpoint. Original api.fanbox.cc authority/path/query/method/headers are preserved. SNI and certificate validation target localhost; this is not FANBOX-host negotiation.

The six actual bounded child scenarios are sequential headers/idle, response-header stall, caller deadline, untrusted certificate, invalid hostname and expired certificate. Independently checked raw inventory: seven complete ClientHello records, 19 decrypted client H2 frames (8 SETTINGS, 4 WINDOW_UPDATE, 5 HEADERS, 2 RST_STREAM), five completed-request GET streams, three responses and three certificate-handshake failures. Accepted peers negotiate TLS1.3/H2 with localhost SNI. Caller cancellation and deadline each produce physical CANCEL (error code 8), separately from body-close behavior. Two completed GETs reuse one connection; the custom-UA third GET uses a new connection after explicit idle closure.

Actual profile bytes include fixed Chrome_146 extension/cipher order, requested trust-anchor extension 0xca34 with 0000, X25519MLKEM768 and X25519, new ALPS, exact SETTINGS order/values, connection window 15,663,105, pseudo order method/authority/scheme/path and HEADERS priority exclusive/weight 255. Session's actual default User-Agent is Firefox/148; that observed combination must not be replaced by an assumed Chrome UA. Direct-factory negative cases preserve their separately observed Go HTTP2 UA.

Comparable versus raw boundaries are declared: canonicalize only actual GREASE codepoints while retaining positions and fixed payloads; compare required random/session/key-share lengths; preserve allowed ECH length domain and fixed fields; retain raw records/digests, random bytes, complete frame bytes and decoded fields. The owned port and parsed current-time token in certificate diagnostics are narrowly projected. Scheduling-dependent SETTINGS ACK position is excluded while its count and all raw frames remain. Runtime elapsed/peer-notification/hold measurements and generated certificates are retained raw, not compared across random runs. Saved raw records are reparsed, digest checked, frames independently consumed and comparable projection recomputed on replay.

Owned children use synthetic certificates, exactly one child-only trust root and an empty child certificate directory. No OS trust change occurs. Child process timeout is 55 seconds with bounded WaitDelay, socket deadline is 45 seconds, peer drain and result/reset/closure waits are bounded, and owned sockets/listeners are closed and joined before snapshot. The helper's no-mode skip is not counted as evidence: parent contracts really invoke all six children.

Explicit gaps remain: TLS1.2, resumption/PSK, HRR, HTTP1 fallback, proxy runtime, HTTP3, other platforms/trust stores, concurrency, body-close cancellation, concurrent Read/Close, compression/legacy deflate, HEADERS-END_STREAM/nonzero-length and unfinished request-body behavior. The denied multiplex/unfinished HEAD/upload supplement was not retried, reconstructed or used as evidence. Previous official pristine Rust native source recovery is source identity only and cannot substitute for these Go runtime observations or future Rust execution.

## Guards, gates and evidence integrity

Independent byte comparison found all 434 production Go/module paths equal to frozen Go. Official archives and every extracted file in the four relevant Go dependencies agree exactly: fhttp 169, tls-client 53, uTLS 311 and x/net 826 files. Harnesses additionally guard named frozen production files, pinned Go1.27.1 standard-library boundary hashes and dependency identities before capture/replay.

The reviewer checked 207 distinct current/historical log, command, exit, freeze and source artifacts against manifest hashes/lengths, resolving superseded native repository references against the retained original source/fixture copies. Original setup/assembly/vet failures remain identified as nonpasses; no failing log was rewritten or counted as a success.

Final gates on the sealed six-file set:

- Solver: capture, 20 exact replays, 10 focused race replays, gofmt; shared vet/normal/race cover its complete package
- Context: final capture, 10 exact replays, 10 focused race replays, vet and gofmt
- Repaired native: fresh capture, unchanged focused replay/race and gofmt; complete shared vet
- Replacement complete shared normal/race/vet/gofmt: all five packages, no regex exclusions. Each test sweep has 98 top-level passes, 472 named pass events, five passed packages, zero failures and one explicit standalone no-mode native-child skip. Both logs show six real child modes. Wall time: normal 32 s, race 41 s, vet 1 s
- Expected/before/after six-file hashes are identical; owner seals and production pre/post guards all exit zero

The earlier shared sweep is explicitly pre-native-repair evidence and does not replace the final sweep. No Cargo command was run by this review or by the three Go-contract owners. Rust RED/GREEN/full-script checks, actual native dependency/runtime reconstruction, public wiring and migration ledger completion remain the parent's later gates.

## Final manifest seals

- `/tmp/fanbox-solver-recovery/manifest.json`
  SHA256 `cfbf052f207460dd1fd8ae115db20d50fe7d0390503224a755ca6256d878678a`
- `/tmp/fanbox-context-ownership-recovery/manifest.json`
  SHA256 `4ef520493a7ad43eb53a68a40953b4aa1fd330cb4d8f69f09cb119700ad8ff8d`
- `/tmp/fanbox-native-profile-final/manifest.json`
  SHA256 `3fed67fb77fe3de1c870c7b30a3573a36468787038e67301c56017ec751922d7`
- `/tmp/fanbox-shared-go-final/manifest.json`
  SHA256 `25961e4d21475f8d684db475b277744fc5ab2ebc97d35c6611278a3b1a409725`

Original pre-repair source/fixture and failure/success logs remain under `/tmp/fanbox-native-profile-recovery`; original shared logs remain under `/tmp/fanbox-shared-go-recovery`. Final repaired execution logs and exits remain under `/tmp/fanbox-native-profile-final` and `/tmp/fanbox-shared-go-final`.

Reviewed and sealed at 2026-10-10T06:22:14.430383+00:00.

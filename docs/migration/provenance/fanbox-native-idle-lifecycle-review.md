# FANBOX native idle/admission/authority source review

Reviewed 2026-10-10 UTC against contracts foundation `51dcb2c4f6e196372f0f022170d8b53f1f5eff3a`. Final reviewed source snapshot taken at 07:02 UTC after the implementation worker's two review fixes. This is a source-only approval of the ten-file idle/admission/authority slice, not runtime equivalence or final acceptance.

## Result

No remaining source blocker found in the reviewed slice. Two concrete issues were reported immediately and fixed by the owning worker; both fixes were independently re-read:

1. **Logical Host was lost on recovered-unsent H2 to H1 retry.** An H2 attempt removes the ordinary Host header. The retry restores the physical URL but previously retained only the parsed authority extension, so a subsequent H1 connection generated its Host from the physical URL. `RequestAuthority` now retains both the validated `Authority` and original `HeaderValue`; the H1 branch restores that logical Host before optional automatic Host generation, including when `set_host` is false. The H2 branch continues to apply the override only to the wire URI after physical connection selection and removes the ordinary Host header. Source: `wreq/src/client/layer/client.rs:136,283,349,756`.
2. **Fresh H2 selection could fail instead of reconnecting if explicitly retired before admission.** `Pool::pooled` publishes a shared sender before the first caller reaches dispatch admission. Explicit cleanup on another thread can retire that idle connection in the gap. The request is wholly unsent and recovered, but the original retry gate refused fresh connections. The worker added a distinct `Retryable.idle_retired` field from the real H2 control state and permits this recovered-unsent case while retaining `retry_canceled_requests`. It does not retry sent requests or broaden ordinary fresh network-error retries. Sources: `pool.rs:218-259`; `client/layer/client.rs:237-248,360-369,824`; `wreq-proto/src/conn/http2.rs:115`.

These fixes are production semantics, not a test flag or a fixture-specific exception. No supplemental runtime probe was used or requested.

## Admission and atomic retirement

- `IdleCloseHandle` controls the actual `http2` Streams `Inner`, using the same mutex that protects raw stream creation. `try_admit` increments pending admissions before entering the asynchronous dispatch queue, and rejects retirement or an existing connection error. `close_if_idle` checks pending admissions and all protocol-open streams and sets retirement while holding that mutex. Raw `send_request` and `poll_pending_open` reject a retired connection under the same lock. There is no count-only check followed by an unprotected close.
- `Admission` is an Arc-backed counted reservation, so cloning an extension does not multiply the count and only the last drop releases it. The wreq-proto dispatcher removes the extension into a local before request conversion and retains it until native `send_request` has produced a real stream. The local also drops on cancellation or early conversion/error exits. Recovered-unsent dispatch errors explicitly remove the reservation before returning the request to the retry layer.
- The request extensions are cleared before the raw stream mutex is taken. The dispatcher extracts `Admission` before that call. Consequently, the added reservation's drop does not recursively lock `Inner` while `send_request` owns it. Existing stream/send-buffer order remains `Inner -> SendBuffer`; neither reservation/control operation takes SendBuffer or Pool locks from inside Inner.
- A busy decision returns `None` without setting retirement, registering a pending close, or saving a close-when-idle intent. A later completion or cancellation does not retroactively activate an earlier busy cleanup.

Principal sources: `http2/src/proto/streams/streams.rs:153-218,337-370,1141-1164`; `wreq-proto/src/conn/http2.rs:138-178`; `wreq-proto/src/proto/http2/client.rs:583-634`.

## Stream lifetime, cancellation, and shutdown acknowledgement

- Retirement uses actual protocol-open state rather than receipt of response headers or Content-Length. A header-stalled GET stays open and therefore busy. Closed canceled/reset tombstones do not keep an otherwise idle connection artificially busy.
- Native cancellation schedules `RST_STREAM CANCEL` through the existing stream/reference and prioritized-write machinery. Scheduled resets are protocol-closed but remain queued. The new `IdleClosing` state drives `streams.poll_complete` before codec shutdown, so retirement can drain an already queued CANCEL instead of dropping the driver and losing it. The existing prioritized completion flushes the codec, and framed shutdown also flushes before `poll_shutdown`.
- Idle closure has its own state, skips sender-drop auto-GOAWAY, and does not invoke graceful/go-away-now closure. The driver rechecks the retired state before open-frame processing and after its pending write-completion pass. A queued ordinary auto-close decision is not used by the idle shutdown path.
- Successful acknowledgement is recorded only after native write flush and IO shutdown return ready. Errors during either stage are stored and wake the close waiters. Premature driver destruction returns a concrete failure through the same acknowledgement state. Existing state EOF handling does not replace an already protocol-closed stream's terminal state with a false broken-pipe failure.
- `wreq::Client::close_idle_connections` is a normal function, not an async function: it synchronously invokes the shared pool control and makes retirement/Busy decisions at call time. The returned future only awaits acknowledgements. Removing a retired entry/drop of its sender starts the operation independently of that future, so dropping it does not undo the action. The SDK's synchronous RawTransport mapping calls and drops this already-started future; the native transport also offers its explicit awaitable mapping.

Principal sources: `http2/src/proto/connection.rs:251-257,285-338,665-683`; `http2/src/proto/streams/streams.rs:172-218,1182-1196,1710-1781`; existing `streams/send.rs:307-323`, `streams/prioritize.rs:503-563`, `streams/state.rs:304-316,342-356,419-421`, `codec/framed_write.rs:173-179`; `wreq/src/client.rs:418-423`; `crates/pixiv-sdk/src/fanbox/native.rs:42-44,116-118`.

## Pool removal, concurrency, and reconnect

- The pool scans and makes per-entry retirement decisions synchronously under its mutex, removes only entries that actually returned a close acknowledgement, and removes only keys whose vectors became empty in that same locked pass. Busy H2 shared entries remain. It never awaits while holding the pool mutex.
- Because decision and entry removal occur in one locked scan, a newer connection for the same key cannot be inserted between the decision and key removal. An already checked-out clone encounters the real retired admission state; if wholly unsent, the configured retry can reconnect. The fresh-selection gap is covered by the second fix above.
- Awaiting uses `join_all`, so all selected shutdown acknowledgements are awaited before a selected error is returned. Dropping this await aggregate still leaves the synchronous retirements/removals in place.
- H1 entries are uniquely pooled idle senders. Removing them drops the sender immediately; the cloned watch receiver observes the actual background driver completion. Its future does not itself trigger or cancel shutdown. Existing H1 return-to-pool logic waits for sender readiness where necessary, so an active unique sender is not scanned as an idle shared H2 entry.
- No new reverse `Stream Inner -> Pool` lock path was found; connection tasks and reservation drops do not manipulate the pool while holding stream state. The reviewed retry returns requests only when the dependency actually recovered an unsent message.

Principal sources: `wreq/src/client/layer/client/pool.rs:151-179,218-259,282-326,562-586`; `wreq/src/client/layer/client.rs:620-666,900-926`; `wreq-proto/src/dispatch.rs` envelope/callback recovery and cancellation paths.

## Authority validation and physical destination

`logical_authority` accepts exactly one Host value, parses general HTTP authority syntax, rejects user-info and empty hosts, validates bracketed IPv6, and validates a supplied decimal port as u16. It has no localhost/FANBOX hardcode or test-only branch. The connection descriptor and pool key are created from the normalized physical URI first; the H2 wire authority rewrite happens only after connection selection. Physical URI, path/query, pool identity, TLS SNI and certificate target remain independent. The original URI is retained for retry/error policy. Explicit Host is preserved/restored for H1 as described in the first fix.

Principal sources: `wreq/src/client/layer/client.rs:182-223,267-307,349-356,756-790`.

## Reference and evidence limits

Read AGENTS.md, migration strategy, `/tmp/pixiv-fanbox-native-next-integration-plan.md`, actual frozen fhttp H2 `closeIfIdle`, and the approved fixture/test inputs. Actual fhttp `http2/transport.go:1004-1023` checks `len(cc.streams)` under `cc.mu`, returns unchanged when busy, sets closed while idle, and directly closes the transport without GOAWAY. Its `shouldRetryRequest`/`canRetryError` at 632-677 permits `errClientConnUnusable` for an untouched fresh request, supporting the second fix.

Sealed input hashes still match:

- Go native witness `internal/services/fanbox/protocol/migration_native_profile_test.go`: `50e22883ecdcde2fc673acc4d8d7ab5633d06056e348c41c50b716e6b685dd7e`
- Six-row fixture `crates/pixiv-sdk/tests/fixtures/fanbox-native-profile.json`: `df753a715efd28c016a2532fe544e992b00a1fe5ce58b756d807c13b9522dd62`

The parent must still run the serialized compile, the unchanged approved six actual synthetic-child rows, and final gates. In particular, source reasoning does not witness physical peer EOF, no emitted GOAWAY, CANCEL wire order, 31.2-second header-stall survival, deadline behavior, full TLS/H2 fingerprint parity or certificate-alert parity. Concurrency remains source-reviewed only. Broader H1/proxy/platform trust, resumption, compression, body ownership and other deferred contracts are not established here. The denied supplemental multiplex/unfinished HEAD/upload operation was not executed, reconstructed, retried, or used as runtime evidence. This reviewer ran no Cargo, Go/native program, test, network request or external peer and made no repository/dependency changes. The sole created artifact is this report.

## Reviewed ten-file snapshot

Paths below are under `third_party/rust/fanbox/`:

| File | SHA256 |
| --- | --- |
| `wreq-6.0.0-rc.31/src/client.rs` | `d95e7d54c5907eaf97c5ca03d669c9c4b4cd7cbd738baa49f0bdd677e50d0377` |
| `wreq-6.0.0-rc.31/src/client/layer/client.rs` | `9bbf90c5bcbe84e1183a2db55131872d04b1d1a5817000bd52219621eecb0f4e` |
| `wreq-6.0.0-rc.31/src/client/layer/client/pool.rs` | `24fc6a07ea0bafdebb7f02de29f2e6d1497a4c10baa523a19e677a8bbdd6e940` |
| `wreq-proto-0.2.5/src/conn/http2.rs` | `74b743eca75a3cbe96d1f14f6e7f23fb690b9b309894b6a33add1b7fd639423a` |
| `wreq-proto-0.2.5/src/proto/http2/client.rs` | `2fc2e695de94e8c8d40802dd00ab8f0ac03af09e89f7cf895ef403527ea5ba1b` |
| `http2-0.5.20/src/client.rs` | `8e92b1b6107a87d24d695f23cd61ffa07d3ca5b223eb296015ed931482a8996d` |
| `http2-0.5.20/src/proto/connection.rs` | `d4255aea194b2fe5c109513aff710e3e3b803f64061624e0aa68162c79fd205e` |
| `http2-0.5.20/src/proto/mod.rs` | `5a7b60c43cbdd23ce1c7371876bc43d0ba43cdff689f0a557e87c989b734b851` |
| `http2-0.5.20/src/proto/streams/mod.rs` | `3005ca522f21ac0f457ac2f69ee108c5d34eb4c3a6222532bcb2b1dc7df1aff6` |
| `http2-0.5.20/src/proto/streams/streams.rs` | `69612f00f61e10fd271be0a861c728892eaa9012cdf5e528d2968c53d3881d61` |

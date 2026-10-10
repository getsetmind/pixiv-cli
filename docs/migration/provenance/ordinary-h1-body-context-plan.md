# Future ordinary HTTP/1 body lease and caller Context plan

Date: 2026-10-10. Contracts base: `b093eae0df1d12dd6a0d5e12127bfcd3e2c167d7`.

This is a source-audit architecture and fixture plan for the next integration after the bounded ordinary HTTP/1 pool activation. It implements nothing and claims no body-lease, caller-Context, cancellation, bounded-drain, gzip or platform execution result. The audit read source and existing evidence only; it ran no Cargo command, test, replay or protocol probe. The previously denied HTTP/2/multiplex/upload/unfinished-HEAD supplemental probe was not retried.

## Source boundary

Line references below refer to the inspected official Go 1.27.1 `src/net/http` source, the official cached Hyper 1.12.0 source, and the current imported ordinary reqwest 0.13.5/hyper-util 0.1.21 candidate. They are source observations, not execution witnesses. The exact upstream archive identities and original payloads are recorded in [ordinary-transport-official-sources.json](ordinary-transport-official-sources.json); the current ordinary imports and ordered patches are recorded in [ordinary-h1-patch.md](ordinary-h1-patch.md).

The existing [nine-row native H1 fixture](../../../crates/pixiv-sdk/tests/fixtures/client-idle-native-h1.json) is produced by `sdk/pixiv/migration_idle_native_test.go`. Its source/evidence distinguish public results, request connection IDs, private Go idle-return traces and independently observed peer shutdown. The current bounded Rust activation maps five rows; four explicit-Close/cancellation rows remain unmapped. The separate [request Context fixture](../../../crates/pixiv-sdk/tests/fixtures/request-context.json), produced by `sdk/pixiv/migration_request_context_test.go`, has 20 non-nil Context cases and four explicitly Go-only nil-Context cases. Its injected transport behavior is not a native cancellation result.

## Recommendation

Introduce a genuine per-response HTTP/1 lease that retains the actual hyper-util `Pooled<PoolClient<B>, PoolKey>` sender, plus a normal raw read/close boundary beneath decompression. Keep sender readiness as a necessary native transport condition, but stop treating it alone as caller body completion.

Pool exclusion alone can be implemented by retaining `Pooled` in hyper-util. Preserving Go's readLoop pause and completion timing more generally calls for a small, opt-in Hyper HTTP/1 raw-reader/control seam and completion gate. This plan recommends that seam rather than activating a decoded-EOF wrapper and claiming the full contract.

Neither an outer SDK request counter nor a body-object timer is a native body lease. No test-only production API is proposed. API names and exact signatures must be settled during implementation and source review; the required semantics below are the design boundary, not a compiled ABI.

## Go body completion and idle return

Official Go `transport.go:2524-2558` returns an eligible bodyless response's connection before delivering the response. Preserve this exception rather than indiscriminately holding a body lease until a caller's zero-byte read. This is a source requirement and does not authorize another unfinished-HEAD probe.

For responses with a body:

- `transport.go:2562-2583` installs `bodyEOFSignal` around the raw response body. The first terminal Read outcome is reported once. EOF waits for the readLoop acknowledgement; a non-EOF error may instead expose the native connection's cancellation cause
- `transport.go:2604-2617` permits reuse only when the connection is otherwise alive, the raw body reached EOF, physical transport EOF was not seen, request writing succeeded and `tryPutIdleConn` accepts or delivers the sender
- The EOF acknowledgement follows the actual idle-return attempt, including rejection. It does not promise that physical peer shutdown has already completed
- `transport.go:2618-2625` observes caller cancellation independently of a subsequent caller Read. A returned unread body therefore needs a live native per-request cancellation observer while its lease is unfinished
- `transport.go:3227-3271` makes Read errors sticky, rejects reads after Close, makes Close repeatable and runs the terminal callback at most once
- `transfer.go:882-891` deliberately returns positive bytes together with `io.EOF` when a fixed-length Read consumes the last bytes. Pool completion precedes returning that final positive Read even if the caller makes no further Read or Close
- `internal/chunked.go:98-155` can likewise return data and EOF when buffered framing permits it. `transfer.go:859-872` validates trailers before accepting successful framing EOF

Pool ordering remains at the real pool boundary. `transport.go:1129-1220` offers returning HTTP/1 senders to eligible existing waiters before checking the future-idle-close flag. `CloseIdleConnections` atomically removes current idle entries and sets that flag at `transport.go:949-964`. A fresh acquisition and a lease return must continue to resolve against the actual pool mutex, not a headers-completion or wire-completion marker.

### Correct Go 1.27 early-Close eligibility

Early Close is not an unconditional discard:

- `transport.go:2420-2440` drains at most 256 KiB+1 bytes and waits at most 50 ms; reaching EOF below the CopyN target permits successful drainage
- `transport.go:2605` tests the response's total `ContentLength`, not the remaining unread length
- The eligibility value is the response `ContentLength` after automatic gzip rewriting. `transport.go:2590` sets that value to -1, so an automatically decoded gzip response can be drain-eligible even when its original compressed wire Content-Length exceeds 256 KiB
- `transport.go:2608` nevertheless drains `body.body`, the raw framed entity reader, rather than user-decompressed output
- `transport.go:2607` unblocks the explicit Close before the background drain completes
- Successful bounded drainage can permit native reuse. Timeout, excess bytes or a raw read failure cannot establish reusable EOF

The sealed `resource-early-close-discards-connection` row deliberately withholds the tail: `migration_idle_native_test.go:538-558` observes peer shutdown before releasing it. This is evidence for that schedule, not an all-early-Close discard rule. The 50 ms bound belongs to a real Go drainage operation; it must not be repurposed as an artificial lifetime timer for buffered body objects.

## Current premature-return path

The current dependency path can finish transport framing before the resource caller uses the buffered bytes:

1. Hyper `src/proto/h1/conn.rs:365-429` updates reading state when its framing decoder reaches EOF, including while returning a final positive data frame
2. Hyper `src/proto/h1/dispatch.rs:247-295` deposits that frame in the body channel and subsequently drops the sender
3. Hyper `src/body/chan.rs` retains one buffered item independently of sender closure; its `Receiver::poll_next` transfers that item separately from terminal channel state
4. Hyper `src/body/incoming.rs:168-176` decrements fixed-length remainder when it hands out an entire data frame; `:233-243` reports `is_end_stream` from this remainder. Neither establishes that a downstream reader consumed the entire frame
5. Imported hyper-util `src/client/legacy/client.rs:356-370` drops `Pooled` when ready or runs the original readiness-only `on_idle` future
6. SDK `crates/pixiv-sdk/src/resource_transport.rs:132-135` creates `StreamReader` over reqwest `bytes_stream`
7. Reqwest `src/async_impl/response.rs:351-352` exposes `BodyDataStream`, and `src/async_impl/client.rs:1042-1057` installs tower-http decompression before response boxing
8. Tower-http `src/compression_utils.rs:157-173` constructs another `StreamReader` beneath its decompressor. Tokio-util `src/io/stream_reader.rs:240-303` retains a partial chunk and advances it only by bytes copied into the caller's buffer

Keep three events distinct:

1. Framing decoder reached raw transport/body completion
2. Raw reader delivered terminal bytes/EOF to its consumer
3. User consumed all decompressed output

Go's `bodyEOFSignal` leases at event 2. Events 1 and 3 are both unsuitable general substitutes.

## Raw gzip ownership

Go puts `bodyEOFSignal` beneath `gzipReader`: `transport.go:2585-2591` and `:3280-3379`. Raw EOF can complete the native lease while decompressed output remains unread. A later decoder error does not itself establish that a raw transport whose framing already completed is unusable.

Do not hold the native sender until user-decompressed EOF. Conversely, placing a raw frame into the decompressor's internal partial-chunk buffer is not enough to prove byte-level raw consumption. A new read path must acknowledge consumption at its actual raw-reader boundary.

Preserve the Go negotiation distinction at `transport.go:2994-3016`: automatically request gzip only when the caller supplied no Accept-Encoding, supplied no Range and the method is not HEAD; automatic gzip decoding applies only when the transport requested it. Go then removes Content-Encoding/Content-Length and exposes unknown decoded length. Selecting the same gzip algorithm alone does not prove identical lazy-read or buffering behavior.

## Proposed production patch sequence

### 1. Extend Go behavioral evidence first

Add the separate behaviors below to the existing producer/evidence workflow without rewriting or weakening the nine sealed expectations. Keep native effects, public results and private idle-return witnesses separate. Only owned synthetic HTTP/1 fixture work is planned; none was executed by this audit.

### 2. Add the opt-in Hyper raw-reader/control seam

Candidate source files:

- `hyper/src/body/incoming.rs`, `chan.rs` and `mod.rs`
- `hyper/src/proto/h1/dispatch.rs` and `conn.rs`
- `hyper/src/client/conn/http1.rs` for normal configuration
- `hyper/src/proto/h1/decode.rs` only if terminal framing information cannot be carried without changing its result representation

The normal abstraction should use Hyper runtime read traits rather than importing SDK or Tokio types. It needs a partial-byte cursor, validated terminal framing/error information, a once-only completion handshake, a cloneable cancellation control that wakes the actual driver and distinct normal EOF, explicit early Close, caller cancellation and abandonment outcomes.

An opt-in completion gate must prevent subsequent request/read-keepalive transitions until the lease permits them. It must not block the response body's own reads or bounded drainage. Preserve bodyless response handling, existing non-opt-in behavior and upgrade ownership.

This is strictly an H1 abstraction. HTTP/2 stream idleness must continue to depend on real stream state, including pending outbound frames and admissions; a body-EOF proxy is insufficient.

### 3. Retain actual Pooled ownership in hyper-util

Candidate source files are `hyper-util/src/client/legacy/client.rs`, `pool.rs` and a dedicated normal lease module if useful.

The per-response native coordinator must actually own `Pooled<PoolClient<B>, PoolKey>`, including its existing `strong_pool` active retention. Replace the H1 readiness-only return branch with coordination requiring both body completion and genuine sender readiness. Normal completion then performs the existing return and acknowledges the reader after that operation. Preserve waiter-first delivery, close-flag reset, pool limits and actual idle-return timestamp/deadline behavior.

Add an explicit consume/discard operation for terminal failure: merely dropping an already-ready `Pooled` can otherwise reinsert it. A native terminal control must wake and retire the real driver; observer death and physical driver completion remain separate from logical lease retirement.

Avoid a completion deadlock. Raw EOF must first release Hyper's completion gate, allowing sender readiness; hyper-util can then return the sender and acknowledge the final reader operation. Hyper cannot wait for pool insertion before allowing readiness.

### 4. Carry ownership through reqwest before decoding and boxing

Candidate source files are `reqwest/src/async_impl/client.rs`, `body.rs`, `response.rs` and normal configuration plumbing.

Provide a normal production reader/close path that retains the raw reader and native lease below decoding and timeout wrappers. Carry it through redirects, retry-response disposal and response conversions. The SDK cannot obtain exact partial-byte acknowledgements from its current `bytes_stream` adapter.

Prefer a reqwest leased-reader path beneath decoding over another tower-http source fork when feasible. Address Go's lazy gzip, raw-error and post-rewrite ContentLength behavior explicitly. Preserve upstream defaults and the separate FANBOX dependency family. Existing custom reqwest clients that did not opt into the new policy must not silently be relabeled as equivalent native body-lease configurations.

If official Hyper is imported for this seam, extend archive/source/license receipts, workspace dependency exclusion/patch configuration, ordered zero-fuzz reconstruction and unchanged upstream inventories. Do not edit cached registry source. No import or dependency activation is performed by this plan.

### 5. Connect the ordinary SDK body and Context ports

- `transport.rs`: carry caller Context to native request execution and response-body collection
- `resource_transport.rs`: replace the current StreamReader ownership boundary with the normal read/close body
- `resource_io.rs`: expose repeatable explicit Close through a real production body abstraction
- `pacing.rs` and operation/retry boundaries: propagate the current operation's Context according to the Go contracts

The Context fixture explicitly permits an injected transport to return success under an already canceled or expired caller. Do not add an unconditional SDK cancellation precheck or response discard. Native cancellation belongs at the native transport boundary; injected dependencies receive the caller Context and determine whether they honor it. Preserve values, deadline relationships, diagnostic scope and A/B/future-C isolation without treating Go pointer/nil representation as a Rust promise.

Go client timeout can derive a shorter per-request Context. Closing its returned body cancels that derived Context only, without canceling the original caller. Relevant native/client source is `transport.go:665-696` and `client.go:351-398,987-1015`; the sealed timeout rows use actual Go client wrapping around injected ports, not native timeout expiry. The existing Rust Context implementation is available, but ordinary Request/ResourceReadRequest and native body ports are still pending integration.

## Required source-race review

- For a final fixed-length read, stage terminal bytes until actual native return acknowledgement and then deliver the positive read. Tokio AsyncRead cannot return positive bytes and EOF together, but the pool effect can precede returning those bytes
- Do not fill ReadBuf and then return Pending while waiting for acknowledgement
- If raw EOF already occurred, competing cancellation may retire the connection without replacing that successful EOF result. Go consults its cancellation cause for non-EOF errors, not for its EOF callback
- Detach a completed request's cancellation observer before that sender can serve a future request; Close after raw EOF must not abort a reused sender
- Early Close transfers the remaining raw reader to genuine bounded drainage. Close acknowledgement and drainage/idle completion are separate events
- Never invoke native lease callbacks while holding the body-channel mutex
- Cancellation and abandonment must wake the actual driver without depending on another caller body poll or peer event
- Driver death before/after return must release active/idle guards without retaining or reinserting a dead sender
- Keep completion once-only across Read, Close, cancellation, driver failure and Drop; avoid strong cycles between driver, pool and response state
- Keep the captured Go cancellation-versus-body-return race allowance. A universally deterministic private idle callback is not supported by the raw Go traces

## Go-first fixture additions, planned only

- Fully sent fixed-length resource stays unread while a same-host overlapping request requires another connection
- Partially consuming a fully buffered final frame does not return its sender
- Exact final positive Read, without another EOF read or Close, establishes native idle return
- CloseIdleConnections during buffered-but-unconsumed ownership keeps the body usable and rejects its later return unless a new acquisition resets the flag
- Small immediately drainable early Close reuses the connection, with an actual Go idle-return barrier
- Known total length above 256 KiB does not drain even when only a small remainder is unread
- Unknown-length and auto-gzip bounded drainage, including 256 KiB versus 256 KiB+1 and withheld-tail timeout
- Valid and invalid chunked trailers around terminal consumption
- Raw gzip EOF with unread decompressed output, preserving transport reuse separately from later decoded results/errors
- Returned-body cancellation without another body poll closes the native peer and leaves B and future C usable
- Before/after terminal EOF cancellation schedules, retaining observed race alternatives honestly

The existing four remaining nine-row mappings stay unchanged: withheld-body explicit Close, active-header cancellation, active-body cancellation and returned-resource cancellation. Tests must use normal SDK/dependency APIs, independently observe actual peer shutdown and exclude server cleanup from shutdown evidence. No test-only source port or artificial body-lifetime timer substitutes for these observations.

## Explicit limits and stopping boundary

Future/request/body Drop needs a separately labeled Rust policy. Go's Body.Close is explicit; these fixtures do not equate garbage collection with Rust Drop. Safe abandonment can retire the genuine native lease, but it is not yet Go-equivalence evidence.

No proposed seam has been implemented or compiled in this audit. Exact gzip buffering, arbitrary concurrent Read/Close, panic/runtime shutdown, native timeout expiry, raw Go identities, HTTP/2 real-stream idle, native platforms and live provider behavior remain unverified. The 20 non-nil Context cases and retained Login integration remain additional runtime work. The current pool activation's gate does not establish any of these future behaviors. Implementation, new Go evidence, focused RED/GREEN, scoped review and the unchanged full Rust script must occur in the parent-controlled sequence before promoting coverage.

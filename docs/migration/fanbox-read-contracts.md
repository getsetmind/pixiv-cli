# FANBOX saved-session read contracts

This is a contracts-only durable checkpoint from published `7078b729cc4dd48ee2b28f8eedcb758bacbc3a0f`, against frozen Go `4b4426487ef18bed276706daec385e0d0a6979f9`. It adds genuine synthetic Go observations before the connected Rust workflow is implemented. Fixture counts do not establish Rust feature or platform completion.

## Boundaries captured

- Public SDK content: 416 rows/25 families, 520 calls, 488 injected API/identity requests and 15 owned anonymous HTTP/1 solver POSTs. All eight routes, typed DTO/runtime fields, five cursor owners, source resolution, safe errors and read/close/context/replay ordering use actual public SDK calls. [Go provenance](provenance/fanbox-content-reads-go.json)
- Public SDK resources: 243 rows/20 families, generated opaque refs, cached versus fresh reopening and locator rotation, validation/request/header/status policy, binary bodies and partial read/error/EOF/close/context ownership. HEAD is an injected in-memory request; supplied HEAD/204/304 bodies remain forwarded. [Go provenance](provenance/fanbox-resource-reads-go.json)
- CLI: 172 rows through actual saved SQLite/config/account selection, facade and SDK, covering all six content leaves, text/JSON/NDJSON, list plans/source classification/stdin, proxy presence, writer/fetch/body/lease failure and cancellation. Root parser/startup failures are separately owned children. [Go provenance](provenance/fanbox-cli-content-reads-go.json)
- MCP: 126 rows/159 actual tool calls, eleven actual schemas and three owned command-leaf→production RunStdio schedules. Selected-account leases, real SDK content/resource responses, pagination/partial discard, fresh resource reopening, cancellation/reuse and EOF drainage are actual observations. [Go provenance](provenance/fanbox-mcp-read-tools-go.json) records the final cleanup-only helper seal.

Every producer captures, replays three times, runs race/vet and verifies empty gofmt. All 434 Go production/module paths and existing saved-account321, media-body80, help22 and prior SDK/native/parser fixtures stay byte-exact. Inputs are synthetic and fallible dependency boundaries are real; no fabricated returned DTO substitutes for an endpoint call.

## Material contracts

CLI single-post JSON is a summary, list JSON is full PostDTO, and MCP has its own assets projection. Text generally ignores writer errors. Single-entity JSON accepts nil-error short writes while list JSON returns short write; NDJSON EPIPE remains success even when joined with lease-close failure. FANBOX does not inherit Pixiv automatic pipe-to-NDJSON.

Fresh post_file reopening can select an equal-ID image locator; an existing cache retains its original file locator. Reopening uses endpoint metadata rather than SDK Post timestamp mapping, so an invalid publish time need not prevent it. Exact EOF remains EOF after cancellation; wrapped EOF becomes a safe read failure. Successful injected transport/body results can survive pre-canceled context; failures prefer the actual context error as captured.

MCP SDK argument normalization rounds large integers through float64 before strict integer decoding. The max-int64 literal is rejected after rounding; a separate exactly representable page/limit pair witnesses logical-offset overflow. Explicit stdio content cancellation produces an ordinary isError/zero result, followed by successful same-session reuse; the in-memory ClientSession caller instead returns context canceled with null result. EOF drains active owned work after the synthetic 200ms request release, closes its body/lease, and emits no late result; that schedule does not prove pending-request cancellation. Independent-account result ordering is canonicalized by sorting; diagnostic durations are projected to nonnegative values, not claimed as exact physical latency. Request and trace order remain retained.

Go database.Open rewrites user_version even on reads. CLI evidence preserves actual physical before/after hashes and separately compares account/schema rows; it does not invent byte-read-only persistence. Full successful cmd/pixiv bootstrap/native/platform runtime remains unproved. Go typed nil/interface/panic, driver/physical byte layout and unrestricted schedules remain separately identified by each fixture.

## Next connected implementation

Implement Session API/media routing, exact content/DTO/cursor/reference behavior, native-only decoded bodies and a FANBOX-specific saved account facade, then connect all six CLI leaves and eleven MCP tools. Reuse the existing actual media-body80 and public RawBody/RawRead boundaries. Standard injected transport remains undecoded. Repeated body Close forwarding is distinct from once-owned facade/lease cleanup.

Private production decoder-source replay can prove in-memory decoder/error/ownership behavior without a test-only production API. Public SDK injection and the unchanged six approved native factory scenarios are separate evidence; neither source replay nor those six proves compressed native wire behavior, HEAD/HTTP1/resumption/HRR/concurrent read-close or other-platform parity. No denied supplemental multiplex/HEAD/upload action is retried. Download/save/auth/browser workflows and all pre-existing transport/ownership/pacing/signal/Windows repeated-cleanup/release-distribution debts remain pending.

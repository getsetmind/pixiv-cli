# Ordinary HTTP/1 actual-pool ownership patch

This patch imports the exact official reqwest 0.13.5 and hyper-util 0.1.21 archive payloads from the sealed [official source receipt](ordinary-transport-official-sources.json). The original 47 reqwest and 62 hyper-util files are preserved byte-for-byte in `ordinary-h1-original/`. Every original and patched file, upstream VCS identity, archive checksum, archive file mode, license, and ordered patch checksum is listed in [ordinary-h1-patch.json](ordinary-h1-patch.json).

The ordinary dependency family is separate from the existing FANBOX family. No original registry source is changed, and all imported license files remain unchanged. HTTP/2 stays disabled at activation; this patch provides no HTTP/2 retirement claim and runs no HTTP/2, multiplex, upload, or unfinished-HEAD probe.

## Public configuration and control

The async normal reqwest client exposes `Client::close_idle_connections()` and `ClientBuilder::pool_idle_retention(bool)` plus `ClientBuilder::pool_max_idle_connections(usize)`. The actual HyperClient is cloned before the existing retry, redirect, cookies, and decompression service layering and retained inside ClientRef. Calling idle close reaches the actual native pool; no marker, standalone wrapper state, or client-rebuild approximation substitutes for that pool.

The corresponding hyper-util builder/client methods expose the same legitimate ownership policy and HTTP/1 idle control. Library defaults remain upstream defaults (retention off, unlimited per-host/global counts) unless configured. The SDK-owned ordinary route must explicitly select retention on, per-host 2, global 100, and the normal 90-second idle timeout. Parent-owned activation and SDK tests establish that connection separately.

## Ownership and transitions

- The original pool mutex atomically removes current idle HTTP/1 senders and sets future-idle rejection. Pending waiters are kept
- Returning senders are first offered to waiters. Only the remaining unused HTTP/1 sender is rejected by the future-idle flag
- A new checkout creation clears the flag once. Polling an already-created checkout does not clear it
- Active unique pool leases hold a strong reference to the actual pool when retention is enabled. The original readiness-based body-return future determines when they become idle
- Every actual HTTP/1 idle insertion receives a fresh entry ID and cancellation sender. Its independent task owns the actual pool and sleeps until the absolute idle-return timestamp plus timeout. There is no outer-client keepalive timer or coarse periodic scanner for these retained H1 entries
- Checkout removes the old cancellation sender. A later return gets a fresh entry ID/deadline. Explicit close, global eviction, and dead-driver removal likewise cancel only the affected entry tasks
- With no idle timeout, a retained idle task waits on its cancellation channel. A pool timer is required for deadlines, matching hyper-util's existing timeout requirement; reqwest supplies its normal Tokio pool timer
- The HTTP/1 driver carries only a weak pool observer. Completion marks the connection dead before locking the pool and removes idle entries with that connection ID. The driver does not retain the pool independently
- The per-host bound rejects an excess returning idle sender. The global bound evicts the oldest actual HTTP/1 idle return across hosts

Sender retirement and physical peer shutdown are distinct. The patch leaves the official Hyper driver/shutdown and reqwest/Rustls request, response, retry, redirect, decompression, and error pipelines intact. Tests observe local peer EOF separately from request connection IDs and response bytes.

## Verification

`ordinary-h1-reconstruct.py` independently verifies original bytes against the sealed receipt, optionally verifies exact cached `.crate` checksums and payloads, reconstructs both imports by applying the ordered patches with zero fuzz, compares every patched file, and verifies unchanged license hashes. Run:

```
python3 docs/migration/provenance/ordinary-h1-reconstruct.py --archive-cache <official-cargo-archive-cache>
```

All new behavior tests, the owned gated DNS resolver, and synthetic peers are in `crates/pixiv-sdk/tests/ordinary_h1_pool.rs`. They call normal reqwest public APIs and use bounded owned loopback HTTP/1 peers. There are no new test-only production APIs, runtime flags, mocks, or helpers in workspace `src/`. Existing upstream tests are preserved as original source payloads; no new tests are added to the imported production files.

The parent recorded a genuine unpatched-upstream missing-native-API RED, then activated only these two exact versions and serialized the first GREEN: all 12 public reqwest tests passed (6.71 seconds compilation and 0.87 seconds tests). Broader SDK Go-fixture comparisons, full Rust script, platform status, and publication provenance are parent-owned and are not implied by this dependency-level GREEN. The strengthened final test file has 13 cases. EOF observations are bounded to one second, shorter than the explicit-close cases’ idle TTL. A real parked checkout is established through normal public custom DNS resolution for an owned loopback dial, then idle close must preserve and serve that waiter without its old polling resetting the future-idle flag. The background dial is released and its physical shutdown observed too. The fresh parent-serialized final run passed all 13 reqwest cases in 3.42 seconds and the separate seven SDK TLS cases in 3.90 seconds; [the raw combined log](ordinary-h1-native-tests.log) is SHA-sealed in the JSON receipt. The new dependency warning was fixed; only the upstream HTTP2-disabled timer warning remains. The driver-observer ownership boundary still requires separately scoped source review; this file does not promote unobserved schedules to verified status.

# FANBOX native dependency modifications

This directory imports the exact official registry releases listed in README.md.
The eight patches in patches/series change the following 15 upstream package
files for the recovered FANBOX native transport. Original and patched bytes are
identified by SHA256 in the provenance manifests. All copyright, attribution and
license text is retained unchanged. The 1,435 packaged BoringSSL source blobs
are unchanged; the extension-shuffle correction is in the upstream build patch.

1. 0001-boringssl-extension-shuffle-bounds.patch

Fix the unused extension-suffix shuffle seed index and inclusive Fisher-Yates swap range in the upstream build patch; leave every packaged native blob unchanged.

- btls-sys-0.5.6/patches/boringssl.patch

2. 0002-btls-requested-trust-anchors.patch

Expose the native requested-trust-anchor ClientHello setter without installing trust anchors or changing certificate verification.

- btls-0.5.6/src/ssl/mod.rs

3. 0003-btls-fallible-ssl-extra-data.patch

Preserve Rust ownership when storing the certificate verification record in native SSL extra data fails.

- btls-0.5.6/src/ssl/mod.rs

4. 0004-wreq-tls-profile-and-certificate-errors.patch

Add default-off requested-trust-anchor and bad_certificate alert policy; retain genuine verifier result, certificate, and internal failure in the error source chain.

- wreq-6.0.0-rc.31/src/tls.rs
- wreq-6.0.0-rc.31/src/tls/conn.rs
- wreq-6.0.0-rc.31/src/tls/conn/service.rs

5. 0005-http2-atomic-idle-shutdown.patch

Atomically retire only HTTP/2 connections without admitted requests or live streams; wait for flush and physical IO shutdown, retaining driver failures.

- http2-0.5.20/src/client.rs
- http2-0.5.20/src/proto/connection.rs
- http2-0.5.20/src/proto/mod.rs
- http2-0.5.20/src/proto/streams/mod.rs
- http2-0.5.20/src/proto/streams/streams.rs

6. 0006-wreq-proto-idle-admission.patch

Reserve admission across the asynchronous HTTP/2 dispatch queue and release it at stream creation, cancellation, or rejected dispatch; expose idle retirement state. Preserve CRLF.

- wreq-proto-0.2.5/src/conn/http2.rs
- wreq-proto-0.2.5/src/proto/http2/client.rs

7. 0007-wreq-logical-authority.patch

Validate one logical Host authority independently of the dial target; preserve it for HTTP/1 retries and use it as HTTP/2 :authority without a redundant Host field.

- wreq-6.0.0-rc.31/src/client/layer/client.rs

8. 0008-wreq-idle-pool-lifecycle.patch

Expose a shared idle-only pool close future, preserve busy HTTP/2 connections, observe HTTP/1 driver shutdown, and retry an untouched request rejected by idle retirement.

- wreq-6.0.0-rc.31/src/client.rs
- wreq-6.0.0-rc.31/src/client/layer/client.rs
- wreq-6.0.0-rc.31/src/client/layer/client/pool.rs

The reconstructed series is a fresh, reviewed source recovery. It is not the
unavailable historical seven-patch series and does not assert that historical
HEADERS-EOS, nonzero Content-Length or legacy deflate compatibility gates have
been recovered. Source provenance does not establish cross-platform runtime
compatibility or downstream release-license packaging.

The existing Go licensebundle gate covers internal/media/ugoira/rust only. A
full root Rust CLI/native dependency and license bundle remains distribution
debt; its success cannot be inferred from that separate gate.

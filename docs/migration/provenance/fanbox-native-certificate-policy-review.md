# FANBOX native certificate-policy review

Review began 2026-10-10 UTC. Repository: `/workspace/scratch/cd334a12dde3/pixiv-cli-rs`; implementation foundation `51dcb2c4f6e196372f0f022170d8b53f1f5eff3a`.

## Current verdict

**Source-only security approval granted for the parent-owned, already authorized six child-owned synthetic witnesses.** Actual corrected callback, fallible per-SSL recording, production native SDK trust/profile, and physical-versus-logical authority source were inspected. The two concrete failure-path findings below were fixed before approval. This approval is limited to those reviewed semantics and does not authorize additional scenarios, denied actions, OS trust changes, or broader native execution. This is read-only review, not native runtime evidence. No Cargo, Go, native program, network, external peer, trust modification, or supplemental probe was run by this reviewer.

## Inspected authority and evidence

Read repository `AGENTS.md`, `/tmp/pixiv-fanbox-native-next-integration-plan.md`, actual Go production native harness and sealed six-row JSON fixture, pristine BoringSSL verification/ex_data implementation, and actual evolving btls/wreq policy source. The approved scenarios are sequential completed GETs, one response-header stalled GET with cancellation, one caller-deadline GET, and untrusted/wrong-host/expired certificates. The denied supplemental multiplex/unfinished HEAD/upload operation was neither reconstructed nor used as evidence.

Fresh SHA256 checks match the sealed inputs:

- Go native harness: `50e22883ecdcde2fc673acc4d8d7ab5633d06056e348c41c50b716e6b685dd7e`
- Six-row fixture: `df753a715efd28c016a2532fe544e992b00a1fe5ce58b756d807c13b9522dd62`

Read-only SHA256/size/path comparison of `third_party/rust/fanbox/btls-sys-0.5.6/deps/boringssl/**` against the recovered official `file-manifests/btls-sys-0.5.6.json` found exactly 1,435 regular files, no changed/missing/extra paths. The separate parent-owned patch artifact is outside those native blobs. This is byte-preservation evidence, not build or runtime proof.

## Native verification semantics

Pristine `ssl/ssl_x509.cc::ssl_crypto_x509_session_verify_cert_chain` initializes the real selected store, leaf and untrusted peer chain; inserts the owning SSL at the reserved X509-store-context ex_data index; selects `ssl_server` purpose for client verification; inherits the SSL verification parameters; then invokes the application certificate callback in place of the ordinary call to `X509_verify_cert`. Thus the Rust callback can retain all native store, purpose, hostname and wall-clock checks by calling `context.verify_cert()` exactly once and never changing verification inputs.

After the callback, native code stores `X509_STORE_CTX_get_error` as session verification result. A false callback is fatal only when verification mode is not `SSL_VERIFY_NONE`; production SDK must keep certificate verification enabled as `SslVerifyMode::PEER` and hostname verification enabled. The callback accepts only `Ok(true)` plus native result `Ok(())`; that condition is stricter than a truthy callback alone. Rejected certificates remain rejected when only their alert-driving result is changed to `CERT_REJECTED`.

Pristine `SSL_alert_from_verify_result` maps `CERT_REJECTED` to `SSL_AD_BAD_CERTIFICATE`, hostname mismatch to the same alert, unknown-authority codes to `UNKNOWN_CA`, expiry to `CERTIFICATE_EXPIRED`, and `UNSPECIFIED`/`OUT_OF_MEM`/`INVALID_CALL`/`STORE_LOOKUP` to `INTERNAL_ERROR`. This supports the intended output-only alert mapping, subject to corrected internal-error handling and the actual approved peer witnesses.

## Findings sent before approval

### 1. Internal verifier failures returned as `Ok(false)` must retain internal alerts

The first actual `wreq/src/tls/conn.rs` callback detected internal errors only via `checked.err()`. This is insufficient: pristine `crypto/x509/x509_vfy.cc::X509_verify_cert` returns zero for `INVALID_CALL`, `OUT_OF_MEM`, and its `UNSPECIFIED` safety-net failure. btls `verify_cert` converts zero to `Ok(false)`, not `Err(ErrorStack)`. The callback therefore translated those internal failures to `CERT_REJECTED`. It was still fail-closed, but mislabeled internal faults as rejected peer certificates.

Required correction: preserve `UNSPECIFIED`/`OUT_OF_MEM`/`INVALID_CALL`/`STORE_LOOKUP` as internal native results and corresponding internal alert; classify internal faults as `Other` or explicitly internal; translate only genuine rejected certificates. No internal failure may yield accepted TLS.

### 2. Per-SSL recording must be fallible or checked before translating the result

The first actual callback used existing btls `SslRef::set_ex_data`, which ignores native `SSL_set_ex_data` return. Pristine `crypto/ex_data.cc::CRYPTO_set_ex_data` allocates or grows a per-SSL array and can return zero. Failed insertion would leak the setter's boxed record and then allow alert translation to replace the only accessible native result. The handshake error fallback would expose `CERT_REJECTED` instead of the true native class.

Required correction: a general safe fallible SSL ex_data insertion wrapper in the owned btls Rust source, or verified pre-initialization followed by allocation-free record mutation; on inability to preserve the record, fail closed with an internal error and never present a translated result as the saved original.

## Ownership, lifetime and concurrency checks

The per-SSL index is allocated once through `LazyLock<Result<Index<Ssl, VerificationRecord>, ErrorStack>>`; each SSL has its own record, avoiding client-global "last failure" state and cross-connection races. The index's data must be `Send + Sync + 'static`, as enforced by btls. Records contain owned DER bytes, a copied typed native result, and owned/shared error details, not borrowed X509-store-context or peer-certificate pointers. The callback's temporary context/certificate references expire before the native context is freed. The native SSL-owning pointer is inserted before callback invocation; callback mutation occurs synchronously inside the exclusive SSL handshake, not in parallel with another operation on the same SSL. Client clones use separate SSL objects while sharing immutable callback/index configuration.

The application callback accesses the native SSL through the existing typed `X509StoreContext::ssl_idx()` and `context.ex_data_mut` APIs; no hand-written FFI casts or permanent pointer escape is introduced by current wreq code. The existing btls callback shim keeps the closure alive through its SSL context; it does not permit replacement while borrowed. Existing upstream panic/allocation behavior is not a new permissive verification path.

## Authentic diagnostic mapping

Current `wreq::tls::CertificateVerificationError` retains the original `X509VerifyError`, optional actual current-certificate DER, optional internal `ErrorStack`, and original `btls::ssl::Error` as source. `kind()` uses typed result constants: unknown issuer/self-signed/leaf-signature, hostname/IP mismatch, expired/not-yet-valid, and Other. No case-name branch or diagnostic-substring classification exists in inspected source. `perform_handshake` reads saved per-SSL failure before dropping the SSL stream or boxing the source.

The sealed Go fixture retains actual Go `*url.Error`, `*tls.CertificateVerificationError`, `x509.UnknownAuthorityError`, `x509.HostnameError`, `x509.CertificateInvalidError`, full original diagnostic strings and raw chains. Rust cannot truthfully reproduce Go's concrete type chain merely by copying its names/text. Current Rust witness declares this language mapping explicitly, records real Rust error messages, checks typed Rust classes, and compares actual peer handshake/wire evidence. It does not claim raw concrete Go-chain equality. Preserve that limitation after runtime checks.

## Final source acceptance checks

- Done: corrected callback rejects every non-success, preserves internal native codes, and uses fallible record storage
- Done: `native.rs::NativeTransport::new` uses `CertStore::builder().set_default_paths().build()`, `.tls_cert_verification(true)`, `.tls_verify_hostname(true)`; `profile.rs::tls_options` selects the explicit optional BadCertificate policy and empty requested trust anchors. Neither source supplies an owned fixture root as production trust. SDK wreq defaults are disabled and selected runtime/proxy/stream features do not enable bundled roots.
- Done: `client/layer/client.rs` parses logical Host into typed `RequestAuthority`, first creates the descriptor from the original normalized physical URI and connects/pools from that descriptor, and only afterward rewrites the wire H2 URI authority. `tls/conn.rs` derives SNI/verification target from descriptor URI, so logical Host cannot change the TLS certificate target.
- Done: reviewed hashes below identify the source snapshot. Official 1,435-blob identity was rechecked after source readiness, with no differences.
- Parent alone may run its authorized six child-owned TLS/H2 witnesses; no parent trust environment mutation or other network scenarios
- Runtime classes and BAD_CERTIFICATE peer alerts remain unestablished until actual observations; this report cannot itself establish parity

## Corrected-source resolution

Finding 1 is resolved in actual `conn.rs`: internal result codes `UNSPECIFIED`, `OUT_OF_MEM`, `INVALID_CALL`, and `STORE_LOOKUP` bypass CERT_REJECTED translation. Contradictory successful-result/failing-return states become UNSPECIFIED; all return false. The reviewed pristine verifier returns only zero or one, so actual native internal zero returns are handled as required.

Finding 2 is resolved in actual btls `SslRef::try_set_ex_data`: existing data is overwritten without native insertion; new storage is boxed, native insertion is checked with `cvt`, and a failed insertion reconstructs and drops the Box. Pristine native ex_data insertion writes the pointer only on success. Callback storage failure sets UNSPECIFIED and returns false before result translation. It cannot expose translated CERT_REJECTED as the originally saved result.

The production Brotli certificate codec performs real compression/decompression through the public wreq Codec. Existing btls decompression output is a fixed-length native CryptoBuffer cursor; output length must equal the declared native capacity before it is accepted. This is not a dummy advertised decoder. TLS trust-anchor extension request copies validated identifiers and does not install any trusted certificates.

## Reviewed snapshot SHA256

- `third_party/rust/fanbox/btls-0.5.6/src/ssl/mod.rs`: `db8631b5207a35579bae384ce2089b7630935416a5fb9e80909d2c6d926cc466`
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls.rs`: `ec561c22414462327959b24e69254c282cb8b34d8148c9456f1ebae72b1b0e6a`
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls/conn.rs`: `4ed66d840d045789733856b8985a8a7b95fff4e0f3f543a0ea93fcf192277cd5`
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls/conn/service.rs`: `69ef152b341e0b669fa03defb234746328abcf4e0a25d229fac808cff42a4ef9`
- `crates/pixiv-sdk/src/fanbox/native.rs`: `d2b595b085799daa2bd6779d651d38f7b459f8aec5238c137c9e3a0679838ea5`
- `crates/pixiv-sdk/src/fanbox/profile.rs`: `5ee389ad80240f7b5e275876b14a9ea17c6c92110c3be158ba4512c83830012a`

Material changes to callback result handling, per-SSL storage, trust/mode/hostname setup, or physical TLS destination require source re-review. Formatting/build-only fixes do not create runtime evidence.

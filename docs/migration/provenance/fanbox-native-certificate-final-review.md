# FANBOX native certificate-policy final-source addendum

Date: 2026-10-10 UTC. Repository: `/workspace/scratch/cd334a12dde3/pixiv-cli-rs`.
Preceding detailed review: `docs/migration/provenance/fanbox-native-certificate-policy-review.md`.

## Verdict

**Source approval renewed for the exact current production hashes below, within the already authorized six child-owned synthetic scenarios. No new material certificate-policy finding or regression was found.** The previously corrected internal-result handling and fallible per-SSL record storage remain intact. The three changed production files were inspected as actual source; their current contents preserve the reviewed trust, verification, alert, authority and codec semantics. This is a semantic re-review, not a claim that hashes alone prove the intervening edits were documentation/formatting only.

This reviewer performed read-only source and persisted-evidence inspection and wrote only this addendum. No source was edited. No Cargo, Go, native program, build, test, peer, network operation, trust modification, or additional scenario was run by this reviewer. The denied supplemental multiplex/unfinished HEAD/upload probe was not reconstructed or retried. This approval supplies no new execution authorization.

## Independently inspected policy

- `wreq/src/tls/conn.rs:347-395` builds the real native connector/store, enables the configured verification mode, and installs the optional BadCertificate callback. The callback calls `context.verify_cert()` exactly once. It accepts only `Ok(true)` together with a successful native result; every other path returns false.
- Native `ssl/ssl_x509.cc:201-264` initializes the selected real trust store, actual leaf and peer chain, reserved owning-SSL context index, client-side `ssl_server` purpose and SSL verification parameters before invoking the application callback. Rust does not replace the store, chain, purpose, hostname or clock inputs. Native `X509_verify_cert` still performs chain/purpose, identity, signature and validity checks. `check_cert_time` uses the existing native verification flags and real `time(nullptr)` in the default configuration; the inspected production setup does not disable time checks or install a fixture clock.
- On a genuine certificate rejection, the callback copies the original typed `X509VerifyError`, owned current-certificate DER when conversion succeeds, and optional owned/shared `ErrorStack` into the owning SSL before setting the alert-driving result to `CERT_REJECTED`. It neither uses test-case names nor classifies error-message substrings. `CertificateVerificationError::kind()` maps typed original native constants. `perform_handshake` retrieves the saved record while the SSL stream is alive and retains the original `btls::ssl::Error` as the source.
- `UNSPECIFIED`, `OUT_OF_MEM`, `INVALID_CALL` and `STORE_LOOKUP`, any verifier `Err(ErrorStack)`, and a contradictory non-success return with an otherwise successful native result bypass rejection-alert translation. Internal codes retain native internal-error treatment; the contradictory-success result becomes `UNSPECIFIED`. All still reject TLS. Pristine `X509_verify_cert` can return zero for internal failures, so the explicit result-code exclusions remain necessary.
- The saved-record index is a fallible `LazyLock<Result<Index<Ssl, VerificationRecord>, ErrorStack>>`; index setup failure prevents connector construction. Records are per SSL, not a client-global last failure. They contain owned data rather than retained X509-store-context pointers. The callback obtains the owning SSL through typed existing context APIs.
- `btls/src/ssl/mod.rs:3694-3709` checks the native new ex_data insertion, drops the Box on insertion failure, and overwrites existing data without another native insertion. Native `CRYPTO_set_ex_data` installs the supplied pointer only after successful allocation/growth. Missing owning SSL or failed saved-record insertion sets `UNSPECIFIED` and returns false before any `CERT_REJECTED` translation. Thus storage failure cannot accidentally expose a translated result as a successfully preserved original cause.
- `NativeTransport::new` still uses `CertStore::builder().set_default_paths().build()?`, `.tls_cert_store(roots)`, `.tls_cert_verification(true)` and `.tls_verify_hostname(true)`. The store wrapper propagates setup errors. The connector helper maps verification true to `SslVerifyMode::PEER`; its absence would make a false verification callback non-fatal in native code. Production code contains no owned peer root injection. The SDK selects wreq with `default-features = false` and runtime/proxy/stream features, not bundled roots. Default-path behavior remains subject to the process's legitimate `SSL_CERT_FILE`/`SSL_CERT_DIR` and build defaults; it is not evidence of Windows trust-store equivalence.
- `client/layer/client.rs` parses one logical Host into typed `RequestAuthority`, rejects ambiguous/invalid authority forms, constructs `ConnectionDescriptor` from the original normalized physical URI, and connects/pools through that descriptor before rewriting only the outbound H2 authority. `ConnectionDescriptor` retains that physical URI in its connection identity. TLS `setup_ssl2` derives SNI and native DNS/IP verification parameters from `descriptor.uri()`, not the logical Host. The existing btls hostname setup retains NO_PARTIAL_WILDCARDS and typed DNS/IP verification.
- `profile.rs` uses the real public `TlsOptions` builder APIs for `CertificateFailureAlert::BadCertificate`, empty requested trust-anchor IDs, real Brotli certificate compression, ALPN/ALPS and the explicit TLS profile. The optional wreq alert policy defaults to Native. The requested-trust-anchor native API validates and copies IDs into extension configuration; it does not install trusted certificates or modify verification inputs.
- Brotli compression/decompression calls real `brotli::BrotliCompress`/`BrotliDecompress` through wreq `Codec::Pointer` and native compressor registration. The btls decompressor writes into a fixed-capacity native CryptoBuffer cursor, rejects decompression errors, and checks that the exact declared output length was initialized before returning success. It is not a dummy advertised decoder. No compressed-certificate runtime parity is inferred from these uncompressed peers.

## Persisted six-scenario evidence consumed

The parent-provided `/tmp/pixiv-fanbox-native-third-focused.log` records a successful focused test run with six actual child scenario passes. Its separate no-mode child-entry scaffold is not meaningful scenario evidence. The final report explicitly records formatting/import cleanup after that focused run and requires final gates against the current snapshot; this source approval does not relabel the earlier run as a full final-source gate.

The existing report and three certificate witness files record genuine native classes and peer alerts:

- `untrusted_certificate`: `UnknownAuthority`, original code 20, `unable to get local issuer certificate`
- `invalid_hostname`: `HostnameMismatch`, original code 62, `Hostname mismatch`
- `expired_certificate`: `Expired`, original code 10, `certificate has expired`

Each persisted certificate witness shows `remote error: tls: bad certificate`, no H2 frames/responses, actual certificate DER, and the genuine Rust error chain. The report records actual DER equality with its owned peer. This is observed rejection/alert evidence, not copied Go concrete error identities. The other three saved witnesses cover sequential completed GET reuse and idle closure, a response-header-stalled GET cancelled after approximately 31.2 seconds, and an approximately 303-millisecond caller-deadline GET. Their existing report records stream-cancel resets and seven actual ClientHello captures under the sealed comparison policy. This reviewer checked all six witness SHA256s against that report and inspected the actual certificate witness fields; it did not replay the peers or independently redo the entire wire comparison.

Evidence SHA256:

- Focused log: `5b4a4fa2b03c2531b2934dc14e24e130fb3b4e4a0f011e7f2c00ef8ac6712648`
- Final report: `080230f53e90295f14bacaab4c4fd83b9733819e53000e4d50e04dfa63b49553`
- `headers_sequential_idle/rust-observation.json`: `d7f446bfcc83ff2c530b8d12712077236b07f8f5bac627f4bd1317febcdce22f`
- `response_header_stall/rust-observation.json`: `19c2e8042a9fa52f27c1314d5bf5fa55e9cb400ed6d5aeb746e77fecf8a6b9ef`
- `caller_deadline/rust-observation.json`: `cb16c2eef524e318af4412da1d611084463061090a218c4c929c860fec201d78`
- `untrusted_certificate/rust-observation.json`: `a6f3ab5a7fa169c80328203521055e48ab0c2544888b09a10659e286d45c61a8`
- `invalid_hostname/rust-observation.json`: `96fbd219ffa6d35e1ee30d9f589d7f881504cbdc97a9a8d0989fdfe5b8011c36`
- `expired_certificate/rust-observation.json`: `66a51c7055b2fcc2bfc0a658811b0cfbb1a768a6e48cbb032b87e6db845c9233`

Witness paths above are relative to `/tmp/pixiv-fanbox-native-third-witness/`.

## Exact current approved production SHA256

- `third_party/rust/fanbox/btls-0.5.6/src/ssl/mod.rs`: `db8631b5207a35579bae384ce2089b7630935416a5fb9e80909d2c6d926cc466` (unchanged from preceding source approval)
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls.rs`: `21489a2d7db5295741a798d1626ca7be9ea4e1ad5ff111103fda8d5f29dfabf8` (renewed approval)
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls/conn.rs`: `4ed66d840d045789733856b8985a8a7b95fff4e0f3f543a0ea93fcf192277cd5` (unchanged)
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls/conn/service.rs`: `69ef152b341e0b669fa03defb234746328abcf4e0a25d229fac808cff42a4ef9` (unchanged)
- `crates/pixiv-sdk/src/fanbox/native.rs`: `22bac9a4d1e7b2988228da5c2f3f7969a75407f3aa28657668341a9d34df58b2` (renewed approval)
- `crates/pixiv-sdk/src/fanbox/profile.rs`: `ba4fad8edaa90f2dfc5d0c1e04679f8218c6570ec797ba0cae7f3ead58724295` (renewed approval)

All six match the final report's recorded final production snapshot. Additional reviewed context hashes:

- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/client/layer/client.rs`: `9bbf90c5bcbe84e1183a2db55131872d04b1d1a5817000bd52219621eecb0f4e`
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/conn/descriptor.rs`: `ad714cdaeba42f69f31a929c14209945028acdb90c036d665bc2435e55ad9048`
- `third_party/rust/fanbox/wreq-6.0.0-rc.31/src/tls/compress.rs`: `07e11e46d727394df4ef3857e13574b5c3d0f7b9b700f40a24d3fd89c54acd91`

## Explicit limits and remaining ownership

The approved observations remain Linux/amd64 child-owned loopback TLS1.3/H2 witnesses at the genuine public native raw boundary. They do not establish private Session/public SDK policy equivalence, Windows or other OS trust parity, TLS1.2 negotiation, PSK/resumption, HRR, HTTP1/HTTP3, proxy runtime, concurrency, compressed-certificate or response decoding parity, body-close cancellation, concurrent Read/Close, unfinished request bodies, or infinite response-header survival. Go concrete type chains and diagnostic wording are not claimed equal to Rust's genuine errors. No real FANBOX/account/media/external host was used by this reviewer.

Final complete Rust/Go gates, dependency/patch manifests, native-source preservation audit and any authorized publication remain parent-owned. No full gate is claimed here. The prior two certificate findings remain resolved in actual code; source changes to verification decisions, storage, trust/mode/hostname setup or physical TLS destination require another review. Denied supplemental probes remain excluded.

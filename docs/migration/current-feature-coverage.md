# Current connected coverage and remaining verification

Inventory at signed checkpoint `40faef3f48af16be4acf3044f103532d518449c1` (2026-10-10). This is a source-referenced engineering inventory, not a declaration that Rust replaces Go. The frozen reference remains `4b4426487ef18bed276706daec385e0d0a6979f9`.

## Public surface

The ledger preserves 673 identities. All 84 CLI entries, 54 Pixiv MCP tools and 11 FANBOX MCP tools are connected/in_progress. No entry is verified. The 59 pending SDK/type/private-adapter rows are not 59 missing user features: several have existing Rust representations but lack complete contract/native verification. Status and feature presence are different observations.

## Connected families

- Artwork detail/search/trending/ranking/recommendations/series: SDK operations, saved-account Execution, CLI and MCP are connected (`pixiv-sdk/src/{pixiv,search,artwork_feeds,artwork_series}.rs`, CLI search/ranking/recommended and MCP counterparts)
- Novels/users/relationships/bookmark lists/details/tags/timeline/MyPixiv: SDK, CLI and MCP paths are connected, including aggregation and pool/replay tests
- Bookmark/follow/comment/reply/stamp mutations: real typed form operations connect to CLI/MCP execution, with synthetic mutation/pool coverage
- Auth/config: multi-account selection/removal/import/export/check/refresh/pool, PKCE/login, callback/handler/startup and persistent settings are connected (`pixiv-cli/src/auth_*.rs`, `pixiv-app/src/{account_service,database,config,url_handler}.rs`)
- Download/ugoira: direct/opaque resource, static PID/URL, record streams, user/bookmark expansion, GIF/APNG/ZIP/raw, standalone metadata, and MCP random recommendation download are connected
- Dictionary: anonymous article/search/group CLI has its own HTTP service (`pixiv-cli/src/dictionary/`), independently of saved-account App API
- FANBOX/browser import: solver/native transport/content/resource/save, saved sessions, five auth leaves, six read leaves, download and eleven MCP tools are connected. Owned browser DB/key material and Linux native transport tests are distinct from live/native OS credential extraction
- Reverse search: local/URL snapshots, SauceNAO/ASCII2D policy, aggregation, output, CLI and MCP are connected (`pixiv-app/src/reverse_search/`)
- Update: explicit and eligible post-success automatic update flows connect to config/proxy/source/cache/coordinator/trust/install and root ownership (`pixiv-app/src/update/`, CLI update/main). The final published gate passed; all 158 behavioral root rows were not claimed covered

Representative published test families and exact bounded counts remain in `contracts.md`, per-feature contract documents and provenance. No inspected ordinary major family is only an empty-success stub or Go fallback. Go deliberately unsupported novel-content and AI-visibility mutations remain the same ContentUnavailable behavior, not invented Rust omissions.

## Concrete unfinished connections

1. Root debug diagnostic started/completed/failed events, scope propagation and diagnostic-writer joining are connected in the current bounded root-diagnostics slice (see [scope](root-diagnostics.md)). The inventory base above predates this slice. Ten actual Linux whole-output rows plus separate routing/exclusion smoke paths do not establish the complete historical root matrix, native cleanup or every MCP/updater runtime scope schedule
2. Ordinary Pixiv HTTP1 ownership-aware idle control is connected in a bounded candidate: actual reqwest/Hyper pool retirement, active/idle retention, deadline tasks and injected caller-owned no-op. Five/nine retirement rows plus four separate ownership lifecycle schedules use explicit custom-client ownership transfer over actual synthetic TLS/TCP peers. Default/OAuth full construction, retained Login, caller Context/explicit Body.Close and buffered-but-unconsumed resource timing remain pending. Frozen normal CLI injects its resolved network client, including an empty proxy override; Execution therefore invokes ownership-aware cleanup without closing that caller-owned client. Login Complete must retain Begin-time choice. Sender retirement, request completion and TLS close_notify alone are not physical TCP shutdown proof
3. Relay serving still uses `hyper::server::conn::http1`; HTTP/2 capability remains unimplemented. Separately, the workspace ordinary reqwest configuration disables defaults and omits its `http2` feature. FANBOX/ASCII2D native HTTP/2 support does not establish ordinary Pixiv/Login or relay protocol parity
4. Auth database Close-error propagation and wider Context/client/IO ownership remain incomplete
5. Rust-native installation/build provenance corresponding to Go `debug.ReadBuildInfo` is unmapped
6. Rust distribution is not connected: `scripts/build.sh` and `scripts/build-platform.sh` still build Go `./cmd/pixiv`; `scripts/internal/licensebundle/licensebundle.go` defaults to only the original ugoira Rust graph. Docker consumes those prebuilt artifacts. Root Cargo release is not a six-platform Rust package/license/install proof

## Native and compatibility boundaries

Fresh published unchanged full gate: formatter, strict workspace/all-target Clippy, 984 workspace passes plus seven subprocess children (six FANBOX native, one login handoff), 991 raw passes, zero failures, 11 existing ignored and release build. This verifies the bounded tested tree, not all features/platforms.

Linux owned subprocess/file/SQLite/codec/native TLS observations coexist with source-driven Darwin/Windows Keychain/DPAPI/association/replacement mocks. They cannot promote native Windows/Darwin/amd64/arm64 execution. Real authenticated services, external media/uploads, live registry/browser/account/security/handler/PATH effects were not exercised. The previously denied supplemental multiplex/unfinished-HEAD/upload probe was not retried.

Additional required debts include Accept-header parity, expired-positive HTTP pacing, parser/raw-byte/EOF/cancellation schedules, simultaneous MCP EOF behavior, Go goroutine/body-close versus Rust future-drop timing, Unix signal restoration, destructive explicitly repeated Windows association cleanup, historical archive/ZIP64 grammar, formal build metadata, licenses/signing/packaging and all supported OS/architecture build/runtime matrices. Public type/API representation differences and Go-only nil/private/open-string witnesses remain named.

Historical sections saying updater was unimplemented describe earlier checkpoints; current updater scope and final validation supersede that state without removing its historical evidence. After root diagnostics, close the concrete SDK ownership/Context, HTTP/2/header/pacing, cleanup/EOF, native build and distribution gaps before final replacement/acceptance.


Ordinary H1 candidate ledger: 614 in_progress,59 pending,zero verified across the same673 identities; only sdk/pixiv:Client.CloseIdleConnections advances. Native SDK mapping is five retirement schedules plus four distinct ownership lifecycles, not all nine retirement rows. See [current scope](sdk-http-ownership.md#connected-ordinary-http1-candidate-2026-10-10). Final full gate/review is recorded before publication; this inventory does not itself certify migration completion.

# Go と Rust の振る舞い比較

公開面の snapshot に加え、固定 Go 実装から取得した入力・結果を両実装で検証する。フィクスチャは [contracts/](contracts/) に置き、本番コードから読み込まない。Rust の結果に合わせて期待値を更新しない。

## ResourceRef

[resource-ref.json](contracts/resource-ref.json) は、参照コミット `4b4426487ef18bed276706daec385e0d0a6979f9` の `sdk/ref.go` を使って取得した 55 ケースを含む。内訳は生成 8 件、text の解析 42 件、JSON の解析 5 件である。

Go の `sdk/migration_resource_ref_test.go` は同じ入力を実行してフィクスチャとの一致を確認する。Rust の `crates/pixiv-sdk/tests/resource_ref.rs` はそのフィクスチャを読み込み、符号化された文字列、製品名、payload、JSON、エラー分類を比較する。追加テストでゼロ値と失敗時に既存参照を変更しないことを確認する。

対象は Pixiv/FANBOX、binary payload、日本語、HTML escape、不正入力、版番号、未知・重複キー、null、byte array、base64 の改行と非正規の末尾 bit、Unicode surrogate、不正 UTF-8、Go の JSON nesting 上限を含む。参照文字列は解析後も元の表現を維持する。製品 SDK が `OpenResource` で行う再検証・取得は、codec とは別の契約として今後検証する。

Rust は `resource::ResourceRef` を提供する。Go の `String` は `as_str`/`Display`、`IsZero` は `is_zero`、text/JSON codec は同名の snake_case メソッドと serde に対応する。失敗を含めた構造化エラーを扱う場合は `unmarshal_json` を使用する。Go の値比較に相当する `Eq` を実装する。

Windows amd64 で比較テストを実行済み。ただし、参照解析に失敗した場合の詳細・原因と、製品 SDK による参照の再検証・取得は未移植のため、関連台帳は `in_progress` とする。成功時の wire 一致だけで完全互換とは扱わない。

```text
go test ./sdk -run '^TestMigrationResourceRef' -count=1
cargo test -p pixiv-sdk --test resource_ref --locked
```

初回取得またはレビュー済みの基準更新には Go テストの `-args -migration-update-resource-ref` を使用する。通常の検証では更新 flag を付けない。

## 共通エラー

[errors.json](contracts/errors.json) に固定 Go 版から取得した 37 ケースを保存する。18 の定義済み Reason、製品名の fallback、操作名、詳細と原因、HTTP status、HTTP/TLS/DNS/local の分類、原因の連鎖、取消と deadline、retry の安全性と期限を比較する。retry 秒数は固定時刻に対して比較し、過去は 0、正の端数は Go の浮動小数点への変換に従って切り上げ、time.Duration の上限も検証する。

Rust の `Error::with_product` と `with_*` builder が Go の `NewError` と ErrorOption に対応する。`RetryAdvice.after: Option<DateTime<Utc>>` が Go の After/HasAfter を表す。`reason_of`/`is_reason` は最初の SDK エラーを分類し、`matches_reason` は Go の errors.Is と同様に内側の原因も照合する。`source` で原因を保持し、`is_canceled`/`is_deadline_exceeded` が取消分類を調べる。製品 SDK は通信の生エラーや response body をこの原因へ直接取り込まず、分類した情報だけを渡す。

CLI の明示 JSON エラーは `code` と `message`、必要な場合だけ正の `retry_after_seconds` を出す。内部の operation、HTTP status、transport、retry の構造体をそのまま出力しない。子プロセスのテストで認証情報なしの経路を使い、stdout、stderr、終了コードと envelope の形を確認する。Go CLI 全体の認証選択・引数エラー・BrokenPipe などの互換性は、この確認に含めない。

空・未知の Reason、任意の redacted cause 型の表現、製品別 HTTP/通信失敗のマッピングと実際の取消は未完了・未検証として台帳に記録する。テスト済みなのは共通エラーの構築・参照と限定した CLI 出力であり、全入口の完全互換を意味しない。response の Debug から upstream body を除くテストも追加した。

```text
go test ./sdk -run '^TestMigrationErrors' -count=1
cargo test -p pixiv-sdk --test errors --locked
cargo test -p pixiv-cli-rs --test errors --locked
```

レビュー済みの基準更新には Go テストの `-args -migration-update-errors` を使用する。通常の検証では更新しない。

## Cursor と Page

[cursors.json](contracts/cursors.json) に固定 Go 版から取得した 52 ケースを保存する。生成 14 件、text 解析 22 件、product/operation/binding/query の照合 5 件、ephemeral instance の照合 6 件、JSON 解析 5 件を含む。追加テストではゼロ値の marshal・検証失敗と、解析失敗時に既存の cursor を変更しないことを確認する。

Rust の `cursor::Cursor` と `CursorOptions` が Go の Cursor と CursorOption に対応する。`identity` は検証済みの非秘密識別子、`instance: Some(...)` は ephemeral binding を指定する。文字列は解析後も元の表現を維持する。instance を空文字で明示することと、instance の指定なしを区別する。

Go の汎用 codec は版番号だけを検証し、product・query・payload の要件は製品 SDK が再検証する。Rust も解析だけで独自に厳しくせず、`validate` と `validate_instance` を別に提供する。`Page<T>` は items と next を持ち、終端はゼロ cursor、空 items は空の Vec とする。Page の直接 JSON 出力はまだ定義していない。

ResourceRef と Cursor は crate 内の共通 codec で base64 と Go の JSON 解釈を共有する。外部テストからだけ使う API は追加していない。既存 ResourceRef の比較テストで、この共通化による wire の変化がないことも確認する。

汎用 codec の比較は Windows amd64 で実行済み。製品 SDK の query digest、account binding、継続 payload の検証、ページ内 checkpoint、実際のネットワーク要求前の拒否、全ページ取得は未移植として台帳に残す。不正データの詳細・原因を含むエラー表示も完全一致とは扱わない。

```text
go test ./sdk -run '^TestMigrationCursors' -count=1
cargo test -p pixiv-sdk --test cursors --test resource_ref --locked
```

レビュー済みの基準更新には Go テストの `-args -migration-update-cursors` を使用する。通常の検証では更新しない。

## Pixiv HTTP エラー分類

[http-status.json](contracts/http-status.json) は固定 Go 版の content API と OAuth の分類から採取した 64 ケースを持つ。16 種類の HTTP status と Retry-After の有無を組み合わせる。Rust は公開の `Client::artwork` と `oauth::refresh` を fixture transport 経由で呼び、reason、status、product、operation、表示、retry の有無と期限を比較する。有効な Retry-After を持つ content API の 429 は 2 回、その他は 1 回の要求となる。

Content API の 400 は InvalidArgument、401 は CredentialsExpired、404 は NotFound、410 は ContentUnavailable となる。OAuth の 400・401 は CredentialsExpired、404・410 は UpstreamError となる。両方で HTTP の 5xx は UpstreamError であり、通信不能の UpstreamUnavailable と区別する。Content API の retry advice は有効な Retry-After を持つ401・429に付く。OAuth は公開 OpenWith の実際の HTTP 経路で比較し、Retry-After を取り込まず retry advice を返さない。

Content API は分類関数から採取し、OAuth は公開 OpenWith の HTTP 経路から採取する。通信原因の分類・取消、認証開始と成功応答の全契約は未移植・未検証として残す。OAuth の refresh 失敗には公開 SDK の Open、code 交換には Complete の operation 名を使う。

```text
go test ./sdk/pixiv -run '^TestMigrationHTTPStatus' -count=1
cargo test -p pixiv-sdk --test http_status --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-http-status` を指定する。

## Client Use と pool の試行・commit・解放

[use.json](contracts/use.json) は固定GoのFacade.Useと実Schedulerを接続した20ケースを持つ。nil context/callback、config loader・factoryの欠落/失敗/nil executor、pool無効時の明示UID、pool有効時の要求UID置換、未commitの安全なrate-limit replay、commit後の停止、callback/closeの単独・同時失敗、close側のSDK retry/cancel cause、callback panic後のcloseを比較する。loader/factory/callback/closeの回数、各attemptのUID/options、凍結UID/期限、全文・SDK reason・取消検索・Gate再取得を保持する。時刻は1000秒、retryは1120秒へ制御する。

RustのFacade.use_clientはUseごとにconfigを読み、必要ならexecutorを生成する。各attemptでClientSessionsを取得し、callbackのcommitをAttemptへ反映してからcloseする。ActiveLeaseはfuture破棄やpanicでもcloseを呼び、Leaseのcached resultで重複解放を防ぐ。callbackとcloseの両方の原因をJoinedへ保持し、SDK reason/取消検索は先頭以外も調べる。std Errorのsourceは単一のため先頭だけを返し、SDK CauseのJoinedと明示検索で複数原因を扱う。

実Databaseを共有するPoolState adapterは選択/凍結のSQLごとにMutexを解放し、OAuthやcontent通信中は保持しない。追加のRustテストは実Database・AccountService・OAuth・SDK clientとFacadeを接続する。42のcontentが合成SDK rate-limit errorを返すと43へ切り替え、両UIDともcontent呼出時点のtoken/revision保存を確認し、42の凍結・43のmarker・2回のcloseを検証する。これは実HTTP retryの比較ではなく、SDKの分類済みerrorをtransport portから返す接続テストである。callback待機taskのabortでも1回のcloseとGate解放を確認する。

```text
go test -race ./internal/services/pixiv ./internal/services/pixiv/pool ./internal/shared/lifecycle -count=1
cargo test -p pixiv-app --test client_use --test pool_session --locked
```

比較はWindows amd64のみ。CLI/MCP composition rootとconfig file、SDK自前HTTP cleanup/options、runtime設定変更の連続Use、実HTTP rate limitとpoolの再試行設定、全選択失敗/枯渇とjoined cause、親子context、SQL実行中の取消、panicとclose panicの同時発生、他OSは未移植または未検証。Generic lifecycle.Run自体のchild context等の契約を、このFacade比較で検証済みとは扱わない。fixture更新は `-migration-update-use` を明示する。

## 保存済みアカウントの refresh・identity・revision と session

[account-open.json](contracts/account-open.json) は固定Goの実account Service・公開SDK・SQLiteで採取した11ケースを持つ。明示UID、設定済みdefaultとsort順のfallback、空DB・存在しないUID・設定defaultの欠落、OAuth identity不一致、refresh中の別writerによるrevision競合、rotated token欠落、零expiry、refresh中のcontext取消を比較する。成功時にだけcontent要求を呼び、その時点で保存済みrevisionが2かつAuthorizationが新access tokenであることを確認する。OAuthのusernameはrotationだけでは保存済みmetadataを更新しない。

RustのAccountServiceは実Databaseのrepository adapterとOAuthを呼び、identity確認後にrevision CASでtokenを保存してからclientを返す。外部テストではClientSessionsのaccount openerへ接続し、同じ11ケースの通信回数・エラー表示と取消cause・DB token/revision/usernameを比較する。成功・失敗・取消後にGateを再取得して解放を確認する。HTTP transportは呼出側が渡す依存境界で、Goの明示HTTPClientと同じケースを比較する。

零expiryのGo clientはcontentを要求できる。Rustのfrom_credentialsが零時刻を期限切れとして拒否していた差を比較テストで検出し、Goに合わせてconstructorで自動expiryを設定しないよう修正した。固定GoのexpiresAtはOpen後に設定されず利用もされない。credentials metadataのexpiryは保持する。refresh中の取消はUpstreamUnavailable・HTTP transport・非公開causeのcontext.Canceledを保持し、表示は `pixiv upstream transport failed` にする。

```text
go test -race ./internal/storage/database -run '^TestMigrationAccountOpenPersistsRotationBeforeContent$' -count=1
cargo test -p pixiv-app --test account_service --locked
```

比較はWindows amd64のみ。default設定ファイルの実adapter、CLI/MCP composition root、poolのUseへの接続、SDK自前HTTPのCloseIdleConnectionsと全connection options、import/login/check/export、SQL実行中の取消、全Unicode/非UTF8 token、HTTP成功と取消の競合・deadline、他OSは未実装または未検証。既存のgeneric session closerは明示portのままで、SDKの自前HTTP cleanupを検証済みとは扱わない。fixture更新は `-migration-update-account-open` を明示する。

## OAuth refresh の不透明 token と応答選択

[oauth-refresh.json](contracts/oauth-refresh.json) は固定 Go 版の公開 OpenWith を通した30ケースを持つ。空入力・cookie名・複数cookie pairを拒否し、単独の `opaque=value`、token内部の空白、cookie pairにならないsemicolonを受け入れる。成功応答のrefresh token欠落は入力tokenを維持する。nested responseはaccess token・refresh token・user IDのいずれかが非零なら選択し、空のnested objectではrootを使う。文字列・整数のID、欠落・null・型違い、非正のexpiry、identity欠落を比較する。

要求のmethod・URL・5つのform field・Content-Type・Authorization不在を確認する。tokenはすべて合成値で、client ID/secretはGo側で非空を確認しfixtureへ保存しない。有効期限はGo/Rust双方で実行前後の時刻に3600秒を加えた区間を確認して3600へ正規化する。Goの零時刻は `0001-01-01T00:00:00Z` として保持する。

従来のHTTP分類fixtureのOAuth部分は分類関数へ人工的なRetry-Afterを渡していたため、実際のadapterが取り込まない値を比較していた。公開OpenWithを経由した採取に修正し、400・401・429のRetry-Afterあり3ケースについてsafe/has_afterをfalseへ修正した。Goの本番実装は変更していない。

```text
go test ./sdk/pixiv -run '^TestMigration(OAuthRefresh|HTTPStatus)' -count=1
cargo test -p pixiv-sdk --test oauth_refresh --test http_status --locked
```

保存済みUIDの確認とrevision CASへの接続は前節の11ケースで比較する。比較はWindows amd64のみ。全JSON wire境界・duplicate key・全Unicode trim・通信原因と取消の全競合・code交換・CLI/MCP認証・他OSは未検証または未実装。認証機能全体の完了とは扱わない。fixture更新は入力や比較対象の変更時に `-migration-update-oauth-refresh` を明示する。

## Retry-After と読み取りの再試行

[retry-after.json](contracts/retry-after.json) は固定 Go 版のヘッダー解析 24 ケースを持つ。符号・空白・先頭ゼロ、秒数の範囲、3 種類の HTTP-date、暦と不一致の曜日、過去・遠い未来、無効なタイムゾーンを含む。Rust の HttpTransport をローカル HTTP サーバーへ接続し、有無と duration を比較する。HTTP-date は実時刻の要求前後から許容区間を求め、秒に丸めず検証する。

Rust の Response は `retry_after: Option<chrono::TimeDelta>` を使い、小数秒を保持する。Content API の GET は有効な Retry-After を持つ 429 にだけ一度待機して同じ要求を再送する。2 回目の 429 はそのまま返し、無効なヘッダーや他の status では再送しない。OAuth の POST はこの再試行を使わない。テストは Tokio の仮想時間を使い、待機時間、再送の method・URL・headers・parameters、成功応答の取得、future を破棄した後に要求が増えないことを確認する。test-util は dev-dependencies に限定する。

比較は Windows amd64 のみ。ヘッダーの全変種、認証再試行、resource の読み取り、pool が最初の429を観測する設定、取消を分類済みエラーとして返す契約は未検証または未移植である。

```text
go test ./internal/services/pixiv/appapi -count=1
cargo test -p pixiv-sdk --test retry_after --test http_status --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-retry` を指定する。

## 作品詳細の DTO と resource identity

[artwork-detail.json](contracts/artwork-detail.json) は固定 Go 版の公開 `Client.Artwork` と `ToArtworkDTO` から採取した 19 ケースを持つ。HTTPClient を注入し、正常・省略・null・負の counters・最大 int64 の identity・未知 kind・legacy AI type・複数ページ・不正 URL・不正日時・入力不正を再現する。Rust の公開 `Client::artwork` で DTO、要求の method/host/path/query と固定の App API ヘッダー、エラー表示まで比較する。

Rust は Artwork、User、ImageResource、ArtworkPage と実行時の Resource を保持する。署名付き URL と request headers は Resource に残し、出力には明示的な `dto::ArtworkDto` を使う。カバー・ページ・著者画像の opaque reference は Go 版と同じ identity を持つ。画像の優先順、meta_pages の配列順からの index、page の fallback 画像でも original variant となる契約、無効な著者画像を省く契約を比較する。空 tags は配列で出し、空 tools/pages と未提供の updated_at は省く。

CLI の detail/search は DTO をシリアライズするよう変更したが、正常系の子プロセス出力や MCP はまだ比較していない。client registry、opaque reference からの再解決・OpenResource・SaveResource は未移植。JSON の重複 key・不正 UTF-8・配列内 null と日時の全変種も未検証。SDK 詳細の 19 ケースだけで縦断移植完了とは扱わない。

```text
go test ./sdk/pixiv -run '^TestMigrationArtwork' -count=1
cargo test -p pixiv-sdk --test artwork_detail --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-artwork` を指定する。

## ArtworkPages と quality variant

[artwork-pages.json](contracts/artwork-pages.json) は Go 版の公開 `ArtworkPages` の 27 ケースと `ArtworkVariantResource` の 27 ケースを持つ。作品詳細と同じ入力でも、ページ取得は公開日時・カバー・著者画像の有効性に依存しない。DTO、エラー表示、要求回数と detail endpoint の query を比較する。追加の許可 host は前後空白を取り、大小文字を区別せず完全一致で検証する。host suffix、http、空 userinfo を含む userinfo、path 不在を拒否し、HTTPS の明示 port は Go 版と同じく許可する。

Rust の `pixiv::ResourcePolicy` と `Client::with_resource_policy` はカバー・ページ・著者画像に共通の方針を適用する。`artwork_variant_resource` は identity の page を保持して variant を置換し、Go と同じ JSON escaping で opaque reference を生成する。空文字・original はゼロ値を含めて元の reference を返す。それ以外では artwork kind と正の ID を要求する。この helper は画質の存在を証明せず、OpenResource/SaveResource 側で再検証・解決する契約である。

この比較も Windows amd64 のみ。URL/JSON の全変種、variant の実在、reference の製品別再検証・再解決・registry・取得・保存、CLI/MCP からのページ操作は未移植または未検証として残す。

```text
go test ./sdk/pixiv -run '^TestMigrationArtwork' -count=1
cargo test -p pixiv-sdk --test artwork_detail --test artwork_pages --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-pages` を指定する。

## 共有 resource の要求・応答

[resource-io.json](contracts/resource-io.json) は Go 版の `OpenResourceRequest.Validate` の 144 ケースと `NewResourceResponse` の 18 ケースを持つ。許可メソッドと大小文字、4 種類のヘッダーの全制御バイト、複数エラーの優先順位、UTF-8 を検証する。Validate は reference の identity を検証せず、製品側が再検証する。

Rust の `resource::OpenResourceRequest` は空 method・GET・HEAD を許可し、ヘッダー値の 0x00–0x1f と 0x7f を拒否する。`ResourceResponse<R>` は caller が所有する AsyncRead stream を保持し、生成時も metadata の読み取り時も body を先読みしない。未読・読了のどちらでも response の破棄が stream を解放する。ヘッダーは7種類の canonical keyだけをコピーし、複数値の順を保存する。Header の返却値や元の map を変更しても内部状態は変わらない。Content-Length は Go と同じ符号付き int64 とし、不正値・範囲外は0となる。

共有 container の所有権は比較済み。実HTTPの取得は次節の transport テストで検証する。製品の reference 再解決・open/save、キャンセル、body の close 時のエラーは未移植または未検証。Go の明示 Close は Rust の所有権による Drop に対応させる。

```text
go test ./sdk -run '^TestMigrationResource' -count=1
cargo test -p pixiv-sdk --test resource_io --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-resource-io` を指定する。

## Resource HTTP ストリーム

[resource-stream.json](contracts/resource-stream.json) はローカル HTTP サーバーを使った Go 版の15ケースを固定する。GET・HEAD、206・204・304・404、途中で切れた body、301・302・303・307・308 の redirect、検証で拒否された redirect、redirect 回数の上限、Location のない応答を含む。送信ヘッダー、URL 検証の順序、応答の許可ヘッダー、読み取り結果と transport エラーを記録する。

Rust の `transport::ResourceTransport` と `HttpTransport` は、body を先読みせず `AsyncRead` として返す。HEAD・204・304 の空 body、非2xxの応答をそのまま返すこと、途中で切れた body の既読バイトと読み取りエラーを比較する。Range・If-None-Match の送信、アプリ用 Referer・User-Agent による上書き、許可した応答ヘッダーだけの公開も検証する。追加の実サーバーテストでは body 送信を待機させ、取得が先に返ることを確認する。

[resource-redirect-headers.json](contracts/resource-redirect-headers.json) の2ケースは、同じホストの別ポートと別ホストへの redirect を比較する。後者では Go と同じく Authorization・Www-Authenticate・Cookie・Cookie2 を除去する。URL validator は初回と各 redirect 前に適用し、上限に達した後は呼び出さない。初回の policy エラーは保持し、redirect 中のエラーと body 読み取りエラーには生の URL や秘密値を含めない。

この比較は Windows amd64 のみ。製品側の reference 再解決・open/save は未移植。cookie jar、subdomain・国際化ホスト名・複数段の信頼境界、URL/Location の全変種、gzip、TLS・DNS・deadline のエラー分類、キャンセルと実通信の中断、close 失敗、他OSは未検証。transport の現在の timeout は60秒で、Go の caller ごとの通信設定の移植も残る。

```text
go test ./internal/services/pixiv/resource -run '^TestMigrationResource' -count=1
cargo test -p pixiv-sdk --test resource_stream --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-stream` または `-args -migration-update-redirect-headers` を指定する。

## OpenResource の metadata 再解決

[artwork-open-resource.json](contracts/artwork-open-resource.json) は Go 版の162ケースを固定する。作品画像の82ケース、小説カバー・ユーザー画像・stamp の42ケース、うごイラ archive の38ケースを含む。single/multi page、カバー、8種類の variant、ページの範囲外、不正 identity、別 product、method の検証優先順位、HEAD、禁止URL、欠落ID、事前の ArtworkPages 取得を比較する。成功時は同じ reference を2回開き、metadata の要求回数・query、解決URL、4種類の条件付きヘッダーと認証ヘッダーの不在を比較する。

Rust の `Client::open_resource` は共有要求の検証、identity の再検証、registry の参照、metadata による再解決、URL policy の再検証を順に行う。metadata から解決したURLと、Artwork/ArtworkPages/search で返した resource のURLをクライアントのregistryに保持する。regular・small・thumb・mini は Go と同じ img-master のパスへ導出し、元の query と fragment を除く。APIエラーはJSON表現の違いを吸収するため、Reason・Product・Operation・Detail・HTTPStatus・Transport・Retry の各SDKフィールドを比較する。

未登録 reference の再解決を実装した kind は artwork・novel_cover・user_profile・stamp・ugoira_archive。小説カバーは選択した画質または既定の優先順を使い、ユーザー画像とstampは variant・page によってURLを変えない。ユーザー画像は4種類の必須objectと公開設定の値を検証し、stampは全項目のID・URLとページ継続の不在を検証する。選択対象が存在していても他の不正stamp項目を無視しない。

うごイラ archive は既定で original、次に medium を選ぶ。指定した画質がなければ別の画質へ切り替えない。metadata・zip_urls objectと非空のframes配列を要求し、frame名の空文字・NUL・絶対パス・ドライブ記号・空/./.. segment・正規化後の重複を拒否する。archiveの再解決ではdelayの0・負値・欠落・nullをGoと同じく受け入れる。これは公開 `UgoiraMetadata` のマッピングや変換処理の完了を意味しない。

novel_image・novel_file の再解決、saveの全kind・環境の契約、キャンセルは未移植または未検証。うごイラ以外のマッピング途中の失敗時に残るregistry、並行取得、全JSONキー・重複/nullの組合せ、percent-encoded URLの画質導出、認証の全状態、SDKから実TLS通信までの接続、他OSは未検証。共有transportの既存実HTTPテストとSDKのfixture transportテストは別の証拠として扱う。

```text
go test ./sdk/pixiv -run '^TestMigrationOpenResource' -count=1
cargo test -p pixiv-sdk --test artwork_open_resource --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-open-resource` を指定する。

## 公開 UgoiraMetadata と DTO

[ugoira-metadata.json](contracts/ugoira-metadata.json) は Go 版の45ケースを固定する。IDの検証、metadataの構造、archiveの画質と順序、frame名、符号付きint64のdelay、エラーの優先順位、DTO、API要求を比較する。delayの0・負値・欠落・nullも受け入れる。公開SDKのframe名検証はarchive再解決より厳しく、バックスラッシュと先頭が `..` の名前も拒否する。

Rust の `Client::ugoira_metadata` は medium、original の順でarchiveを返し、画質ごとにURL policyを適用する。archiveのURLは生成時にregistryへ登録する。後続のframe検証が失敗しても登録済みURLは残るため、比較テストで失敗後のOpenResourceにmetadata再取得がないことも確認する。正常に返したarchive referenceの再利用は追加のRustテストで確認する。

`UgoiraMetadataDto`・`UgoiraArchiveDto`・`UgoiraFrameDto` は実行時のURLと要求ヘッダーを出力しない。空配列、未知の画質文字列、ゼロreferenceのnull、符号付きdelayも検証する。Rust CLIのうごイラJSON表示もこのDTOを使用するが、Go CLIの全オプション・表示・終了コードとの互換性は未検証である。

この比較はWindows amd64のみ。全JSONキーの大小文字・重複・nullの組合せ、全失敗経路のregistry、並行取得、CLI/MCPの全契約、保存・変換、他OSは未移植または未検証として残す。

```text
go test ./sdk/pixiv -run '^TestMigrationUgoira' -count=1
cargo test -p pixiv-sdk --test ugoira_metadata --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-ugoira` を指定する。

## Resource の原子的な保存

[resource-save.json](contracts/resource-save.json) は Go 版の `SaveResource` と `SaveResourceURL` の40ケースを固定する。新規・空body・上書き・部分読み取り失敗・取消とdeadlineのcause・不正な保存先・禁止URL・非2xxを比較する。保存結果、完成ファイル、進捗のTotal/Done、resource要求回数、bodyのClose回数、一時ファイルの残存を記録する。保存先はテスト用ディレクトリに隔離し、成功時のPathが呼び出し時の文字列と一致することを比較する。

Rust の `resource::SaveOptions`・`SaveProgress`・`SavedResource` と `Client::save_resource`・`save_resource_url` は同じ契約に対応する。metadataの再解決はOpenResourceと共有し、公開操作名をエラーに保持する。完成先と同じディレクトリに一時ファイルを作り、64KiB以下のバッファで転送し、sync後にrenameする。失敗時は完成先を変更せず、一時ファイルを削除する。空bodyも完成ファイルとして保存する。

Go版の進捗は読み取り後、書き込み前に通知されるため、その順序も保持する。不正・欠落のContent-Lengthは0、負値はそのままTotalに使う。bodyのCloseエラーは保存結果を変えない。保存中の読み取り失敗は制御されたLocalStateErrorとし、取消/deadlineの型付きcauseだけを保持する。追加のRustテストは読み取り待機中のfutureを破棄し、既存ファイルの保持、一時ファイルの削除、bodyの解放を確認する。

この比較はWindows amd64のfixture transportのみ。SaveResourceの比較はugoira_archiveのidentityを使っており、全kind・不正identity・API失敗・redirect/TLSとの組合せは未検証。Unixの0700/0600、symlink・特殊path・disk full・sync/close失敗、並行保存、context/deadline APIとの対応、CLI/MCPからの保存も未検証である。現在のファイル操作は同期的に実行するため、async runtimeへの影響も未検証として残す。

```text
go test ./sdk/pixiv -run '^TestMigrationResourceSave' -count=1
cargo test -p pixiv-sdk --test resource_save --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-save` を指定する。

## Bookmark・follow の form 更新操作

[form-mutations.json](contracts/form-mutations.json) は Go 版の9操作・178ケースを固定する。作品bookmarkの追加/削除と旧名AddBookmark/RemoveBookmark、小説bookmarkの追加/削除、follow/unfollow、廃止済みAI表示設定を含む。ID・restrictの検証優先順位、publicの既定値、大小文字と空白の扱い、重複・空・Unicode・制御文字を含むtags、送信method/path/form、HTTPエラーのSDKフィールドと要求回数を比較する。formのキー順は正規化し、同じキーの複数値の順は保持する。

Rust の `pixiv` は各request型と操作に対応し、旧名wrapperも固有の操作名を保持する。Goの別名requestはRustのtype aliasで表現する。Restrictは未知値も表現できるStringとし、追加操作の境界でpublic/privateを検証する。削除ではrestrict/tagsを送信しない。SetAIArtworkVisibilityはVisibleの値にかかわらず通信せずContentUnavailableを返す。

更新は `transport::Transport::post_form` を通し、2xxの本文をJSONとして解釈しない。空本文・不正JSON・エラーらしい本文でもHTTP成功を保持する。429のRetry-Afterを読み取り用GETと同じ自動再送には使わず、retryのSafeだけを既存SDK分類に従って返す。ローカル実HTTPの6ケースはform encodingと空/不正JSON・204・429を検証する。追加テストはvirtual timeで設定した送信間隔を確認する。OAuthのJSON応答処理は既存契約テストで引き続き検証する。

この比較はWindows amd64のみ。bookmarkのオフラインread-backは次節で検証する。CLI/MCPの操作・出力、followのread-back、全認証状態・取消/deadline・TLS/DNS・redirect・実サービスの状態変更・他OSは未検証。HTTP成功だけで実サービスの状態変更を証明したとは扱わない。

```text
go test ./sdk/pixiv -run '^TestMigrationFormMutations' -count=1
cargo test -p pixiv-sdk --test form_mutations --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-mutation` を指定する。

## Bookmark 詳細とオフライン read-back

[bookmark-detail.json](contracts/bookmark-detail.json) は Go 版の作品・小説bookmark詳細の60ケースを固定する。ID、404、その他のHTTP失敗、JSON型・null・欠落、登録済みtagの選択と順序、重複・空tag、未知restrictを比較する。未bookmarkの応答では作品自身のtagを返さず、restrictも空にする。404・欠落またはnullのbookmark_detailも空状態になる。未bookmarkを示す応答でも、既知フィールドの型が不正ならマッピング前に拒否する。

Rust の `artwork_bookmark`・`novel_bookmark` は対応するrequestとdetailを返す。小説detailとDTOは作品用のRust type aliasで表現する。DTOはtagsをコピーし、空tagsをGo版と同じnullで出力する。追加のRustテストはDTO側のtags変更が元のmodelに影響しないことを確認する。

各製品のオフラインread-backは、追加→詳細取得→削除→詳細取得の4要求と2つのDTOを比較する。fixture内の状態を更新するため、更新直後の状態と削除後の空状態を同じclientで確認できる。実サービスに更新要求を送るテストではない。

この比較はWindows amd64のみ。全JSONキーの大小文字・重複・不正UTF-8、実transportとの接続、全認証・取消/deadline、CLI/MCP、followのread-back、実サービスの状態と他OSは未検証である。

```text
go test ./sdk/pixiv -run '^TestMigrationBookmarkDetails' -count=1
cargo test -p pixiv-sdk --test bookmark_detail --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-bookmark` を指定する。

## 小説のブックマークタグ一覧

[novel-bookmark-tags.json](contracts/novel-bookmark-tags.json) は、Go の `UserNovelBookmarkTags` の38ケースを固定する。ID、restrict、cursor の検証順序と通信前の拒否、送信先と query、HTTP エラー、必須の bookmark_tags 配列、タグ名・件数の型、DTO と空の next を比較する。restrict の空文字は public に変換せず、そのまま送信する。

Rust の `user_novel_bookmark_tags` は `UserNovelBookmarkTagsRequest` を受け取り、`Page<BookmarkTag>` を返す。Go の対応環境における int を i64 で表現し、負の件数や最大値、重複タグ、配列順、空白だけの名前を保持する。欠落・null の件数は 0。空の配列は成功するが、配列の欠落・null や空の名前は malformed_upstream_response。next_url は null または欠落だけを受け入れ、空文字でも拒否する。継続が非対応という Go の契約を維持し、非ゼロ cursor は要求を送らず invalid_cursor にする。`BookmarkTagDto` は model の名前をコピーする。

比較対象は Windows amd64 の fixture transport。作品のブックマークタグ一覧とそのページ送り、全 JSON キーの大小文字・重複・不正 UTF-8、実 transport との接続、全認証・取消/deadline、CLI/MCP、実サービス、他 OS は未検証。

```text
go test ./sdk/pixiv -run '^TestMigrationNovelBookmarkTags' -count=1
cargo test -p pixiv-sdk --test novel_bookmark_tags --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-bookmark-tags` を指定する。

## 作品のブックマークタグ一覧とページ送り

[artwork-bookmark-tags.json](contracts/artwork-bookmark-tags.json) は Go の61ケースを固定する。必須配列・名前・件数、next_url の host/path/query と正の offset、cursor の product/operation/binding/query digest・payload と検証順序、送信 query、各ページの DTO と cursor 文字列を比較する。先頭→空の中間→最終の3ページも同じ client で取得する。

Rust の `user_artwork_bookmark_tags` は `UserArtworkBookmarkTagsRequest`（小説用 request の type alias）を受け取る。next_url は継続 offset だけを取り出し、元の user_id・restrict を保持して次の要求を組み立てる。次の URL に書かれた異なる user_id・restrict は採用しない。cursor は Go と同じ query digest と payload 形式を使い、保存済みの文字列を受け入れる。binding の不一致や不正 payload、0 以下の offset は通信前に拒否する。

Windows amd64 の fixture 比較であり、全 JSON キーの大小文字・重複・不正 UTF-8、URL の全構文境界、実 transport・全 HTTP/retry 条件、全認証・取消/deadline、CLI/MCP、実サービス、他 OS は未検証。

```text
go test ./sdk/pixiv -run '^TestMigrationArtworkBookmarkTags' -count=1
cargo test -p pixiv-sdk --test artwork_bookmark_tags --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-artwork-tags` を指定する。

## ページ URL と detail の入力・エラー

[page-references.json](contracts/page-references.json) は Go の `ParseURL` と `Reference.CanonicalURL` の68ケースを固定する。6種の reference、所有ユーザー ID、locale、前後空白、host・userinfo・port、percent encoding、query の ID 重複、整数境界、未知 kind と canonical URL を比較する。Rust は `reference::parse_url`・`Reference::canonical_url` に対応し、`ReferenceKind` は未知値を表現できる String。URL の元文字列・query・fragment は reference に保持しない。パスを正規化して別のリソースとして解釈しない。

[detail-input.json](contracts/detail-input.json) は、Go の detail command と本番の exit/error 出力処理で122ケースを固定する。client factory と artwork fetch はテスト側の実行ポートへ差し替え、解析後の ID・factory 呼び出し回数を記録する。有効な作品入力は認証エラー、不正な入力は factory を呼ぶ前の入力エラーを返す。これは実認証・アカウント保存の比較ではない。

Rust は CLI バイナリを子プロセスとして実行し、stdout・stderr・終了コードを比較する。明示 JSON は object key 順だけを正規化し、通常出力は文字列を比較する。無効な入力には無効な proxy 設定も与え、入力エラーが client 構築より先に返ることを確認する。SDK の invalid_argument は Go と同じ終了コード 1。引数・flag の usage error は別の契約として未検証。

この比較は Windows amd64。Go と URL ライブラリの全構文差、Unicode 空白・不正 UTF-8、detail の成功出力・TTY・JSON/NDJSON・record/text pipeline・BrokenPipe、flags/aliases、novel/user と content、全設定・認証・通信、MCP と他 OS は未検証。既存の試作 `reference::artwork_id` は ugoira で引き続き使われており、この入口の URL 互換も未検証。

```text
go test ./sdk/pixiv -run '^TestMigrationPageReferences' -count=1
go test ./internal/cli -run '^TestMigrationDetailInput' -count=1
cargo test -p pixiv-sdk --test page_references --locked
cargo test -p pixiv-cli-rs --test detail_input --locked
```

基準を意図して更新する場合だけ対応する Go テストの `-args -migration-update-reference` または `-args -migration-update-detail-input` を指定する。

## 作品 detail の成功出力

[detail-output.json](contracts/detail-output.json) は Go の実 SDK と detail command を組み合わせた72ケースを固定する。通常表示・JSON・NDJSONそれぞれで、既存の作品 DTO/エラー fixture と追加の HTML caption を比較する。JSON は DTO の数値 ID、NDJSON は文字列 ID・type・canonical URL を含む record。未知 artwork kind は NDJSON record へ変換せず、Go と同じ command error を返す。JSON のキー順だけを正規化し、通常表示は byte に相当する文字列を比較する。

Rust の CLI は `artwork_detail` を呼び、実 SDK client と output writer を渡す。SDK transport の差し替えは crate の `tests/` に置く。Human モードでは caption の HTML を scraper で文書として解析し、script/style を除外、br と block 要素の改行を保持する。表示できない文字は Go の QuoteToGraphic に対応するエスケープを使う。JSON/NDJSON では caption の元の markup を保持する。

この比較は Windows amd64 の SDK→CLI出力処理。実バイナリの成功通信、TTY、設定による JSON 選択、record/text input pipeline、集約 JSON、BrokenPipe・writer failure、全 HTML/Unicode version の差、flags/aliases とその衝突エラー、MCP、他 OS は未検証。今回追加した `--ndjson` と `-j` の引数契約全体も未検証。CLI の入出力・エラーを揃えただけで作品 detail の全入口が完成したとは扱わない。

```text
go test ./internal/cli/commands/pixiv/detail -run '^TestMigrationArtworkDetailOutput' -count=1
cargo test -p pixiv-cli-rs --test detail_output --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-detail-output` を指定する。

## detail の writer failure と終了コード

[detail-writer.json](contracts/detail-writer.json) は Go の detail command と本番の終了処理を組み合わせた27ケースを固定する。通常表示・JSON・NDJSONについて、BrokenPipe、権限エラー、その他の writer failure を、先頭から失敗・8 byte 後に失敗・成功の3条件で比較する。テスト用の作品は SDK が返す illustration と同じ RawKind を持つ。部分出力、診断、終了コードを記録し、NDJSON 中の BrokenPipe だけは診断なし・終了コード0、通常表示と JSON の BrokenPipe および他の writer failure は終了コード1とする。

Rust は実 SDK client から `artwork_detail` を呼び、本番バイナリも使う `finish_command` で終了条件を比較する。出力エラーの io::Error と種別を保持し、SDK のエラーに置き換えない。比較用の writer と transport は crate の `tests/` に置く。成功時の JSON と診断 envelope の object key 順だけを正規化し、部分出力は文字列をそのまま比較する。

Windows amd64 の注入した writer error に対する契約であり、OS の実パイプを使う子プロセス、SIGPIPE、TTY、stderr 自体の失敗、Search/Ugoira の全出力契約、他 OS は未検証。前節の writer failure の未検証範囲は、この27ケースを除く実プロセス・OS境界に残る。

```text
go test ./internal/cli -run '^TestMigrationDetailWriter' -count=1
cargo test -p pixiv-cli-rs --test detail_writer --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-writer` を指定する。

## MCP illust_detail の schema とハンドラー

[mcp-detail.json](contracts/mcp-detail.json) は Go の実 MCP セッションで tools/list と tools/call を実行し、33ケースを固定する。作品応答17ケースと入力16ケースを使い、schema、取得前の exactly-one 検証、URL の種別、SDK エラー、未知 kind の拒否、成功・失敗の text content と structured records、pool の実行回数と HTTP 要求回数を記録する。SDK には既存の作品 fixture を直接返す transport を渡す。JSON-RPC connection の応答を client の structured content 再デコード前に捕捉し、テスト用モデル再変換や client 側の数値変換を期待値に混ぜない。

Rust の `pixiv-mcp` crate は同じ SDK と fixture transport を使い、ハンドラーへ到達する32ケースと tool metadata を比較する。成功では text content は件数の要約だけとし、完全な作品は structured content に置く。失敗でも空の records を返し、isError を立てる。`pixiv-record` crate で canonical record の生成を共通化し、CLI の NDJSON と MCP の双方から使う。共有 record 自体は既存 Go 出力の17ケースでも比較する。

Go の MCP サーバー wrapper は structured content を float64 経由で変換するため、大きな数値の user ID は丸められる。Rust の MCP 出力もこの wire の振る舞いを保つ。共有 record、文字列の record ID・URL、opaque resource reference、CLI の DTO/NDJSON の数値はこの変換に含めない。

数値の illust_id に int64 最大値を指定した1ケースは、Go MCP の入力変換で handler 前の JSON-RPC invalid params となる。この応答も固定するが、Rust の比較対象は現在ハンドラーと metadata に限り、この1ケースは未検証。Rust の stdio/JSON-RPC セッション・tool 登録・schema 検証・initialize・通知・キャンセル・並行処理、pool lease と credential 更新、他53 tools、他 OS は未移植または未検証であり、MCP 実行環境が完成したとは扱わない。

```text
go test ./internal/mcpserver/pixiv -run '^TestMigrationMCPArtworkDetail' -count=1
cargo test -p pixiv-mcp --test artwork_detail --locked
cargo test -p pixiv-record --test artwork --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-mcp-detail` を指定する。

## MCP JSON-RPC・stdio の detail セッション

[mcp-rpc.json](contracts/mcp-rpc.json) は Go の JSON-RPC connection を直接使い、30ケースを固定する。3版の protocol negotiation と未知・空版の fallback、ping、unknown method/tool、logging/setLevel、必須 params、arguments の省略/null/配列/型違い、未知 field、整数として表せる小数、大きな数値 ID の拒否、成功 record、通信中の cancellation を比較する。初期化には Go の serverInfo・instructions・capabilities を保持する。Go SDK の unknown method は code 0、logging の未知・空 level は正常応答であり、一般的な実装へ置き換えて期待値を変えない。

Rust の `stdio::serve` は改行区切り JSON-RPC を読み、応答の ID を保持し、通知へ応答しない。独立した tool future を進め、進行中の呼び出しがある場合も ping へ応答する。cancellation 通知では該当 future を中止し、SDK transport の future を解放する。停止中の fixture transport を使う双方向セッションで ping・cancel・解放を検証する。通常 EOF は処理中の応答を排出して終了する。

前節の33ケースも stdio から比較し、ハンドラー前の int64 最大値の拒否を含めて確認する。CLI バイナリの `mcp` コマンドはこの runtime を使う。子プロセスで初期化・tools/list・入力エラー・認証エラー・EOF を実行し、stdout が JSON-RPC のみであること、stderr と終了コードを確認する。

tools/list の Rust 登録は現在 illust_detail と search_illust であり、全54 tools の登録比較は未完了。全 JSON-RPC 構文・不正 UTF-8・重複 key・params/schema の全型と境界・複数の不正 field の診断順、全 ID 型と重複・再利用、開始前の cancel・deadline・disconnect・SIGINT、ログ通知・progress・全並行処理、認証保存/pool lease/credential 更新、CLI の proxy flags と設定・起動エラー、他 OS は未移植または未検証。前節の「stdio/initialize と数値 ID 拒否は未検証」は、ここに示すケースの範囲で解消した。

```text
go test ./internal/mcpserver/pixiv -run '^TestMigrationMCPRPC' -count=1
cargo test -p pixiv-mcp --test stdio --locked
cargo test -p pixiv-cli-rs --test mcp_stdio --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-mcp-rpc` を指定する。

## SearchArtworks の検索条件・ページ送り・checkpoint 基準

[search-artworks.json](contracts/search-artworks.json) は固定 Go SDK の118ケースを記録する。検索語、target、sort、期間、日付、content type、AI mode、縦横比、解像度、tool、bookmark 範囲、cursor context の既定値・有効値・不正値を含む。実際の HTTP query、完全な Artwork DTO、次ページの cursor、エラー理由とメッセージを保存する。既存の作品詳細 fixture も検索応答に使い、返却項目の欠落を確認できるようにする。検索語と context の特殊文字、および検索語→target→sort、日付→bookmark 範囲の検証順も固定する。

AI の only は取得後のローカルフィルターであり、checkpoint の消費位置はフィルター後の件数に対して適用する。途中ページがフィルターで空になっても次ページは残る。checkpoint の加算・超過・整数 overflow、検索語と cursor context の変更、旧 binding と別 continuation kind を含む。OAuth で確認したアカウントと未確認の client instance の双方で、同一・別 client による cursor 再利用を検証する。ランダムな instance ID だけを固定文字列へ正規化し、再利用の成否は実際の異なる client で確認する。

Rust の外部テストは同じ118ケースで、query・DTO・cursor envelope と payload・エラーを比較する。`SearchArtworksRequest` と `search_artworks` は検索条件を受け取り、items と next を含む Page を返す。`checkpoint_search_artworks` は元の query と現在の cursor から消費位置を加算する。HTTP query と cursor の digest は別に組み立て、ローカルフィルター・解像度・context を検索条件の binding に含める。上流の next_url は許可した検索 endpoint と query key を検証して offset だけを取り出し、URL 自体は再実行しない。

`Client::from_credentials` は OAuth credentials のアカウント ID を設定する。有効期限は credentials の metadata に保持し、固定 Go 版と同様に client の自動期限判定へ設定しない。OAuth 応答の ID は文字列と整数の双方を受け入れる。access token だけから作る client はアカウントを推測せず、ランダムな非秘密の instance ID に cursor を結び付ける。Rust テストでも正規化前の instance ID の形式を確認し、別 client での再利用の拒否を比較する。

CLI の試作検索も型付き request を使うように更新したが、現在は先頭ページを表示する。CLI/MCP の全検索条件・出力・ページ上限、全 JSON wire 境界・cursor payload の大文字小文字や重複 key・複数不正値の全検証順、乱数生成失敗時の constructor の契約、通信・認証保存・他 OS は未実装または未検証。SDK の検索互換全体を検証済みとは扱わない。

```text
go test -race ./sdk/pixiv -run '^TestMigrationSearchArtworks' -count=1
cargo test -p pixiv-sdk --test search_artworks --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search` を指定する。

## MCP search_illust の schema・論理ページ・ローカルフィルター

[mcp-search.json](contracts/mcp-search.json) は Go の実 MCP セッションを144ケースで実行する。tool metadata、必須 word、enum・date pattern・minimum・未知 field・入力型・大きな数値の拒否、ハンドラーの検証順、全 SDK 検索条件、論理 page/limit、作品フィルターと bookmark strategy を固定する。SDK には複数ページの fixture を返す HTTP transport を渡し、pool の実行回数・HTTP query・JSON-RPC の生の tool result を記録する。

Rust の外部テストは同じ144ケースを stdio から実行し、schema、検索前の JSON-RPC error、tool result、全 HTTP query を比較する。検索は SDK の型付き request を使い、通常の経路では作品の kind と ID で重複を除き、id・type・全タグ・view/page 件数の条件を論理 skip/limit より前に適用する。limit 省略は最初の非空の論理バッチ、0 は全件を表す。先頭ページが空、または AI フィルターで空になった場合も次ページを取得する。後続ページの失敗では途中までの records を返さず、既定のエラー用 pagination を返す。

bookmark 範囲の auto は local として扱い、App query の bookmark bounds を外して候補を取得し、各作品の公開 bookmark 件数で判定する。best_effort は上流 bounds を残してローカルでも確認する。範囲と採用 strategy を cursor context の digest に含め、バッチ内の limit 到達では SDK checkpoint を作る。filter metadata は範囲・membership unknown・実際の strategy・partial/complete_for_source を返す。server は根拠のない fallback をせず、Go と同じ upstream_unavailable を返す。

範囲なしでも bookmark_strategy を明示すると、Go は通常経路の作品重複除去と illust_filter を適用しない。この経路も比較し、通常検索の処理へ置き換えて挙動を変えない。records の canonical identity と DTO は共有 crate を使い、MCP の structured numbers の float64 変換を pagination と filter にも適用する。

stdio と CLI 子プロセスの tools/list には illust_detail と search_illust を登録する。他52 tools、検索中の cancellation/deadline・pool の再実行と credential 更新・全 account 状態、全 JSON wire 境界と複数不正 field の診断順、cursor 循環の診断、長期 duration の半年/一年と日付境界、他 OS は未移植または未検証。長期 duration の展開は実装しているが、この固定 fixture は実時刻に依存する正常系を含まない。Rust の HTTP query 比較から Go の pool 実行回数の互換まで検証済みとは扱わない。

```text
go test -race ./internal/mcpserver/pixiv -run '^TestMigrationMCPSearch' -count=1
cargo test -p pixiv-mcp --test search_illust --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-mcp-search` を指定する。

## CLI/MCP の半年・一年の日付範囲

[search-date-inputs.json](contracts/search-date-inputs.json) の固定時刻を Go の CLI と MCP の実際の quickDateRange helper に渡し、それぞれ [search-dates-cli.json](contracts/search-dates-cli.json) と [search-dates-mcp.json](contracts/search-dates-mcp.json) に168ケースを記録する。東京の午前0時の前後、UTC・UTC-8・UTC-12・UTC+14、月末、うるう日、1900/2000年、年0・年9999の境界、未対応 duration を含む。実時刻を期待値に含めず、Go の本番 helper は変更しない。

共通のアプリケーション処理用 `pixiv-app` crate を追加し、`dates::quick_date_range` を Rust の外部テストで双方の Go 結果と比較する。半年・一年では東京の日付へ変換し、同じ日号が対象月にない場合は月末へ丸める。それ以外の duration は明示日付へ展開しない。年の表記は Go と揃え、負の年は符号の後に最低4桁、10000年は余分な `+` を付けずに表す。元の MCP の `%Y` に任せる実装では、この10000年の表記が異なる。

MCP の実行処理は現在時刻を共通 helper に渡し、日時計算の重複を除いた。前節の「半年/一年と日付境界が未検証」は、この168ケースの日付計算について解消した。CLI の検索 flags への接続は次節で検証する。長期 duration の実 MCP セッションから SDK HTTP query までの比較、clock の極端な範囲・全 timezone・他 OS は未実装または未検証。Go の汎用 AddMonthsClamped の全範囲を移植済みとは扱わない。

```text
go test -race ./internal/cli/commands/pixiv/search ./internal/mcpserver/pixiv/tools/search_illust -run '^TestMigrationQuickDateRanges' -count=1
cargo test -p pixiv-app --test search_dates --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-dates` を指定する。Go の CLI と MCP の package は別々の出力ファイルを更新する。

## CLI の検索日付 flags

[search-date-options.json](contracts/search-date-options.json) は固定 Go 版の実際の `search` コマンドを Cobra root に登録して取得した213ケース。日・週・月の period、片側・両側の日付、うるう日、年0/9999、空白除去、不正な書式・日付・順序、period と日付の併用、複数の不正値に対する検証順を human/JSON/NDJSON の3モードで記録する。Pooled 実行回数、fixture HTTP の GET・path・全 query、stdout/stderr、終了コードを保存する。Go の本番コードと時計は変更しない。応答は作品0件・継続なしを表す `{"illusts":[]}` とする。

Rust CLI に `--period`、`--start-date`、`--end-date` を追加し、クライアント設定前に解決する。半年・一年は前節の共通 helper を使い、その他の期間は SDK の duration に渡す。`SearchDateOptions` は本番の Clap 引数と SDK request 作成に使い、テスト用の API は追加しない。

外部テストは213ケースの adapter 検証結果を比較し、正常33ケースでは実際の Rust SDK を fixture transport へ接続して Go の全 query と比較する。異常174ケースは Rust CLI の子プロセスで、stdout・stderr・終了コードを比較する。無効な proxy を設定し、日付検証がクライアント設定より先に行われることも確認する。NUL を含む6ケースは OS の argv で表現できないため adapter の検証だけを比較する。負の年の入力では Clap の既定の flag 解釈が Go と異なったため、日付 flags はハイフンで始まる値も受け取り、日付検証エラーを返す。

半年・一年の42ケースは固定時刻を渡し、共通 helper の Go 基準結果と SDK request の日付・空 duration を比較する。CLI の本番時計をテスト専用に差し替えない。

この比較は Windows amd64 のみ。正常時の CLI 表示・出力形状、実 CLI 子プロセスから HTTP までの正常系、全 flags と entity、複数ページ、設定・account/pool、入力パイプ、時計と SDK の全境界、他 OS は未実装または未検証。fixture に保存した Go の正常 stdout を Rust との比較済みとは扱わない。

```text
go test -race ./internal/cli -run '^TestMigrationSearchDateFlagsValidateBeforeAccountAndMatchQueries$' -count=1
cargo test -p pixiv-cli-rs --test search_dates --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-date-options` を指定する。

## CLI の作品検索条件

[search-options.json](contracts/search-options.json) は固定 Go 版の実際の CLI・SDK を接続した210ケース。`search-by`、`sort`、`content-type`、`ai-mode`、`aspect-ratio`、`resolution`、`draw-tool` の既定値・正常値・不正値と、複数条件や日付を組み合わせた検証順を human/JSON/NDJSON の3モードで記録する。期待値は本番の Go コマンドと SDK から取得し、Go の本番コードは変更しない。

Rust の `SearchOptions` は本番の Clap 引数を受け取り、型付き SDK request を作成する。content-type は Go と同じ空白除去と小文字化を行い、`illustration` を `illust` に対応させる。大文字 I に点が付いた `İLLUST` も含めて比較する。AI・aspect-ratio・resolution と search-by は文字列を厳密に判定する。draw-tool は Go と同じく値をそのまま query に渡し、表示されているカタログにない値も CLI では拒否しない。

検証順は search-by、content-type、AI、aspect-ratio、resolution、period・日付とする。不正 sort は Go と同じく adapter では拒否せず、SDK が通信前に返す `invalid_argument` を維持する。空 sort は SDK の既定値に対応し、`popular_desc` も受け入れる。

111ケースで fixture transport へ実際の Rust SDK を接続し、Go の GET・path・全 query と比較する。99ケースでは CLI 子プロセスの stderr・stdout・終了コードを比較する。adapter が拒否する87ケースは無効な proxy を設定し、SDK の sort エラー12ケースは空 token と proxy なしで通信前のエラーを確認する。Go の Pooled 実行回数と adapter の解決可否も比較するが、account/pool の移植済みとは扱わない。

Windows amd64 の fixture 比較であり、正常時の実 CLI 子プロセスから HTTP・表示までの接続、AI only と全条件を組み合わせた実 CLI の全ページ、rating・bookmark のローカル処理、残りの flags・entity・入力パイプ、設定・account/pool、全 Unicode の大小文字、他 OS は未実装または未検証。検索コマンド全体の完了条件は満たしていない。

```text
go test -race ./internal/cli -run '^TestMigrationSearchSelectorsMatchFrozenQueriesAndValidationOrder$' -count=1
cargo test -p pixiv-cli-rs --test search_options --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-options` を指定する。

## 共通のローカル rating・作品種別フィルター

[search-local-filter.json](contracts/search-local-filter.json) は Go の実際の `searchfilter.NormalizeFilter`、`CursorContext`、`Matches` から取得した168ケース。rating と content-type の空値・既定値・別名・空白・大小文字・不正値、および両方が不正な場合のエラー優先順位を固定する。正常な80ケースでは、6種類の `x_restrict` と7種類の作品種別を組み合わせ、3,360件のマッチ判定を記録する。未知の制限値は all で保持し、明示的な rating では除外する。

共通の `pixiv-app::search_filter` に正規化、ローカル判定、`filter/v1` と canonical rating/content-type を SHA-256 に渡す cursor context を実装した。空の作品種別と illustration は illust として扱う。aggregate の illust-and-ugoira は manga を含まない。不正な rating は content-type の検証より先に拒否する。

CLI の既存 content-type 正規化と、MCP の illust_filter によるローカル種別判定をこの処理へ接続した。MCP の呼び出しでは rating は all とし、正規化は作品ごとに繰り返さず1回の検索で共有する。既存 CLI の検索条件・日付テストと MCP の144ケースも回帰検証する。

この比較は Windows amd64 の合成値のみ。CLI の rating flags、ローカル判定後の論理ページ・重複排除、cursor context の SDK request への接続・resume、bookmark との組合せ、Go の直接構築された不正な Filter・零値、全 Unicode、他 OS は未実装または未検証。共有 helper の実装を検索コマンド全体の移植完了とは扱わない。

```text
go test -race ./internal/shared/searchfilter -count=1
cargo test -p pixiv-app --test search_filter --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-filter` を指定する。

## CLI の rating と論理ページ収集

[search-pages.json](contracts/search-pages.json) は固定 Go 版の実際の CLI コマンド、共有 pagination、SDK を接続した90ケース。3ページの fixture 応答に sfw/r18/r18g/未知の x_restrict と複数の作品種別、同一ページ内・ページ間の重複を含める。rating と content-type、limit の省略・明示0・正数、論理 page、先頭の空ページ、後続の malformed 応答、入力不正と offset overflow を検証する。

Rust CLI に `--rating`、`--limit/-l`、`--page/-p` を追加した。rating は content-type より先に正規化し、all 以外の条件では共通 filter の cursor context を SDK request に設定する。rating/x_restrict は HTTP query に追加しない。ローカル判定後に skip/limit を適用し、limit の省略では最初に結果のある upstream batch、明示0では全ページを返す。Go CLI のローカル収集は重複作品を保持するため、MCP の重複除去処理をそのまま流用しない。

90ケースの adapter 検証結果・全 GET/path/query と、成功76ケースの実際の JSON presenter 出力を Go と比較する。後続ページの malformed 応答4ケースでは収集結果を返さず、JSON の部分結果も公開しない。入力不正10ケースは無効な proxy を設定した CLI 子プロセスで stderr・終了コードを比較し、クライアント設定前に拒否することを確認する。JSON は `{"illusts":[...]}` envelope とし、HTML 文字を Go と同じく escape する。比較は JSON の構造・値・配列順を維持して行う。

Windows amd64 の fixture 比較であり、正常 CLI 子プロセスから実 HTTP までの接続、実 cursor の resume・循環検出、checkpoint の副作用、全境界・取消/deadline、account/pool 再実行、bookmark の全組合せ、他 OS は未検証。CLI/MCP の単一流 traversal は末尾の共通 pagination に移した。出力と非ローカル JSON spool の検証範囲は次節に記録する。

```text
go test -race ./internal/cli -run '^TestMigrationSearchRatingAndPages' -count=1
cargo test -p pixiv-cli-rs --test search_pages --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-pages` を指定する。

## CLI 検索の出力・JSON spool と出力失敗

[search-output.json](contracts/search-output.json) は前節の fixture 12種類を参照し、human/NDJSON の72ケースと JSON の48ケースを合わせた固定 Go 版の120ケース。正常 writer・通常の書き込み失敗・BrokenPipe、JSON では10バイトだけ書き込んで失敗する writer を組み合わせ、全 HTTP query、stdout/stderr、エラー、終了コードを比較する。human は見出し・URL・引用された title・作者・bookmarks/views/tags を byte 一致で確認し、NDJSON は既存の共通 record 変換を使い、各行の構造と値・行順を比較する。JSON は構造・値・配列順、途中書き込み時は既知の envelope の10バイト prefix を比較する。

非ローカル検索はページを取得するたびに出力し、後続の malformed 応答があっても先行出力を保持する。rating によるローカル検索は収集の成功後に出力し、後続ページの失敗時には stdout が空のままとなる。先頭の空ページの補充、論理 page、重複保持、ローカル結果が空の場合の human 見出しも確認する。書き込み失敗後の追加要求停止、human の BrokenPipe が失敗・明示 NDJSON の BrokenPipe が成功となる終了コードと診断も比較した。

非ローカル JSON は取得前にランダム名・排他的作成の一時ファイルを開き、各ページの DTO を逐次書き込む。ページをまたぐ DTO の配列は保持せず、全取得成功後に固定サイズの copy buffer で stdout に転送する。Go 側は隔離した temp directory で取得中の spool と終了後の削除を確認した。Rust 側も隔離した子プロセスで、取得前の spool・後続要求時に先行ページが既にディスク上にあること・成功/後続エラー/出力失敗/future の取消後の削除を確認する。ローカル rating 検索は Go と同じくメモリ収集を維持する。Unix では作成 mode を0600にしているが、Unix 実行と Windows ACL は未検証。

検証は Windows amd64 の CLI adapter と実際の終了処理に限定する。正常の CLI 子プロセス・TTY/pipe の自動切替、全 Unicode の引用、human/NDJSON の partial writer、未知の kind/不正 ID による record 変換失敗、SDK の取消/deadline、pool の replay/commit 境界、bookmark の全組合せ、他 OS は未検証。temp directory 不正・容量不足・seek/read/delete 失敗、プロセス強制停止後の回収、長時間・大容量時の実測も未検証。

```text
go test -race ./internal/cli -run '^TestMigrationSearchOutput' -count=1
cargo test -p pixiv-cli-rs --test search_output --locked
cargo test -p pixiv-cli-rs --test search_spool --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-output` を指定する。

## CLI 検索の bookmark 条件と continuation binding

[search-bookmark.json](contracts/search-bookmark.json) は固定 Go CLI の393ケース。3種類の件数範囲、strategy の省略/auto/local/best_effort/server、limit の省略/0/2と論理 page、rating の省略/r18、human/NDJSON/JSON を組み合わせた360ケースに、入力不正27ケース・負の bookmark 件数と後続 malformed 応答6ケースを加える。前節のページ fixture の件数を ID×10 に変更した合成データを使い、全 GET/path/query、出力、エラー、終了コードを比較する。

Rust CLI に `--bookmark-min`、`--bookmark-max`、`--bookmark-strategy` を追加した。未指定と明示0を区別し、日付→件数範囲→論理ページ→strategy の検証順を維持する。auto は local に解決する。local は通常の候補流から件数を判定し、best_effort は upstream に件数範囲を送ってからローカルでも判定する。server は固定 Go 版と同じく検証済み premium evidence がないエラーで拒否する。strategy だけの指定は範囲不足のエラーとする。

件数と rating の判定後に skip/limit を適用し、同一作品の重複を保持する。途中切り詰めでは実際に消費した upstream position の SDK checkpoint を作成する。JSON に min/max・membership・実際の strategy・partial/complete_for_source を含む filter metadata を出し、全出力モードで後続失敗時の部分結果を公開しない。入力不正18ケースは無効な proxy を設定した CLI 子プロセスでも比較し、クライアント設定より前に拒否することを確認する。

[bookmark-context.json](contracts/bookmark-context.json) の192ケースで、Go の BookmarkContext と CLI の context 結合を比較する。nil/0/10/64bit最大値、strategy、先行の rating context を固定し、結合順を維持する。共通 helper を CLI と MCP から使い、MCP の既存144ケースも全 Rust チェックで再検証する。

Windows amd64 の fixture 検証であり、正常 CLI 子プロセスから実 HTTP、SDK checkpoint の失敗・実 cursor の resume/別query拒否・循環、全入力組合せ、bookmark と全 content-type/他 selector の組合せ、writer の途中失敗、取消/deadline、pool/account/premium 状態、巨大な結果、他 OS は未検証。単一流 traversal は共通化したが、strategy 解決の MCP/CLI 共通化は残る。JSON の object key 順だけ正規化し、フィールドの省略と値・配列順・metadata は維持する。

```text
go test -race ./internal/cli -run '^TestMigrationSearchBookmark' -count=1
go test -race ./internal/cli/commands/pixiv/search -run '^TestMigrationBookmarkContexts' -count=1
cargo test -p pixiv-cli-rs --test search_bookmark --locked
cargo test -p pixiv-app --test bookmark_context --locked
```

基準を意図して更新する場合だけ Go テストにそれぞれ `-args -migration-update-search-bookmark`、`-args -migration-update-bookmark-context` を指定する。

## 共通の単一流 pagination

[traversal.json](contracts/traversal.json) は固定 Go の TraversePages と CollectFilteredPagesFrom を直接実行した840ケース。負数/0/正数の skip/limit、one-batch、通常/奇数 predicate、後続 fetch・predicate・checkpoint・consume の失敗、零 checkpoint、空の循環 batch を組み合わせる。結果と順序、全 cursor 要求、checkpoint の消費位置・next cursor、returned/has_more、エラーを比較する。失敗時の nil と成功時の空配列を区別し、通常 traversal の出力済み部分と進捗を filtered collection の破棄結果と混同しない。

Rust の [pagination.rs](../../crates/pixiv-app/src/pagination.rs) を CLI の通常/rating/bookmark 検索と MCP 検索から呼ぶ。SDK cursor は decode・再生成せず cursor trait で扱い、切り詰め時だけ入口の SDK checkpoint を呼ぶ。CLI の streaming/spool とローカル結果の出力、MCP の重複除去と strategy の特殊条件は adapter に保持する。MCP の limit 省略の内部値は上限0と one-batch に変換する。CLI の90/120/393ケースと MCP の144ケースを期待値を変更せず再検証した。

この840ケースは単一流・zero initial cursor の Windows amd64 fixture 検証である。複数流と合成 cursor の再開は次節で検証する。実 SDK cursor の循環/checkpoint失敗・再開、取消/deadline、pool replay/commit、他 OS は未検証。Rust では callback は型で必須とするため Go の nil callback 診断との対応はまだ記録していない。

```text
go test -race ./internal/shared/pagination -run '^TestMigrationTraversal' -count=1
cargo test -p pixiv-app --test traversal --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-traversal` を指定する。

## 複数流の StreamState と再開

[streams.json](contracts/streams.json) は固定 Go の CollectStreamsFrom を実行した3072ケース。2つの順序付き候補流に重複・空ページを含め、skip/limit/one-batch、predicate の有無、8種類の初期 state、fetch/predicate/checkpoint/zero checkpoint/cursor 循環を組み合わせる。結果の順序・重複、流をまたぐ単一のページ予算、各 cursor・checkpoint の消費位置、current/cursors、returned/has_more、失敗時の nil と進捗の破棄を比較する。

初期 state は先頭・第2流・完了位置・非零のページ内 cursor・不正 current・cursor 数不一致を含む。初回が成功して has_more の場合は、その返却 state を再び同じ Go/Rust の収集に渡し、残りの項目・完了位置・追加要求・エラーも比較する。cursor はテスト transport 内だけで decode し、本番 engine は不透明な値のまま扱う。

Rust の共通 engine に Stream/StreamState/collect_streams を追加し、checkpoint のある単一流 collection をこの engine へ接続した。CLI rating/bookmark と MCP bookmark collection もこの経路を使う。通常の streaming は既存 traverse_pages を使い、CLI の90/120/393ケース・MCP の144ケース・単一流840ケースを期待値を変更せず再検証する。

Windows amd64 の合成 cursor と2流の fixture 検証であり、複数の実 SDK 操作を使う公開コマンドはまだ接続していない。実 SDK cursor の binding/identity・保存と再開、0流/3流以上、全境界、callback 不在の型対応、取消/deadline、pool/account 切替、他 OS は未検証または未実装。

```text
go test -race ./internal/shared/pagination -run '^TestMigrationStreams' -count=1
cargo test -p pixiv-app --test streams --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-streams` を指定する。

## 認証 DB の schema と既存ファイル

[database.json](contracts/database.json) は固定 Go の database.Open を実行した19ケース。新規作成、v3 の再開、旧 v1 の nullable creator_id と schedulable の追加、LF/CRLF と既知の旧 checksum、未設定・異なる application_id、user_version、migration の名前・checksum・版の欠落を比較する。schema と index の SQL、台帳、合成アカウントの UID・順序・token bytes・credential revision・時刻を検証し、失敗後の DB 状態も確認する。

Rust の Database::open は既存と同じ pixiv-cli.db と Go の SQL migration を使う。migration ごとの transaction、schema が既に満たす場合の台帳追加、application_id と user_version を維持する。接続には Go と同じ foreign_keys/synchronous/secure_delete/trusted_schema と待機なしの busy timeout を設定する。接続内 pragma の実行値、lock と並行接続はまだ比較していない。

Go の別テストは隔離した Rust テスト子プロセスを明示実行し、Rust が新規作成したファイル・Go v3 ファイル・Go 旧 v1 ファイルを、Go で再開して同じ結果を確認する。Rust の ignored helper はこの3ケースから実行され、通常の workspace test だけでは実行されない。実ユーザーの home・DB・資格情報は使わない。

SQL テキストの改行だけ LF に正規化し、migration の applied_at は実行時刻なので比較から除く。既存アカウントの created_at/updated_at は除外しない。SQLite driver 固有のエラー文は empty-ledger の失敗箇所までを比較し、同ケースの残存 schema は厳密に比較する。他の明示的な拒否診断は全文を比較する。固定 Go は user_version が3でも連続した第4台帳行を受け入れるため、Rust もこの振る舞いを維持する。これを未知 schema の安全性の証明には使わない。

Windows amd64 の合成 DB 検証である。アカウント repository、refresh CAS、pool lease、設定・CLI/MCP の接続、取消/強制終了復旧、disk/permission/lock 障害、全旧 schema、不正台帳の全組合せ、Windows ACL、Unix mode、他 OS は未移植または未検証。既存の SQL を本番 asset として共有し、テスト fixture は本番依存に含めない。

```text
go test -race ./internal/storage/database -migration-rust-database -count=1
go vet ./internal/storage/database
cargo test -p pixiv-app --test database --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-database` を指定する。Rust/Go のファイル相互検証には両 toolchain が必要であり、`-migration-rust-database` を省略した実行をその証拠には使わない。

## Pixiv アカウント repository と credential revision

[accounts.json](contracts/accounts.json) は固定 Go の repository を実行した31操作の連続した契約。空一覧・未登録 UID、新規保存・再インポート、明示/自動 sort_order、nullable metadata の更新・解除、refresh rotation と古い revision、削除・再保存、空 batch・不正入力・重複 UID・SQL unique 制約による全件 rollback を比較する。再インポートは token と username と revision のみを更新し、metadata・pool 状態・並び順を保持する。新規保存の revision は入力にかかわらず1で、schedulable は true になる。

各操作前に合成 DB の既存 created_at/updated_at を11/22へ制御する。操作後は、それらの保持と操作の開始・終了の Unix 秒内にある更新時刻を区別して検証する。実行時刻だけを -1 に置き換え、他の列・token・null・配列順は正規化しない。既存アカウントの作成時刻の保持もこの比較に含める。一般の時刻/clock 障害は未検証である。

Rust では公開の storage 境界に PixivAccount と repository 操作を追加した。token は private に保持し、入力と返却の copy、Debug の秘匿性を検証する。NotFound/CredentialConflict は型で区別する。明示的な入力・不在・revision 診断は Go と全文一致し、SQLite driver 固有の診断は制約種別と対象列を比較する。driver のエラー文そのものの差は残る。

Go と Rust の8件同時更新試験は、共有する1 connection で成功1件・revision conflict 7件・最終 revision 2 を検証する。Rust は Database が Sync ではないため呼び出し側の Mutex で共有する。これは別 connection/別 process の lock や取消の検証ではない。

実ファイルの相互試験では Go が保存したアカウントを Rust で rotate/metadata 更新/追加し、Go が token・revision・metadata・作成時刻を読み直す。Go がさらに rotate/削除した結果を Rust で確認する。ignored Rust helper は Go の明示フラグから2回実行する。

Windows amd64 の合成 DB に限る。FANBOX repository、pool selection/lease、SQLite の別接続・別プロセス競合、取消・強制停止、refresh 応答の UID 照合と CAS のアプリケーション接続、全境界/障害、config・CLI/MCP、他 OS は未移植または未検証である。

```text
go test -race ./internal/storage/database -migration-rust-database -count=1
go vet ./internal/storage/database
cargo test -p pixiv-app --test accounts --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-accounts` を指定する。

## Pixiv pool の保存状態と chooser

[pool.json](contracts/pool.json) は固定 Go の pool repository と chooser を実行した51操作。round_robin の循環と marker、random の候補数/選択/不正 index/失敗、attempted UID の除外、参加設定の一括更新と検証、凍結期限を短縮しない Freeze、期限切れ解除、status、選択失敗時の transaction rollback を比較する。候補・marker・status と最短凍結時刻、返却 account と保存後の全 account 列、診断を検証する。

選択失敗は no_local_account/no_schedulable_account/all_frozen/exhausted を型と時刻で区別する。status の最短凍結時刻には参加停止中の account も含み、選択失敗の最短時刻は参加中だけを対象にする。選択の callback は純粋な snapshot を受け取り、snapshot 外の UID は拒否する。callback 不在の Go 診断も Rust の Option で維持する。

選択時の期限解除・marker 更新は1つの transaction で扱う。失敗時は期限解除も戻り、成功時の返却 account は Go と同じく更新前の updated_at を保持して pool_last_selected だけ true にする。保存した updated_at は明示的な now に一致する。参加設定は単一の IN query/update を使い、marker を書き換えない。

時刻は前節と同じ11/22の制御を使う。Freeze/参加設定の実行時刻だけ操作 window を検証して -1 にし、選択・status の明示 now と凍結値はそのまま比較する。token を含む account のデータは合成値のみであり、chooser の snapshot は token を含まない。

Go/Rust の追加テストは default random source で候補1件を選ぶことと、空 snapshot では不正 strategy より先に exhausted と earliest を返すことを確認する。Rust の default random は OS entropy と rejection sampling を使う。分布・entropy 障害・実行回数の一致は検証していない。

Windows amd64 の保存層と chooser の比較である。Scheduler の rate-limit/retry/replay/commit、session の lease/refresh とアカウント寿命、取消/並行・別 process・DB 障害、巨大な UID 集合と全境界、不正 strategy の全文字列、config・CLI/MCP の接続、他 OS は未移植または未検証。

```text
go test -race ./internal/storage/database -migration-rust-database -count=1
go vet ./internal/storage/database
cargo test -p pixiv-app --test pool --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-pool` を指定する。

## Pixiv pool Scheduler の安全な再試行と取消

[scheduler.json](contracts/scheduler.json) は固定 Go の Scheduler.Run を実行した97ケース。実 DB を使い、rate limit の Safe/HasAfter/期限、分類と wrapped cause、commit、試行中と開始前の取消、期限切れ、成功、候補枯渇、設定と storage の不備、不正 UID・重複選択、凍結失敗を比較する。選択時の attempted UID、試行順、clock の呼出順、凍結期限、最終 DB の参加・凍結・marker、診断全文・SDK 分類・retry advice・exhausted/取消/期限切れの判定、selected/froze イベントの全フィールドと順序を確認する。合成 storage 障害だけは狭い PoolState 境界から返す。

Rust の Scheduler は async の試行と共有 Attempt を使う。commit 済みの失敗は取消より優先し、試行が成功を返した場合は取消後でも成功を返す。未 commit で SDK が明示的に安全な未来の rate-limit retry を許可した場合だけ次の account を選び、固定の再試行回数は設けない。凍結期限は選択時からの retry duration を、凍結時の clock に加える。これを進む clock のケースでも比較する。時刻・UID・null/空配列を正規化しない。

Go と Rust の追加テストは、実際に待機中の試行へ別タスクから Attempt.commit と取消を通知する。commit は複数回呼んでも戻らず、未 commit の結果は取消、commit 済みの結果は元の SDK エラーとなり、どちらも再選択・凍結をしない。Rust の Context は共有する取消・deadline の通知と最初の原因の保持も検証する。

Windows amd64 の Scheduler 基盤の比較であり、認証 session の lease/refresh、CLI/MCP/config と実通信への接続は未移植。Database adapter の取消は操作前の確認に限り、実行中 SQL の中断、親子 context の deadline・取消伝播、全ての取消競合、別 connection/process、巨大な候補集合、clock の極値、未知 selection kind の全 Unicode 表現、他 OS は未検証である。SDK の wrapped cause はエラーの分類と全文を保つが、任意の Go エラー型との互換を証明するものではない。

```text
go test -race ./internal/storage/database -migration-rust-database -count=1
go vet ./internal/storage/database
cargo test -p pixiv-app --test scheduler --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-scheduler` を指定する。

## 診断 scope と Scheduler のイベント

[diagnostics.json](contracts/diagnostics.json) は固定 Go の診断 scope を実行した64ケース。scope の有無、nil sink、child scope、明示 module、context/Scope の直接通知、取消の組合せを比較する。Event の全13フィールドを検証し、空 module は scope から補い、明示 module は保持し、request ID は常に scope の値で上書きする。child は sink を引き継いで module と request ID だけ変更し、scope がないときは sink を作らない。取消済みでも scope があれば通知を続ける。

Rust の Scope は共有 sink を持ち、Context の scope 派生は取消状態を共有する。sink 不在は無通知で、stdout/stderr へ直接書かない。Event は Go と同じ文字列と符号付き値、request ID は u64、duration は符号付き ns で表す。比較 fixture の大文字フィールド名は Go struct の既定 JSON 名であり、CLI/MCP の出力 schema を追加したものではない。

Scheduler は UID が正の場合に selected を通知し、その後に既試行 UID を拒否する。froze は保存に成功した後だけ通知する。失敗・commit・取消・成功・候補枯渇で通知回数が変わることを、前節の97ケースで比較する。credential と任意のエラー本文はイベントへ渡さない。

Windows amd64 の opt-in sink 境界までの検証である。CLI/MCP 起動時の scope 設定、presenter の表示・秘匿と URL 処理、ネットワーク・認証・ダウンロードなど他 subsystem のイベント、sink の並行実行・障害、フィールドの全境界値、他 OS は未移植または未検証。

```text
go test -race ./internal/shared/diagnostics -count=1
go vet ./internal/shared/diagnostics ./internal/storage/database
cargo test -p pixiv-app --test diagnostics --test scheduler --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-diagnostics` を指定する。

## Pixiv refresh rotation の Gate

[gate.json](contracts/gate.json) は固定 Go の Gate を実行した12ケース。nil/zero の未設定、callback 不在、成功・失敗・callback 内の取消、占有中の取消・期限切れ・待機後の取消、panic 後の再取得を比較する。エラー全文、取消/期限切れの種別、callback の実行有無、処理後の再利用を確認する。未設定の検証が callback 検証より先に行われ、callback で取消しても返却した元のエラーを保持する。

Rust の Gate は共有する1枠の semaphore を使い、clone も同じ枠を待つ。Go の Acquire/Release は Rust の acquire と所有する Permit の drop に対応する。run は callback の終了・エラー・panic で guard を解放し、実際の task abort でも枠が戻ることを Rust の追加テストで確認する。Default は Go の zero Gate に対応する未設定で、new が使用可能な Gate を作る。待機中は Context の取消と deadline を受け付ける。

既に取消済みで空き枠もある場合、固定 Go の select は取得成功と取消の両方を選び得る。Rust も無条件に取消を優先せず、同時に ready の2分岐から選ぶ。この非決定的なケースの選択割合や待機順の公平性は比較していない。Go の未取得 Release の待機・二重 Release は Rust の所有権 API では表現しない。実行中 callback の強制中断や refresh の成果を保証する仕組みではない。

Windows amd64 の Gate 境界の比較である。認証 client/Lease と Gate の接続、refresh UID 検証・token rotation 永続化との接続、CLI/MCP/bootstrap、複数 Gate/別 process の協調、全取消競合・負荷・他 OS は未移植または未検証。

```text
go test -race ./internal/services/pixiv/pool -count=1
go vet ./internal/services/pixiv/pool
cargo test -p pixiv-app --test gate --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-gate` を指定する。

## client の取得・解放と Gate/Lease の接続

[lease.json](contracts/lease.json) は固定 Go の Lease の7ケース。nil、解放関数不在、成功・失敗・panic、8件の同時 Close を比較する。Value は Close 後も保持し、解放関数は1回だけ実行し、各 Close は同じエラー instance を返す。最初の Close が panic した場合も解放済みとなり、以後は再実行せず成功を返す。Rust は共有 Mutex 内で解放を直列化し、panic を記録して lock を解放してから再送出する。Close のエラーは Arc で同じ instance を共有する。nil Lease は Rust の Option 不在に対応する。

[sessions.json](contracts/sessions.json) は固定 Go の Facade.Open の17ケース。Context/Accounts/Gate の不在、zero Gate、占有中の取消、既定/明示/負 UID、opener の失敗・部分 client・nil client・panic、closer の失敗・panic を比較する。client を返す前に Gate を取得し、返した Lease が明示 Close まで枠を保持すること、Close の繰り返しでも1回だけ client と Gate を解放することを確認する。部分 client の open 失敗は close して両エラーを残し、opener/closer の panic 後も Gate を再取得できる。合成 opener と明示 closer は実際の account/client 境界であり、通信を呼ばない SDK client を使う。

Rust の ClientSessions は account opener・Gate・明示 close callback を接続する。user ID と接続 options は opener へそのまま渡し、0 の既定 account 選択は account service の責務として残す。options は generic 型で保持し、比較では AcceptLanguage の受渡しを確認する。SDK の全 Options 対応を証明するものではない。Lease は明示 Close が必要で、単なる drop は close callback を実行しない。Rust の取得待機中 task abort は、まだ client を返していない Gate の解放まで追加テストで検証する。

SessionError は open と close のエラーを順に保持し、全文は Go errors.Join と同じ改行で結合する。errors() は全原因、classified()/is_canceled()/is_deadline_exceeded() は全原因を検索する。Rust std::error::Error の source は先頭原因のみであり、複数原因の全探索はこれらの API を使う。

Windows amd64 の client lifetime と明示 adapter の比較である。Go の CloseClient 不在時の既定 CloseIdleConnections に対応する SDK transport の解放は未実装で、Rust adapter には明示 closer を要求する。Facade.Use の pool replay/commit と close エラーの結合、実 account service の既定選択・refresh・UID 照合・CAS、設定と全 SDK options、CLI/MCP/bootstrap、親子 Context、全取消競合・panic/負荷、他 OS は未移植または未検証。

```text
go test -race ./internal/shared/lifecycle ./internal/services/pixiv -count=1
go vet ./internal/shared/lifecycle ./internal/services/pixiv
cargo test -p pixiv-app --test lease --test sessions --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-lease` または `-args -migration-update-sessions` をそれぞれの package に指定する。

Go/Rust DB 相互テストは、Go 用 native 環境と通常 Cargo のキャッシュを混在させないため `target/go-interop` を使う。CARGO_TARGET_DIR が指定されている場合は、その下の go-interop を使う。初回は独立したビルドが必要だが、以後は同じ場所を再利用する。対象テスト・実行回数・結果の照合は変えない。

## 設定 snapshot と default account の読み込み基準

[config-snapshot.json](contracts/config-snapshot.json) は固定 Go の公開 Store／Snapshot API から取得した98ケースを Rust の設定読み込みと比較する。全 runtime フィールド、指定した alias の Value／Text／Source／HasValue、Pixiv／FANBOX の default UID、読み込み・runtime・UID 各段階のエラー全文と removed_setting の分類を記録する。入力 TOML と環境変数は合成値で、各ケースは一時ディレクトリだけを使う。読み込み後にファイルの byte 一致を確認し、欠損ファイルが作成されないことも検証する。

通常の文字列設定は Go の文字列化を許し、bool は文字列の ParseBool を許す。一方、account_pool とサービス別 proxy／user agent／solver は型を厳密に検証する。空文字列は欠損とは区別し、明示した環境変数が空でも file へ戻らない。Pixiv のサービス別 user agent と未知キーは runtime に読み込まない。solver の空表は無効だが、proxy を明示して URL がない場合はエラーになる。

default UID は通常 runtime と別の入口で検証する。正の整数と整数値の float を受け入れるが、文字列 UID は拒否する。不正 UID を含むファイルでも通常 runtime の読み込みは成功する。複数の不正設定を同時に置き、普通設定・web の墓碑・残りの普通設定・pool・サービス別通信・solver のエラー優先順位を記録する。TOML の構文エラーと重複キーも基準に含む。duration の i64 境界・長い小数・単位、数値の指数表記・非有限値、ローカル日時と offset 付き日時の文字列化も比較する。

追加の Go テストでは、取得済み snapshot の file／env の不変性と次回 Current の再取得、snapshot のファイル読み込み1回、default UID の都度読み込み、path／read エラーの同一 instance の伝播、file port 不在を確認する。書き込み・初期化 port は呼ばれると失敗する。比較 fixture の生成は成功した subtest だけを保存しないよう、失敗時は更新を中止する。

Windows amd64 で基準を取得した。Windows の環境変数名は大文字小文字を区別しないため、https_proxy／HTTPS_PROXY を同時指定するケースには同じ合成値を使う。両者に異なる値を置いた場合の優先順位は、この fixture では検証しない。Rust の Store は明示された実ファイルを読み込み、Snapshot は捕捉した file／env を保持する。runtime の PoolConfig と default UID reader を実 AccountService／Facade／DB／refresh／pool replay に接続した Rust テストでも確認する。不正設定は account opener／pool factory を呼ぶ前に止まり、SchedulerError が型付き ConfigError と I/O cause を保持する。CLI／MCP の bootstrap への接続、設定パス・書き込み・コメント保存・権限、全 TOML／型変換の値域・構文エラー診断、非 UTF-8 入力、別 OS は未完了である。設定 parser は [toml 0.9.8](https://docs.rs/toml/0.9.8/toml/) の Table を使用する。構文エラーの Go 形式への変換は fixture の重複キーと閉じ角括弧の欠損を比較した範囲に限り、すべての診断が一致するとは扱わない。設定読み込みの比較を Rust の設定コマンドの互換性検証とは扱わない。

```text
go test ./internal/config/settings -count=1
go vet ./internal/config/settings
cargo test -p pixiv-app --test config_snapshot --test pool_session --locked
```

基準を意図して更新する場合だけ `go test ./internal/config/settings -run TestMigrationConfigSnapshot -count=1 -args -migration-update-config-snapshot` を実行する。取得後の通常テストは更新フラグを付けずに比較する。

## CLI／MCP の接続設定の選択

[connection-options.json](contracts/connection-options.json) は固定 Go の CLI composition が SDK Options を構築する25ケース。コマンドの proxy override、Pixiv サービス別 proxy、全体 proxy の優先順位、明示した空文字列による直接接続、選択されなかった不正 proxy の無視、4種類の proxy scheme、IPv6、合成 userinfo、path／query、不正 URL の分類と秘匿したエラーを記録する。Go の実 HTTP client の proxy 関数・全体 timeout と、SDK の pacing option も取得する。通信は行わない。

Rust の CommandConnection は取得済み RuntimeConfig から proxy と pacing interval を選ぶ。比較対象は選択した proxy、interval、不正 proxy の分類とエラー全文、実 HttpTransport の構築可否である。open_transport は選択した proxy と interval を transport へ渡す。Rust の parser エラーには入力 URL や parser cause を残さず、Debug も同じ安全な文字列を返す。実通信の pacing と全体 timeout の比較は次節に記録する。App API の redirect、TLS／connection pool、すべての URL 構文、CLI／MCP bootstrap への接続はまだ比較していない。

```text
go test ./internal/cli -count=1
go vet ./internal/cli
cargo test -p pixiv-app --test connection --locked
```

基準を意図して更新する場合だけ Go テストに `-run TestMigrationConnectionOptionsPreserveProxyPresenceAndPacing -args -migration-update-connection-options` を指定する。

## HTTP transport の共有 pacing と timeout

[pacing.json](contracts/pacing.json) は interval=0／125ms の2ケース。Go の公開 OpenWith で OAuth refresh を行った後、同じ SDK client の HTTP transport に content GET・resource GET と redirect・mutation POST を流す。method／path の5通信と、開始間隔の下限を比較する。Go の resource request／content model の契約をこのテストで再検証するものではない。

Rust は HttpTransport::with_pacing により、通常 send・post_form・resource の各 redirect hop に同じ RequestPacing を使う。transport の clone も待機状態を共有する。Rust 側はローカル TCP サーバーを使い、各通信の method／path と開始間隔を比較する。最初の比較では Go の RoundTripper 呼び出しと Rust の実時計によるサーバー到着時刻を使ったが、全体検証中に Rust 側の間隔判定が失敗した。現在は Tokio の時計を停止し、受信スレッドも同じ runtime の時計で観測する。実ソケット通信と Go の期待値、既存の5msの許容幅は維持する。pacing を一時的に外すと125msのケースが失敗することも確認した。時計の値を fixture に保存せず、下限判定を boolean に正規化する。125ms は移植した実装の定数ではなくテストで与える interval である。

Go の既定 SDK New の全体 timeout=0 を再現テストで確認し、Rust の試作で設定していた60秒の全体 timeout を削除した。Rust テストは応答を保留したローカルサーバーに対し、Tokio の仮想時計を61秒進めても通信が完了しないことと、応答を再開すると成功することを確認する。reqwest 0.13.5 の既定 timeout/read_timeout が None であることも採用版のソースで確認した。

Go では pacing 待機中の取消が inner transport を呼ばず last start を変更しないことを確認する。Rust では resource の pacing 待機を task abort し、同じ transport の POST が残りの待機時間で再開できることと、取消した request がサーバーに届かないことを仮想時計で確認する。task abort と Go の context cancel は別の入口として記録する。

Windows amd64 での transport の比較である。SDK Options 全体との対応、client 再構築・pool replay ごとの pacing の寿命、context から全通信への取消、独立した request deadline、並行中の全競合、App API redirect、TLS・SOCKS wire・connection pool、既定 idle connection cleanup、CLI／MCP bootstrap、他 OS は未完了または未検証である。Client.with_pacing と transport の pacing を同時設定した場合の契約も未比較で、CLI の interval は CommandConnection が transport に渡す。

```text
go test ./sdk/pixiv -count=1
go vet ./sdk/pixiv
cargo test -p pixiv-sdk --test pacing --locked
```

基準を意図して更新する場合だけ Go テストに `-run TestMigrationPacingSharesOAuthContentResourceRedirectAndMutationStarts -args -migration-update-pacing` を指定する。

## 保存済みアカウントの実行経路

`crates/pixiv-app/src/execution.rs` は設定 Store、実 DB、AccountService、Gate、Facade、pool Scheduler を本番の組み立てとして接続する。`Execution::http` は account attempt ごとに transport を作成し、pacing の状態を account 間で共有しない。CLI の作品詳細と MCP の作品詳細・既存検索はこの経路と既存 `.pixiv-cli` のパスを使い、初回設定作成も行う。他の CLI と MCP ツールの接続は未実装である。

`crates/pixiv-app/tests/pool_session.rs` の `execution_reads_current_connection_and_default_account_before_refreshing_and_persisting` は、この組み立てを使い、実設定ファイルの変更が次の実行の default account・proxy・間隔に反映され、refresh の CAS 保存が SDK content request より先に完了することを確認する。`execution_pool_overrides_the_requested_account_and_persists_replay_state` は実 DB の pool 選択、未確定の rate limit から別 account への replay、credential revision と last selected を確認する。retry の応答時刻だけは実時計の120秒後を fixture に与える。固定時刻を使う既存テストも維持する。

`execution_rejects_configuration_and_proxy_errors_before_acquiring_an_account` は、設定・proxy エラーが transport 作成より前に返ること、型付き原因と秘匿性、明示空 proxy override を確認する。Go の参照は `internal/cli/composition.go`、既存の `TestFacadeUseBuildsPoolFromCurrentConfig`・`TestFacadeUseReplaysUncommittedAttemptsAndClosesEachLease`、connection/config snapshot 比較である。新規の CLI 子プロセス・MCP セッションの互換性を証明したものではない。

`Transport::send` と `post_form` の future は Send を要求し、汎用 transport の OAuth を app の非同期 callback から実行できるようにする。テスト専用の transport・時計・分岐を本番 src へ追加しない。transport constructor のエラーは現在 Gate の取得後に発生するが、Go の HTTPClient constructor は facade 呼び出し前である。この順序差、App API redirect と TLS/OS の通信差は未解消として扱う。

## 保存済みアカウントを使う作品詳細 CLI

`internal/cli/migration_detail_accounts_test.go` は Go の実 `detailDeps`、設定 Store、アカウント Service、Facade、SQLite を使い、未認証・既定アカウント欠落・既定 UID 不正・空 pool・pool 不正・proxy 不正を通常/明示 JSON の12ケースとして [detail-accounts.json](contracts/detail-accounts.json) に固定する。startup hooks を実行する root は使わない。設定の初回作成、更新確認、URL handler の証拠ではない。

`crates/pixiv-cli/tests/detail_accounts.rs` は隔離 HOME/USERPROFILE に設定を置いて実 Rust CLI を起動し、同じ出力・エラー分類・終了コード・設定の無変更を確認する。`detail` は試作の `PIXIV_ACCESS_TOKEN` 経路を使わず、既存パスの DB を開いて `saved_artwork_detail` と `Execution::http` を呼ぶ。通常出力の設定解決は DB を開く前、明示 JSON はその override を先に適用する。

既存の72件の detail-output と27件の detail-writer は、従来の SDK 直結に加えて実 DB・refresh CAS・本番 Execution・CLI presenter の経路でも同じ期待値で実行する。共有 fixture は `tests/support/saved_account.rs` に分離し、content request の前に rotated token と revision が保存済みであることを確認する。正常出力の実 CLI 子プロセスから HTTPS までを通した比較はまだ未実施である。

detail-input の既存 fixture は Go の偽 FetchArtwork が valid ID に対して `pixiv:Artwork: unauthorized` を返す入力境界用の契約である。Rust の valid 入力は同じ ID に解決することを維持し、起動後のエラーを新たに固定した実アカウント経路の `pixiv:auth: unauthorized: no pixiv account is authenticated` と比較する。不正入力の出力・終了コードは既存 fixture のまま維持し、DB を開かないことも確認する。既存 Go fixture の期待値は変更しない。

startup hooks・proxy flags・全 entity/record input・MCP のアカウント実行・content 取得中の Context 取消・通信/OS の未解消差分は残る。作品詳細を検証済みとはしない。

## 作品詳細の初回起動と設定作成

[config-initialization.json](contracts/config-initialization.json) は実 Go DefaultStore の初回生成・空ファイル・未知キー/コメント/CRLF を含む既存ファイル・不正 TOML の4ケースである。初回生成の bytes と2回目の非上書きを固定する。Rust Store の `ensure_defaults` は本番の `config/default.toml` を排他的に作成し、書き込みと sync の失敗時は未完成ファイルを削除する。既存ファイルは読み込み・修正せず保持する。

[detail-startup.json](contracts/detail-startup.json) は Go の実 Run/root を通した10ケースで、初回作成が作品 ID 検証より先であること、設定構文エラーが ID エラーより先であること、設定エラー・ID エラー時には DB を作らないことを固定する。通常/明示 JSON、設定生成 bytes、stdout・stderr・終了コード・DB の有無を Rust の実子プロセスと比較する。Go の更新 cleanup と URL handler だけは既存のテスト seam で無効化し、OS 連携の証拠とはしない。

Rust の作品詳細起動は設定作成と Runtime 検証を入力解決の前へ接続する。初回生成と読み込みは同じ Store を使う。設定はその後の JSON output resolver と SDK options/pool で Go と同様に fresh read する。未知キー・コメント・改行を含む既存設定は変更しない。

Unix 向けのテストは、ディレクトリ 0700・新規ファイル 0600・既存ファイルの mode 保持を確認する。Windows 実行ではそのテストは実行対象外で、他 OS の証拠とはしない。close エラーの報告、write/sync/cleanup 失敗の比較、全 filesystem error の表示・symlink・並行初期化・他 OS と startup hooks は未検証である。Rust の close は File の drop に依存し、Go の Close エラーを返す契約は未移植である。

## 保存済みアカウントを使う MCP 起動

`internal/cli/migration_mcp_accounts_test.go` は Go の実 SDKPorts、設定 Store、Facade、アカウント Service、SQLite と MCP server を組み合わせる。未認証・既定アカウント欠落・既定 UID 不正・空 pool を作品詳細と検索で呼び、8ケースの wire result を [mcp-accounts.json](contracts/mcp-accounts.json) に固定する。Rust の `crates/pixiv-cli/tests/mcp_stdio.rs` は隔離 HOME/USERPROFILE の実バイナリで全 result、stderr、終了状態と DB 作成を比較する。

Rust の MCP 起動は設定作成・Runtime 検証・既存 DB の読み込みを経て `Execution::http` と `stdio::serve_saved` を呼ぶ。作品詳細と検索は共通 `Execution::read` を使い、SDK の型付きエラーを Facade に返してから MCP のエラー結果へ変換する。エラーを callback 内で正常な tool result に変えて pool の判断を妨げない。

`saved_account_stdio_detail_preserves_go_results_and_persists_refresh_before_content` は既存の33ケースを実 DB・OAuth refresh・CAS 保存・MCP セッション経由でも比較する。内容取得の前に rotated token と revision が保存済みであること、入力/schema エラー時の認証・取得回数も確認する。検索の既存144ケースも直接 Client と保存済みアカウントの両経路で同じ結果・query を比較する。

`saved_account_cancellation_releases_gate_and_reuses_persisted_refresh` は既存 Go の JSON-RPC 取消結果と詳細 result を再利用する。pool 無効・有効の両方で、内容取得中の取消による通信 future の破棄、freeze が追加されないこと、次の呼び出しの Gate 再取得と保存済み rotated token の refresh を確認する。pool 全状態・全取消タイミングの証拠ではない。

作品詳細の MCP pool replay と lease 解放は次節の範囲で比較した。検索の途中ページ継続、EOF/disconnect による全通信取消、startup hooks、全 MCP ツール、TLS と他 OS は未検証または未移植である。現在の検索は replay ごとに収集全体を再実行し、Go の checkpoint を使う途中継続との互換性は未完了である。SDK/App の pool 単体比較だけで MCP の検証済みとはしない。

## MCP 作品詳細の pool 再実行

[mcp-pool.json](contracts/mcp-pool.json) は実 Go AccountService・SQLite・Gate・Facade・Scheduler・SDK と MCP session を組み合わせた4ケースである。別アカウントでの成功、切替先の応答不正、全アカウントの rate limit、再試行できない401を同じセッションで2回呼ぶ。wire result、OAuth/content の account 順序、Close callback 回数、保存 token/revision、freeze と last-selected、終了後の Gate 再取得を固定する。組み立ての HTTPClient 境界だけを fixture transport に置き換え、CLI startup は通さない。

rate limit の最初の応答には Retry-After=0、SDK の再試行後には120を与える。Go/Rust の既定 SDK が同じアカウントで1回再試行してから pool へ返すことと、後続 account の切替を待機なしで比較する。絶対時刻は両実装の実時計で決まるため、保存された freeze が未来にあるかだけを boolean にする。freeze の正確な時刻計算は既存の scheduler 比較で別途確認する。

`crates/pixiv-mcp/tests/pool.rs` は同じ body・条件を実 Rust Execution と duplex stdio に与え、2回の result と実 DB 状態を比較する。各 SDK client が所有する transport の Drop 回数を Go の Close callback 回数と比較し、account attempt ごとに所有物が解放されることを確認する。HTTP idle connection の cleanup API 自体の互換性は、この回数比較では証明しない。

作品詳細の上記 pool replay と、pool 有効時の内容取得取消後の再利用は Windows amd64 で確認した。OAuth 中・Gate 待機中・切替中の MCP 取消、全 scheduler 状態、並行呼び出し、idle cleanup、検索の途中ページ継続、disconnect/EOF と他 OS は未検証である。

## 作品詳細 CLI の proxy 指定

[detail-proxy.json](contracts/detail-proxy.json) は実 Go Run/root を通す22ケースである。設定内の不正 proxy、`--proxy` の明示空値と上書き、`--no-proxy` の true/false、両フラグの併用、重複 proxy の最後の値を通常/明示 JSON で固定する。入力・設定エラーと併用エラーが重なるケースも含める。既存の更新 cleanup と URL handler のテスト seam だけを無効化し、保存済み account と設定の実経路を使う。

`crates/pixiv-cli/tests/detail_proxy.rs` は隔離 HOME/USERPROFILE の実 Rust バイナリで stdout・stderr・終了コード、設定の保持と DB 作成の有無を比較する。実 Runtime/入力/DB の処理順序に沿って override を解決し、`Execution::read` に渡す。指定の有無は値と別に保持し、`--no-proxy=false` は設定を維持するが、`--proxy` との併用時には false でも Go と同じエラーを返す。重複 flag の上書きは detail コマンドに適用する。

固定 Go の detail に user ID フラグはなく、既存設定によるアカウント選択を維持する。実 proxy を通す通信、全 flag 構文と help、他 CLI の proxy フラグ、novel/user/content、record 入力と他 OS は未検証または未移植である。MCP の proxy 指定は次節の範囲で比較する。この比較だけで detail 全体を検証済みとはしない。

## MCP 起動とツール呼び出しの proxy 指定

[mcp-proxy.json](contracts/mcp-proxy.json) は実 Go Run/root と MCP の組み立てを通す10ケースである。設定の不正 proxy、明示上書き・空値・no-proxy false、両フラグの併用、設定エラーの優先順位と重複 proxy を固定する。更新 cleanup と URL handler の既存 seam を使い、最後の `runMCPStdio` だけを in-memory transport へ置き換える。構築済みの実 server で作品詳細と検索を呼び、各 wire result、起動診断、終了コード、設定保持と DB 有無を採取する。

`crates/pixiv-cli/tests/mcp_proxy.rs` は隔離ホームの実バイナリで同じ起動を行う。明示した不正 proxy と併用は DB 作成前に拒否する。設定内の Pixiv proxy はツール呼び出し時に解決するため、設定だけが不正な場合も MCP session は起動し、各 tool result にエラーを返す。stdout の CLI 診断と JSON-RPC を区別し、成功起動時は ID で両ツールの result を照合する。

本番 stdio の `serve_saved_with_proxy` は上書き値を各ツールの共通 Execution に渡す。上書きなしの `serve_saved` は既存の API と既定値を維持する。`saved_proxy_override_reaches_detail_and_search_account_connections` は既存の正常 Go fixture を再利用し、両ツールで設定値・空値・明示値が実アカウント接続へ届くこと、OAuth refresh と保存後の content result を比較する。

これは選択値と起動・アカウント経路の比較であり、実 HTTP/SOCKS proxy wire、TLS、redirect、全 flag 構文/help、reverse search・他ツールの接続、他 OS の証拠ではない。

## 通常 HTTP transport の redirect

[http-redirect.json](contracts/http-redirect.json) は `sdk/pixiv/migration_redirect_test.go` が SDK の実 HTTPClient と Go の標準 redirect 処理を通して採取する23ケースである。応答の境界は fixture RoundTripper に置き換え、HTTP 以外の scheme は本物の DefaultTransport で拒否する。GET/POST の301・302・303・307・308、相対 query、明示 Referer、同一ホストの別 port・subdomain、別ホストと元ホストへの復帰、Location 欠落・不正 escape・非 HTTP scheme、10要求の上限、各 hop の pacing を固定する。

`crates/pixiv-sdk/tests/http_redirect.rs` は隔離ローカル HTTP proxy と実 reqwest を使い、23ケースを JSON 読取と `post_form` の両経路で比較する。method・絶対 URL・body・必要ヘッダー・要求数・最終 status・失敗の有無を照合する。pacing は既存の比較と同様に仮想 Tokio clock で観測し、ソケットの read timeout は実時計で制限する。外部 DNS や実サービスへアクセスしない。

通常 API は Go と同じ method 変換と body/header 除去を行う。一度 body を除去したら後続の307でも復元しない。資格情報関連の6ヘッダーは、初期ホストから信頼しない宛先へ移った時点で除去し、その後も復元しない。共有 reqwest の自動 redirect は無効のままとし、画像 resource は既存の URL validator と手動追従を維持する。

この比較は HTTP transport の境界を検証する。SDK の固定 HTTPS URL を使う実 CLI/MCP 正常系、HTTPS downgrade、TLS/SOCKS、user-info/IDN/IPv6/全 URL 構文、独自 Host・CookieJar・CheckRedirect、redirect body の drain と connection reuse、通信失敗の型付き分類・原因・診断、他 OS は未検証または未移植である。redirect の失敗 boolean が一致しても、エラー表示全体の互換性を証明したとはしない。

## SDK の通信失敗と body 読取

[transport-failure.json](contracts/transport-failure.json) は Go の実公開 SDK を通した21ケースである。Artwork・OpenWith・AddArtworkBookmark の各入口へ、接続拒否・reset・EOF・TLS record エラー・不正 HTTP head・2xx/503 の body 読取失敗を与え、reason・transport・detail・表示・HTTP status・retry・要求回数を固定する。通信失敗の型付き原因と body だけをテストの HTTPClient 境界で置き換える。

`crates/pixiv-sdk/tests/transport_failure.rs` は同じ公開操作と実 HttpTransport を使用する。テスト用 Transport は固定 endpoint を隔離ローカル宛先へ変更するだけで、接続拒否・RST・EOF・不正 TLS record・不正 HTTP head・body 切断を実ソケットで起こす。接続拒否の port は listen しない socket で保持し、他のプロセスによる再利用を防ぐ。要求数は SDK から transport への呼び出し数を比較し、接続できなかった要求を wire の受信数と同一視しない。

Rust は reqwest の型付き原因を辿り、入れ子の io::Error に含まれる rustls::Error も分類する。原始エラーの文字列を分類へ使わず、URL・資格情報・応答内容を公開エラーへ保持しない。JSON 読取は HTTP status を判定する前に body 全体を読み、途中切断は2xx/503の両方で Go と同じ通信エラーにする。完全に読み取れた不正 JSON は既存の malformed 分類を維持する。form も body 読取失敗を返す。

この比較は Windows amd64 で実施した。DNS・proxyconnect の型付き分類、timeout の実測、Context deadline/取消の全条件、TLS certificate/alert の全種類、正常 HTTPS endpoint と実 CLI/MCP wire、ネットワーク診断イベント、他 OS/arch は未検証または未移植である。socket2 は dev-dependency に分離し、rustls は本番の型付き原因の識別に使う依存である。

## Trending tags の SDK・CLI・MCP

[trending-tags.json](contracts/trending-tags.json) は Go の70ケースである。`search --trending-tags` の JSON/human 出力、作品サンプルの DTO、空リスト・不正応答、検索語の拒否、18種類の併用不可フラグと明示した既定値・false、stdin を読まないことを固定する。実 Go Run/root の設定・DB 作成順と起動診断も比較する。

Rust の CLI は保存済みアカウントから `Execution::read` で型付き SDK を呼ぶ。直接 client と保存済みアカウントの両方で同じ70ケースを比較し、OAuth refresh の CAS 保存後に Bearer を使うことを確認する。作品サンプルは一覧 DTO と同じく pages を持たず、作者の comment と is_followed は保持する。併用不可フラグは値だけでなく明示指定の有無で判定する。

[mcp-trending.json](contracts/mcp-trending.json) は実 Go MCP session の `trending_tags_illust` schema と16応答を固定する。Rust の直接呼出し・stdio・保存済みアカウントの stdio で同じ structured content、text、isError と通信要求を比較する。CLI の行表示と異なり、MCP の text はタグの制御文字をそのまま保持する。空リストは `No trending tags found.` を返す。公開 schema は本番の `crates/pixiv-mcp/schemas/` に置き、テスト用の応答データは含めない。

検証環境は Windows amd64 である。入力 schema 拒否の全条件、取消・pool replay・disconnect、正常 CLI の実 HTTPS、resource の全利用、他 OS/arch は未検証。通常の user 検索は未移植であり、trending tags の接続で search 全体を完了扱いしない。


全体検証で、既存の search pool 比較に実時間の秒境界による不安定さが見つかった。`all_rate_limited` の残り秒数は固定した120と直接比較せず、各429応答の時刻から120秒の範囲に DB の凍結期限があることと、出力前後の実時刻からその期限までの残り秒数が正しいことを検証する。その項目だけを比較用の値へ合わせ、他の envelope 項目は Go と完全に比較する。本番の時刻取得・retry 計算や Go fixture は変更していない。


## 作品ランキングの SDK・CLI・MCP

[artwork-ranking.json](contracts/artwork-ranking.json) は固定 Go の71ケースである。全16モードと既定値、日付、DTO、複数ページ、不正な next_url、cursor payload と mode/date binding を固定する。作品ランキングの cursor は検索と異なり binding=1 であり、アカウントを跨いで使える。正の consumed を持つ cursor は Go と同じく batch 内の切捨てに使わない。Rust の RankingMode は String alias、各モード定数は `RANKING_MODE_*` の `&str` として対応する。query digest と endpoint ごとの allowlist 付き継続 URL parser は検索と共有し、検索の既存比較も維持する。

[cli-ranking.json](contracts/cli-ranking.json) は Go の204ケースである。作品ランキングの human 見出しと順位、JSON/NDJSON、論理 limit/page、空・不正応答、入力/type 拒否、実 Go Run/root の設定・DB 作成順と診断を固定する。Rust の直接 client と保存済みアカウント経由で同じ結果と query を比較し、refresh の CAS 保存と Bearer 使用も確認する。実 Rust バイナリの204起動ケースも比較する。JSON は既存の private spool でページごとに蓄積し、全取得が成功して lease を解放した後に出力する。

[mcp-ranking.json](contracts/mcp-ranking.json) は実 Go MCP session の schema と60応答である。全16モード、日付、空・不正応答、論理 page/limit と offset overflow、illust_filter と重複除去、pagination を固定する。Rust の直接呼出し、stdio、保存済みアカウントの stdio で同じ result と query を比較する。検索と共通の schema 検証、list plan、filter 判定、record/result 生成を使う。

検証環境は Windows amd64 である。小説ランキングは次節の範囲で接続した。全 flag 構文/help・proxy・出力 override/TTY、部分 writer 失敗、取消・pool replay・disconnect、schema 拒否と整数精度の全条件、実 HTTPS と他 OS/arch は未検証。作品ランキングの接続を ranking 全体や全環境の完了として扱わない。

## 小説ランキングの SDK・CLI

[novel-ranking.json](contracts/novel-ranking.json) は Go の74ケースである。全16モードと既定値、`filter=for_android`、複数ページ、next_url、cursor payload と mode binding、Novel DTO を固定する。小説ランキングも global cursor を使う。NovelDto の `updated_at` は null を出力し、tags は空配列を保持する。カバーは original、large、medium、square_medium の順に選び、禁止 host は拒否する。作者画像の不正 URL は Go と同じく空画像へ変換する。

[cli-novel-ranking.json](contracts/cli-novel-ranking.json) は Go の222ケースである。小説の human 見出しと行、JSON の novels キー、NDJSON の文字列 ID/type/canonical URL、論理 limit/page、入力拒否を固定する。`--date` は空値を明示した場合も client を開く前に拒否する。Rust は直接 SDK と保存済みアカウントの両経路で出力・query を比較し、OAuth refresh の CAS 保存と Bearer 使用も確認する。実 Rust バイナリの222起動ケースで診断と設定・DB 作成順を比較する。

作品と小説は mode 定数、cursor の検証と生成、作者・画像のマッピング、account pool と private JSON spool を共有する。JSON は全取得成功と lease 解放後に出力する。Go MCP に小説ランキングの登録はないため、公開ツールを追加しない。

検証環境は Windows amd64。部分 writer 失敗、取消、pool replay、全 flag 構文/help・proxy・出力 override/TTY、実 HTTPS と他 OS/arch の全条件は未検証である。小説の詳細・検索は次節の範囲で接続した。text やシリーズ操作は別の未移植項目として残る。

## 小説詳細の SDK・CLI・MCP

[novel-detail.json](contracts/novel-detail.json) は Go の37ケースである。`/v2/novel/detail` と novel_id、ヘッダー、Novel DTO、画像、正の小説・作者 ID、series_next/series_prev の型と正の ID、不正応答と入力拒否を固定する。詳細はランキングと同じ小説・作者・画像のマッピングを使い、resource reference に対応する URL を client 内に保持する。

[novel-output.json](contracts/novel-output.json) の126ケースを直接 SDK と保存済みアカウントの両経路で比較する。human は ID/title/author の1行、JSON は NovelDto、NDJSON は canonical novel record を出力する。[novel-input.json](contracts/novel-input.json) の122ケースと [novel-startup.json](contracts/novel-startup.json) の24ケースは実 Rust バイナリで比較する。ID/URL の entity 判定、初回設定と DB 作成順、設定不正の優先順を確認する。`--content` は Go と同じ ContentUnavailable を ID の解析と account 取得より先に返し、非 novel では flag の使用を拒否する。

[mcp-novel-detail.json](contracts/mcp-novel-detail.json) は実 Go MCP session の schema と43ケースである。Rust の直接呼出し、stdio、保存済みアカウントの stdio で record/result と要求数を比較する。required novel_id、未知 key、非整数、float64 経由の int64 overflow の拒否を含む。整数の schema 検証は既存検索と共有し、Go の構造体名と整数型に対応する診断を操作ごとに保持する。公開カタログに novel_detail を追加し、既存4ツールも維持する。

検証環境は Windows amd64。小説の record/text 入力パイプ、aggregate JSON、全 flag 構文/help・TTY/出力 override、部分 writer 失敗、取消/deadline、全数値/JSON 構文、実 HTTPS と他 OS/arch は未検証。小説検索は次節の範囲で接続した。シリーズ・ユーザー詳細は別の未移植項目として残る。

## 小説検索の SDK・CLI・MCP

[novel-search.json](contracts/novel-search.json) は Go の76ケースである。検索語、target/sort/duration、必須フィールド、Novel DTO、複数ページ、next_url と検索語に結び付く global cursor を固定する。空白だけの語は拒否し、受理した語は加工せず query と digest に使う。正の consumed は Go と同じく無視する。

[cli-novel-search.json](contracts/cli-novel-search.json) は Go の273ケースである。`search --type novel` の human/JSON/NDJSON、論理 limit/page、複数語、入力/type 拒否、作品専用フラグの明示指定と起動順を固定する。直接 SDK・保存済みアカウント・実 Rust 子プロセスで比較する。NUL を含む語は OS の argv に渡せないため、Go・Rust の起動比較では stdin を使う。小説ランキングと実際の一覧取得・ページ処理・JSON spool を共有し、JSON は全取得成功と lease 解放後に出力する。

[mcp-novel-search.json](contracts/mcp-novel-search.json) は実 Go MCP session の schema と74ケースである。直接呼出し・stdio・保存済みアカウント stdio で result と query を比較する。ID・全タグ・最小閲覧数の判定後に論理ページと重複除去を適用し、refresh CAS 保存と Bearer 使用も確認する。公開カタログに search_novel を追加する。

検証環境は Windows amd64。`pixiv novel search` の互換コマンドは未接続。部分 writer 失敗、取消/deadline、pool replay、全 flag 構文/help・TTY、全 schema 拒否・整数型診断・数値精度、実 HTTPS と他 OS/arch は未検証である。

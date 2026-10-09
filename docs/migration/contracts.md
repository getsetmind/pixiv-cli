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

検証環境は Windows amd64。小説の record/text 入力パイプ、aggregate JSON、全 flag 構文/help・TTY/出力 override、部分 writer 失敗、取消/deadline、全数値/JSON 構文、実 HTTPS と他 OS/arch は未検証。小説検索は次節の範囲で接続した。シリーズ操作は別の未移植項目として残る。ユーザー詳細は後述の範囲で接続した。

## 小説検索の SDK・CLI・MCP

[novel-search.json](contracts/novel-search.json) は Go の76ケースである。検索語、target/sort/duration、必須フィールド、Novel DTO、複数ページ、next_url と検索語に結び付く global cursor を固定する。空白だけの語は拒否し、受理した語は加工せず query と digest に使う。正の consumed は Go と同じく無視する。

[cli-novel-search.json](contracts/cli-novel-search.json) は Go の273ケースである。`search --type novel` の human/JSON/NDJSON、論理 limit/page、複数語、入力/type 拒否、作品専用フラグの明示指定と起動順を固定する。直接 SDK・保存済みアカウント・実 Rust 子プロセスで比較する。NUL を含む語は OS の argv に渡せないため、Go・Rust の起動比較では stdin を使う。小説ランキングと実際の一覧取得・ページ処理・JSON spool を共有し、JSON は全取得成功と lease 解放後に出力する。

[mcp-novel-search.json](contracts/mcp-novel-search.json) は実 Go MCP session の schema と74ケースである。直接呼出し・stdio・保存済みアカウント stdio で result と query を比較する。ID・全タグ・最小閲覧数の判定後に論理ページと重複除去を適用し、refresh CAS 保存と Bearer 使用も確認する。公開カタログに search_novel を追加する。

検証環境は Windows amd64。`pixiv novel search` の互換入口は次節の範囲で接続した。部分 writer 失敗、取消/deadline、pool replay、全 flag 構文/help・TTY、全 schema 拒否・整数型診断・数値精度、実 HTTPS と他 OS/arch は未検証である。

## 小説検索の互換コマンド

[cli-novel-search-compat.json](contracts/cli-novel-search-compat.json) は Go の99ケースである。`pixiv novel search` の検索条件・論理ページ・human/JSON/NDJSON、複数語と stdin、実子プロセスの診断と設定・DB 起動順を固定する。構文を受理する90ケースでは直接 SDK・保存済みアカウントからの出力も同じテスト処理で比較する。互換入口にない type/rating/trending-tags の明示指定を拒否する9ケースは実子プロセスで比較する。

Rust の互換入口は受理したオプションを共通の SearchInput/SearchOptions へ変換し、既存の小説検索へ渡す。公開するフラグを Go の互換入口に限定し、未知の長いオプションは起動時に Go と同じ usage 診断へ変換する。NUL を含む検索語も実起動では stdin 経由で比較する。

検証環境は Windows amd64。グループ単体の入力/help、全 flag 構文と診断順、全 bool/整数境界、実 TTY/HTTPS、部分 writer 失敗・取消・pool replay と他 OS/arch は未検証である。

## ユーザー詳細の SDK・CLI・MCP

[user-detail.json](contracts/user-detail.json) は Go の145ケースである。user/profile/profile_publicity/workspace の必須オブジェクト、正の ID、profile/workspace の全フィールドの null と不正型、公開範囲の bool/public/private、profile image、method/path/query を固定する。DTO に出さない birth/address_id/job_id も型を検証する。Rust の Profile/Workspace DTO はモデルの Serialize と型 alias で対応し、UserDetailDto は既存 UserDto を使う。

[user-output.json](contracts/user-output.json) は Go の99ケースである。human の全 profile/workspace 表示、完全な JSON envelope、canonical user record の NDJSON、空・不正応答、Web ページ URL の user-info/query/fragment 除去と16種類の入力を比較する。直接 SDK と保存済みアカウントの両経路を使い、refresh CAS 保存と Bearer 使用を確認する。[user-startup.json](contracts/user-startup.json) の24ケースは実 Rust 子プロセスで入力と設定の優先順、ユーザー URL、content/type の拒否、設定・DB 作成順を比較する。

[mcp-user-detail.json](contracts/mcp-user-detail.json) は実 Go MCP の schema と25ケースである。直接呼出し・stdio・保存済みアカウント stdio で complete envelope を持つ user record、要求数、required user_id、未知 key、非整数と float64 経由の int64 overflow、不正応答を比較する。公開カタログへ user_detail を追加する。

検証環境は Windows amd64。全 ID/URL 構文・JSON/数値構文、record/text 入力パイプ・aggregate JSON、TTY/出力 override/全 flag、部分 writer 失敗、取消/deadline・pool replay・disconnect、resource の実取得、実 HTTPS と他 OS/arch は未検証である。


## ユーザー検索の SDK・CLI・MCP

[user-search.json](contracts/user-search.json) は Go の64ケースである。SDK SearchUsers の検索語、UserPreview DTO、必須 user_previews と正のユーザー ID、フィールドの null・不正型、継続 URL、global cursor と検索語 binding を固定する。query は word と継続時の offset を使う。Go の検索応答に含まれる illusts/novels は不正型の場合も無視し、DTO では両方を空配列として出力する。

これに対応する Rust SDK の `search_users` は、既存の cursor・継続 URL・User mapper と resource registry を使う。型付きの UserPreview と UserPreviewDto を追加し、Linux amd64 で同じ64ケースを比較した。canonical user record の preview envelope と正の ID も外部テストで確認した。

[cli-user-search.json](../../crates/pixiv-cli/tests/fixtures/cli-user-search.json) は Go の実 CLI から取得した309ケースである。human/JSON/NDJSON、論理ページ、空の batch の補充、重複、後続ページの失敗、writer 失敗、禁止フラグと実子プロセスの起動順を固定する。Rust の `search --type user` は既存の stdin・設定・認証・proxy・ページ処理・JSON spool・NDJSON・writer commit 処理へ接続し、直接 SDK・保存済みアカウント・実子プロセスの各比較が通った。`pixiv user search` の互換入口は今回の範囲に含めず、未移植として残す。

[mcp-user-search.json](contracts/mcp-user-search.json) は実 Go MCP の schema と192ケースである。SDK 応答、ローカル ID filter、重複の除去、filter で空になる batch の補充、論理ページ、部分取得後の失敗、schema と validation の順序を固定した。Rust の直接呼出し・stdio・保存済みアカウント stdio から同じ192ケースを比較し、既存の list/pagination/record と schema の数値 binding を使う。

当初の Go の追加5ケースで、Rust の大小文字・重複キー処理に差を確認した。後述の raw wire 比較では User と SearchUsers の取得を生の応答へ接続し、既知の NAME 欠落と重複処理の差を解消した。作品・小説と未キャッシュの resource 再解決は従来の Value 処理を使い、この変更の範囲に含めない。

検証環境は Linux amd64。共有通信、全 input/flag/help・TTY、CLI 正常子プロセスの実 HTTPS、取消/deadline・pool replay・disconnect、resource の実取得、他 OS/arch は未検証である。全契約の検証状態は引き続き in_progress とする。

### 2026-10-09 cloud Linux amd64 の検証状態

Go の SDK・CLI・MCP の対象比較と `go vet ./internal/cli ./sdk/pixiv ./internal/mcpserver/pixiv` は成功した。公開契約の manifest・SDK・MCP schema・台帳を確認する `go test ./scripts/tests/migration -run '^TestMigration' -count=1` も成功した。Go 本番コード・go.mod・go.sum は固定参照コミットとの差分がない。

Rust の SDK・record・MCP の全テストは81 passed、0 failed、0 ignored で、同じ3 crate の all-target Clippy も成功した。Rust 1.93 の nonminimal_bool を満たすため、既存 SDK 検索の query 条件と MCP 検索・ランキングの filter/bookmark 条件を論理的に等価な式へ変更し、既存の検索・ランキング比較も通した。条件、範囲、重複除去の機能は変えず、warning の許可設定は追加していない。

依存取得では、CONNECT 403・実行取消と公式 Go proxy の archive redirect 失敗が発生した。同じ公式経路の通常の再取得で依存がそろい、Go CLI fixture の取得と offline の Rust workspace Clippy まで成功した。途中の実行取消では終了 status を取得できなかったため、その実行を成功に数えていない。

workspace の実テストで、追加した search_user を含めた catalog の検証と、既存 bookmark/date/options/page テストの隔離 HOME が不足していたことを確認した。catalog は既存7ツールの schema 比較を維持して8番目の search_user を追加し、各子プロセスには一時 HOME/USERPROFILE を指定した。

selector の従来 fixture は認証済み owner/SDK を注入した比較であり、SDK 内の sort validation error を未認証の実起動に流用していた。既存の SDK error/envelope の比較は維持し、[search-options-startup.json](../../crates/pixiv-cli/tests/fixtures/search-options-startup.json) の別の99ケースで、Go の実起動の診断と config/DB 作成順を比較した。これは元の期待値の緩和ではなく、注入済み SDK と実起動を異なる実行条件として別々に検証する変更である。対応する Rust 比較は成功した。


最終の `scripts/check-rust.ps1` は Linux amd64 で終了0となり、本番ソースのテスト属性検査、Formatter、workspace all-target Clippy、workspace テスト（158 passed、0 failed、3 ignored）、release build が成功した。3 ignored は既存の Go account/DB 相互テスト用の入口2件と、親テストから実行する spool 子プロセス用の入口1件であり、新たな skip/ignore は追加していない。最終チェックは約134秒、初回の release build は約61秒だった。Go の参照テスト、Rust のテストと本番 build を実 Pixiv 資格情報・ライブアクセスなしで実行した。

今回の実装・比較・環境回復は約33分で、その中には依存取得と実行取消の調査、既存テストの隔離 HOME と実行条件の修正も含む。最終の検証時間だけから工程全体の遅延原因を断定しない。このユーザー検索の接続時点では、大小文字・重複キーの共有 wire 差分と前述の未検証範囲があったため、全契約の verified や Rust への最終切替とは扱わない。


## ユーザー詳細・検索の raw wire JSON

[user-wire.json](contracts/user-wire.json) は、Go の raw JSON 177ケース（SearchUsers 82、User 95）である。応答を文字列として保存し、重複キーと入力順を維持した。既知フィールドの ASCII 大小文字、Unicode SimpleFold の ſ/K、escape されたキー、整数の構文と範囲、未知の 9e999、null と重複の順序、DTO・cursor・要求を固定した。未知の配列の総深さ10000を受理し、10001と末尾の余分な JSON を拒否する場合も比較した。

Go のオブジェクト処理はフィールドの型で異なる。検索 preview の user と profile_image_urls は値 struct のため、後続オブジェクトを既存フィールドへ merge し、null では以前の値を保持する。detail の必須4 section と search の必須リストは各出現でリセットする。scalar の null は保持、pointer の null は解除となる。既知 scalar の型エラーは後の正常値でも解消せず、publicity の custom decoder が記録した無効値は後の正常値で解消する。

Rust の Transport に JsonResponse と既定の send_json を追加し、既存の send・Response の構築を維持した。既定の adapter は既存 Value を JSON 化するため、既に失われた重複と順序は復元できない。実 HttpTransport は生の応答を渡す。User と SearchUsers だけがこの取得を使い、認証・ヘッダー・status/retry・pacing を既存処理と共用する。

対応する decoder は、宣言済みの schema に一致するキーを入力順で処理し、必要な reset/merge/null と型検証を適用する。未知の値は IgnoredAny で読み飛ばし、raw フィールドは借用する。任意の map の小文字化や、未知フィールドを含む全体の Value 化は行わない。既存の WireUser と profile/workspace/DTO mapper を使い、本番ソースにテスト用分岐・helper は追加しない。

CLI/MCP の比較にも raw 文字列の入力を追加した。従来の User CLI 66・SearchUsers CLI 279・User MCP 14・SearchUsers MCP 182ケースは各 JSON 行の byte を維持し、schema も変更していない。新たに User の11入力と SearchUsers の10入力を追加した結果、CLI の3 mode は99/309行、MCP は25/192行となった。直接呼出し・保存済み account・stdio で Go の出力を比較し、保存用テスト transport の send_json も refresh CAS/Bearer 検証後に転送する。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、162 passed・0 failed・既存3 ignored、release build が成功した。実 HTTP reader の重複/未知数値、旧 Value transport の adapter、raw status/retry と秘匿性のテストも通った。最終検証の Clippy は約6秒、test build は約23秒、release build は約17秒である。実装・Go 契約の取得・入口の比較・検証は約24分で、Go 本番コード・go.mod・go.sum は変更していない。

Go の対象契約と vet は成功し、detail/MCP の全体検証も通った。CLI の全体検証は、従来の command surface 検査が cli.linux-amd64.json 不在で失敗したことを記録する。同テスト以外は通ったが、Linux の公開面 snapshot の取得・検証を成功扱いにはしない。

この変更の対象は User と SearchUsers の応答 decode である。作品・小説の detail/search/ranking/trending に埋め込まれた User、未キャッシュ user_profile resource の metadata 再取得は Value 処理を使い、大小文字・重複・順序の差が残る。全 JSON/通信/SDK Options、取消/deadline・pool replay・disconnect、live HTTPS・実 resource・TTY、他 OS/arch も未検証であり、台帳の in_progress と最終切替の条件を維持する。

## bookmark・follow の CLI・MCP 更新操作

[cli-bookmark-follow.json](../../crates/pixiv-cli/tests/fixtures/cli-bookmark-follow.json) は、固定 Go の実コマンドから取得した278ケース（260件の操作比較と18件の追加 startup 入力）である。bookmark add/remove の artwork/novel、follow add/remove と user follow の互換入口を対象に、正の ID、URL・不正入力、restrict・複数 tag、record NDJSON の type/ID/JSON 検証、skip/fail-fast、form、HTTP status、stdout/stderr と終了コードを固定した。text stdin と NDJSON を最初の JSON 非空白 byte で区別し、text の末尾 LF/CRLF を1つだけ除く。入力の read error で返された不完全な bytes を mutation として実行しない。取消時は次の record を読まず、失敗を skip 診断へ変換しない。Go の実 command は取消確認前に入力 mode を検出するため、raw pipeline の事前取消 no-read 比較と command の途中取消比較を区別する。

[mcp-bookmark-follow.json](contracts/mcp-bookmark-follow.json) は、実 Go MCP の6 schema と172ケースである。add_bookmark/remove_bookmark、add_novel_bookmark/remove_novel_bookmark、follow_user/unfollow_user の validation・SDK 操作名・form・status・完全な mutation envelope を固定する。Rust では直接呼出し、stdio、保存済みアカウントの stdio を比較し、2 account の429でも各 mutation を1回だけ送ることを確認する。既存8 tool の登録と比較を残す。

両入口は既存の SDK mutation と form 契約178ケースを使う。共有 Execution::write は read と分離し、public SDK を呼び出した後は失敗も committed とする。ネットワークエラーから実サービスの未受理を保証できないため、保存された account pool で失敗した mutation を換号再送しない。account を開く前の失敗の扱い、refresh CAS・Bearer・proxy・lease 解放は既存処理を使う。multi-record pipeline の各操作では、保存された refresh token と credential revision を次の account open に引き継ぐ。

この変更は bookmark と follow の更新入口に限定する。bookmark detail/list/tags、follow の一覧と read-back、comment/reply/stamp その他の更新操作、実サービスの状態変更は別の工程である。全 input/flag/help・JSON 構文・TTY、実正常子プロセスの HTTPS、通信/取消/deadline/disconnect の全条件、他 OS/arch は未検証として残す。Linux CLI の command surface snapshot 不在も前の記録を維持する。


Rust の実 CLI 子プロセスでは、278件の startup 入力のうち275件で stdout/stderr・終了コード・config/DB の作成順を Go と比較した。empty pipe・不正 target・拒否 record は config のみを作成し、最初の actionable ID まで proxy 検証と DB/Execution の作成を遅らせる。成功した Execution を cache し、各 record の write は個別に committed の境界を持つ。既存の proxy flag の範囲と shared argument_error は変更しない。

残る3件は root の command 選択前に --no-proxy を置く入力である。bookmark/follow では Go が unknown command "add" の終了1、Rust が unknown option '--no-proxy' の終了2を返す。user follow では Go が usage の終了2、Rust が unknown option の終了2を返す。いずれも config/DB を作らない。Go の期待値は fixture に保持し、startup_unverified として Rust 比較の対象外であることを明示する。これらを互換性の成功件数に含めず、共有 root parser の未完了範囲へ残す。


Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、169 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約21秒、test build は約25秒、release build は約13秒で、全チェックは約150秒である。Go の CLI・pipeline・bookmark/follow command・MCP・SDK・services・migration 台帳検査も成功し、関連 vet と gofmt を確認した。CLI の全体テストは既知の cli.linux-amd64.json 不在の command surface 検査だけを除外し、この未実行項目を成功扱いにしない。実装・契約取得・入口比較・レビュー・検証は約19分であった。

Go 本番コード・go.mod・go.sum は固定参照コミットから変更せず、実 Pixiv 資格情報とライブ mutation を使用していない。台帳は全体の verified へ変更せず、前述の3 parser差分と他の共有検証・OS/arch・実状態変更の未完了範囲を維持する。

## 小説シリーズと公開 content 契約

[novel-series.json](../../crates/pixiv-sdk/tests/fixtures/novel-series.json) は、Go SDK の NovelSeries 49ケース、NovelContent の4エラー入力、3種類の公開 content DTO の比較である。series metadata、novels の必須 field・null/不正型、next_url と last_order、query-bound cursor、整数境界、資源 DTO を固定する。継続 URL の series_id は未指定や異なる値でも受け入れる Go の条件を維持し、次の要求には元の series_id と last_order を使う。既存 offset cursor の wrapper は変更せず、同じ処理を key 指定で使う。

固定 Go の NovelContent は、非正 ID に invalid_argument、正 ID に content_unavailable を返し、HTTP を実行しない。これは現行 App API の公開契約であり、Rust でも明示的に維持する。block/mark の kind は開いた文字列型として未知の値を保存し、公開 model・DTO と変換を追加する。空 blocks/marks は []、省略された image/file/unknown/ruby とゼロ resource は null となる。非ゼロ opaque resource の image/file、同時に存在する optional variant、未知 payload の copy も Go と比較し、DTO へ URL・request headers・期限・資格情報の要否を出さない。

[cli-novel-series.json](contracts/cli-novel-series.json) は、実 Go CLI の69ケースである。series --type novel の human/JSON/NDJSON、logical page/limit、空シリーズ、後続エラーで部分出力しない条件、URL/type と入力検証、実起動の stdout/stderr・終了コード・config/DB を比較する。[cli-novel-series-stdin.json](contracts/cli-novel-series-stdin.json) の99ケースでは text stdin の ID/URL・LF/CRLF・空・multiline・不正入力と、明示引数がある場合の no-read を確認する。検索と同じ text reader の処理を共用し、trim や複数行の分割を追加しない。

CLI のシリーズ metadata は最初に取得したページから保持し、--page 2 でも後続ページの値へ置き換えない。全対象 novel を収集した後に出力し、JSON も account lease の中で commit する。[cli-novel-series-pool.json](contracts/cli-novel-series-pool.json) の3シナリオでは後続429で最初の account の metadata/items を破棄して全体を再実行し、後続 malformed、refresh 保存、gate 再利用と出力時の lease を比較する。JSON object key 順は方針に従って正規化するが、array 順・null・値・empty array と本文の表現は維持する。

[cli-novel-content.json](contracts/cli-novel-content.json) の36ケースは、detail --type novel --content を ID/URL/record 解釈・設定・DB・account・HTTP より先に拒否する Go の既存順序を固定する。Rust の既存早期拒否を維持し、content の成功や余分な fetch を作らない。

[mcp-novel-series-content.json](contracts/mcp-novel-series-content.json) は、実 Go MCP の2 schema と50ケースである。novel_series の最初の cursor-zero metadata、dedup、logical paging、空の先頭ページ、batch 内の切り詰めによる has_more、後続エラーの全体失敗、nullable page/limit と int/int64 binding を比較する。novel_content は ID 検証の後に保存済み account を取得し、SDK の content_unavailable を返す。CLI の早期拒否とこの順序を意図的に区別する。

[mcp-novel-series-pool.json](contracts/mcp-novel-series-pool.json) の2シナリオは、後続429から account を切り替えて metadata/items を最初から取り直す場合と、後続 malformed で全 records を破棄する場合を固定する。続けて呼び出しても refresh/account 状態と gate を再利用できることを比較した。stdio の既存14 tool と新たな novel_series/novel_content の16 tool を保持する。共有 schema validator の追加は nullable integer union に限定し、既存 scalar schema の検証順と診断を変えない。

今回の範囲は小説シリーズと現行 content 公開契約である。artwork series は別の未移植操作として残し、series 全体の完成とは扱わない。NovelSeries envelope と埋め込まれた Novel/User を含む従来の Value decode の大小文字・重複・順序、resource 再解決、共有 root parser の3差分、Linux command surface snapshot 不在、全 input/flag/help・TTY・通信/取消/deadline/disconnect、実 HTTPS/資源/状態変更、他 OS/arch の未検証も維持する。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、182 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約3秒、test build は約13秒、release build は約16秒で、最終全チェックは約115秒である。最初の全チェックは tool catalog の追加位置で既存 CLI の ordered-prefix 回帰テストが失敗した。旧14 tool の順序を保持して新2 tool を末尾へ追加し、必須チェック全体を再実行した。失敗ログと終了値は別に保存し、部分チェックを成功扱いにしていない。

Go の CLI・series command・MCP・SDK・migration 台帳検査、関連 vet と gofmt は成功した。CLI 全体検査は、既知の cli.linux-amd64.json 不在による command surface 検査だけを除外し、この未検証を維持する。実装・契約取得・入口比較・独立レビュー・検証は約22分で、Go 本番コード・go.mod・go.sum は固定参照から変更していない。実 Pixiv 資格情報・ライブアクセスを使用せず、台帳の in_progress と最終切替条件を維持する。

## 作品シリーズの SDK・CLI・MCP

[artwork-series.json](../../crates/pixiv-sdk/tests/fixtures/artwork-series.json) は固定 Go の83ケースである。`/v1/illust/series` の `illust_series_id`、offset/last_order の continuation、global cursor binding、必須の series detail user、null/empty のリスト、DTO と要求を固定した。scalar cursor の正の s はこの操作では無視され、負数や混合 state は拒否される。両 continuation key を含む next_url を一方へ短縮しない。既存のランキング cursor helper を再利用し、作品一覧には detail の pages を追加しない。返された cover/profile の resource は記憶済み URL から取得でき、再取得を増やさない。

[artwork-null-tags.json](../../crates/pixiv-sdk/tests/fixtures/artwork-null-tags.json) は detail/ranking/search の各5ケース、合計15ケースである。シリーズの固定契約で共有 decoder の null tag slot の差が見つかったため、Rust 変更前に既存3操作を追加で固定した。Go は null slot を空 tag として保持し、numeric tag/name は拒否する。Rust の共有 tag decode はこの動作だけを修正した。raw-wire 大小文字・重複・順序の修正へ範囲を広げていない。

CLI は [cli-artwork-series.json](contracts/cli-artwork-series.json) の69出力/実起動、[cli-artwork-series-stdin.json](contracts/cli-artwork-series-stdin.json) の99入力、[cli-artwork-series-pool.json](contracts/cli-artwork-series-pool.json) の21 pool/writer ケースを比較する。69ケースには21成功、19非空成功出力が含まれ、成功 fixture が意図した必須 metadata と要求回数を持つことも Go 側で確認する。series --type artwork は既存の作品 ranking list engine を共有し、novel の入口と処理を保持する。human/NDJSON は出力後に replay せず、JSON は出力前の失敗で初期 cursor から再取得し、account lease 解放後に commit する。CLI は重複を保持する。valid JSON/NDJSON の比較は object entry 順だけを正規化し、scalar の escape/数値表記、配列順、whitespace と改行を保存する。不完全な writer 出力は byte 単位で比較する。pool の JSON stderr は構造比較であり、rate-limit の retry_after_seconds は実 SQLite 処理中の経過時間と保存 deadline から上下限を検証する。この診断の全字句表記は未検証である。

MCP は [mcp-artwork-series.json](contracts/mcp-artwork-series.json) の34ケースと [mcp-artwork-series-pool.json](contracts/mcp-artwork-series-pool.json) の2 account pool シナリオで schema・直接呼出し・stdio・保存済み account を比較する。kind+ID の重複を論理 pagination 前に除き、再試行では全取得結果を破棄する。失敗 envelope の pagination は Go と同じ初期状態へ戻る。既存16 tool の順序と schema を保ち、illust_series を17番目へ追加した。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、193 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約6秒、test build は約25秒、release build は約18秒で、全チェックは約170秒だった。Go の SDK・MCP・CLI・series command・migration 台帳テスト、関連 vet と gofmt は成功した。CLI 全体検査では既知の cli.linux-amd64.json 不在の公開面検査だけを除外し、未検証として残した。補助チェックの初回には存在しない series package path を指定して失敗したが、実在する internal/cli/commands/pixiv/series に訂正して関連チェック全体を再実行した。失敗と成功の終了値を別に保存した。

独立レビューと字句を保持する比較の追加確認を行い、Go 本番コード・go.mod・go.sum は固定参照から変更していない。実装・契約取得・入口比較・検証は約18分で、実 Pixiv 資格情報・ライブアクセスを使用していない。作品・小説の Value decode の raw-wire 差分、共有 root parser の3差分、Linux 公開面 snapshot、全 flag/help/TTY・通信/Options・取消/deadline/disconnect・実 HTTPS/resource、他 OS/arch は未検証である。台帳は in_progress を維持し、全体移植の完了や最終切替とは扱わない。

## 関連作品・推奨作品の SDK・MCP

[artwork-feeds.json](contracts/artwork-feeds.json) は RelatedArtworks と RecommendedArtworks の固定 Go 155ケースである。DTO、binding v2 の cursor bytes、account/client/query の隔離、ordered query と多重値、offset=0、未知/不正 continuation と検証段階を比較する。verified account は同じ account の別 client で継続でき、別 account と匿名 client では拒否する。関連作品は indexed seed_illust_ids/viewed を順序を持つ repeated [] query に変換する。推奨作品は viewed[ prefix を取り除き、offset/bookmark/include params をそのまま保存する。offset=0 を先頭ページへの省略に置き換えない。

Go の next_url parser と次回 cursor validation は同じ検証ではない。不正な numeric/bool 推奨値や関連 offset に余分な array key がある応答では、最初の取得が cursor を返し、再利用時に失敗する。関連 illust_id の不一致は endpoint 再実行前の upstream error になる。Rust はこれらを最初の malformed response に変更しない。最初の比較で related 再利用エラーの cause text の欠落を確認し、Go と同じ秘匿化済み cause を追加した。Go fixture を変えずに再実行し、155ケースと既存 ranking/series の回帰比較が通った。URL safety parser を共用し、scalar continuation の意味は保持した。

[mcp-artwork-feed.json](contracts/mcp-artwork-feed.json) は illust_related と illust_recommended の schema・114入力/filter/pagination/errorケースであり、直接・stdio・保存済み account から比較する。related の生成 schema は任意 pointer の null を許し、handler が ID、plan、filter の順で検証する。recommended の明示 schema は null/minimum 違反を先に拒否する。schema の複数不正 field は Go map の診断順が安定しないため、単一不正 field を固定し、関連の semantic multi-invalid 検証順とは区別する。全5 filter field、kind+ID の重複除去を logical pagination 前に適用し、空または全件 filter された batch は omitted limit でも非空になるまで補充する。

[mcp-artwork-feed-pool.json](contracts/mcp-artwork-feed-pool.json) は4つの保存済み2 account replay/state/release/reuseシナリオである。各試行は取得結果と dedup state を初期化し、途中失敗の部分結果を次の試行へ残さない。pool 比較は account 選択を確認し、cursor/query の正確な順序・多重値は SDK155ケースで確認する。既存17 tool の順序と schema を保持し、新しい related/recommended の2 tool を末尾へ追加した。

固定 Go に作品 related の CLI は存在しないため、新たな CLI を作成していない。recommended CLI は novel/user/all を含む全体の縦断移植として pending を維持する。作品応答は既存 Value decode であり、共有 raw-wire 大小文字・重複・順序・不正 UTF-8、cursor payload の大小文字・重複/null と任意 JSON precision、取消/concurrency/disconnect、全通信/Options・実 HTTPS/resource、他 OS/arch は未検証である。前の root parser3差分と Linux 公開面 snapshot 不在も解消扱いにしない。

この工程の Go 全体再検査で、既存 novel_content-argument-p の未知 property が2つある診断の非決定性を確認した。Go map iteration により [page limit] と [limit page] の順が変わる。最初の5回は成功し、20回の再現検査では差を複数回確認したため、成功するまでの再実行を検証証拠にしない。既存 fixture は変更せず、Go テストに最初の差分 byte と前後の診断を出す処理だけを追加した。Rust の固定 fixture 比較は通っても、この複数不正 field 診断順の全互換を証明しない。台帳の novel_content に追加の未検証として記録した。

Go の scoped 全体検査は終了0で、SDK・MCP・CLI・migration 台帳、関連 vet と gofmt が成功した。MCP は opt-in の `-migration-skip-nondeterministic-novel-property-order` を使い、novel_content-argument-p の診断全文が観測した2通りのいずれかであることを確認した上で、この行の未知 property の順だけを未検証にする。他の診断内容、result・calls・requests、schema、全ての他の行は元の byte 比較を維持する。flag 無指定の検査は従来通り strict で、非決定的な順で失敗し得る。CLI は既知の cli.linux-amd64.json 不在の公開面検査だけを除外した。失敗ログ、20回再現ログ、scoped 成功ログを分けて保持し、未検証を成功扱いにしていない。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、196 passed・0 failed・既存3 ignored、release build が成功した。最初の全チェックは既存 novel-series catalog set が追加2 tool を含んでおらず失敗した。旧17 tool の比較を保持して2 tool を追加し、全チェックを再実行した。最終 Clippy build は約0.4秒、test build は約2秒、release build は約19秒だった。独立レビューは feed 実装と、診断順だけを隔離する Go 比較を確認した。実装・契約取得・比較・検証は約15分で、Go 本番コード・go.mod・go.sum は固定参照との差がなく、実 Pixiv 資格情報・ライブアクセスを使用していない。台帳の in_progress と全体切替条件を維持する。

## 小説・ユーザー推奨 SDK と混合 recommended MCP

[novel-user-recommendations.json](contracts/novel-user-recommendations.json) は RecommendedNovels と RecommendedUsers の固定 Go 166ケースである。小説の構造化 params と binding v2、ユーザーの scalar offset と binding v1、account/client/query binding、明示 offset=0、DTO、sample works、query 順序/多重値と cursor payload を比較する。ユーザーの sample は endpoint が全 ID を検証した後、publish time/resource の map-time 失敗を Go と同じく省略する。通常の SearchUsers と違い sample artwork/novel を保持する。既存 artwork params helper と URL safety を再利用し、前の155作品 feed 比較も通った。

[recommendation-sample-wire-gaps.json](contracts/recommendation-sample-wire-gaps.json) は別に固定した6 Go ケースである。4つの sample null/type ケースは RecommendedUsers の範囲だけで修正した。null tools は空文字列の要素、null meta_pages はゼロ値 page になり、不正な page_index/extension 型は mapping 前に endpoint 全体を拒否する。他操作の schema を厳しくしていない。already_recommended の <>& と U+2028/U+2029 は Go と同じ escape を cursor payload に保存する。これら5ケースは Rust と比較済みで、uppercase user の1ケースは Go evidence だけを残し、対応 Rust 比較から明示的に除いた未検証の共有 raw-wire gap である。未検証を passing comparison の件数に含めない。

追加の mock resource 比較では推奨小説と推奨ユーザー/sample の計6 cover/profile reference を public OpenResource で開き、覚えた URL を使って追加 metadata API 要求をしないことを確認した。実画像の取得や全 resource edge の証明ではない。

[mcp-recommended.json](../../crates/pixiv-mcp/tests/fixtures/mcp-recommended.json) は recommended の schema と279ケースである。all/illust/manga/novel/user、全 filter、conflict、page/limit、records 順、kind ごとの pagination、異常系を直接・stdio・保存済み account で比較する。Go 先行の追加6ケースで nested integer overflow の In.illust_filter.id/min_views/min_pages、In.novel_filter.id/min_views、In.user_filter.id の診断を確認した。元の273行と schema は変更していない。最初の Rust 比較は Go の null bodies を受け取る fixture loader で失敗し、その後の比較で overflow の struct 名が recommendedIn になっている差を検出した。fixture を変えず、test loader と production binding 名をそれぞれ修正して比較全体を再実行した。

混合 artwork の論理 raw window は1回だけ取得してから illust/manga に filter・分割する。専用 illust_recommended の先 filter と混同しない。all に明示的な illust_filter.type があると両 section で同じ type を保持し、Go と同じ重複 records を返す。visual has_more は raw window の値を各 section が共有し、filtered output が空でも変えない。小説とユーザーは各 filter を論理 pagination 前に適用する。

[mcp-recommended-pool.json](../../crates/pixiv-mcp/tests/fixtures/mcp-recommended-pool.json) は2つの実 DB/2 account replay mode を固定する。Go の aggregate output は試行 callback の外にあるため、後続 feed の retry で既に完了した section の records を保持して追加する。成功時に重複した7 records があっても pagination は最終試行の各 section 件数を示す。Rust はこの既存の動作を勝手に改善しない。最終失敗では fresh error envelope の records[]/pagination{} を返す。account 42から43への切替、endpoint trace、lease 解放と次回 reuse も比較する。既存19 tool の順序/schema を保ち、recommended を20番目へ追加した。

この工程は recommended CLI の全 kind を移植するための読み取り依存を揃えるものだが、CLI 自体は pending を維持する。共有 raw-wire/cursor payload の大小文字・重複/null・不正 UTF-8・任意 JSON precision、schema 複数不正 field の診断順、取消/concurrency/disconnect、全通信/Options・実 HTTPS/resource、他 OS/arch は未検証である。前の root parser3差分、Linux 公開面 snapshot 不在、novel_content の2未知 property の順序非決定性も解消扱いにしない。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、201 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約7秒、test build は約30秒、release build は約19秒で、全チェックは約2分半だった。Go SDK・MCP・CLI・migration 台帳、関連 vet/gofmt、固定参照との本番 Go/go.mod/go.sum の無差分チェックも成功した。既知の Linux 公開面検査だけの除外と、novel_content の未知 property 診断順だけを隔離する opt-in flag は継続し、該当未検証を通過扱いにしていない。独立レビューは scoped sample/type correction、exact overflow diagnostics、aggregate/replay と catalog を確認した。実装・契約取得・比較・検証は約14分で、実 Pixiv 資格情報・ライブアクセスを使用していない。台帳の in_progress と全体切替条件を維持する。

## recommended CLI の全 kind

[cli-recommended.json](../../crates/pixiv-cli/tests/fixtures/cli-recommended.json) は固定 Go の231ケースである。KIND、--type と --content-type、all/illust/manga/novel/user、human/JSON/NDJSON、limit/page、複数 batch、空/異常応答、後続失敗、proxy/format conflict を直接 SDK と保存済み account で比較する。flag-only の成功条件は EOF stdin と有効な date/body を使い、API fetch と成功出力を Go 側で確認する。読取失敗が先に発生する条件とは混同しない。追加15ケースでは空・別名・大小文字・空白の raw kind が Go SDK の Unknown に map され、明示 subtype や all split から除外されることを固定した。raw kind の独自正規化で受理範囲を広げない。

作品の raw page window を選んだ後に subtype filter を適用し、filter で空になっても補充しない。初期の raw empty batch は共有 traversal に従う。単独 illust/default --type artwork の content-type all は raw subtype を保ち、positional manga は manga filter を強制する。CLI の重複は保持する。all は作品 window を1回取得して illust、manga の順に分割し、小説、ユーザーを独立した同じ plan で続ける。MCP の filtered window/dedup や retry records 蓄積へ置き換えない。

[cli-recommended-startup.json](contracts/cli-recommended-startup.json) は216 Go 起動ケースで、204件を実子 process、12件の synthetic reader failure を public Read API で比較する。[cli-recommended-stdin.json](contracts/cli-recommended-stdin.json) は165 Go text binding/実起動ケースである。明示 KIND は stdin を読まず、末尾 LF/CRLF を1つだけ除いて trim/split しない。config/DB/auth/proxy、型/plan の検証順と、JSON/NDJSON の明示 false、auto format を保持する。NDJSON conflict は JSON resolver より先に検証し、明示 NDJSON は JSON resolver を通らない。invalid UTF-8 の2入力を --type あり/なしの計4 Go 境界として先に固定し、recommended だけで kind/conflict の診断を保つ変換を追加した。無効 bytes が有効な ASCII keyword になることはない。他コマンドの text/wire decode は変更していない。

[cli-recommended-pool.json](contracts/cli-recommended-pool.json) の129ケースと [body fixture](contracts/cli-recommended-pool-bodies.json) は synthetic real DB/facade/Gate による replay、stdout commit、writer・state・lease を固定する。single JSON は lease 解放後に公開する。all human/JSON は試行ごとの私有 temp spool に全 section を保持し、全取得後に lease 内で commit する。all NDJSON は visual window 全取得後に分割出力し、各 record の writer attempt 前に committed とする。出力済みの失敗を account replay しない。retry では新しい spool と初期 cursor からやり直し、Go MCP aggregate の既完了 records 蓄積とは異なる。

既存の JsonSpool の pretty-array 出力を保持し、all 用の raw/compact section 操作を追加した。JSON の先頭 brace は API fetch 前に書く。Go/Rust の実 temp 作成失敗、全取得前の私有出力と cleanup、short/broken/other writer を比較した。subprocess は同じ active test の child-mode で実行し、新しい ignore を追加していない。JSON/NDJSON 比較は object entry 順だけを正規化し、scalar spelling、whitespace、array 順と改行を保つ。

最初の focused invocation は既存 url crate の test-only dependency edge が lock に未反映で停止した。offline metadata で edge 1行だけを更新し、package version を変えず locked 検査を再実行した。次に fixture の Value transport と保存済み token の期待値を修正し、Go fixture を変えず比較した。補助 regression の test target 名を json_spool と誤指定した失敗も記録し、実在する search_spool に訂正して比較した。これらの失敗ログを最終成功ログと分けて残す。

実装した全 kind は scoped 比較済みであるが、共有 raw-wire/任意 JSON precision/cursor payload、root parser3差分と Linux 公開面 snapshot、全 flag/help/TTY/OS startup hooks、通信/Options・取消/deadline/concurrency/disconnect・正常実 HTTPS、他 OS/arch は未検証である。novel_content の非決定的な未知 property 診断順も前の記録を維持する。台帳の in_progress と全体切替条件を保持する。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、209 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約3秒、test build は約11秒、release build は約18秒で、全チェックは約3分だった。Go SDK・MCP・CLI・recommended command・migration 台帳、関連 vet/gofmt と固定参照の本番 Go/go.mod/go.sum 無差分チェックも成功した。既知の Linux 公開面検査だけの除外と、novel_content の未知 property の診断順だけを隔離する opt-in flag は維持し、未検証を通過扱いにしていない。独立レビューと、既存 ranking・series・private spool の回帰比較も通った。実装・Go 契約取得・入口/失敗境界比較・検証は約16分で、実 Pixiv 資格情報・ライブアクセスを使用していない。

## ユーザーの作品・小説一覧

[user-works.json](contracts/user-works.json) は UserArtworks/UserNovels の Go 147ケースである。正の ID、type の default/illustration/illust/manga/ugoira、検証順、DTO、offset/next_url、cursor、query 順と多重値を比較する。type は query/digest 前に canonicalize する。両操作は global binding1 であり、account/client identity による追加の拒否をしない。UserNovels の wire `filter=for_android` は digest の user_id に混ぜない。UserID getter は未知0/verified snapshotを元と置換 client で確認した。cover/profile は mock OpenResource で追加 detail API 取得なしに開き、一覧 DTO に detail pages を追加しない。

[cli-user-works.json](contracts/cli-user-works.json) は400 Go 出力・format・window・writerケース、[startup](contracts/cli-user-works-startup.json) は270隔離起動ケース、[pool](contracts/cli-user-works-pool.json) は108 synthetic DB/account/Gateケースである。CLI は重複を保持し、omitted ID を最初の取得 client から解決して retry でも cache する。explicit99/omitted42 は account 交代でも変えない。novel heading は解決前の `novels by 0` を保持する。固定 Go は standard /users/42 URL も list resolver で拒否するため、user detail と同じ URL acceptance に広げない。ID label を含む SDK error の分類を保持する。

human/JSON/NDJSON、auto と明示 false、page/limit、stdin の1 LF/CRLFだけの除去と読取 failure/skip、config/ID/type/plan/proxy/format/DB/auth の順序を比較した。invalid UTF-8 ID と type の Go 境界も固定し、この入口での診断を保つ。JSON は共有私有 spool を使い、lease 解放後に公開する。前のランキング・series・recommended の source と描画を保持し、user 用の text 表示だけを選ぶ。pool は commit前/後/空結果の retry、writer failure、refresh/state/selection/freeze/revision と lease close を比較する。

[mcp-user-works.json](contracts/mcp-user-works.json) は schema と129入力/result行、[pool](contracts/mcp-user-works-pool.json) は8 synthetic 保存済み poolケースである。default user は listing と別の execution で解決し、その target を保持する。target42を選ぶ identity callback、listing43、retry42の各要求と refresh/state/open/close を比較する。MCP は filter/dedup を logical pagination 前に適用し、試行ごとに初期化する。未知 identity の plain error は CLI の CurrentUser/Unauthorized と同じにしない。root numeric/bool argument の Go diagnostic は number/bool であり、schema の integer/boolean type 名とは区別する。既存20 tool の順序/schemaを保ち2 toolを末尾へ追加した。

2つの argument22 行は page0/limit-1 が同時に不正で、Go schema map の走査により limit と page の最初の診断が変わる。129行の固定 fixture は変更せず、両 tool の real Go schema呼出しを128回ずつ行って2つの診断全文を独立に確認した。default strict 検査の bounded count4 でも実際に失敗を再現した。opt-in `-migration-skip-nondeterministic-user-works-violation-selection` はこの2行の全文を観測した2通りへ限定し、他の field・row・schema を厳密に比較する。Rust は固定 fixture と比較するが、この violation selection の完全一致を成功扱いにしない。前の novel_content の property-order 未検証とは別に記録する。

最初の CLI compile では heading変数の shadowing を修正し、auto format fixtureの writer が Go と異なる setup であった点を一致させた。fixtureの期待値は変えていない。最初の full gateは追加した testのFormatter、続くClippyは testのelse-if/type complexityで停止した。整形、論理的に等価な枝の整理、test-only type aliasを適用し、warning許可や skipを追加せずに全チェックを再実行した。失敗ログを最終成功ログと分けて残す。

追加した [残る操作一覧](remaining-features.md) は、実装入口の不足と共有検証の不足を区別する。Value decodeのraw-wire/cursor大小文字・重複/null/不正UTF8/任意precision、root parser3差分・Linux公開面snapshot、全flag/help/TTY/OS hooks・通信/Options・取消/deadline/concurrency/disconnect・実HTTPS/resource・他OS/archは未検証として維持する。全体のverifiedや最終切替とは扱わない。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、218 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約1秒、test build は約37秒、release build は約23秒だった。Go SDK・MCP・CLI・user command・migration 台帳、関連 vet/gofmt、固定参照の本番 Go/go.mod/go.sum 無差分も確認した。Go の broad 検査では不在の Linux 公開面 snapshot の1検査を明示除外し、novel_content の未知 property 順と今回の2行の violation selection はそれぞれの opt-in flag だけで隔離した。未検証を通過扱いにしていない。独立レビューと既存 ranking・series・recommendation の回帰比較も通った。契約取得から最終検証までは約27分で、実 Pixiv 資格情報・ライブアクセスを使用していない。

## ユーザーの関係一覧

[user-relationships.json](contracts/user-relationships.json) は UserFollowing/UserFollowers/RelatedUsers/UserBlockedUsers の固定 Go 524ケースである。正IDと exact restrict（空は public）、検証順、path/query、DTO/resource、next_url と identity binding1 cursor を比較する。前の UserArtworks/UserNovels と異なり、この4操作は認証 identity に依存し、未知 identity は同じ client instance だけの ephemeral cursor となる。foreign account/global/instance cursor の拒否と同じ client の継続を固定した。following/followers の restrict は query/digest に入り、blocked の wire filter=for_android は digest に入らない。next_url の許可 base query 値が元 request と異なっていても独自に拒否せず、正 offset だけを取り出して元 query を再送する。

shared ordered user decoder をこの4操作へ接続した。following/followers/related の preview sample は Go と同じく捨てて User だけを map する。blocked は `users` が present なら null/invalid でも `user_previews` より優先する。item の nested `user` pointer が非nullなら flat field を読まず、nullなら item 全体をもう1回 flat struct として読む。pointer の object merge/null reset、scalar/struct null の保持、medium pointer の null clear、途中の type error が後の valid duplicate で消えないことを raw ordered fixture で確認する。既存 User/SearchUsers の casefold/duplicate/null と transport 回帰比較を維持し、無関係な wire family を全体修正扱いにしない。

[cli-user-relationships.json](contracts/cli-user-relationships.json) は790出力・format・window・writerケース、[startup](contracts/cli-user-relationships-startup.json) は588ケース、[pool](contracts/cli-user-relationships-pool.json) と [body](contracts/cli-user-relationships-pool-bodies.json) は189 real synthetic account/DB/Gateケースである。following だけは auto NDJSON を使わず、related は必須ID、他3操作は omitted target を最初の fetch client から解決して retry でも保持する。重複・human heading・JSON private spool/lease後公開、明示 false、restrict/ID/plan/proxy/output/config/DB/auth の順を維持する。userworks の text binding/ID resolver を共通 helper として再利用した。588 startup の584行は process 比較（explicit reader-error4行は public reader bypass も確認）、omitted reader-error4行は public API の注入 reader と固定 exit/stderr/state を比較する。8 reader-error 行を黙って skip しない。invalid UTF-8 の ID/restrict 検証順も Go/Rust で固定した。

[mcp-user-relationships.json](contracts/mcp-user-relationships.json) は560 schema/input/result行、[pool](contracts/mcp-user-relationships-pool.json) は16 real synthetic account/DB/lease行である。user_following/user_followers/related_users/blocked_users を既存22 tool の末尾に追加する。related/blocked の compatibility restrict は schema に残すが fetch/query では無視する。following の plan→filter→identity、related の filter→identity→plan、followers/blocked の identity→plan を保持する。CLIと異なり related も default identity を選べる。default target は collection と別 execution で1回だけ解決し、filter/dedup-before-window と各 retry の初期化、refresh/state/lease を比較する。

最初の SDK invocation では既存 test target を user_search と誤指定し、実在する search_users に訂正した。MCP direct test は null arguments の helper decode と保存済み refresh-token setup が Go/stdio 境界と異なって停止した。固定 fixture を変更せず helper を修正し、失敗ログを成功ログと分けて残す。共有 cursor payload/不正UTF8/任意precision、残る wire family、root parser/Linux公開面 snapshot、既知の非決定的診断選択、全flag/help/TTY/OS hooks、通信/Options・取消/deadline/concurrency/disconnect・実HTTPS/resource・他OS/archは未検証として維持する。特に relationship default identity 解決中の取消はこの fixture 群だけでは保証しない。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、225 passed・0 failed・既存3 ignored、release build が成功した。Clippy build は約8秒、test build は約42秒、release build は約25秒で、全チェックは約3分だった。Go SDK・MCP・CLI・user command・migration、関連 vet/gofmt、本番 Go/go.mod/go.sum の固定参照との無差分も確認した。Go broad 検査の Linux 公開面 snapshot 不在の1検査除外と、既知 novel property-order/userworks violation-selection の opt-in 隔離は維持し、未検証を成功扱いにしていない。独立レビューと既存 userworks/detail/search/raw transport/stdio/catalog の回帰比較も成功した。契約取得から検証までは約16分で、実 Pixiv 資格情報・ライブアクセスを使用していない。

## ブックマーク一覧と GET query bytes

[bookmark-lists.json](contracts/bookmark-lists.json) は UserArtworkBookmarks/UserNovelBookmarks の固定 Go 248ケースである。ID/restrict/tag と cursor の検証順、DTO/null/type、resource、global binding1、正の max_bookmark_id（MaxInt64を含む）と query binding を比較する。following と違い空restrictを public に置換せず wire/digest でも空を維持する。許可された next_url の base値の不一致・空を独自に拒否せず、元 request の query と取り出した continuation を送る。既存 artwork/novel mapping と共通 cursor を再利用し、resource を追加 detail API なしに開く。

special tag の frozen raw query は `~` を literal、`*` を `%2A` とする。最初の比較で reqwest form encoder の `%7E`/literal `*` 差分を再現し、fixtureを緩和せず既存 Go-compatible digest escape を GET に再利用した。[http-query.json](contracts/http-query.json) は Go net/http と Rust HttpTransport の実 local HTTP capture 8ケースで、Unicode/記号、pair encounter順・重複値、existing raw query の維持、空paramsを比較する。bookmark SDK の全記録 query も actual HttpTransport で capture して固定 Go raw bytes と比較し、test-only encoder の自己比較にしない。POST form/OAuth は変更せず、既存回帰比較を維持する。これだけで TLS/DNS/proxy/全Optionsや任意生bytesの全通信互換とは扱わない。

[cli-bookmark-lists.json](contracts/cli-bookmark-lists.json) は1873主ケース、[records](contracts/cli-bookmark-lists-records.json) は1296 record入力ケース、[startup](contracts/cli-bookmark-lists-startup.json) は724ケース、[pool](contracts/cli-bookmark-lists-pool.json) と [body](contracts/cli-bookmark-lists-pool-bodies.json) は240 real synthetic account/DB/Gateケースである。bookmark list の artwork/novel/all と user bookmarks を保ち、list は URL/text/record、user alias は narrow text ID の異なるresolverを保持する。3169 direct・3121 saved比較を行い、未知identity48行は directだけで比較する。724 startup の720行は process 比較し、注入reader8行はpublic境界でも確認する（explicit4行はreader bypassの後process比較）。Go private committed bit は fixtureに保持するがRust test専用APIを追加せず、実replay/request/output/state/leaseから間接観測する。

単独一覧は重複を保持し、omitted target を最初の fetch client で解決して retry でも cacheする。novel heading は事前の `novel bookmarks by 0` を維持する。all は artwork→novel の単一 logical budget/batch checkpoint を共有 collect_streamsで処理し、filter/dedupを加えない。all の target は試行ごとに解決し、完全取得した bufferをlease内で公開する。JSON allはrecords envelope、単独はDTOarrayである。writer/EPIPE・明示 false/auto/error envelope・empty/late failure・refresh/state/lease境界を比較し、既存 add/remove の public enum と動作を保持する。

text/record classificationはconfig初期化前に1byteずつ読み、`{`でrecord remainderを保留する。configの後にtype/planを検証してからremaining recordを読み、target/proxy/outputへ進む。初期の eager read差分は invalid pageでGo1byte/Rust全record消費として再現し、byte期待値を維持したまま resolve_target境界へ分離した。早期reader errorのUsageと後続record reader errorのplain分類も保つ。bookmark-only parserでlone UTF-16 surrogateをGoの置換へ合わせ、valid pairとescaped backslashを区別する。既存 mutation parserは変更しない。捕捉した不正UTF8入力以上の全malformed sequenceを検証したとは扱わない。

[mcp-bookmark-lists.json](contracts/mcp-bookmark-lists.json) は170 schema/input/result行、[pool](contracts/mcp-bookmark-lists-pool.json) は12 real DB/account/replay行である。既存26 toolを維持し user_bookmarks/user_novel_bookmarks/bookmark_list_all を追加する。artworkのfilter/dedup-before-window、novelのidentity-before-plan、allのno-filter/no-dedup・ordered stream/単一budget、別executionでdefault targetを一度解決して保持する境界を比較する。whole-page replayでfailed recordsを捨てる。最初の fixture が offset、無効publish date、誤ったresponse fieldを使い、意図した成功行が malformed setupの失敗として通っていた点を独立レビューと実成功assertionで発見した。入力を正しGoを再取得し、singleton2件/aggregate3件のpool成功、後続page/novelでの失敗、直接window/stream/precision成功をcapture前に明示assertする。以前の不十分なpassログを別に保存し、成功証拠として数えない。

最初の SDK target名誤指定、GET byte不一致、MCP helper参照、CLI auto output/JSON false error scopeのtest setup差分とrecord eager readの失敗を記録する。fixtureの期待値を独自に変更せず、実Go入力の修正が必要なsetupだけを再captureした。残る raw-wire/cursor/任意precision・root parser3差分/Linux公開面snapshot・既知の非決定的diagnostic、全flag/help/TTY/OS hooks・通信/Options・取消/deadline/concurrency/disconnect・実HTTPS/resource・他OS/archは維持する。全体切替や完全verifiedとは扱わない。

最初の全Rust gateは既存userworks poolのwall-clock上限assertionで停止し、releaseへ到達しなかった。固定Go schedulerは attempt前currentとSDK absolute afterとの差を、attempt後freezeNowへ加えてepochを保存する。従来の `diagnostic_start +120` 上限はattempt中の経過時間を無視し、second境界で失敗する。invocation開始I・終了Dを実観測し、既存lowerを保って `floor(D + (D-I) +120)` をfull nanosecond精度から計算するtests/support helperを7pool比較へ再利用した。任意の秒の余裕、fixture変更、production clock/API変更は加えず、diagnostic retry-secondsの実start/end区間と全非timing断言を維持する。独立レビューで式を確認し、既存Go97 schedulerケースのadvancing-clock固定timestampによるexact freeze比較も根拠として保持する。7suiteの8対象testは再比較に成功した。最初の全gate失敗ログを再実行ログと分けて残す。

Linux amd64 の最終 scripts/check-rust.ps1 は終了0で、Formatter、workspace all-target Clippy、233 passed・0 failed・既存3 ignored、release build が成功した。最終 Clippy build は約1秒、test build は約3秒、release build は約28秒で、全検証は約3分だった。Go SDK・MCP・CLI・bookmark/user command・migration、関連 vet/gofmt、本番Go/go.mod/go.sum固定参照無差分も確認した。Linux公開面snapshot不在の1検査除外と既知novel property-order/userworks violation-selectionのopt-in隔離を保持し、未検証を通過扱いにしていない。独立レビューと既存mutation/search-bookmark/catalog/detail/tag/OAuth/HTTP/関連poolの回帰比較を維持した。契約取得・接続・失敗修正・検証は約31分で、実Pixiv資格情報・ライブアクセスを使用していない。残る実機能入口は操作一覧に保持する。

## Bookmark detail/tag の CLI・MCP 入口

既存 SDK の artwork/novel bookmark detail、artwork/novel bookmark tags を再利用し、CLI `bookmark detail --type artwork|novel` と `bookmark tags --type artwork|novel|all`、MCP の5入口を追加する。既存MCP29 toolの順序を保持して末尾へ追加する。実アカウントや実Pixiv通信は使用しない。

CLI固定入力はmain1,600、record7,315、body582、pool312とstartup835（実process830、reader failure5）に分ける。main/record/bodyの9,497行のうち869行はCobra leaf parser固有の未対応flag診断であり、RustのClap leaf parserによる拒否だけを検査する。残る8,628行は実処理の出力・診断・要求・入力消費を比較し、うち8,592行では合成保存済みアカウント境界も比較する。869行のexact leaf診断/終了値は未検証として固定Go証拠を保持する。Rustに存在しないCobra leaf rendererの期待値をroot実行へ翻訳しない。実際のGo Run/Rust executableの診断は独立startup fixtureと比較する。

CLI detailのempty tagsはnull、tags allはartwork→novel順に`type`を付け、namespaceの異なる同名tagを統合しない。text/URL/record入力、既定kind、出力mode/false/config precedence、reader/writer失敗、autoと明示NDJSONのEPIPE境界を保持する。detailはlease解放後に出力し、tagsはwriter失敗前にcommitする。single tagsはidentityのtargetをreplay中に保持し、allは試行ごとに再解決する。pool固定312行では合成DB、revision、selection/freeze、要求、lease、commitとreplayを比較する。内部private commit値を観測するtest-only APIは追加せず、公開結果と実際のreplayから検査する。

MCPは202 direct行と16 real-pool行を固定する。toolは`bookmark_detail`、`novel_bookmark_detail`、`bookmark_tags`、`novel_bookmark_tags`、`bookmark_tags_all`。single tagsは(name,count)全体でdedup、allは重複を保持して`content_type`を付ける。singleのidentity解決はplanより先、allはplanより後。既定targetは別Executionのidentity取得後にreplay中も保持する。

Go artwork detailのempty tagsはnullで宣言schemaのarrayに違反し、実stdioのvalidation error RPC codeは0になる。この実在する挙動を保持する。novel detailのempty tagsは[]。複数schema違反を持つ`bookmark_tags_all-argument-25`と`-26`のfirst diagnosticはGo map走査で変動するため、exact choiceは未検証。固定期待値を変えず、Goの明示`-migration-mcp-bookmark-reads-allow-diagnostic-order`だけで、当該2行の独立観測済み完全message候補を受け入れる。code、result、要求等の他項目は厳密比較し、Rustは固定期待値を比較する。

SDKのtag GET queryはGoのkey順に合わせてartworkのoffset/restrict/user_id、novelのrestrict/user_idとする。既存GET encoderのpair順/escapeとPOST form/OAuthは変更しない。既存SDK bookmark/detail/query回帰も実行する。

行列は[監査記録](contract-matrix-audit.md)のとおり過剰な直積を含む。現在の固定証拠や失敗行は削除せず、以後は共有fixture/helperと関係する境界の組合せを使用する。row数をdistinct behavior数や移植率として示さない。

User/SearchUsers/関係一覧以外のraw-wire casing/duplicates/order/null、不正UTF-8、arbitrary JSON precision、cursor payload、全通信、取消/parallel/disconnect、root parser既知3差分、Linux公開面snapshot、全flag/help/TTY/OS hook、他OS/archと署名配布は引き続き未検証。

今回の対象Go replayは7.190秒、Rust main比較は9.88秒、実startup835行は15.42秒、pool312行は1.33秒、MCP direct202行は0.56秒、pool16行は0.13秒だった。fixture総量は14,329,126 bytes。速度はこのLinux cloudの観測値であり、他platformの検証や独立behavior数ではない。

```text
go test ./internal/cli -run '^TestMigrationBookmarkReads' -count=1
go test ./sdk/... ./internal/cli/commands/pixiv/bookmark -count=1
go test ./internal/cli -run '^TestMigration' -skip '^TestMigrationCLIContractKeepsCommandsAliasesAndFlags$' -count=1
go test ./internal/mcpserver/pixiv -count=1 -args -migration-skip-nondeterministic-novel-property-order -migration-skip-nondeterministic-user-works-violation-selection -migration-mcp-bookmark-reads-allow-diagnostic-order
go vet ./internal/cli ./internal/cli/commands/pixiv/bookmark ./internal/mcpserver/pixiv
go test ./scripts/tests/migration -count=1
cargo test -p pixiv-cli-rs --test bookmark_reads --test bookmark_reads_startup --locked
cargo test -p pixiv-cli-rs --test bookmark_reads_pool --test mutations --locked
cargo test -p pixiv-mcp --test bookmark_reads --test bookmark_reads_pool --locked
cargo test -p pixiv-sdk --test artwork_bookmark_tags --test novel_bookmark_tags --test bookmark_detail --test bookmark_lists --test http_query --locked
```

Go broad regressionはSDK・bookmark command・CLI migration・MCP・vet・migration台帳検査が成功した。Linux公開面snapshot不在による1 testの除外、既知2scopeと今回2行の非決定的診断を明示しており、strict全scope成功とは扱わない。初回Rust比較ではleaf/rootの境界混同、saved fixtureのidentity42固定、raw responseのsend override不足が失敗した。境界を分離し、共有test helperとmockを実際のtransport動作へ合わせて修正した。固定Go期待値は変更せず、失敗ログと再検査ログを保持する。

最終 `scripts/check-rust.ps1` はLinux amd64でexit0、210秒。Formatter、Clippy（全target、warnings denied）、workspace tests（239 passed、既存helper3 ignored）、workspace release（29.90秒）が成功した。初回formatter失敗と、旧29-tool catalog期待値の失敗も保存した。catalogは旧29全entryを変更せず5toolを追加し、各追加entryを固定Go metadataとも比較した。Go本番・go.mod・go.sumは参照commitから差分0。scoped確認の成功を全wire/通信/他platformや最終切替のverifiedとは扱わない。

## Timeline following/latest の SDK・CLI・MCP

Goの実在する4feedを縦断移植する。SDK `FollowingArtworks`/`FollowingNovels`/`LatestArtworks`/`LatestNovels` と各request型、CLI `timeline following`/`latest`、MCP `timeline_illust_following`/`timeline_novel_following`/`timeline_illust_latest`/`timeline_novel_latest` が対象。MyPixivの3feedとその入口は別scopeとして残す。既存Artwork/Novel DTO・resource・cursor、filter、logical traversal、renderer、saved account/poolを再利用する。

SDK [timeline.json](../../crates/pixiv-sdk/tests/fixtures/timeline.json)は221 targeted行。restrict/defaultとcontent_type、検証順、query bytes、typed DTO/resource、空/null/不正field、next URL、operation/query/account/instance bindingを固定する。Followingはaccountまたはanonymous instanceに結び付き、Latestはglobal cursorである。LatestArtworksはoffsetとmax_illust_idのresponse continuationを受け取るが、正のoffset cursorで次を取得するとHTTP前UpstreamErrorになる実際のGo挙動を保持する。LatestNovelsはmax_novel_id。LatestArtworksのfilter=for_androidはwire queryに含み、cursor query digestには含まないGo境界を維持する。unrelated opposite list fieldを無視する2例も固定した。scoped cursorの検証/options生成をtimelineと既存user_relationshipsだけで共有し、既存関係一覧も回帰検査する。

anonymous cursor envelopeのランダム`i`だけは既存Go `migrationSearchCursor`と同じplaceholderに置換する。payload、他のfield、binding結果、error textは変更しない。同じanonymous clientで続行し、別clientで拒否される実結果を比較する。ランダムnonce bytesの一致は未検証。

CLI [main75](contracts/cli-timeline.json)の73行は実処理をdirect/saved境界で比較し、2行はClap leafの拒否だけを検査する。Cobra leaf固有のexact診断/exitは未検証。shared [body map](contracts/cli-timeline-bodies.json)、[実startup22](contracts/cli-timeline-startup.json)、[real-pool18](contracts/cli-timeline-pool.json)を使用する。followingのillust alias、全subtype、restrict、latestのillust/manga旧aliasとchanged content-type競合、novelのchanged subtype拒否、default illust、zero stdin read、設定/引数/proxy順、human/JSON/NDJSON/明示false/auto、logical window、commit/EPIPE/replayとleaseを維持する。Following artworkのlocal filterはupstream cursorを保持し、logical windowより前に適用して空のfiltered pageを跨ぐ。

[Scalar flags33](contracts/cli-timeline-flags.json)はoutput/body直積を増やさず、Go base0のhex/octal/binary/underscore/sign/range、strconv boolean、missing value、actual Run errorを固定する。libraryからbinaryへ渡す実際のtimeline command configurationを使い、normalized parsed_valueも比較する。timelineだけにparserを付け、他commandのparserは変更しない。uint64 overflowが後続不正文字より先に選ばれ、signed範囲判定はsyntaxの後になる順序を保つ。BELとU+2028を含む診断は既存Go quote helperを再利用する。

CLI leaf captureのanonymous NewWithはランダムinstanceを含むrepeated-cursor診断を生成したため、Rust比較前に合成OpenWith account42でfixtureを安定化した。最初の生観測は[non-comparable evidence](contracts/cli-timeline-nondeterminism.json)に保持し、replay失敗を観測していない段階で失敗証拠を捏造しない。account42の同じrepeated-cursor入力/結果はmainに保持する。anonymousの正確な生診断文字列は未検証。

MCP [direct213](contracts/mcp-timeline.json)と[real-pool8](contracts/mcp-timeline-pool.json)を固定する。following restrict omitted→public、latest artworkは必須content_type illust|manga、各filter・schema/numeric/null・typed Records・logical budget/dedupを比較する。旧34tool全metadataを保持して4toolを追加し、38件のcatalogを独立Go基準とも比較する。poolは2callずつ行い、discarded account42 metadataを返さずaccount43でwhole collectionをreplayする成功とmalformed failure、query continuation、rotation-before-content、revision/freeze/selection、closeとgate reuseを検査する。

初回Rust比較でother_client fixtureがverified identityを失い、saved MCP fixtureがdirect tokenを期待していた2件は、actual Go/shared saved helperと同じtest setupに修正した。Go期待値とproduction挙動は変更していない。実startupのmalformed scalar失敗と独立reviewのoverflow/quote差分はGo-first追加4行で修正した。

finite scalar行は全flag/help/TTY/OS startupの証明ではない。timeline以外の既存CLI scalar parserのbase0/bool forms、group単体help、既存root parser3差分/Linux公開面snapshot、非user raw-wire casing/duplicates/order/null/UTF-8/precision、全cursor payloadと通信/auth/取消/concurrency/resource、他OS/archと署名配布は未検証。

対象Go fixtureの最終replayはSDK0.022秒、CLI全timelineを3回で1.385秒、MCP direct7.680秒、pool0.355秒。Rust focused比較はSDK2 test0.08秒＋既存relationship0.09秒、MCP direct0.76秒/pool0.08秒、CLI main/normalized flags0.24秒/pool0.09秒/startupとflags0.79秒だった。これらはLinux cloudの観測値であり、他platformやdistinct behavior数へ換算しない。fixture/evidence10 filesは1,745,217 bytes。

```text
go test ./sdk/... ./internal/cli/commands/pixiv/timeline -count=1
go test ./internal/cli -run '^TestMigration' -skip '^TestMigrationCLIContractKeepsCommandsAliasesAndFlags$' -count=1
go test ./internal/mcpserver/pixiv -count=1 -args -migration-skip-nondeterministic-novel-property-order -migration-skip-nondeterministic-user-works-violation-selection -migration-mcp-bookmark-reads-allow-diagnostic-order
go vet ./sdk/pixiv ./internal/cli ./internal/cli/commands/pixiv/timeline ./internal/mcpserver/pixiv
go test ./scripts/tests/migration -count=1
cargo test -p pixiv-sdk --test timeline --test user_relationships --locked
cargo test -p pixiv-mcp --test timeline --test timeline_pool --test stdio --test novel_series_content --locked
cargo test -p pixiv-cli-rs --test timeline --test timeline_startup --test timeline_pool --locked
```

Go broad regressionのSDK、timeline command、CLI migration（33.843秒）、MCP（78.188秒）、vet・migration台帳検査は成功した。既存のLinux snapshot不在による1 test除外と3個の限定diagnostic opt-inを保持し、strict全Go確認とは扱わない。full Rust gateではCLI-hosted MCPの旧34 countとbookmark-readの旧34 names assertionが順に失敗した。旧assertionの意味を保持して38件へ追加し、implemented namesと独立Go metadata lookupをtests/の共通helperで再利用する。schema-loaderとregistrationの一致、request-ID、独立hostの検査は別に保持する。失敗ログも保存する。

最終 `scripts/check-rust.ps1` はLinux amd64でexit0、218秒。Formatter、全target Clippy（warnings denied）、workspace tests（248 passed、既存helper3 ignored）、workspace release（32.00秒）が成功した。共通catalog helperへの整理後の全treeで実行し、途中の旧catalog失敗を成功扱いにしない。Go本番・go.mod・go.sumは固定参照commitから差分0。operation/入口のscoped確認を、全wire・通信・他platform・最終切替のverifiedとは扱わない。

## MyPixiv の SDK・CLI・MCP

固定Goの実在する3feedを移植する。SDK `MyPixivArtworks`/`MyPixivNovels`/`MyPixivUsers` と各request型、CLI `mypixiv users` と `mypixiv works [USER_ID]`、MCP `mypixiv_illusts`/`mypixiv_novels`/`mypixiv_users` を接続する。実Pixiv通信・実アカウント操作は使用しない。

SDK [195 targeted行](../../crates/pixiv-sdk/tests/fixtures/mypixiv.json)でDTO/resource、body型/null、exact query bytes、positive offset、operation/account/instance bindingを固定する。artwork/novel feedのwire queryはoffsetだけで、next URLにもrestrict/filterを許さない。MyPixivUsersはidentityをcursorより先に検査し、wireのuser_id/filterをempty cursor digestへ混ぜない。user previewは既存のexact raw decoderを再利用し、body fixtureをRawValueで渡す。重複keyのmergeは既存wire契約の範囲であり、今回の195行による新たな比較とは数えない。MyPixivArtworksのowner IDもpositiveが必須であり、他feedのvalidatorを変更しない。cached resourceのopenと既存timeline/関係一覧の回帰を比較する。anonymous envelope iは既存Go共通helper同様instance placeholderで比較し、nonce bytesは未検証。

CLI [main77行](contracts/cli-mypixiv.json)の75行は直接処理と合成保存済みExecutionのexact比較、2行はCobra leaf parserに対するRust拒否のみであり、exact standalone診断/exitは未検証。[startup25行](contracts/cli-mypixiv-startup.json)は24実process比較と1 injected reader-error証拠で、後者はmainの入力境界で比較し実process通過と数えない。[scalar33行](contracts/cli-mypixiv-flags.json)は実startupとnormalized production parser値を比較する。[pool18行](contracts/cli-mypixiv-pool.json)は要求/replay/commit境界、selection/freeze/revision、writer/EPIPE、leaseを比較する。

usersはno-inputでautomatic NDJSONを行わず、authenticated UIDのheadingとraw `ID NAME`を出力する。worksはoptional text入力を解決し、automatic NDJSONを保持する。aggregate worksはartwork/illust・novel、USER_ID付きはmangaも受け付ける。explicit-IDは既存UserArtworks/UserNovels SDKへ委譲するが、MyPixiv固有のraw author/tag/title human表示を共有listのsource variantで維持する。mangaもheadingはartworks by ID。timelineの実Go scalar parserをMyPixivへ再利用し、他command familyを変更しない。

MCP [135行](contracts/mcp-mypixiv.json)は16共有bodyからschema/DTO/record/filter-before-window/dedup、exact出力・要求と直接/raw/保存済みstdio境界を比較する。[6 real-pool行](contracts/mcp-mypixiv-pool.json)では3toolのsuccess/malformedを各2callで検査し、全collection replay、OAuth保存、account42→43のusers identity、gate再利用を保持する。旧38tool全entryを変更せず3toolを末尾へ追加し、41件のordered catalogと全metadataを独立Go referenceへ比較する。既存schema-loader full-arrayの断言も3entryだけ追加する。novel/user record renderingは既存production-internal helperを再利用する。

fixture9 filesは810,546 bytes（約0.77 MiB）、489行であり、distinct behavior数や移植率ではない。新たな全直積は作らず、境界の相互作用と共有bodyを使用する。最初のMCP比較は追加toolのunknown-tool guard不足で失敗し、固定期待値を変えずadditive dispatchを修正した。次の比較で既存38-entry schema-loader assertionが失敗し、意味を保持して3entryを追加した。最初のfull gateのClippy nonminimal_boolはDe Morgan同値の条件に変更し、失敗ログを残す。

User/SearchUsers/4関係一覧/MyPixivUsers以外のraw-wire casing/duplicates/order/null、不正UTF-8・任意precision・全cursor payload、既存root parser3差分/Linux snapshot、timeline/MyPixiv以外のscalar forms、group help/全flag/help/TTY/OS hooks、既知の非決定的Go diagnostic、全通信/Options/auth/取消/deadline/concurrency/disconnect、実HTTPS/resource、他OS/archと配布の署名/信頼は未検証。scoped比較成功と操作全体のverified・最終切替を区別する。

```text
go test ./sdk/... ./internal/cli/commands/pixiv/mypixiv -count=1
go test ./internal/cli -run '^TestMigration' -skip '^TestMigrationCLIContractKeepsCommandsAliasesAndFlags$' -count=1
go test ./internal/mcpserver/pixiv -count=1 -args -migration-skip-nondeterministic-novel-property-order -migration-skip-nondeterministic-user-works-violation-selection -migration-mcp-bookmark-reads-allow-diagnostic-order
go vet ./sdk/pixiv ./internal/cli ./internal/cli/commands/pixiv/mypixiv ./internal/mcpserver/pixiv
go test ./scripts/tests/migration -count=1
cargo test -p pixiv-sdk --test mypixiv --test timeline --test user_relationships --locked
cargo test -p pixiv-mcp --test mypixiv --test mypixiv_pool --test timeline --test timeline_pool --test user_relationships --test stdio --locked
cargo test -p pixiv-cli-rs --test mypixiv --test mypixiv_startup --test mypixiv_pool --test timeline --test timeline_pool --test user_relationships --test mcp_stdio --locked
```

Go broad回帰はSDK/MyPixiv command、CLI migration40.218秒、MCP122.823秒とvetが成功した。Linux snapshot不在の1test除外と既存3scopeの非決定的diagnostic opt-inを保持し、strict全Go成功とは扱わない。SDK最終fixture replayは0.023秒、CLI captureは0.471秒、MCP最終main/pool replayは約8.4秒。Rust focusedはSDK0.05秒と既存timeline0.08秒/関係0.09秒、CLI main/flags/identity0.25秒、pool0.08秒、startup/flags0.79秒、MCP main5.63秒/pool0.06秒。cloud Linuxでの観測値を他platformやdistinct behaviorへ外挿しない。独立レビューでdocsの重複key比較claimを確認し、このfixtureに実際にはないため削除した。期待値や行は変更していない。

最終 `scripts/check-rust.ps1` はLinux amd64でexit0、285秒。Formatter、全target Clippy（warnings denied）、workspace tests（258 passed、0 failed、既存helper3 ignored）、workspace release（32.54秒）が成功した。旧catalog失敗とClippy失敗後の最終source treeで全gateを通し、部分ログを成功扱いにしない。Go本番・go.mod・go.sumは固定参照から差分0、gofmt/vet・台帳validatorも成功した。今回の契約固定・接続・修正・検証は約17分。scoped Linux比較を全wire/通信/他platformのverifiedや最終切替とは扱わない。

## User detail/search の互換入口と CurrentUser/Username

Goの実在する `user detail USER_ID` と `user search WORD...` の入口を追加し、既存SDK・detail/search execution・JSON/list/poolを再利用する。MCPの新toolは追加せず、既存41toolを維持する。SDKは公開 `CurrentUser`/`CurrentUserRequest` とcached `Username` を接続する。

CLI [61 actual startup行](contracts/user-compat-startup.json)はisolated homeの実Go Run/候補executableでstdout/stderr/exit、config/DB作成、input・proxy/auth・validation順を比較する。専用detailはtext-valueのみでrecord入力を受け付けず、NDJSON flag/automatic出力を持たない。固定Goはuser URLもuser_id-prefixed resolver errorで拒否するため、root `detail --type=user` のURL受入と混同しない。empty/whitespace IDは `input value is required` を固定し、新入口のresolverだけを補正する。

[8 profile表示行](contracts/user-compat-output.json)はhuman/JSONと全control-text欄を固定する。専用detailのhumanはSafeLineでescapeし、既存root detailのraw表示を維持する。shared fetch/JSON writerへproduction presentation choiceを渡し、numeric countは変更しない。どちらのhuman表示もwebpageのuserinfo/query/fragmentを出力しない。JSONは元のDTOを保持する。

owner user-searchは既存[309行](../../crates/pixiv-cli/tests/fixtures/cli-user-search.json)のfixtureを再利用する。新入口に存在しないroot-only selector/type overrideの54行をowner比較から除くが、既存root比較とfixtureは保持する。Rust ownerは255行を実optionsからcanonical executionへ通し、writer・保存済みaccount・paging/replayを比較する。Goのactual owner再実行は225行で、残る30 raw wire-body行は既存shared canonical Go契約を再利用し、owner固有のGo replay成功件数へ加えない。WORD argsをspaceでjoinし、独自のflag surface/text-input・auto NDJSONを維持する。

SDK [9 CurrentUser行](contracts/current-user.json)は既存user detail/wire bodyを参照し、rich DTO、uppercase/duplicate workspace、malformed/missing field、HTTP error、unknown identityのGo結果を固定する。CurrentUserはidentityをtransportより先に確認し、query `filter=for_android&user_id=42` とoperation-specific error、shared raw DETAIL decoding/publicity/resource mappingを保持する。Usernameは取得profileのnameではなく、constructorのverified credential snapshotをcopyして返す。credentialの後続変更、Arc共有、unknown identity/no HTTP、redacted Debugを検査し、新たなClient/Credentials clone APIは追加しない。

既存Go CurrentUserが要求するX-User-Id欠落を、実Rust比較の失敗で確認した。Go-firstでUser/FollowingArtworksのverified presence/valueとtoken-only absenceも固定し、shared GET builderだけへpositive cached UID headerを追加する。POST/OAuthその他headerを変更せず、全通信契約の解決とは扱わない。既存User/wire/transport/MyPixiv/novelと全workspaceの回帰を維持する。

新fixture3filesは78行・48,205 bytesであり、既存search255行を再利用する。到達しないroot-only flag/decoder/bodyの直積を増やさず、独自input/renderer/error/identity/header境界を対象にする。parent検証のtest-target誤指定、human numeric countへのescape適用compile失敗、startup件数assertionの算術誤りも、actual header欠落失敗と分けて記録する。期待値・既存行は削除/翻訳せず、compile条件・exact件数断言と実装を修正する。

root parser3差分/Linux snapshot不在、timeline/MyPixiv以外のscalar lexemes、group/全flag/help/TTY/OS hooks、User/CurrentUser/SearchUsers/4関係一覧/MyPixivUsers以外のraw-wire casing/duplicates/order/null、不正UTF8/任意precision/全cursor payload、全通信/Options/header/auth/取消/deadline/concurrency/disconnect、実HTTPS/resource、他OS/arch・配布信頼は未検証。入口実装とscoped比較を全体verified/最終切替に外挿しない。

```text
go test ./sdk/... ./internal/cli/commands/pixiv/user -count=1
go test ./internal/cli -run '^TestMigration' -skip '^TestMigrationCLIContractKeepsCommandsAliasesAndFlags$' -count=1
go vet ./sdk/pixiv ./internal/cli ./internal/cli/commands/pixiv/user
go test ./scripts/tests/migration -count=1
cargo test -p pixiv-sdk --test current_user --test user_detail --test user_wire --test user_wire_transport --test mypixiv --test novel_series --locked
cargo test -p pixiv-cli-rs --test user_compat_startup --test user_detail --test user_search --test search_pool --locked
```

Go broad regressionのSDK（pixiv1.521秒）、user command、CLI migration（61.295秒）とvet/gofmt・台帳validatorは成功した。Linux snapshot不在の1test除外を維持し、strict全Go scopeとは扱わない。SDK focusedのCurrentUser/header3testは0.07秒、CLI startup61行0.77秒、detail2test0.20秒、既存root/新owner search3test9.27秒、既存search pool0.39秒。これらはcloud Linuxの観測値であり、他platform/behavior数へ外挿しない。MCPのGo/sourceは今回変更せず、既存独立Go fixtureとRust全workspace回帰を保持する。

最終 `scripts/check-rust.ps1` はLinux amd64でexit0、294秒。Formatter、全target Clippy（warnings denied）、workspace tests（264 passed、0 failed、既存helper3 ignored）、workspace release（34.52秒）が成功した。header補正・専用renderer・empty resolver・count断言修正後の最終source treeで全gateを通し、部分ログを成功扱いにしない。Go本番・go.mod・go.sumは固定参照から差分0。独立source/docs reviewと最終台帳検査を通し、今回の固定・接続・修正・検証は約17分だった。scoped Linux比較を全wire/通信/他platformのverifiedや最終切替とは扱わない。

## Comment・stamp read の SDK・CLI・MCP

固定Goの実在するread操作だけを移植する。SDK `ArtworkComments`/`NovelComments`/`Stamps` とrecursive comment、nullable access-control/page、stamp/resource DTO、CLI `comment ID --type artwork|novel` と `comment stamps`、MCP `illust_comments`/`novel_comments` を接続する。Goにstandalone MCP stamps-list toolは存在せず追加しない。create/reply/delete/stamp mutationと各入口は未実装として台帳に保持し、readの比較を外挿しない。

SDK [144 targeted行](contracts/comment-reads.json)はartwork55・novel55・stamp34で、query bytes/global cursor/operation/ID binding、recursive parent ID、date-over-created_at・empty commentのcaption fallback、zero/negative/nullable metadataを固定する。commentのuser IDはunknownも許容し、profile resource失敗をswallowするGo mapperを維持する。invalid dateはendpoint名ではなく`Comment`のerror。numeric comment_access_controlがlegacy objectより優先し、0/negativeもnumeric値のまま保持する。

comment/stampのwireは既存ordered schema decoderへoperation-specific schema/kindを追加して再利用し、既存User/detail/search/関係schemaは変更しない。scalar null保留、pointer null clear、recursive parent merge、casefold・required-list resetを対象例で比較する。Stamps next_urlはGoのRawMessage pointer同様null/non-nullのpresenceだけを保持し、arbitrary exponent9e999でも後続nullがclearできる。payloadの数値変換を挟まない。全precision/UTF8/depthの検証済み宣言ではない。

Stampsはempty query、missing/null next_urlだけを受け付け、全itemのID/URLを検査する。新Stampsとresource re-resolutionは一つのvalidatorを再利用し、raw decoderもstamp branchだけへ接続する。actual Goのduplicate cache/reopen unitと4 fresh raw casesで、read時cacheは最後のduplicate URL、fresh clientは最初のmatching ID、unrelated malformed itemも拒否、uppercase/order/nullを固定する。resource callerのOpenResource error operationを保持し、他resource branchを変更しない。Go ParseRequestURI対URL parserの全normalization境界は未検証。

Goのfractional/offset caseはUTC .1234Zを返すが、Chrono default DTOは.123400Zとなった。実比較失敗を残し、new CommentDTOだけのSerializeをRFC3339Nano相当のtrailing-zero trimに補正した。DateTime modelと他DTOのserializerは変更せず、他DTO fractional日時は別の未検証範囲として保持する。

CLI [98 exact main行](contracts/cli-comment-reads.json)はdirect/保存済みexecutionのDTO/要求/input/output/errorsを比較する。[81 exact pool行](contracts/cli-comment-reads-pool.json)は9scenario ×3mode ×artwork/novel/stampsで、safe replay/commit/partial prefix、selection/freeze/revision、lease/writer順を固定する。namespaceのDTO/endpoint、modeのempty commit/EPIPE/partial-prefix、comment対stampsのlease内外が異なるため交差を残し、既存body/helperを共有する。[30 actual startup行](contracts/cli-comment-reads-startup.json)はconfig/DB作成とstdio/exitのstage順を比較する。rejection-only parser行はない。

commentは選択した全logical traversalを収集してからlease内で出力し、later-page失敗は出力0で最初からreplayする。metadataはattemptごとにresetし最初のnonnull値を保持する。stampsはReadで取得しlease解放後にwriterへ渡す。Go comment resolverは対応URLも拒否するため、独自のURL受入へ緩めない。text-input/arityはconfig前、target/type/page/proxy/format semanticsはconfig後・DB前、invalid proxyはDB/executionへ進むGo stageを保持する。Go Fprintfのsingle-write境界をhuman line単位で維持し、error-bearing short prefixと成功short countの1追加caseも比較する。JSON/NDJSONのbody/newline write境界と他familyのwriterは変更しない。

MCP [72 direct/raw/saved行](contracts/mcp-comment-reads.json)と[4 real-pool行](contracts/mcp-comment-reads-pool.json)はtyped schemas/comments/pagination/metadata/errorsとwhole traversal replayを比較する。generic Go formatted-value dedupeはruntime image URL、unquoted fieldのwhitespace collisionとparent pointerを含む。DTO JSON keyを使わずparentlessの全Go valueをformatし、別decoder allocationのparent付きcommentは区別する。旧41tool全entryを維持し2toolを追加、43件のordered names/full metadataを独立Go基準にも比較する。

fixture8filesは429行・672,579 bytesで、別のactual Go resource unit例はfixture row件数へ加えない。SDK144、CLI98+81+30、MCP72+4をdistinct behavior数・移植率へ変換しない。新しいschema/body/input/output全直積は作らず、poolの上記相互作用だけを残す。

失敗ログはSDK lifetime/import compile、fractional DTO、MCP metadata RefCell !Send、CLI Go URL rejection、single-write prefix、config-stage順、nullable Go argsのtest helperを区別して保持する。固定期待値を変えず、型・同期境界・実装とtest setupを修正した。独立reviewと最終focusedはSDK3test0.04秒、CLI main0.27秒/pool0.33秒/startup0.69秒、MCP direct2.59秒/pool0.04秒で成功した。

Go broad SDK/comment command・CLI migration40.551秒・MCP104.916秒とvet/gofmtが成功した。Linux snapshot不在の1test除外と既存3scopeのdiagnostic opt-inを保持し、strict全Go passとは扱わない。

full Rust scriptは最初のFormatter差分で停止し、cargo fmt適用後の再実行はFormatter/Clippy通過・workspace test compilation中に、予期しないexternal telemetryの送信審査で拒否された。元の部分logには終了sentinelがなく、成功とは数えない。ユーザーの明示承認後、Microsoft公式のPOWERSHELL_TELEMETRY_OPTOUT=1をpwsh起動前に設定し、Cargo offline・既存network restriction・同じscripts/check-rust.ps1の全stageを維持して再実行した。終了sentinel0、263秒、Formatter/Clippy・workspace272passed/0failed/既存3ignored・release34.54秒を確認した。元の拒否/部分logと承認後のfresh log/終了sentinelを別に保存する。

残る全wire/UTF8/precision/depth、other DTO時刻、URL normalization、root parser/Linux snapshot・全scalar/flag/help/TTY/OS hooks、全通信/header/Options/auth/取消/deadline/concurrency/disconnect、実HTTPS/resource、他OS/archと配布信頼を維持する。scoped比較・review成功でwhole migrationの完了や最終切替を宣言しない。

## Comment・stamp mutation の SDK・CLI・MCP

固定Goに実在するpost/reply/delete/stamp ×artwork/novelの8 SDK操作、CLI `comment create|reply|delete|stamp` の4leaf、同じ8 MCP toolを接続する。成功IDはupstream responseだけを返し、後続readbackから推測しない。既存read/list/resourceと旧43toolを維持し、8toolを末尾へ追加して51全metadataを独立Go基準と比較する。実アカウントへのwriteは行わず、全fixtureは合成transport/SQLite/local HTTPを使用する。

SDK [136 targeted行](contracts/comment-mutations.json)はvalidation順target→body→parent、target→stamp、空文字だけのbody拒否とwhitespace維持、stamp_id/parent_comment_id分離、deleteのcomment_id-only formを固定する。mutation responseは既存ordered decoderのoperation-specific schemaで両known fieldをunusedでも検査する。nonnull top-level comment_idがnested comment.idより優先し、0/negativeでもfallbackせずmalformedとなる。duplicate pointer object merge/null clear・casefold・型/順序を対象例で比較する。全schema/UTF8/precision/depthへの外挿はしない。

共有POSTは既存content_requestを再利用し、Goで先に固定したverified positive X-User-Idとtrimmed Accept-LanguageをGET/POSTへ反映する。空/token-only identityと空/whitespace languageのabsenceを保持し、OAuth/resource headerは変更しない。add/reply/stampの429 Retry-Afterを保持するJSON transportと、deleteのheader adviceを捨てるform transportを区別し、requestは1回だけとなる。Go401/403のsession refresh replayは既存共有authの未検証範囲として保持する。local HTTP例はcontent-type/form encoding・decoded値とduplicate response bytesの保持を確認し、実HTTPS/account operationの証明とは扱わない。

CLI [146行](contracts/cli-comment-mutations.json)のうち140行はfull output/input/request/commitをexact比較する。6 isolated Cobra unknown-flag行はparser拒否・read/wire/output/commitなしだけを検査し、standalone rendererのexact診断は未検証として固定期待値を保持する。actual Go root childのunknown-optionは別rendererでexit2となるため、[46 startup行](contracts/cli-comment-mutations-startup.json)のexact executable stdout/stderr/exitと混同しない。expected messageを変換して合格にはしない。

CLI mutationはhuman/jsonのみでlistingのNDJSONを継承しない。対応Pixiv URLを独自に受け付けず、single Writeの成功short count・writer error/partial prefixを維持する。parent/stamp数値flagはGo pflagのbase0/overflow/syntaxを既存timeline parserと共通化し、crate-private共有だけを追加する。hex/octal/malformed/forbidden cross-fieldとconfig/auth/DB順をstartupで固定する。root grammar・他family全flag/help/TTY/hookは未検証のまま保持する。

[22 real CLI pool行](contracts/cli-comment-mutations-pool.json)と[8 real MCP pool行](contracts/mcp-comment-mutations-pool.json)は2account・SQLite/Facade/Schedulerでwriteの境界を固定する。全mutationはerrorでもcommittedとなる共有不変条件を維持する。realpoolの対象429例ではsecond accountへreplayしないことを観測する。refresh revisionは保存され、選択account42を保持しaccount43のstateは変更しない。CLI writer error後もlease解放/active0とoutputのlease外実行を比較する。MCP429後の別read probeでgate再利用を確認する。単なるcallback boolだけをpoolの証明と数えない。

MCP [99 direct/saved行](contracts/mcp-comment-mutations.json)は8 schema/envelope/form/guardを比較する。deleteは失敗時もinput comment_idを保持するが、create/reply/stampはSDK成功後のreturned IDだけを出力する。parent/stamp/user IDをpayloadに推測追加しない。large returned ID9007199254740993のstructured wrapperはGo float64丸めを維持し、SDK整数結果とは区別する。既存tool全metadata/dispatch/cancel/panic経路を保存し、8入口を追加する。

fixture6filesは457行・305,470 bytesで、451 exact比較行と6 rejection-only行を区別する。SDK header/retry/local HTTPの別testはfixture row件数へ加えない。common form/decoder、timeline parser、CLI output/account/pool/test helper、MCP schema/output/write machineryを再利用し、無意味な全直積を作らない。行数をdistinct behavior数・移植率と扱わない。

SDK全97test、CLI focused3test（main0.29秒・pool0.05秒・startup0.95秒）、MCP focusedとstdio9test（main0.25秒・pool0.04秒・stdio0.08秒）が成功した。Go SDK/両endpoint/appapiとvet、CLI broad migration38.155秒とvetが成功した。CLIの既存Linux snapshot不在1testを除外する。MCP broadは既存bookmark複数違反のfirst diagnostic選択で93.399秒後に失敗し、独立再観測も別rowで選択が変わった。固定期待値と失敗logを維持し、既存3 opt-inでのscoped rerunは123.639秒・終了0となり、別結果として記録する。strict全Go passとは扱わない。

full Rust gateはunused追加DTOを除いた後のtrailing blankのFormatter差分で一度停止し、cargo fmt適用後に同じscripts/check-rust.ps1を再実行した。POWERSHELL_TELEMETRY_OPTOUT=1をpwsh起動前に設定し、Cargo offlineと既存network restrictionを維持した。終了sentinel0、331秒、Formatter/Clippy・workspace280passed/0failed/既存3ignored・release36.00秒の完了を確認した。元Formatter失敗logと終了1を別に保持する。

read checkpoint時点のmutation未実装という記録は、この新しい比較範囲で更新する。残るwire/UTF8/precision/depth、other DTO時刻/URL normalization、6standalone renderer/root parser/Linux snapshot、全flag/help/TTY/OS、Options/auth/TLS/取消/deadline/concurrency/disconnect、実HTTPS/resource、他OS/arch・配布信頼は保持し、whole migration完了や最終切替を宣言しない。

## Config group/path/get/set/unset

固定Goのroot config groupと4leafを接続する。auth/login/import/exportはこのscopeへ混ぜず、実credential・ユーザー設定は操作しない。全testはisolated child/temp directory・synthetic secretを使い、外部サービスへの要求はない。CLIは12managed aliasだけを公開し、Storeの20live aliasと2removed tombstoneのschemaとは区別する。get/setはremoved aliasを即拒否し、unsetはcleanupを許す。root helpにもgroupを追加して発見できるようにするが、root全help/flagのexact比較へ外挿しない。

Go [113 targeted CLI行](contracts/cli-config-commands.json)は100actual Rust executable comparisonと13isolated production ConfigCommand reader/writer boundaryを比較する。12keyのget/set/unset、2tombstone、sensitive key、environment precedence/override notes、group/leafhelp、ordinary stdin-fill/reader error、sensitive stdin、writer errorをtargetedで固定する。3個のhelp writer error例はGoが無視するため成功のままとし、success leafのwrite errorとは区別する。standalone parserのexpected診断を実rootへ変換せず、この新fixtureにrejection-only行はない。

config quiet startupはensure_defaultsだけを使い、Runtime検証やDBを要求しない。malformed既存TOMLでもpathは成功し、getはrequested settingだけを解決してunrelated runtime violationに妨げられない。group/helpは設定を作らない。unknown get/unsetはensure後のhandler拒否、unknown setは事前validation拒否となる。ordinary pipeline errorはensure前だが、sensitive bodyはRunE相当のensure後に読む。このstage順をfile bytes/config existence/DB absence/stdio/exitで固定する。

SauceNAO keyはargvでのvalueを許さずnon-TTY stdinだけを受け付け、getはunsetでも<redacted>を返す。first existing envは空文字でもoverrideとして扱い、proxy/secret override noteは値を表示しない。override note errorとgetの<unset> write errorはGo同様無視し、後者はconfig value is unsetを返す。ordinary key/valueのwhole stdinと1LF/CRLF removal、help bool/shorthand cluster/lone '-'、arity/key/read/write precedenceを維持する。

Store [35 targeted行](contracts/config-mutations.json)はget/set/unsetのfresh read、全12managed key成功、Store-only alias、removed、domain/input validation、environment notesとsparse document bytesを固定する。既存Snapshot/setting normalizationを再利用し、CLI membershipだけを別に判定する。cached official toml_editのlossless items/decorとGo tomledit formatting adapterでcomment/unknown value・空白/CRLF/quoting/absent unsetの対象例を比較する。全quoted/dotted/inline/duplicate/invalid syntax variantの互換宣言ではない。formatter diagnosticsからraw source excerptを出さず、synthetic credentialのmalformed document regressionで秘匿性を確認する。

private writerはsame-directory create_new staging、private mode、single Write/exact-count/sync/explicit close、replaceとparent/new-ancestor durabilityを実装する。precommit失敗でold targetを保持し、close/cleanup errorをjoinしてNotCommitted/Committed/Unknownを保持する。initializerもexplicit closeし、complete fileはclose-only errorで消さず、incomplete fileはcleanupする。Go frozen sourceとのread-only reviewを行い、Unix成功/private permissions・read failure・failed validation bytes保持を合成FSで確認する。read failure testはreplacement failureの証拠として数えない。

Windows backendはexisting targetへflags0 ReplaceFileWとdisposable recovery backup、absent target/recoveryへflags0 MoveFileExWを使う。1177restore失敗ではsource/backupを保持しUnknown、commit後backup cleanup errorでもCommittedを維持する。cached windows-sys signatureとGo recovery sourceをreviewしただけで、Windows compile/run/ACLやforced syscall/short-write/postcommit failuresは未検証。source実装と実行済み証拠を区別し、単純renameでこれらを置き換えない。

新fixture2filesは148行・97,351bytes。schema/value/parser/quiet stage/writer/FSの独立境界をtargetedで固定し、existing snapshot/initializer、search input reader、private directory logicを共有する。app focused11test（new mutation/privacy6、initializer2、snapshot3）とCLI4test（boundaries0.02秒、startup0.33秒）が成功した。Go settings全suite/vetとCLI113 replay/vet、broad CLI43.367秒が成功した。Linux公開面snapshot不在の既存1testを除外し、strict全Go passとは扱わない。

full Rust gateはprivate cleanupのcollapsible-if lintで一度停止し、意味を変えない条件連結へ修正した。その後のrunは終了0になったが、test実行後にsingle Write/count-checkのGo差分をsourceで補正したため最終treeの証明とは数えない。元失敗log/終了1とこの中間runを保持し、最終sourceで同じscripts/check-rust.ps1を全stage再実行し、終了sentinel0・254秒、Formatter/Clippy・workspace290passed/0failed/既存3ignored・release cached0.13秒の完了を確認した。POWERSHELL_TELEMETRY_OPTOUT=1、Cargo offline、既存network restrictionを維持し、この最終runだけをfinal-tree full gate成功として数える。

残る全TOML/非UTF8、FS read/write/close/cleanup/commit fault、platform ACL/Windows/macOS、symlink/concurrency/OS startup、root全flag/help、auth/config他入口・配布信頼を保持する。下位Store/commandのscoped比較をwhole migration完了としない。

## Auth list・pool status/enable/disable

固定Goのauth group/listとpool group/status/enable/disableを、既存local DB/AccountService/pool transactionへ接続する。OAuth・login・実credential生成・実アカウント変更は行わず、synthetic credentials/temp storesだけで固定する。use/remove/import/export/check/refresh/login/hidden callbackはpendingのまま保持し、bare auth helpでGoの残るcommandを削らず、未実装入口は明示errorとする。root helpのauth discoverabilityを追加するが、pending subcommandの実行や全helpの比較済み宣言はしない。

Go [104 targeted CLI行](contracts/cli-auth-accounts.json)は91actual executable/rootと13isolated reader/writer boundaryでstdio/exit/config/DB/stateを比較する。auth自身の--json boolを使いoutput.json設定を継承しない。error machine selectionはflagの値だけでなくChangedも使うため--json=falseのerrorも区別する。prefix help/subcommand discovery・bool/shorthand/unknown exact matching・stdin whole-value/引数優先・UID trim/+decimal・bell quotingを固定し、既存input/quote/config path/helperを再利用する。

Go DTOのempty listはaccounts:null、empty pool statusはaccounts:[]となる。HasTokenはsummary経由で常にtrueでありsecret bytesを検査・表示しない。premium_checked_at/warningはGo mapperがコピーしないので推測追加しない。listはBackground、status/changeはcommand contextを使う。human writer errorは無視するがJSON writer errorは返し、JSON successful short countはGo同様無視する。5short-write行・human/JSON writer error・noinput/read failureを別境界として比較する。

Domain [12行](contracts/auth-account-views.json)と別のpaired Go/Rust unitはpool snapshot→list→per-account default fresh readを固定する。implicit fallbackはその都度先頭stored accountを再取得し、explicit stale UIDはerrorではなく全default:false、empty listはdefault storeを読まない。複数default summaryが生じる場合のadapterはGo同様最後のIDを採用する。real DB snapshotはexpired <=now freezeをtransactionally clearしupdated_atを変更しない。disabled future freezeもstatus earliestへ入る。

既存contextless DB APIを残し、genuine context-aware variantsを同じvalidation/transactionへ追加する。selected/all membershipはunknown/duplicate/invalid UIDでbatch全体を失敗させ、freeze/marker/credential revision・token/premium/cache/created_atを変更しない。Go-first trigger-abort unitでselected/all rollbackを比較する。snapshot/settersはtransaction前/commit前にcontextを検査するが、blocked SQLite/mutex中の即時取消を実証したとは扱わない。storageの既存equal-now比較と、service clockの全境界を区別する。

Go-first year10000・max i64・mixed正常future/maxのhuman/JSON例で、Chrono range外のtimestampを黙って落とさない。integer UTC formatterはGoのexpanded positive yearへ'+'を付けず、JSONは[0,9999]外をGoのTime.MarshalJSON errorで拒否する。Go Time.BeforeはUnix+62135596800をwrapping i64のinternal secondsとして比較するため、earliestを文字列sortや単純epoch minにせず対応keyを使う。対象例を全日時/range/clockの保証へ外挿しない。

初回Rust reader-error比較はGo exit2に対してexit1で失敗し、shared Usage分類へ修正した。max synthetic stateはGo test helperのfloat64 roundtripが9223372036854775807を9223372036854776000へ丸めていたため、typed int64/*int64のactual SQL captureへ修正した。元bad artifactと失敗logを保持し、6max/mixed state値だけを正し、stdout/stderr/config/exitは変更しない。Rust値を誤ったfloatへ正規化して合格にはしない。

新fixture2filesは116行・94,038bytesで、CLI91actualroot/13boundaryとdomain12を区別し、別unitをrow件数へ加算しない。app4focusedとCLI3focused（boundaries0.05秒/startup0.58秒）が成功した。Go account/storage全synthetic suite/vet、CLI104 replay/vet、broad CLI37.817秒が成功した。Go Cargo interop flag未指定のskipと既存Linux snapshot不在1testの除外を保持し、strict全Go passとはしない。

full Rust scriptはapp testのcollapsible-else-ifとauth flag scannerのnonminimal-boolで停止した。意味を変えない形へ補正し、各失敗log/終了1を保持して同じ全stageを再実行した。POWERSHELL_TELEMETRY_OPTOUT=1・Cargo offline・既存network restrictionを維持し、終了sentinel0・310秒、Formatter/Clippy・workspace297passed/0failed/既存3ignored・release35.62秒の完了を確認した。既存auth/pool groupのwindows-amd64 foundation証拠は保持するが、新local CLI scopeの比較はLinuxのみでありWindows実行へ外挿しない。

実startup/update/OS hooks・diagnostics/DB close報告、全時間/UID/UTF8/flag/help、SQL実行中取消/blocked mutex/並行、Windows/macOS/全arch・配布信頼を保持する。local auth4leafの比較をauth全機能やwhole migrationの完了としない。

## Auth use/remove・default 保存・terminal selection（2026-10-09）

固定Goの`auth use [UID]`と`auth remove [UID] [--yes]`を既存account/configへ接続する。合成DB・temp config・injected IO・Unix PTYだけを用い、実アカウントの選択・削除や資格情報の作成を行わない。MCPに存在しないauth入口を追加しない。

[CLI fixture](contracts/cli-auth-selection.json)は68行・58,147bytes。62行を実root executable、6行をreader/writer dependencyでexact比較する。別のGo/Rust port比較は5 selection/confirmation行と3 List-failure行。既存seed/state/error処理を再利用し、不要な全flag×output×stateのCartesian matrixは増やさない。plain human writesの失敗はGo同様無視し、JSON write errorは報告する。UIDはsigned decimal int64でpositiveだけを受け入れ、`+`・whitespace/stdin・最大値・invalid・max1/unknown flag/helpを対象例で比較する。

UID指定時もcontrollerは最初のListを実行する。空/whitespace UIDはterminal選択へ進み、非TTYでは`uid is required`となる。選択label/optionsはfresh list順とraw usernameを維持する。JSONでもTTY promptは実行し、`--yes`はremoveの確認だけを省略してselectionを省略しない。removeはcontrollerのList→selection/confirmation→wrapperのList→domain remove→wrapperのfresh Listを保持する。第二Listの失敗はmutation前、第三Listの失敗はdelete後のcommitted stateとoutputなしで報告する。

Domain/configは埋め込みGo-first tablesの9 call-order/failure行、3 missing-account actions、6 exact document行、4 read/private-write failure行を固定する。`UseAccount`はGet→Set。`RemoveAccount`はexplicit default read→matching default clear→repository deleteであり、delete失敗時もdefaultを戻さない。Go source commentのrollback説明より実コードと再現結果を優先する。matching stale defaultのdirect domain removeでもclear後にnot-foundとなる一方、CLI wrapperはpre-delete Listで対象を検査する。CLIが表示するimplicit fallback defaultはconfigへ書き戻さない。

`AccountService`の既存構築を維持するadditive management/default-storeと、normal CLIで使う`AccountSelection` compositionを共有する。default set/clearは既存lossless document formatter/private atomic writerを再利用し、owned UID commentの置換、unknown/comment保存、empty auth table除去、absent clear時のwrite、positive validation-before-read、fresh file read、read/write failures・Unix private modesを対象例で確認する。既存Windows private writerの未検証事項は継承する。

Terminalは実stdin/stdoutの両TTYを要件とし、raw mode/cursorの復元、arrow/tab wrap・case-insensitive/Unicode filter・rune backspace・Ctrl-W/X clear・Ctrl-U ignore・Ctrl-D/current・interrupt/EOF・Escape Vim j/k・7-row pagingと、strict yes/no/default-false・invalid retry・cursor editingを実Go Survey入力で先に固定する。Rust Unix PTYは本番promptに加えて実`pixiv` executableでdefault selection・remove cancel・confirmed removalのstdout/config/DB/token stateを確認する。最初のremove-success testはimplicit fallbackをpersisted Some(1)と誤認して失敗した。固定Go fixtureのclear後configとdefault1 outputを確認し、testだけをNoneへ直した。production/Go期待値は変更せず、失敗logを保持する。

initial colored ANSI fragmentは`CLICOLOR_FORCE=1`の固定Go bytesをexact substring比較する。full redrawはRust save/restore+erase-to-endとGo offset/resetが異なるため、全cursor positioning/wrapped width（Go x/text EastAsian width対Rust unicode-width/max1）・color environment・malformed CSI/非UTF8・native Windows console実行の互換証明とは扱わない。Ctrl-Dでfilterがno-matchの場合、固定Goはempty options indexでpanicし、Rustは`no available options`を返す具体差分を保持する。当該Rust regressionはbounded errorの検査であり、Go parityの成功行に数えない。

Focused app5・CLI5・最終terminal PTY7が成功した。Go related account/config/auth/CLIと台帳suite/vetは終了0（CLI40.729秒）。Go Cargo interop flag未指定のskipと既存Linux公開面snapshot不在1testの除外を保持し、strict全Go passとはしない。

full script初回はlib module順序/main行のFormatter差分で終了1となり、失敗logを保持してformattingだけを補正した。POWERSHELL_TELEMETRY_OPTOUT=1・Cargo offline・既存network restrictionのまま全stageを再実行し、終了sentinel0・346秒、Formatter/Clippy・workspace314passed/0failed/既存3ignored・release40.50秒の完了を確認した。独立read-only reviewは記録したterminal差分以外のblocking regressionを報告しなかった。

対象scopeの成功をauth全体・全terminal・全platform・whole migrationのverifiedへ外挿しない。残るauth import/export/check/refresh/login・callback/URL handler、startup/update/diagnostic/DB close、全clock/UTF8/flag/help/SQL取消/並行・Windows/macOS/各arch/配布は台帳に残す。

## Auth import/export・versioned bundle・private output（2026-10-09）

固定Goの2leafを、token argv/stdin/masked TTY import、offline bundle restore、current/UID/all export、stdout/output/forceまで接続する。既存OAuth・account repository・config/default・private close/replace・terminal line editorを再利用する。全credentialはsynthetic fixtureであり、実credentialの読取・upload・signinや実アカウントのwriteを行わない。

[CLI fixture](contracts/cli-auth-transfer.json)は61行・47,756bytes。51行はactual root executable、10行はreader/writer/diagnostic dependencyをexact比較する。別8 OAuth casesは実Go`pixiv.OpenWith`のsynthetic HTTP transportと同じRust async実行で、argv/stdin・added/updated・human/JSON・request form・rotated token/DBを比較する。Go-only opaque nonUTF8 stdin→OAuth formは証拠を保持し、Rust String/form boundaryの未解消差分としてpassing comparisonへ数えない。既存seed/state/helperを共有し、全flags/state/outputのCartesian matrixは増やさない。

stdin token readerは末尾LF/CRLFを1つだけ除き、残るCR/LFを拒否する。whitespaceはreaderでopaqueに保持し、serviceのTrimSpaceまで遅延する。positional tokenはreaderのline制限を通らない。bundle classifierの先頭判定はJSON whitespace4bytesだけで、Go-json.Validのhuge numeric literal・invalidUTF8/unpaired UTF16 stringも分類後のproxy-combination診断へ進む。importのinteractive判定はinput/output両TTYであり、export/既存TextValueのstdin-only判断を変更しない。bare flag prefixのCobra discoveryは既存auth scannerと同じにし、`--help export`をleaf helpへ読み替えない。

オンラインimportはbefore List→network options→OAuth→credential save→explicit default policy→fresh summaryの順を保つ。OAuth failure/cancelはsaveなし、default read/setや後続summary失敗は既にsaved credentialを戻さない。owned synthetic Transportのdropと既存Context取消を検査し、Go caller-injected HTTP clientのCloseIdleConnectionsがno-opであることを実行確認した。SDK-owned explicit idle close・cloned/shared reqwest poolの寿命・実HTTPSの保証とはしない。

bundle restoreはstrict decode→fresh explicit default/list→単一DB batch transaction→必要時だけdefault set→outcomes。credential batch failureは全体rollbackし、post-commit default write failureはDB credentialsを保持してerrorを返す。bundle defaultが無ければ入力先頭UID、local explicit defaultがあればstaleでも維持する。order/revision/metadata/pool state・replacement statusを実temp SQLiteで比較する。bundle branchはnetwork/runtime/updateを不要とするGo requirementを保持するが、実OS update/diagnostic hooksの全証明ではない。

exportはexplicit UIDでもAccountsWithTokensと各rowのfresh default readを先に行い、UID0だけその後CurrentUserを読む。pool cleanupを行わない。`--force` without changed outputとall+UIDを先に拒否し、blank changed outputはsuccessful export後に拒否する。export UID errorはraw inputを再表示しない。bare stdoutはstored tokenのexact bytes+LFを単一writeし、error/short-writeをredacted`write stdout failed`へ包む。bundle JSONはGo同様invalidUTF8を各byteでU+FFFDへ置換し、compact field order/omitempty・HTML escapingを保つ。list/import/Debugのtoken非公開契約を変えない。

strict codecはexact duplicate object key scanをtyped decodeより先に行い、known fieldのGo casefold・null・ordered first diagnostic・unknown fields・number lexeme・schema/version/account/default validationを対象例で比較する。diagnosticのfmt%qとexport JSON escapingを混同しない。cached `unicode-general-category=1.1.0` Unicode16と47 frozen Go17 newly-printable rangesを組み合わせ、Go1.27.1の全1,112,064 valid scalarsについてunknown/duplicate-key errorの2 SHA256を独立比較する。generation/evidenceはtests配下に置き、1,114,112 slots・4,803 additions/0 removalsとsource hashを保存する。これは当該codec key diagnosticの比較であり、既存全CLI quoting/wireをverifiedへ外挿しない。

private writerはexisting parentのmode/ownerを維持する。noforceはexclusive target creationでありatomic visibilityを約束せず、incomplete targetをcleanupする。forceはprivate temporary→replace→protection/parent syncで、committed/unknown/not_committedを分け、Unix final symlinkをreplaceしてreferentを変更しない。path/tokenをDisplay/Debugへ漏らさず、underlying error categoryを保持する。reviewで発見したWindows committed backup-cleanup errorもACL reset/syncを実行する。Windows atomic protected creation・ACL/recoveryコードは実装するが、target compile/run・owner/ACL・injected cleanup/protection/durability failureの未検証を継承する。

TTY secretは本番のraw mode/restorationとmasked editing・trim・empty validation retry・interruptをGo-first keysとUnix PTYで比較する。empty answerとnext tokenを同じGo read-ahead chunkに入れるとSurveyがreader再作成時に残りを捨てるためEOFになる一方、Rust direct fd readerは次answerを保持する具体差分を記録する。staged interactive retryのGo検査と混同しない。full renderer/cursor/width/color/CSI/native Windows debtは前checkpointから保持する。

初回focused compileはgetrandomのStdError変換、次はtest-only Context::background仮定で失敗し、既存APIに補正した。startup comparisonのhelp-prefix失敗は固定Go期待値を維持してdiscoveryを補正した。pre-classifier/quote-fix full scriptは339passed/既存3ignoredで終了0だが、final validationには使わない。post-fix scriptはFormatter/Clippy通過後、duplicate debug variantsのdisk exhaustion（ENOSPC/linker BusError）で終了1となった。失敗証拠を保持し、rebuildable target/debugだけを削除してsingle target/pinned toolchain/既存環境のまま全scriptを再実行する。POWERSHELL_TELEMETRY_OPTOUT=1・Cargo offlineを保持した最終scriptは終了sentinel0・369秒、Formatter/Clippy・workspace342passed/0failed/既存3ignored・release36.84秒を確認した。独立read-only reviewも最終classifier/quote修正を確認した。Go related CLI/auth/account/secret writer/ledger suiteとvetは終了0（CLI59.530秒）。Go Cargo interop flag未指定のskipと既存Linux公開面snapshot不在1testの除外を保持し、strict全Go passとはしない。

generic malformed-JSON syntax diagnostic、serde depth128対Go10000、source duplicate scannerのnil-map panic対Rust safe rejection、nonUTF8 token import、SDK lifecycle/context/実HTTPS、terminal packet/rendering、Windows/全arch/配布を未検証または具体差分として残す。操作全体のverified・auth全体・whole migrationの完了とはしない。残るauth check/refresh/login・callback/URL handlerはinventoryに保持する。

## Auth check/refresh・rotation/profile・all partial commit（2026-10-09）

固定Goのcheck/refreshをcurrent/explicit UID/stdin・human/JSON・refresh all・proxy全presenceへ接続する。既存OAuth・account repository/revision CAS・config/default・AccountOutを再利用し、credentialsは合成値、profile/OAuthはmock、DB/configはtempだけを使用する。Goには対応auth MCP toolが無いため、存在しない入口を追加しない。

[CLI fixture](contracts/cli-auth-validation.json)は42 actual-root行・33,291bytesで、startup/input/UID parse/error priority/help/output/config/DB保存bytesをexact比較する。別12組合せ（current/explicit/stdin × human/JSON × check/refresh）は実Go OAuth/profileと同じsynthetic request/rotation/metadata/outputを比較する。domain20例は3Go/Rust testでordered errors・identity/CAS・取消・fresh summaryを比較する。全flag/state/outputの直積は増やさない。

CheckはGetのerrorを直接返し、OAuth後のreturned-record UIDでidentity/revision CASを行い、username/UIDだけの最小summaryを返す。default=false・premium不明・pool field省略を保持する。既存OpenAccountはrequested UIDを使い続け、shared rotationのrefactorによる変更を専用regressionで防ぐ。Refreshはwrapped select→rotation commit→CurrentUser profile→fresh account nameを保持したmetadata commit→fresh account/default summaryの順で、profile/metadata/final default失敗が既にcommitted tokenを戻さない。premiumCheckedAtはDBにUnix秒を保存するがDTOへ出さない。期限切れfreezeはsummaryから省略するだけのsingle refreshと、List cleanupを先に行うallを区別する。

allはBackgroundでIDsをListし、command contextで順にrefreshし、全成功後だけまとめてoutputする。後の失敗でも先行credential/metadataを戻さない。default UID0の選択はnetwork optionsより先で、explicit UIDはdefault lookupを省く。initial startup runtime検査を維持しつつ、override無し（--no-proxy=falseを含む）はaccountごとにruntimeをreloadし、explicit proxy/--no-proxy=trueはloaderを呼ばない。proxyの排他はChangedのpresence、clearはtrueだけで判定し、既存import/exportの共通helperへ同じ挙動を抽出する。JSON frozen時刻のyear範囲errorは全refreshのcommit後に返し、partial JSONを出さない。human premiumはyes/no/unknown。

Focused app3、CLI validation7、42行startup、既存transfer OAuth4が成功した。最初のproxy2testはglobal HTTPS_PROXYがconfigより優先する既存契約を見落とし失敗した。失敗logとexpectationを保持し、Go同様environmentより優先する[pixiv.network].proxy_urlという実config境界で合成fixtureを隔離した。production環境precedenceは変更しない。full script初回はappのsignature折返し/module順のFormatter差分で停止し、logを保持してcargo fmtだけを適用した。

直前checkpointのduplicate debug variantによるdisk exhaustionを受け、published-clean時点でrebuildable target/debugだけを削除した。以後このcheckpoint全buildに一貫してtask-local CARGO_PROFILE_DEV_DEBUG=0・CARGO_PROFILE_TEST_DEBUG=0を設定する。debug情報だけを省き、optimization・assertions・test内容・script全stage・release profileは変更しない。CARGO_INCREMENTAL=0・Cargo offline・POWERSHELL_TELEMETRY_OPTOUT=1も保持する。最終full scriptは終了sentinel0・298秒、Formatter/Clippy・workspace354passed/0failed/既存3ignored・release39.10秒で完了した。

Go related CLI/auth/account/ledger suitesとvetは終了0（CLI44.111秒）。既存Linux公開面snapshot不在1testの除外と、Go Cargo interop flag未指定skipを保持し、strict全Go passとしない。独立read-only reviewは最終runtime/proxy/timestamp/rotation境界を確認し、scoped source finding無し。

nonUTF8 stored tokenの既存SDK String/form差分、SDK-owned CloseIdleConnections/shared pool/context/実TLS・全profile wire、実startup/update/diagnostic/DB close、SQL実行中取消/blocked mutex/全clock/flag/help/Windows・macOS・全archを未検証として残す。auth login/callback/URL handlerは未移植。今回の対象例成功をauth全体・whole migration verifiedへ外挿しない。

## Login SDK session・account completion依存（2026-10-09）

CLI loginのlocal HTTP/manual/TTY・remote relay/handoff・hidden callback/URL-handlerを縮小実装せず、先に一貫したSDK/app依存を固定する。このcheckpointはbrowserを開かず、host protocol登録・実signin・実credential生成/保存を行わない。PKCEのproduction random生成はmock OAuth session用であり、実Pixivへのgrant/accessは取得しない。

LoginSessionはCloneが共有するArc/atomic gate、Default empty handle、非消費callback acceptanceを持つ。invalid inputはgateより先に拒否し、valid inputはOAuth開始前にgateを消費するので、HTTP failure・cancel・future abortでも再使用できない。PKCE verifierは64random bytes、stateは32bytes、S256 challengeとGo query順を比較する。secret-bearing verifier/state/code/callbackをSDK SessionのDebug/Displayへ出さず、CredentialsのDebug/Serializeもtokensを省く。appの公開AuthorizationURLとCallbackOrCodeはGo同様取得/formatできるため、それらまでredactedと呼ばない。

Go実装はcommentと異なり、exact official HTTPS redirectとpixiv://account/loginの両方でstate省略を許す。その他URLもmatching stateなら受け入れる。known first query value、percent decoding、trim、malformed URL parse時のbare-code fallbackを固定し、arbitrary allowlistやURL正規化で置き換えない。official OAuth start/callbackのpure predicateはsession callback acceptanceとは別にし、origin/decoded pathを比較する。reviewで見つかったuserinfo/host forbidden characters、UTF8 percent-host・IPv6 zone/empty zoneを先にGoで再現してから狭く修正する。最終freezeはcallback30行・official predicate10行・response12行で、別one-shot/concurrent/transport/cancel/PKCE checksとRust7testを対応させる。

login responseは既存refresh decode部品を再利用するがrefresh動作は変えない。nested responseのnonzero markerによるroot fallback、refresh-token-requiredとUID/trim validationの異なるerror、access token欠落とzero/nonpositive expiryの成功、typed error/statusを比較する。全raw JSON casing/ordered duplicates/null/precision/UTF8/clock極値の互換証明とはしない。

app LoginService Start/CompleteとCompleteLoginは既存repository/default/summaryへ接続する。missing sessionを先に拒否し、OAuthを消費した後だけmissing account serviceを検査する。credentials validation→save revision1→direct configured default read（use=trueでも省かない）→必要時set→fresh account/default summaryの順を保つ。save/read/set/summary失敗で先行commitを戻さない。tempDBを使い、save後cancelでもconfig read/setが先にcommitし、summary Getのcancelを返すGo境界を比較する。Go4testの9ordered variants＋別post-save cancellation、missing/used sequence・pre-canceled save・canceled OAuth safe Display/causeとRust app4testを比較する。

Focused SDK7・app4が成功した。app初回compileはsummary Serializeという存在しないAPIを仮定し失敗したため、productionへserializationを追加せず実Debug/fields境界へtestだけを直した。次のcanceled OAuth testはDisplayにcontext causeを期待して失敗した。実Goはsafe transport Displayでcauseからcontext.Canceledを検索できるため、追加Go freeze後にexact Display＋is_canceled検査へ直した。両失敗logを保持する。

最終Go SDK/account/ledger related suiteとvetは終了0（SDK1.719秒、account0.120秒、ledger0.294秒）。Go Cargo interop flag未指定skipは残す。このscopeはCLI snapshotを変更せず、その不在testを実行したとは呼ばない。validation環境は前checkpoint同様offline・telemetry optout・task-local dev/test debug情報0・incremental0で、scriptの全stageとrelease設定は不変。最終full gateは終了sentinel0・291秒、Formatter/Clippy・workspace365passed/0failed/既存3ignored・release42.45秒を確認した。独立read-only reviewはparser修正とSDK/app順序を確認し、scoped blocker無し。

nonUTF8 callback query/hostのGo byte string対Rust String/form、direct SDK Context API、Go LoginOptionsのStart時HTTPClient injection・owned client生成/explicit CloseIdleConnections、全URL parse/wire/clock/random生成failure/全concurrency/platformは未検証または未移植として残す。caller-owned transportのComplete時注入をowned HTTP parityへ外挿しない。CLI login local/remote/hidden callback/handler・OS登録/restoration・全auth/whole migrationは未完了。

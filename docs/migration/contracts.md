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

Content API の 400 は InvalidArgument、401 は CredentialsExpired、404 は NotFound、410 は ContentUnavailable となる。OAuth の 400・401 は CredentialsExpired、404・410 は UpstreamError となる。両方で HTTP の 5xx は UpstreamError であり、通信不能の UpstreamUnavailable と区別する。retry advice は 401・429 と OAuth の 400 にだけ付く。

この比較は分類済み HTTP 応答に対する契約である。通信原因の分類・取消、認証開始と成功応答の全契約は未移植・未検証として残す。OAuth の refresh 失敗には公開 SDK の Open、code 交換には Complete の operation 名を使う。

```text
go test ./sdk/pixiv -run '^TestMigrationHTTPStatus' -count=1
cargo test -p pixiv-sdk --test http_status --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-http-status` を指定する。

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

この段階では共有の container と validation の比較のみ。HTTP の実ストリーム、HEAD/204/304 の空 body、redirect・cookie・条件付き取得、partial read・通信失敗、製品の reference 再解決・open/save、body の close 時のエラーは未移植または未検証。Go の明示 Close は Rust の所有権による Drop に対応させるが、実HTTPへの適用は別に検証する。

```text
go test ./sdk -run '^TestMigrationResource' -count=1
cargo test -p pixiv-sdk --test resource_io --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-resource-io` を指定する。

## Resource HTTP ストリームの Go 参照契約

[resource-stream.json](contracts/resource-stream.json) はローカル HTTP サーバーを使った Go 版の15ケースを固定する。GET・HEAD、206・204・304・404、途中で切れた body、301・302・303・307・308 の redirect、検証で拒否された redirect、redirect 回数の上限、Location のない応答を含む。送信ヘッダー、URL 検証の順序、応答の許可ヘッダー、読み取り結果と transport エラーを記録する。

今回は Go 参照契約の追加のみで、Rust の実HTTPストリームは未移植。先読みしない取得、cookie jar、キャンセル、TLS、製品側の reference 再解決・open/save は別途検証する。

```text
go test ./internal/services/pixiv/resource -run '^TestMigrationResourceStream' -count=1
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-stream` を指定する。

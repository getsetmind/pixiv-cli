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

`Client::from_credentials` は OAuth credentials のアカウント ID と期限を設定する。OAuth 応答の ID は文字列と整数の双方を受け入れる。access token だけから作る client はアカウントを推測せず、ランダムな非秘密の instance ID に cursor を結び付ける。Rust テストでも正規化前の instance ID の形式を確認し、別 client での再利用の拒否を比較する。

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

MCP の実行処理は現在時刻を共通 helper に渡し、日時計算の重複を除いた。前節の「半年/一年と日付境界が未検証」は、この168ケースの日付計算について解消した。CLI の検索 flags・日付入力検証から共通 helper への接続、長期 duration の実 MCP セッションから SDK HTTP query までの比較、clock の極端な範囲・全 timezone・他 OS は未実装または未検証。Go の汎用 AddMonthsClamped の全範囲を移植済みとは扱わない。

```text
go test -race ./internal/cli/commands/pixiv/search ./internal/mcpserver/pixiv/tools/search_illust -run '^TestMigrationQuickDateRanges' -count=1
cargo test -p pixiv-app --test search_dates --locked
```

基準を意図して更新する場合だけ Go テストの `-args -migration-update-search-dates` を指定する。Go の CLI と MCP の package は別々の出力ファイルを更新する。

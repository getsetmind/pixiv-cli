# pixiv-cli-rs の移植方針

Rust 版は、Go 版の機能と利用者から見える契約を維持して移植する。実装を増やす前に契約をテストで固定し、機能単位で SDK・CLI・MCP を検証する。全機能の検証が終わるまで、Rust 版を Go 版の代替として案内しない。

## 参照実装と現在地

参照実装は、この fork のコミット `4b4426487ef18bed276706daec385e0d0a6979f9` に固定する。取り込み済み upstream は `2b3ccbb1ca54bec9effdf8fc5e57515bda2370ab`。派生側で追加された辞典検索、うごイラ、ダウンロード報告、機械可読エラーも移植対象に含める。以後の upstream 更新は別の変更として扱い、比較対象を暗黙に更新しない。

既存の Go 実装、[CLI リファレンス](../en/cli-reference.md)、[SDK リファレンス](../en/sdk.md)、[MCP リファレンス](../en/mcp-tools.md)、既存テストを契約の資料とする。資料同士に差がある場合は、固定した実装の再現テストで確認し、判断を記録する。

方針を追加した時点の Rust 版は試作段階で、作品詳細、先頭ページの検索、うごイラの一部と OAuth の基礎に限られていた。その時点の Rust 契約テスト 5 件は限定的な入力検証や秘匿性を確認するもので、機能互換の証明ではなかった。現在の実装と検証範囲は [公開契約の台帳](ledger.json) と [比較テスト](contracts.md) に記録する。初期状態の記述やテスト件数を現在の移植範囲・完了の根拠に使わない。

## 維持する契約

| 境界 | 維持する内容 |
| --- | --- |
| CLI | コマンド、別名、引数、フラグ、既定値、入力 URL、設定の優先順位、終了コード、stdout/stderr、JSON/NDJSON、自動出力切替、パイプ切断時の振る舞い |
| MCP | ツール名、入力・出力 schema、structured content、エラー、cursor、キャンセル、stdio の JSON-RPC。stdout にログを混ぜない |
| SDK | 操作、型付き DTO、値の意味、検証、エラー分類、retry、ページ送り、resource reference、認証とキャンセル。Rust の型・async API には Go API との対応を記録する |
| 保存状態 | 設定パス・形式、未知キーとコメントの保存、SQLite schema、アカウント選択、トークン更新、credential revision、pool lease、ファイル権限 |
| 通信 | method、query/form、ヘッダーの要件、proxy、TLS、redirect 制限、timeout、retry、rate limit、資格情報を送る範囲 |
| ファイル・配布 | 命名、画像品質、原子的な保存、上書き、ZIP 検証、変換品質、OS 連携、更新検証、対応プラットフォーム |

Go と Rust のソース互換は成立しないため、SDK の移植は操作とデータ・振る舞いの対応を確認する。Go 利用者向けの module を未完成の Rust crate で置き換えない。

cursor と resource reference は不透明なデータでも外部契約である。版、binding、query digest、identity と検証条件を維持し、不正 cursor を先頭ページに戻して処理しない。既存の保存済み値を受け入れられる範囲を明示し、差が必要なら移行方法を先に決める。

## 機能台帳

公開面の追跡は [公開契約の台帳](ledger.md) と [ledger.json](ledger.json) に記録する。公開面を取得したことと、振る舞いの互換性を検証したことは区別する。

以下は移植範囲の分類であり、完了件数ではない。最初の工程で CLI のコマンド・別名・フラグ、登録済み MCP ツールと schema、公開 SDK 操作・DTO を列挙し、各操作に固有 ID を付ける。静的検索だけでなく、help と MCP の tools/list など実際の公開面も確認する。1 操作ごとに、参照ソース・既存テスト・Rust 実装・互換テスト・対象 OS・状態・未解決差分を記録する。行が未作成の範囲も未完了として扱う。

| 分類 | 必須範囲 | 参照ソース・テストの入口 | 方針追加時の Rust |
| --- | --- | --- | --- |
| 共通契約 | エラー、retry、cursor、resource reference、入力 URL、出力と終了コード | `sdk/error.go`、`sdk/cursor.go`、`sdk/ref.go`、`internal/cli/envelope_test.go` | 一部の試作、互換未確認 |
| 認証・設定 | PKCE、callback/relay、複数アカウント、選択・削除、import/export、refresh 永続化、pool、proxy、設定の読み書き | `internal/cli/commands/pixiv/auth/`、`internal/config/`、`internal/storage/database/` | OAuth の一部、保存系未移植 |
| 作品探索 | detail、全検索条件、local filter、全ページ、ランキング全モード、おすすめ、関連、feed | `sdk/pixiv/`、`internal/cli/commands/pixiv/`、`sdk/pixiv/cursor_test.go` | detail/search の一部、互換未確認 |
| 小説・シリーズ・ユーザー | 詳細、本文、検索、シリーズ、作者、作品一覧、関連・フォロー一覧 | `sdk/pixiv/`、`internal/mcpserver/pixiv/tools/` | 未移植 |
| 更新操作 | bookmark、follow、comment、reply、stamp と解除・削除、集約操作 | `internal/mcpserver/pixiv/pixiv_mcp_*mutation_test.go`、`sdk/pixiv/` | 未移植 |
| 画像・うごイラ | resource、画像品質・ページ範囲、入力パイプ、直リンク、ZIP/GIF/APNG、失敗時の報告・後始末 | `sdk/pixiv/resource_test.go`、`internal/media/downloader/`、`internal/media/ugoira/rust/`、`internal/cli/commands/pixiv/download/report_test.go` | metadata の一部、保存・変換は未移植 |
| FANBOX | session、creator/post/feed/tag、ページ送り、resource、solver と通信設定 | `sdk/fanbox/`、`internal/services/fanbox/`、`internal/mcpserver/fanbox/` | 未移植 |
| 辞典・逆画像検索 | 辞典 search/article、日本語・英語、SauceNAO/ascii2d、URL/ローカル入力 | `internal/cli/commands/pixiv/dic/`、`internal/cli/commands/pixiv/search/reverse.go` | 未移植 |
| MCP 実行環境 | 全ツールの登録、schema、結果、エラー、並行処理、アカウント寿命、stdio | `internal/mcpserver/`、`internal/cli/commands/pixiv/mcp/` | 未移植 |
| OS・配布 | browser session 取得、URL handler、署名付き更新、install/package、6 OS/arch の実行 | `internal/cli/commands/update/`、`internal/services/`、`scripts/` | 未移植 |

## テストを書いてから移植する手順

各操作を次の順に進める。CLI、MCP、SDK で共通の処理を使い、入口ごとの出力契約は別々に検証する。

1. 固定した Go 版で、正常系、境界値、エラー、状態変更を再現する。既存テストに不足する契約を追加し、Go 版で通ることを確認する。
2. 同じ入力と応答フィクスチャを使う Rust の契約テストを書く。期待値は Go 版の振る舞いから決める。失敗理由が未実装または既知の差分であることを確認する。
3. Rust の実装を追加し、SDK・CLI・MCP の対応するテストを通す。実装と同じ計算をテストに写すことや、mock が返した値だけを確認することは避ける。
4. 同じフィクスチャで両実装を比較し、出力・通信・保存結果の差分を確認する。差分を隠すために期待値や正規化を緩めない。
5. Formatter、Linter、関連テスト、本番ビルドを通して台帳を更新する。1 つでも未確認の契約があれば「検証済み」にしない。

まず作品詳細を 1 つの縦断機能として完成させる。欠けている DTO 項目、明示 JSON のエラー envelope、終了コード、MCP schema、resource reference まで含める。その次に検索の全条件と複数ページを移植し、先頭ページだけで完了扱いしない。

### 実装単位と検証の実行

作業の区切りは、利用者が実行できる機能とその依存を接続した単位にする。認証では Gate・Lease・refresh・保存を個別に追加するだけで終えず、client の取得と解放へ接続して検証する。基盤のモジュール数や比較ケース数を進捗の代わりにしない。未接続の依存を明示し、同じ機能の完了へ必要な順に進める。

実装途中は対象の契約テストを実行し、機能単位の変更が揃った時点で Formatter・Clippy・workspace 全テスト・release build と関連する Go 検証をまとめて実行する。小さな補助モジュールごとに全検証と commit を繰り返さない。必須チェックを省略せず、成功後は対象差分や検証条件が変わらない限り同じ検証を再実行しない。Go の参照 fixture も、対象入力や比較項目が変わった場合だけ明示的に更新する。

同じ target directory を使う Cargo の検証は順に実行する。Go/Rust 相互テストが内部で Cargo を起動する場合も含め、見かけ上独立した Go テストとの同時実行でビルドロックを競合させない。相互テストを独立した target に分ける場合は初回ビルドの費用とキャッシュを確認する。Go 用の CC と native 環境を Rust の子プロセスへそのまま引き継ぐと、通常の Rust 検証との設定差で native 依存を再ビルドする。2026-10-09 の Windows 調査では、コード未変更で Go 用環境へ切り替えると Cargo が LIB の変更を検出し、SQLite/TLS を含む依存の再ビルドに約18秒かかった。検証環境を揃えるか、キャッシュを分けて設定を安定させる。

### 現在の作業順と区切り

2026-10-09 のフロー見直しでは、直近12コミットのうち10件が DB・認証・pool・設定の基盤だった。`Execution` でこれらを組み立てた後も、CLI の `main.rs` は `PIXIV_ACCESS_TOKEN` と `Client::new` を使い、MCP stdio は直接 Client を受け取る。個別の比較テストが増えても、保存済みアカウントから作品詳細を取得する起動経路は完成していない。次の作業はこの未接続箇所を閉じる。

作業中の機能は作品詳細の1つに絞る。検索・新しい SDK 操作・認証管理の全操作へは広げず、作品詳細で必要になる既存の依存を接続する。共通基盤の追加が必要になったら、作品詳細のどの入力・状態・出力で差が再現するかを先に確認する。一般的な境界値の洗い出しだけを理由に作業範囲を増やさない。発見した未対応契約は台帳に残し、必要な順に実装する。

| 順序 | 次に接続・検証する内容 | 区切りの証拠 |
| --- | --- | --- |
| 1 | 既存 `.pixiv-cli` の設定・DB を使う CLI 起動。初回設定作成と入力・設定・認証エラーの順序も維持する | 隔離ホームで Go/Rust の stdout・stderr・終了コード・保存結果を比較。実ユーザーのホームへアクセスしない |
| 2 | 作品詳細の account 取得、refresh CAS、SDK fetch、出力、lease 解放 | 既存 DTO・出力 fixture を再利用し、実 DB と同じ実行経路で正常系・失敗系・pool replay を比較 |
| 3 | 同じ実行経路を使う MCP の作品詳細 | JSON-RPC セッションで schema・結果・認証エラー・キャンセル・解放を比較 |
| 4 | 作品詳細に残る resource・通信・OS の互換差分 | 台帳の差分を1件ずつ再現し、対応する比較と対象環境の証拠を記録 |

各作業の開始時に、現在の差分と直前の検証結果を確認してから、上表で最初に未完了の経路を進める。調査の区切りは次の実装を決められるところまでとし、同じソースの読み直しや方針の再説明を進捗に数えない。Go の既存 fixture と比較 harness を再利用し、同じ要件に別の基盤テストを増やす前に接続した経路で検証できるか確認する。

実装中は変更した境界の比較テストで失敗と修正を確認する。接続した経路の変更が揃ったら必須チェックをまとめて実行し、そこで初めて commit・push の区切りにする。フローや文書だけを修正した場合に、変更のない Rust の全チェックを繰り返さない。チェック失敗後は原因の対象を絞って修正し、必要な全チェックを再実行する。Formatter・Clippy・本番コードとテストの分離は引き続き必須とする。

進捗報告では、接続して使える機能、比較で確認できた範囲、未接続・未検証の範囲を分ける。台帳の着手行の割合やテスト件数を完成率へ換算しない。全対象での検証完了が0件でも、それを実装量が0であるという説明に使わない。作品詳細の一部が接続できても、detail の他 entity、全認証操作、検索、他サービス、配布が完成したとは扱わない。

### 比較テストの設計

- 通信を fixture transport または隔離したローカルサーバーで再現し、応答だけでなく送った method、path、query/form、必要なヘッダー、要求回数を確認する。本番の URL 制限をテスト都合で解除しない。
- CLI は子プロセスで実行し、stdout、stderr、終了コードを捕捉する。TTY/pipe、明示 JSON/NDJSON、自動 NDJSON、BrokenPipe を区別する。MCP は JSON-RPC セッションとして tools/list、呼び出し、エラー、キャンセルを検証する。
- JSON の object key 順だけは正規化できる。欠落と null、空配列、配列順、整数の範囲、日時、cursor、resource reference、retry 秒数は契約として比較する。時刻・乱数・一時パスは制御するか、理由を記した限定的な正規化を使う。
- 検索は 2 ページ以上、filter でページ内の一部・全部が除かれる場合、cursor の別 query/別 account への流用を検証する。不正 cursor は通信前に拒否する。
- 更新操作は fixture 内で比較する。同じ bookmark や comment を実サービスに Go/Rust で二重送信しない。
- 設定と DB は合成データのコピーで before/after を確認する。refresh の同時更新、CAS、pool lease、キャンセル・プロセス停止後の復旧を含める。テストは実ユーザーのホームや認証 DB を使わない。
- ダウンロードは部分 body、失敗した redirect、ZIP の重複・不正 path、容量上限、キャンセルを再現する。失敗時に完成扱いのファイルが残らないことを確認する。GIF/APNG は decode 後の frame 数・delay・寸法と品質を検証し、必要な範囲だけ byte 一致も使う。
- 資格情報、Cookie、署名付き URL、proxy password は fixture・golden・差分ログに残さない。偽の秘密値を使い、出力に漏れないこともテストする。

ライブ試験は、fixture で検証できない通信・OS 連携を補う。読み取りでも時点依存の応答を golden にしない。書き込み試験は明示的に許可されたテスト用アカウントでのみ行う。ライブ試験が未実行なら、その範囲を未検証として記録する。

## 本番コードとテストの分離

Rust の本番コードは `crates/<crate>/src/`、契約テストは `crates/<crate>/tests/` に置く。共有ヘルパーは `tests/support/mod.rs`、フィクスチャは `tests/fixtures/` に置く。複数 crate で使う比較 harness は別の非公開テスト用 package とし、本番 package の通常依存に含めない。

`src/` には `#[test]`、`#[tokio::test]`、`#[cfg(test)]`、mock、fixture、テスト専用分岐を入れない。テストのためだけに内部関数を公開しない。公開契約や実際の transport/storage 境界から検証する。テスト専用依存は `[dev-dependencies]` に置く。

既存 Go テストは `_test.go` に分離されており、その構成を維持する。既存の独立したうごイラ Rust crate と vendor は、新規 workspace と区別する。対象にした時点で同じ品質基準を適用し、vendor に機械的な整形を掛けない。

## Formatter・Linter・必須チェック

リポジトリ直下から次を実行する。

```powershell
./scripts/check-rust.ps1
```

スクリプトは workspace の `src/` にある代表的なテスト属性を検査し、以下を順に実行する。1 つでも失敗すれば停止する。属性検査は補助であり、別名 macro、外部 module の取り込み、テスト用データや依存の混入はレビューでも確認する。

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

整形が必要なら `cargo fmt --all` で修正して再実行する。warning を無条件に許可する設定や、チェック失敗を成功扱いにする処理は追加しない。既存の全体的な検証に加え、機能台帳に対応する互換テストの実行を必須とする。新しい test package は workspace に登録してチェック対象に含める。

本方針の追加時点では上記 4 コマンドが成功し、既存 Rust テスト 5 件が通った。Windows の release ビルドでは、MSVC linker の import library 作成メッセージに対する `linker_messages` warning が 1 件出た。Clippy の warning とは分けて記録し、全機能互換や他 OS の検証済みとは扱わない。

Go 側を変更する場合も対象 Go toolchain の gofmt、go vet、関連テストを通す。現在の Windows 環境では Go 全体テストに次の既知の失敗がある。これらは成功扱いにせず、環境・失敗テスト・再現結果を分けて記録する。

| パッケージ | 既知の失敗の内容 |
| --- | --- |
| `internal/storage/file/secret` | `TestWriteSecretFileForceReplacesDifferentWindowsOwner` の Windows owner 変更権限 |
| `scripts/internal/homebrewformula` | 削除済み template を参照するテスト |
| `scripts/tests/installers` | Windows architecture fixture と対応 architecture 判定の不一致 |

削除済み GitHub Actions はこの方針の追加に伴って復活させない。CI を導入する際は同じチェックを必須 job にし、OS 別の互換テストも組み込む。ローカルチェックだけで CI や他 OS の検証済みとはしない。

## 移植順序と実装上の判断

1. **契約の固定**: 公開面の台帳、Go 基準結果、比較 harness、隔離 fixture、品質チェックを用意する。
2. **共通基盤と認証・保存**: エラー、cursor/resource、設定、DB、refresh、pool を検証する。試作中の OAuth にそのまま依存して機能を広げない。
3. **作品詳細の縦断移植**: SDK・CLI・MCP と resource の契約を揃え、比較方式が機能することを示す。
4. **検索・一覧・読み取り**: ページ送りと filter を先に揃え、ランキング、feed、ユーザー、小説、シリーズへ広げる。
5. **更新操作・媒体・追加サービス**: 状態変更、画像保存、うごイラ、FANBOX、辞典、逆画像検索を台帳に従って移植する。
6. **OS 連携と配布**: browser session、URL handler、更新、install/package と各 OS/arch を実機または適切な runner で検証する。

FANBOX の TLS fingerprint、browser cookie の OS ごとの取得、署名付き更新の信頼鍵は依存選定前に小さな実証を行う。現在の reqwest 採用だけでは Go 版の通信互換を証明できない。うごイラは既存の Rust 実装をまず再利用し、アルゴリズム変更と移植を同時に進めない。派生 repo の配布では署名者と信頼鍵を明示し、upstream の署名者と混同しない。

CLI と MCP は SDK と共通のアプリケーション処理を呼ぶ。サービス固有の TLS、solver、認証を必要に応じて分離し、すべてを 1 つの HTTP client に押し込まない。SDK の型付きデータを汎用 JSON だけに置き換えて項目不足を隠さない。

## 切替・完了条件

各操作は、正常系・異常系・ページ送り・状態・各入口の契約テストが揃い、差分が解消され、必須チェックが通った場合に「検証済み」とする。Rust 側のテスト件数や実装ファイル数から移植率を算出しない。

全体の切替には、公開面の全操作が台帳にあり、未移植・未検証・未承認の差分が残っていないことが必要である。対応対象は Windows/macOS/Linux の amd64/arm64。従来の Linux glibc 要件を含む配布条件も検証し、Windows だけの成功から他環境の成功を推定しない。

検証中の Rust バイナリは候補として明示的なパスで実行し、既存の `pixiv` を自動で置き換えない。`.pixiv-cli` の設定・DB を引き継ぐ互換性は合成データで先に確認する。新しいディレクトリを作って既存アカウントを見えなくする変更や、旧 Go 版で読めない schema 更新を暗黙に行わない。保存形式の変更が必要なら、backup、移行、rollback を検証してから切り替える。

機能縮小が必要になった場合は、台帳に影響と代替案を示して合意を得る。未実装処理を成功・空結果として返すことや、黙って Go に fallback して Rust の互換不足を隠すことはしない。

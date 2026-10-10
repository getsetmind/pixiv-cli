# 公開契約の台帳

[ledger.json](ledger.json) は固定 Go 版の公開契約を列挙する機械可読台帳である。SDK の型・定数・関数・メソッド 524 宣言、CLI 84 コマンド、Pixiv MCP 54 ツール、FANBOX MCP 11 ツールの計 673 項目を登録した。CLI の各コマンドに属するフラグと別名、MCP の schema は参照 snapshot に含める。

各項目は `pending` から開始する。現在の Rust 試作に対応する操作があっても、互換テストと対応付けるまでは検証済みにしない。この台帳は公開面の追跡用であり、保存・通信・キャンセル・OS 連携の内部契約は [移植方針](strategy.md) に従って追加の振る舞いテストで確認する。

振る舞いの比較結果と Go/Rust の対応は [比較テスト](contracts.md) に記録する。`verified_platforms` に実行環境が記録されていても、`status` が `in_progress` なら契約全体の検証は未完了である。

## 取得した基準

| 基準データ | 取得方法 | 検証範囲 |
| --- | --- | --- |
| [SDK](reference/sdk.json) | Go AST を解析し、公開型、値、関数、receiver を持つ公開メソッドを抽出 | 宣言の集合と型・signature。定数の実行時値や動作は別途検証 |
| [CLI](reference/cli.windows-amd64.json) | 実際の Cobra command tree を構築し、各コマンドと local/persistent/inherited flags を列挙 | Windows amd64 の構成。引数検証、実行結果、他 OS は別途検証 |
| [Pixiv MCP](reference/mcp-pixiv.json) | 本物の server に in-memory MCP client で接続し、tools/list を全ページ取得 | 公開 tool 名、description、input/output schema など。tool の実行結果は別途検証 |
| [FANBOX MCP](reference/mcp-fanbox.json) | 同上 | 同上 |

[manifest.json](reference/manifest.json) に参照コミット、取得ツールチェーン、CLI 取得環境、各 snapshot の SHA-256 を記録する。生成時には追跡中の本番 Go コード・go.mod・go.sum が参照コミットと一致することを確認した。新しいテスト以外の Go コードは変更していない。

snapshot は比較に使う仕様データであり、本番コードから読み込まない。SDK は各宣言の source と行番号を持つ。CLI は command path、MCP は service と tool name をキーにする。別プラットフォームの CLI は `cli.<os>-<arch>.json` として取得し、台帳に新しい公開項目があれば追加する。未取得のプラットフォームでは CLI snapshot テストが失敗する。Windows の snapshot を他 OS の証拠として流用しない。

## 台帳の更新

- `reference`: 参照 snapshot と項目のキー。
- `status`: `pending`、`in_progress`、`verified` のいずれか。
- `rust_sources`: 対応する Rust 実装のリポジトリ相対パス。
- `tests`: 契約を検証するテストの相対パス。必要なら `#テスト名` を付ける。
- `verified_platforms`: 実際に検証した OS/arch。
- `differences`: 未解決の互換差分。

着手時に対応ソースと不足契約を記録し、テストを追加するたびに対応付ける。`verified` にするには、関連する振る舞いテストの成功と対象環境の証拠が必要である。台帳検査は重複・欠落・未知 ID・不正 status・存在しない証拠ファイルを検出し、証拠が空または差分が残る `verified` を拒否する。ただし、テスト内容と検証範囲の妥当性はファイル存在だけでは証明できないため、結果と内容のレビューも必要である。

## 確認コマンド

既存 Go 実装をビルドできる Go/CGO・ネイティブライブラリ環境で実行する。設定・認証情報・実サービスへのアクセスは不要である。CLI テストではホームを一時ディレクトリに隔離し、コマンドの実行や startup hook は呼ばない。

```text
go test ./internal/cli ./scripts/tests/migration -run '^TestMigration' -count=1
go vet ./internal/cli ./scripts/tests/migration
```

通常実行で snapshot や台帳を更新しない。公開面の変更を検出した場合は、変更理由と固定 Go 版との差を確認する。Rust の出力に合わせて基準を作り直さない。

初回取得や、合意された基準更新に限り、次の明示 flag を使う。実行対象の Go ソースと参照コミットの一致を先に確認し、生成後に manifest の digest、台帳の対象項目、差分をレビューする。

```text
git diff --exit-code 4b4426487ef18bed276706daec385e0d0a6979f9 -- '*.go' ':(exclude)*_test.go' go.mod go.sum
go test ./internal/cli -run '^TestMigrationCLI' -count=1 -args -migration-update-cli
go test ./scripts/tests/migration -run '^TestMigration(SDK|MCP)' -count=1 -args -migration-update-surfaces
```

上記の git diff は追跡中のファイルの比較であり、新規・未追跡の本番 Go ファイルは別途 status で確認する。manifest は通常のテストから自動更新しない。

## Reverse-search Go-first evidence

The CLI search and MCP reverse_search entries now link actual connected Go test producers before Rust implementation. Their existing status and verified_platforms are unchanged. This contracts-only boundary is documented in [reverse-search-contracts.md](reverse-search-contracts.md); fixture count is evidence, not feature completion.

## Reverse-search connected continuation (2026-10-10)

The connected candidate adds shared source/provider assembly and actual CLI/MCP output/cleanup to the historical Go-first freeze. Only `mcp:pixiv:reverse_search` changes from pending to in_progress; CLI search was already in_progress. The673 identities remain unchanged:612 in_progress,61 pending and zero verified. Existing verified_platforms are retained without additions. [Current bounded scope](reverse-search-contracts.md#connected-rust-candidate-2026-10-10) records core91/99, SauceNAO268 non-nil runtime/277 harness rows (nine nil-receiver representational rows), ASCII2D176/192 public rows, CLI147/148 and separately scoped MCP52 raw-wire/35 direct-result/lifecycle schedules. Third full-script terminal exit0 and focused Go success coexist with the retained broad Go schema-diagnostic failures and native/compatibility debts; declaration counts are not a completion percentage.

## Signed updater Go-first evidence (2026-10-10)

The existing `cli:pixiv update` row links all seven frozen Go producers while remaining pending; rust_sources and verified_platforms stay empty and no other row changes. [Updater scope](updater-contracts.md) distinguishes root170 (158 behavioral/12 Go-only), coordinator305 (296/nine), release/cache160, source200 (167/33), Install199/detector34/cache13, source-driven Windows26 and owned native Linux preflight41. Private/nil/language identity and native limits remain explicit. All673 identities and612 in_progress/61 pending/zero verified statuses stay intact. [Durable evidence mapping](provenance/updater-evidence-map.json) retains final logs and failed/correction history; discarded public probes, failed race witnesses and zero-match commands are excluded from successful validation. Fresh unchanged Rust baseline full gate passed in268 seconds (919 workspace targets plus seven native children,926 raw passes, zero failures,11 existing ignored; fmt/strict Clippy/tests/release). [Final validation](provenance/updater-contracts-final-validation.json) records unchanged complete inputs. Neither Go counts nor baseline preservation makes the updater usable or verifies a new platform.

## Connected updater candidate (2026-10-10)

Only `cli:pixiv update` advances from pending to in_progress, with normal app/CLI source and test ownership and no verified platform. The 673 identities are preserved: 613 in_progress, 60 pending and zero verified. [Bounded implementation scope](updater-implementation.md) retains original 1,148 Go observations plus 58 actual supplemental observations without changing published fixtures. Injectable CLI 109 rows and actual binary 18 frozen parser/help observations do not establish all 158 behavioral root rows. Diagnostics/error-joining, native transport/OS/distribution and wider lifecycle debts remain explicit. Final gate and scoped review evidence are recorded independently.

## SDK HTTP ownership Go-first evidence (2026-10-10)

Constructor/Open, retained Login/close and representative content/resource entries now link four unchanged-Go producers and their sealed fixtures. All673 identities/statuses/verified platforms and Rust mappings remain unchanged; CloseIdleConnections remains pending until genuine owned native controls are connected. [Bounded evidence](sdk-http-ownership.md) distinguishes30 ownership,21 Login,9 native HTTP/1 and24 Context cases from Rust implementation and native-platform verification. The published full Rust baseline is preserved, not relabeled as a new ownership pass.

## Ordinary HTTP/1 candidate (2026-10-10)

Only sdk/pixiv:Client.CloseIdleConnections advances from pending to in_progress. All673 identities and prior verified_platforms are preserved:614 in_progress,59 pending,zero verified. Constructor/Open entries gain normal SDK/native source and test links, with validated Go mappings versus retained anonymous-resource Rust extensions explicit. [Current bounded mapping](sdk-http-ownership.md#connected-ordinary-http1-candidate-2026-10-10) covers five of nine native retirement rows plus four distinct ownership lifecycles, not nine retirement rows. Remaining Context/Close/retained Login/buffered-body/default-construction/HTTP2/platform/distribution obligations remain differences.

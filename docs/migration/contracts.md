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

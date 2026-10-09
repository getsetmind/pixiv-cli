# Bookmark read CLI 契約行列の監査

2026-10-09 の固定 Go fixture を監査した。行数を機能数や検証済みの割合に換算しない。現在の入力・失敗・期待値は削除せず、以後の契約固定で不要な直積を増やさないための記録とする。

| fixture | 行数 | bytes |
| --- | ---: | ---: |
| cli-bookmark-reads.json | 1,600 | 2,095,843 |
| cli-bookmark-reads-records.json | 7,315 | 10,528,787 |
| cli-bookmark-reads-bodies.json | 582 | 806,667 |
| cli-bookmark-reads-startup.json | 835 | 554,415 |
| cli-bookmark-reads-pool.json | 312 | 341,137 |
| cli-bookmark-reads-pool-bodies.json | body map | 2,277 |

合計10,644行、14,329,126 bytes（13.67 MiB）。Go の対象 replay は7.190秒。初期10,623行との差12行は、`--type` 自体を省略した既定kindの実入力であり、明示artworkとの区別を保存する。さらに既存startup826行を保持し、未対応flag4形状と明示JSONのroot診断を9代表入力だけ追加した。Rustの最終835 startup行の実process比較は15.42秒、mainのexact/rejection比較は9.88秒、312 pool行は1.33秒。全体gateのruntimeはcontracts.mdに記録する。

main/records/bodies の完全な観測結果（operation、kind、path/query、commit、error、stdout/stderr、exit、read bytes、proxy/json callback）を集計すると、異なる結果は433/1,013/147種類だった。これは冗長性の目安であり、同じ診断になる異なる不正入力の意味を消す基準ではない。record fixture は92入力を含み、5,359行ではHTTP要求が発生しない。

明確な過剰直積の例は、detailで未対応の`--ndjson`がrecord読み取りより先に失敗する736行（92入力×2kind×該当mode/引数）である。各record内容はこの経路の結果に影響せず、同じflag失敗を反復している。mainでも同一flag失敗がartwork39回、novel38回現れる。現在の固定証拠は保持する。

異なる境界として必要なのは、kindとendpoint/DTO、record型とID解決・読み取り量、出力modeとエラーenvelope、writer failureとcommit/EPIPE、設定と起動順、poolのidentity選択・target保存・replay・leaseである。raw bodyの型/null/数値違反も、結果が同じでもdecoderの異なる境界を試すため一括削除しない。

以後の固定では次を使用する。

- 共通record/response/出力処理の既存fixtureとhelperを再利用する
- 独立dimensionは共有したparameterで表し、decoder/renderer全組合せをコピーしない
- parser失敗は実際の各公開境界で代表入力を固定し、到達しないrecord/body/writerの直積を作らない
- 型、数値端点、null、順序、エラー優先順位などの境界入力と、相互作用するdimensionの組合せを明示する
- 正常なkind/output形とwriter/commit/replay境界は省略しない
- fixture bytesとGo/Rust runtimeをcheckpointで記録する

Cobraのleaf harnessとGoの実際のRun/root parserは、同じ未対応flagでもexit/messageが異なる。片方の期待値を他方の実行境界に当てはめない。Rustに存在しないleaf診断rendererを比較済みとせず、その差と実際のroot startup比較を分けて記録する。

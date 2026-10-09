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

## 次checkpoint: timeline

同じ方針をfollowing/latestに適用し、SDK221、MCP213+pool8、CLI main75（73処理/2拒否）、startup22、pool18、scalar33の590行に絞った。body mapと過去のrich DTO/helperを再利用し、BindNoInputのrecord×kind×mode直積は作らない。10 fixture/evidence filesは計1,745,217 bytes（1.66 MiB）。anonymous cursor生診断の815 bytesはcomparison rowに数えない。Scalarの4追加はoverflowと不正suffixの優先順位、control/Unicode quotingというdistinct behaviorを固定するためであり、writer/body/outputとの直積は増やしていない。観測runtimeはcontracts.mdに記録する。

### MyPixiv checkpoint

MyPixivはSDK195、MCP135+pool6、CLI main77+startup25+scalar33+pool18の489行を固定し、共有bodyを使った9fixture filesは810,546 bytes（約0.77 MiB）。CLI mainは75 exactと2 rejection-only、startupは24 real processと1 injected reader-error証拠に分け、後者を実process比較の件数へ加えない。scalar33は値正規化と実startupを共通fixtureで比較する。kind/output/inputの全直積を追加せず、aggregate対explicit-ID、users対worksの入力/autoNDJSON、identity-before-cursor、offset-only continuation、filter/window、writer/commit/replayの相互作用を対象とする。旧fixtureや失敗行は削除しない。

### User compatibility checkpoint

新fixtureはCurrentUser9、専用user profile8、実startup61の3files・78行・48,205 bytes。owner searchの共通fixtureは255行を再利用し、新規copy/matrixは作らない。root-only flagの54行をowner選択から除くが既存rootテスト/期待値は維持する。Go owner225行と既存shared raw-wire30行の証拠を区別し、255件全てが今回Go ownerで再実行されたとは記さない。専用detailのtext入力/safe表示/empty error、CurrentUserのidentity/query/error/headerとcached Usernameが今回の新しい境界である。

### Comment/stamp read checkpoint

SDK144、CLI98 main+81 pool+30実startup、MCP72+4 poolの429行を8files・672,579 bytesに固定する。全CLI行はexact比較で、leaf renderer拒否のみの行はない。pool81は9scenario×3mode×3operationで、comment lease内buffer/metadata/replayとstamps lease外writer、namespace DTO/query、mode別empty commit/EPIPE/partial prefixを区別する。shared bodiesと既存pool/saved helpersを再利用し、その他のbody/input/schema dimensionは無条件に交差させない。1 duplicate-resource unitと4 raw-resolution unit例は行数へ加えない。成功short writerはhuman1例だけ追加し、全mode/kindへ直積拡張しない。fullgate未完了のtelemetry拒否をscoped比較成功から分けて保持する。

Comment mutation checkpointは6fixture・457行・305,470bytes。SDK136、CLI146（140exact/6rejection-only）+46actual startup+22realpool、MCP99+8realpoolで、451exactと6standalone parser/no-effectを区別する。validation/wire/headerとleaf/schema/write-commitの独立境界をtargetedで固定し、既存common helperを再利用する。先のread matrixをそのままmutation全直積へ拡張しない。

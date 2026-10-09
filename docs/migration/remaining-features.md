# 残る操作と検証範囲

2026-10-09 のコード・固定 Go 公開面・[台帳](ledger.json) を照合した作業用の一覧。台帳の673項目は型・定数・別名も含むため、pending 件数を機能数や移植率に変換しない。下記は次の縦断移植候補であり、全契約の検証済み一覧ではない。

## 未移植の入口・操作

| 操作群 | 既存 Rust の部品 | 残る範囲 |
| --- | --- | --- |
| download・媒体処理 | SDK resource open/save、ugoira metadata、CLI metadata 試作 | Go の download 計画・進捗・record 入力・失敗/取消、MCP download/random recommendation、ugoira 取得/変換等の全 workflow |
| auth の操作入口 | 合成 DB/config/account、refresh CAS、pool/lease | Go の CLI auth import/export/login/check/refresh・callback/URL handler 等。list/use/remove・pool status/enable/disableは下記scopeで接続。下位部品の存在を入口実装と数えない |
| 公開 client lifecycle | OAuth/HTTP client 基盤 | Go の Client.CloseIdleConnections 等の残る公開契約。通信テストだけの不足と区別する |
| 追加サービス・OS・配布 | Pixiv 共通処理 | FANBOX、辞典、reverse search、update/install/browser/URL handler 等。各 Go 実装の実在範囲を確認してから小さな単位で固定する |

入口確認は `crates/pixiv-cli/src/main.rs`、`crates/pixiv-mcp/src/stdio.rs`、公開 SDK は `crates/pixiv-sdk/src/pixiv.rs` と関連 module を基準にする。Go 側の対応 command/tool/SDK を先に固定し、未実装の空結果・成功や Go fallback で不足を隠さない。

## 実装済みの比較に残る未検証

auth use/remove（full gate314passed・既存3ignored、terminalの具体差分あり）、auth group/list・pool group/status/enable/disable（full gate297passed・既存3ignored）、config group/path/get/set/unset（full gate290passed・既存3ignored）、comment/stampの8mutation SDK・4CLI leaf・8MCP入口（full gate280passed・既存3ignored）とread SDK・CLI/comment MCP入口、user detail/searchの互換CLI入口とCurrentUser/Username SDK、MyPixiv works/usersのSDK・CLI・MCP、timeline following/latestのSDK・CLI・MCP、user作品一覧のSDK・CLI・MCPと、bookmarkのSDK list/detail/tag・CLI list/detail/tags全kind/user bookmarks・MCP list/detail/tags/tag-allの入口は実装済み。bookmark add/removeとfollow/unfollowも既存checkpointで接続した。これらを未移植機能数へ加えず、[比較範囲](contracts.md)と次の検証負債を区別する。

- raw-wire 大小文字・重複・順序・null: User/CurrentUser/SearchUsers/4関係一覧/MyPixivUsers/comment read/Stamps/stamp resolutionの対象例を比較したが、当該familyの未比較variantと他familyは残る
- 全familyの不正UTF8・任意JSON precision・depth・全cursor payloadの未比較境界
- root parser の3差分、Linux 公開面 snapshot 不在、timeline/MyPixiv以外の既存scalar flagのbase0/bool forms、group単体help・全 flag/help/TTY/OS startup hook
- novel_content の複数未知 property の非決定的な診断順。固定 fixture は保持し、opt-in 検査でも exact order だけを未検証とする
- UserArtworks/UserNovels の page0/limit-1 同時違反時の first diagnostic 選択。Go map 走査により page と limit が入れ替わる2行だけを隔離し、固定 expectation は保持する
- Bookmark tag-all の複数schema違反2行のfirst diagnostic選択。固定期待値を保持し、Go opt-in検査は独立観測した完全messageだけを許容する
- Bookmark reads869行とtimeline2行・MyPixiv2行・comment mutation6行のCobra leaf parserはRustの拒否のみを検査し、exact standalone診断/exitは未検証。実root startupの比較とは区別する
- Timeline anonymous cursorのランダムinstance bytesと生repeated-cursor診断。binding結果を比較し、観測を別fixtureで保持する
- 他DTOのfractional日時とstamp URL parser normalizationの全境界
- verified/token-only GET/POSTのX-User-Id・trimmed Accept-Languageとadd/delete Retry-Afterの対象例を比較したが、その他header・全通信/Options・401/403 refresh replay、TLS/DNS/redirect、取消/deadline/concurrency/disconnect、正常実 HTTPS と実 resource
- config sparse documentの全quoted/dotted/inline/duplicate/syntax/非UTF8、private close/cleanup/commit故障、Windows ReplaceFileW compile/run/ACL、symlink/concurrent write
- auth terminalのCtrl-D/no-matchは固定Go panic対Rust bounded errorの具体差分。全cursor/redraw/width/color-env/malformed CSI/非UTF8・native Windows consoleは未検証
- auth localの実startup/update/diagnostic/DB close・SQL実行中取消/blocked mutex、service clock全境界/日時/flag/help/非UTF8
- Windows/macOS/Linux の amd64/arm64、配布・更新の署名/信頼条件

各 checkpoint の scoped 比較成功と、操作全体の verified・最終切替を区別する。次の操作を終えたらこの一覧と台帳を更新し、未検証を削除して成功扱いにしない。

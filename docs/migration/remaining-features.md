# 残る操作と検証範囲

2026-10-09 のコード・固定 Go 公開面・[台帳](ledger.json) を照合した作業用の一覧。台帳の673項目は型・定数・別名も含むため、pending 件数を機能数や移植率に変換しない。下記は次の縦断移植候補であり、全契約の検証済み一覧ではない。

## 未移植の入口・操作

| 操作群 | 既存 Rust の部品 | 残る範囲 |
| --- | --- | --- |
| user の作品一覧 | SDK・CLI・MCP の UserArtworks/UserNovels を今回比較 | 全 wire/通信/他 platform の検証は別に残す |
| user の互換入口 | top-level detail/search の user mode、follow | `user detail`、`user search` の Go 入口。現在の同等 SDK 呼出しだけで入口互換としない |
| user の関係・公開一覧 | following/followers/related/blocked の SDK・CLI・MCP を今回比較 | CurrentUser/Username の公開 SDK 契約、全通信と他 platform の検証 |
| bookmark の読み取り | SDK artwork/novel list・detail/tag、CLI list全kind/user bookmarks・add/remove、MCP3list・add/remove | CLI detail/tags、MCP detail/tags/tag-all の入口と全共有検証 |
| timeline・MyPixiv | artwork/novel/user DTO と cursor | following/latest、MyPixiv works/users の SDK・CLI・MCP |
| comment・stamp | エラー、record、更新操作の commit 基盤 | artwork/novel のコメント一覧・作成・削除・返信・stamp、stamp 一覧と各入口 |
| download・媒体処理 | SDK resource open/save、ugoira metadata、CLI metadata 試作 | Go の download 計画・進捗・record 入力・失敗/取消、MCP download/random recommendation、ugoira 取得/変換等の全 workflow |
| auth・config の操作入口 | 合成 DB/config/account、refresh CAS、pool/lease | Go の CLI auth/import/export/login/refresh/use/pool と config path/set/unset 等。下位部品の存在を入口実装と数えない |
| 公開 client lifecycle | OAuth/HTTP client 基盤 | Go の Client.CloseIdleConnections 等の残る公開契約。通信テストだけの不足と区別する |
| 追加サービス・OS・配布 | Pixiv 共通処理 | FANBOX、辞典、reverse search、update/install/browser/URL handler 等。各 Go 実装の実在範囲を確認してから小さな単位で固定する |

入口確認は `crates/pixiv-cli/src/main.rs`、`crates/pixiv-mcp/src/stdio.rs`、公開 SDK は `crates/pixiv-sdk/src/pixiv.rs` と関連 module を基準にする。Go 側の対応 command/tool/SDK を先に固定し、未実装の空結果・成功や Go fallback で不足を隠さない。

## 実装済みの比較に残る未検証

- User/SearchUsers/4関係一覧以外の raw-wire 大小文字・重複・順序・null、不正 UTF-8、任意 JSON precision と cursor payload の境界
- root parser の3差分、Linux 公開面 snapshot 不在、全 flag/help/TTY/OS startup hook
- novel_content の複数未知 property の非決定的な診断順。固定 fixture は保持し、opt-in 検査でも exact order だけを未検証とする
- UserArtworks/UserNovels の page0/limit-1 同時違反時の first diagnostic 選択。Go map 走査により page と limit が入れ替わる2行だけを隔離し、固定 expectation は保持する
- 全通信/Options、TLS/DNS/redirect、取消/deadline/concurrency/disconnect、正常実 HTTPS と実 resource
- Windows/macOS/Linux の amd64/arm64、配布・更新の署名/信頼条件

各 checkpoint の scoped 比較成功と、操作全体の verified・最終切替を区別する。次の操作を終えたらこの一覧と台帳を更新し、未検証を削除して成功扱いにしない。

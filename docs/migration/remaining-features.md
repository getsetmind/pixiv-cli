# 残る操作と検証範囲

2026-10-09 のコード・固定 Go 公開面・[台帳](ledger.json) を照合した作業用の一覧。台帳の673項目は型・定数・別名も含むため、pending 件数を機能数や移植率に変換しない。下記は次の縦断移植候補であり、全契約の検証済み一覧ではない。

## 未移植の操作と残る機能・環境

| 操作群 | 既存 Rust の部品 | 残る範囲 |
| --- | --- | --- |
| download・媒体処理 | SDK resource open/save、ugoira metadata、CLI metadata 試作 | Go の download 計画・進捗・record 入力・失敗/取消、MCP download/random recommendation、ugoira 取得/変換等の全 workflow |
| auth の残る契約 | CLI login/local/remote・hidden callback/install-handler・通常startup、SDK/app保存、default endpoint/handoff state、Linux XDG/Darwin/Windows backendへのadapterは接続済み | 全flag/help/TTY/raw-wire/取消・実browser/association/native OS検証、公開SDK HTTPClient ownership/idle-close。3つの新CLI入口はin_progressで、比較範囲は下記とcontracts.mdに分ける |
| relay HTTP/2 transport | HTTP/1 relay listener・synthetic trustによるnative TLS | Go TLS serverのHTTP/2 negotiation/servingはactual Go testで確認したが、Rust relayはHTTP/1のみ。HTTP/2 capabilityは未移植であり、単なる検証不足としない |
| OS URL-handler association | shared manifest・native browser/process、Linux XDG/Darwin/Windows ensure/disable/temp restore/delegate・ShellExecuteExW、CLI hooks/automatic startup policy | native OS/arch compile/link/runと実desktop integration・ACL・全IO/privilege/race |
| 公開 client lifecycle | OAuth/HTTP client・login one-shot 基盤 | Go の Client.CloseIdleConnections・LoginOptions Start時HTTPClient ownership・LoginSession.CloseIdleConnections 等の残る公開契約。通信テストだけの不足と区別する |
| 追加サービス・OS・配布 | Pixiv共通処理、browser/URL-handler backendとpending update cleanup | FANBOX、辞典、reverse search、公開update/install workflow・配布・署名/信頼条件。pending cleanupだけでupdate全体を実装済みとはしない |

入口確認は `crates/pixiv-cli/src/main.rs`、`crates/pixiv-mcp/src/stdio.rs`、公開 SDK は `crates/pixiv-sdk/src/pixiv.rs` と関連 module を基準にする。Go 側の対応 command/tool/SDK を先に固定し、未実装の空結果・成功や Go fallback で不足を隠さない。

## 実装済みの比較に残る未検証

CLI auth login、hidden callback/install-handlerと通常startupは接続済みで、今回の最終gateは終了0・277秒、top-level531passed/既存child1/raw532/ignored10・release成功で、scoped reviewとともに[CLI login checkpoint](contracts.md#cli-loginhidden-callbackstartup-の接続2026-10-09)へ記録する。Go loginのvalidation17行のうちRustの同じfixture比較は13行、local/remote12flow、terminal7行のうちPTY比較6行であり、Go dependency factory4errorとnative EOFは未比較。hidden fixture81CLI/4installer/12automatic/11persistent policyはparser・注入したdispatch/startup・安全な実Linux子プロセス64行に分ける。root helpの対象行はhidden名の非表示だけを確認する。exportはrequires_configと独立したrequires_startupでhooksから除外する。rootはhooks/default config/runtimeをleaf conflictより先に実行し、blocked config directoryのGo mkdir/notdirとRust Fileexists17診断差分を保持する。pending cleanup27行と環境proxy16行も対象境界の証拠であり、全auth・native OS・公開SDKのverifiedへ外挿しない。

auth check/refresh（full gate354passed・既存3ignored、対象fixture42root行・domain20例、詳細はcontracts.md）、auth import/export（full gate342passed・既存3ignored、具体差分は下記）、auth use/remove（full gate314passed・既存3ignored、terminalの具体差分あり）、auth group/list・pool group/status/enable/disable（full gate297passed・既存3ignored）、config group/path/get/set/unset（full gate290passed・既存3ignored）、comment/stampの8mutation SDK・4CLI leaf・8MCP入口（full gate280passed・既存3ignored）とread SDK・CLI/comment MCP入口、user detail/searchの互換CLI入口とCurrentUser/Username SDK、MyPixiv works/usersのSDK・CLI・MCP、timeline following/latestのSDK・CLI・MCP、user作品一覧のSDK・CLI・MCPと、bookmarkのSDK list/detail/tag・CLI list/detail/tags全kind/user bookmarks・MCP list/detail/tags/tag-allの入口は実装済み。bookmark add/removeとfollow/unfollowも既存checkpointで接続した。これらを未移植機能数へ加えず、[比較範囲](contracts.md)と次の検証負債を区別する。

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
- auth transferのnonUTF8 token importは既存SDK String/form boundary差分。codec malformed syntax/depth128対Go10000・source scanner panic、安全なRust拒否、secret retry read-ahead・Windows secret ACL/force recovery/durability failureは未検証または具体差分
- auth terminalのCtrl-D/no-matchは固定Go panic対Rust bounded errorの具体差分。全cursor/redraw/width/color-env/malformed CSI/非UTF8・native Windows consoleは未検証
- login SDK/appのone-shot/保存依存とCLI local/remote/callback/handler hooksは接続するが、実browser/OS登録と全flag/help/TTY/native EOF・Go dependency factory4errorは未比較。nonUTF8 callback query/form、direct SDK Context、全URL/raw-wire、random failureと公開SDK owned-client/idle-closeは残る
- remote handoff native transportはGoに無いAccept: */*をreqwest0.13.5が追加する具体差分が残る。Go不在/Rust実headerのcaptureを保持し、同じheadersとは扱わない
- remote handoffの137URL fixture/private stateとstreaming clientの37JSON/21env-proxy・local native HTTP対象例、login default環境proxyの16HTTP/CONNECT/未知scheme・port0例は比較するが、nonUTF8 URL/env values・全Go URL grammar/JSON/native error・全TLS/SOCKS/proxy実通信/redirected CGI/IDNA/IPv6zone/任意proxy URL grammar・gzip/wire read-error chunking・Windows sidecar lock compile/run/ACLと全並行scheduleは未検証。arbitrary Go Readerの0nil/data+EOFはRust HandoffReadに対応しない。state自体にはGo同様TTLを追加せず、relay serverのcaller context期限は対象例で比較するが、全clock/cancel/並行scheduleは残る
- relay serverのsession/proof/resultとfinal latchはHTTP1/local TLSの対象例を比較するが、HTTP/2は上記の未移植capability。全TLS configuration/handshake failure・乱数故障・JSON/HTTP wire・全cancel/disconnect/claim/notify schedule・native platformは未検証
- callback endpoint/default paths/dispatch/browser actionと実CLIのlazy default state配線は対象例で比較する。invalid linkはHOME解決前、remote startはnetwork応答後にstateを解決する順序を保持する。Linux previous-handler/XDG、Darwin/Windows association/ShellExecuteExWはsource-driven mock/synthetic providerで比較するが、実OS登録/browser・Windows USERPROFILE/path cleaning・ACLと全URL/JSON/storage故障は未検証
- local loginのnative hostname/service-port bind・Go net.OpError/AddrError exact診断、全HTTP/MIME/raw parser/wire/backpressure/flush・blocked prompt cleanup、duplicate waiter全scheduleは残る。missing-PEM Go ServeTLS failureはbound listenerがGCまで残り得るため、このerror行のlistener cleanup parityは未確立である
- auth localの通常startup policyとpending Windows cleanup27行は接続するが、native Windows実行・公開update/install全workflow・全diagnostic/DB close・SQL実行中取消/blocked mutex、service clock全境界/日時/flag/help/非UTF8は残る
- shared handler manifest23JSON行・nil/empty・depth/private storageを比較し、native browser7testはsynthetic Linux process/host-mocked dispatchを比較する。Windows/macOS/BSD native compile/run・全PATHEXT/lookup/env/rawerror・全merged-output post-start fault・Windows association ensure/disable/install/restoreはsource-driven mockで対象例を比較するが、native Windows実行とstartupの全native IO/permission/並行scheduleは残る
- Linux XDG backend/context processは対象例で比較したが、native IOのGo PathError書式差分、実desktop/全filesystem/privilege/env/signal/cancel raceは残る。persistent nonUTF8 pathのGo JSON-lossy restore結果を別証拠で保持し、temporary raw pathのlossless cleanupと混同しない
- Windows/macOS/Linux の amd64/arm64、配布・更新の署名/信頼条件

- Darwin associationの27flow/5cache-kind/2script/private compiler lifecycleはSHA-anchored frozen Go source-driven mock比較。Swift/plist assetsはbyte-exactだが、実Darwin compile/LaunchServices/native cancellation・全IO/permission/random faultは未検証。query/error Stringのinvalid UTF8はGo raw bytes対Rust replacementの具体差分。Combined raw binary保持と構築Captured accessor検査を、全native post-start errorの実行証明にしない

各 checkpoint の scoped 比較成功と、操作全体の verified・最終切替を区別する。次の操作を終えたらこの一覧と台帳を更新し、未検証を削除して成功扱いにしない。

- Windows associationは固定Go sourceのSHAを確認したmock比較。persistent first/repeat・failure residue・full registry treeのcopy/export/import・private backup・endpoint-first cleanup・wrapped exit-only absence・ownership substring・previous ProgID delegationを対象例で比較する。ShellExecuteExWはfull ABI/mask/null fields/showとcontext/UTF16/error orderingをmock検査する。native Windows compile/run・live HKCU・実Shell/registry loader・ACL・全IO/random/privilege/raceは未検証。temporary cleanupはGo同様2回目が復元済みtreeを消し、削除済みbackupのimport failureを無視する破壊的挙動があり、明示的な繰り返しを安全・idempotentとは扱わない。所有されたDropはexplicit cleanup後に再実行しない。CLIのowned installation adapterも自動cleanupを1回に限定する

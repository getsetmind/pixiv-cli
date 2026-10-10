# 残る操作と検証範囲

2026-10-10 のコード・固定 Go 公開面・[台帳](ledger.json) を照合した作業用の一覧。台帳の673項目は型・定数・別名も含むため、pending 件数を機能数や移植率に変換しない。下記は次の縦断移植候補であり、全契約の検証済み一覧ではない。

## 未移植の操作と残る機能・環境

| 操作群 | 既存 Rust の部品 | 残る範囲 |
| --- | --- | --- |
| download・媒体処理 | direct URL/opaque ResourceRef、static artwork PID/URL、ugoira GIF/APNG/ZIP/raw・record・user/bookmark全visual kind/全page展開・MCP randomの対象workflow、保存済みExecution・SDK atomic save、5quality/pages/templates/MIME publication | 全native publication/IO/parallel schedule、public Record/SDK lifecycle。bounded対象比較だけでdownload全体を完了としない |
| standalone ugoira metadata CLI | SDK metadata/DTOと旧prototype入口 | 保存済みaccount/config/proxy、detail→kind preflight→metadata、original-first archive、人間/JSON出力・pool replay・writer/lease/cancel契約。媒体変換workflowとは別で次の接続scope |
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
- root parser の3差分、timeline/MyPixiv以外の既存scalar flagのbase0/bool forms、group単体help・全 flag/help/TTY/OS startup hook
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

## 直リンク・opaque ResourceRef download の接続中

app26行、CLI63行、MCP25行、root27行を固定Goで先に採取し、shared reportから実CLI/MCP stdio・保存済みclient・SDK atomic saveへ接続する。root実Linux子プロセスは安全な23行を選択し、native hook/read-error4行は実行しない。MCP通知取消はpublished prefixを残した通常RPC結果とし、downloadだけcooperative Contextで処理する。CLI writerはclient callback内で書き、保存後のwriter failureでファイルやcommitを戻さない。最終gateは終了0・226秒、contract555/raw556/ignored10・release42.91秒で成功し、scoped reviewと具体差分をcontracts.mdへ記録する。全downloadのverifiedとはしない。

Go productionにはpersisted resume/job schemaがなく、download保存へのprogress callback注入もない。別のprogress rendererの存在をdownloadの新挙動へ読み替えない。user/bookmarkの受理済み入力をinvalid source・空成功へ隠さず、未移植errorとして残す。static作品/PID・ugoira・recordは各節の対象比較として接続する。HTTP2 serving・余分なAccept header・公開SDK ownership/idle-close/direct Contextと全native OS/parser/wireの既存負債も引き続き残る。

static illustration/mangaとunknown upstream kindのPID・artwork URLは、全5quality・pages・filename/directory templates・MIME publication・partial cancel/commitをGo-firstで一緒に固定する。MCP startup adapterはpathと両templateをreal RuntimeConfigから同じproduction conversionで渡す。direct resourceはGoもtemplateを使わないので、その対象比較をvisual namingへ外挿しない。user/bookmark全visual kindとrandomは別の残るworkflowとして保持する。ugoiraとrecordの後続対象比較は各節へ記録する。

実CLIのOS interrupt→Contextはowned watcher/main explicit contextへ接続中。Go root12行とMCP graceful-close4行を先に固定し、owned Unix SIGINT/SPI atomic cleanup＋実Rust MCP binaryのSIGINT/EOFを比較する。Go physical connection close対Rust future/body drop、Go Stop unregister対Tokio persistent Unix handlerの差分を残し、native Windows/全TTY/signal/raceを成功扱いしない。

今回のinterrupt checkpointはlogin detached Backgroundを維持し、MCP closing中のper-request cancellation notificationを処理し、通常responseを抑止してgraceful completionを待つ。normal EOFとcompleted-request disposalもGo-firstで固定する。最終gate後はstatic downloadの次の縦断scopeへ進むが、handler restoration/native platformの残る差分は維持する。

## static artwork download の対象比較

Go app82行・saved CLI40行・MCP29行とowned stdio通知取消1schedule、actual root preflight8行を固定する。Go IDsは昇順となり、page_count/vector不整合で同名pathをoverwriteする観測も保持する。MIMEはresponse→supported signature→MIME absent時だけsuffix fallbackとし、corrected targetはhard-linkでoverwriteしない。leading-dot basenameのGo filepath.Ext差分を追加5行でGo-first修正する。shared missing-capability/runtime red、root NDJSON error redとMCP defaults API redは実行済みであり、全成功と混同しない。

app fixtureの初回seedがwire illustをpublic Kindへcastしていた入力モデルの誤りを、public illustration/unknown+RawKindへ修正して実Goで再採取する。元の合法open-string custom producerはpermanent Go-only regressionに残し、Rust closed enumのrepresentation gapをpendingへ明示する。native SDK wireのunknown kindはCLI/MCP双方でstaticとして実比較する。runtime worker boundはGo GOMAXPROCSとRust available_parallelismの同一設定へ読み替えない。実accounts/外部media/authenticated network/native registry/browser/associationは使わない。

fullgate/review最終証拠はcontracts.mdへ追記する。user/bookmark全visual kindsとrandom、public Record/SDK lifecycle、HTTP2/Accept・Tokio handler restoration等は残る移植・検証対象である。recordの後続対象比較は次節へ記録する。

## ugoira artwork download の対象比較

GIF/APNG/ZIP/rawをstatic/directと同じsaved CLI/MCP共有経路へ接続する。Go-first app70・CLI67・MCP41・Linux native26・rawZIP73行を固定し、archive選択、quality/pages、template fallback、MIME/metadata/frames、原子的な保存・隔離、partial/Context、native old-output/temp cleanupを対象比較する。CLI artifactのugoira page omissionとMCP file-only quality/frames、frame_report/failure classified fieldsの実omissionを保つ。

元tracked core sourceのoffline buildは32 checked vendor patchesと既存root lockを再利用し、vendor1,633 file checksums・元root276 entriesを確認する。新resolved encoder21 dependency version置換とparallel miniz_oxide 0.9.1追加をprovenanceへ残し、旧Go native manifestと同一とはしない。caller future-dropは独立child Contextのowned cancellationでlate publicationを抑止し、parentを再利用する。Linux FIFO/gate waitの観測は成功画像encoding途中の全token cancel scheduleではない。全6platform/native IO/errors/FFI injected token-release失敗、全ZIP grammar/GODEBUG/65,536-entry runtime、public SDK/HTTP2/Accept、Tokio signal restoration、Windows repeated cleanupの既存リスクは未検証のままである。

record経路は既存が受理するartwork/illust/manga/ugoiraを同じExecution callbackへ接続し、今回はugoira成功/隔離の2行と既存invalid-record regressionsを比較する。multi-record grammar/skip/fail-fast/typed prefix/cancel/lifecycleの後続対象比較はrecord節へ記録する。user/bookmark展開の全visual kind・random recommendationも引き続き残るworkflowで、download全体をverifiedへ変更しない。CLI private events/per-attempt errorとMCP private manager/hooks、rawZIP private failure reportはpublic Rust observationsとの同一性を主張しない。

このugoira checkpointの最終fullscriptは終了0・296秒、meaningful594/raw595passed・ignored11、release58.65秒。Formatter/Clippyの先行failureは保持し、dependency-onlystyle metadataとequivalent ZIP64 let-chainで修正後に全stageをfresh実行する。レビューとGo full4packages/vet/ledgerが成功したbounded artwork scopeであり、残るrecord/user/bookmark/random/native/public SDKの完了へ外挿しない。

## record download の対象比較

codec168行、saved58行にEOF success/cancel追加1行、root59行をGo-firstで固定する。実Rust public command・保存済みSQLite/Execution・SDK metadata/resource save・native encoderからoutputs/requests/files/accountsへ接続する。安全なnative preflight14行、public startup composition39行、fatal presentation-only4行を分け、Go private observer・arbitrary reader/close schedulesは同一性を主張しない。shared JSON helperの変更がbookmark raw inputのNBSP suffix受理を広げないよう、whole-body対line trimの追加9行を先に固定した。unchanged fullscript終了0・305秒、255 targets/meaningful605/raw606passed・ignored11、optimized release34.13秒、Go全5packages/vetと独立reviewが成功した。全downloadのverifiedではない。

user/homeとpublic bookmark URLの全visual kind・全page展開、MCP random recommendationが次の接続scopeである。公開Record全体、OAuth streaming body/SDK Context・client ownership・idle close、HTTP2/Accept、全native IO/他platform、signal restorationとWindows repeated cleanup等の既存負債を保持する。

Linux CLI登録inventoryはrecord checkpointで実Go Cobraから追加採取し、元Windows snapshotとbyte一致した。snapshot不在の既存failureは証拠logへ保持するが、現在の不足とは扱わない。native全動作・全flag/help/TTY/OS hookと他platformの未検証は残る。

## user/bookmark download の対象比較

Go-first app52・保存済みCLI41・MCP21と実stdio取消・再利用2scheduleを全page/all-visual-kindの共有downloadへ接続する。userの取得済みprefixとkind/source別failure、bookmarkのqueued全media abort、positive global ID dedup/detail refetch、既存cursor guard、CLI account poolのwhole-attempt replay/skip markerを固定Goどおり保持する。MCP configured account42/default43は追加の公開Account compositionで比較し、旧saved APIとCLI user0/proxy動作を維持する。他MCP toolのaccount-aware embeddingは残る。

取消は実parent Contextと元SDK causeを区別する。追加actual SDK4行とactual Native localhost pending CONNECT4行を先に固定し、cancel/deadlineのtyped transport原因を比較する。外部upstream/TLS/body/mediaを使わず、全ready-result raceやSDK公開Context/lifecycleの成功へ外挿しない。CLIの成功した取消済みpage→next fetchはzero/expired SDK Client timer awaitの実redから修正する。HTTP側RequestPacingの非zero expired timer awaitは別の具体差分として保持する。

private Go manager discovery/factory・close countersとRust公開API/Dropは同一視しない。CLI単一media workerはtest-owned caller affinity、native encoder/SDK blocking IOは元のaffinityで比較する。MCPは厳密なOAuth/list prefixと並行media suffix multisetを区別する。全URL grammar/非UTF8/並行schedule/native IO/他platform、public Record・SDKHTTP2/Accept/ownership/idle-close・signal restoration・Windows repeated cleanupは未完了のままである。MCP random recommendationのfirst-page shuffle→truncate→downloadが次のworkflowとなる。download全体のverifiedや、新しいpersisted resume/job/progress契約とは扱わない。

このscopeの最終unchanged fullscriptは終了0・284秒、258 summaries/meaningful613/raw614passed・0failed・既存ignored11、release46.05秒で成功した。先行Clippy helper failureを保持し、equivalent let-chain修正後に全stageをfresh実行する。Go full関連4packages/vet＋既存SDK pacing3tests/vet＋migration validator/vetが成功した。独立reviewとfrozen manifestの対象はこの接続scopeに限定し、全migration完了へ読み替えない。

## MCP random recommendation download の対象比較

Go-first options77（別Go-private18）・genuine direct batch27・saved49とowned stdio取消/再利用を実MCP/CLIへ接続する。元saved48のSHA projectionを保持し、post-publication .jpg→.png stat scheduleを通常SaveClient Dropで再現する。抽選は全first-page DTO/cursor検証→full shuffle→truncate→Manager positive ID dedup/sortで、duplicate不足を埋めない。distinct subsetはcardinality/subset/sorted outputを比較し、random draw自体を固定しない。公開Account one-leaseを使いpool Execute/replayへ変えない。

Go physical peer/idle-close対Rust future/owner Drop、getrandom entropy failure対Go global math/rand、全encoding/SQLite/OS stat/ready-result/並行native schedulesは別の未完了境界である。SDK公開Context/HTTP2/Accept/ownership・HTTP expired-positive RequestPacing、signal restoration、Windows repeated cleanup、全native IO/他platformと他MCP account-aware embeddingを引き続き残す。最終fullgate/review証拠はcontractsへ追記し、download全体をverifiedに変更しない。

このrandom scopeの最終unchanged fullscriptは終了0・235秒、259 Running＋5 Doc-tests、265 summaries、meaningful631/raw632 passed・failed0・既存ignored11、release45.30秒で成功した。元hangは原因未確定として保持しtest-owned complete PID handshakeを追加し、stale catalog failure後に全stageをfresh実行した。次はstandalone `pixiv ugoira` の保存済みmetadata CLI、その後FANBOX・辞典・reverse search・updateと残る互換境界を接続する。

## standalone ugoira metadata CLI の対象比較

Go-first owner97・saved workflow74・root startup52を保存済みmetadata CLIへ接続する。Rustはowner90（options/source78、parser拒否10、help surface2、うち実SDK/output60）・saved70＋同じExecutionのbody取消後reuse8・startup45 exact＋help-state2を比較する。callback(false)のlease内出力、detail→kind→metadata、全attempt replay、retryable writer cause、original-first、全frame/signed timing、short nil write、JSON/proxy presenceとstartup順を保持する。元legacy env-token metadata-only入口は固定Goの保存済みcommandに置き換える。

Go-only owner port/model7・CloseClient failure4・debug/startup hook5、raw parser/help bytes、Usage wrapper typed source、全ready-result/HTTP/native/他platformは未完了である。body比較はofficial requestを記録したtest-owned loopback HTTP routeであり、外部HTTPS成功へ外挿しない。SDK公開Context/HTTP2/Accept/ownership/idle-close・HTTP expired-positive pacing・signal restoration・Windows repeated cleanup等の既存負債も残る。台帳のugoiraだけをpending→in_progressとし、verified_platformsを増やさない。次の独立sliceはanonymous辞典article/searchのCLI、その後FANBOX・reverse search・updateと残る互換境界である。

このscopeの最終unchanged fullscriptは終了0・253秒、262 Running＋5 Doc-tests、268 summaries、meaningful637/raw638 passed・failed0・既存ignored11、optimized release34.83秒で成功した。related Go owner全test/internal CLI ugoira比較/vet、3fixtureのcapture/replay/race/gofmtと独立reviewを対象として記録する。先行targeted Clippy helper failureは保持し、同等のlet-chainへ修正後に全stageを実行する。全Go production/module434・native/vendor/Cargo/script4834 pathsが元bytesのままであり、全migration完了とは扱わない。

## anonymous dictionary CLI の対象比較

固定Goのowner207・actual-root startup79・service/native181行を匿名article/searchへ接続する。Rustは実service・独立native HTTP・CLI presenterとowned processを使い、config順序、環境proxy、counter best-effort、JSON/NDJSON・limit・short nil writes・error sourceを保持する。辞典は指定した1pageの取得で、認証済みSDK Client/App API・MCP/account/poolを使わない。Go-only Reader/hooks/TLS/update/private RoundTripperとlifecycle、直接empty-Accept wire差・truncated-body immediate source差、全parser grammar/native IO/他OSは未完了である。台帳のdic3件だけをin_progressにし、全platform verifiedへ変更しない。

次はFANBOX session/read/resource/CLI/MCP、reverse image search、signed updateと残る互換境界へ続ける。既存SDK HTTP2/Accept/public Context/client ownership/idle-close・HTTP expired-positive pacing・signal restoration・Windows repeated cleanupを隠さない。FANBOX nativeのChrome fingerprint transportを通常reqwestで同一と主張しない。


この辞典scopeの最終unchanged fullscriptは終了0・255秒、265 Running＋5 Doc-tests、271 summaries、meaningful654/raw655 passed・failed0・既存ignored11、optimized release35.92秒で成功した。related Go3packages/vetとmigration validator/vet、4fixtureのGo-first capture/replay/race/gofmt、独立source/provenance reviewを限定して記録する。先行body2assertion/Clippy failuresを保持し、元expectationを変更せず修正した。native empty-Accept/source/lifecycle等の具体差分は残り、辞典を全OS verifiedや全migration完了へ変更しない。

2026-10-10 cloud recovery starts from published `33c6866ae7b87c195c20263f1a7074c091b08227`. Lost FANBOX prototypes are not current verification. Fresh contracts-only identity/options/protocol and malformed-redirect evidence is tracked in [fanbox-recovery.md](fanbox-recovery.md); the unchanged restored Rust baseline full gate passed independently. FANBOX Rust identity, saved accounts/auth, solver, exact native TLS/HTTP2/media/idle lifecycle, CLI/MCP/process wiring, content/resource/download and browser session extraction remain unfinished. Contracts-only `in_progress` ledger entries do not establish implementation or platform parity. Repeated body Close wire-CANCEL, concurrent Read/Close, HTTP1/resumption/HRR and non-Linux native checks remain explicit; the stopped supplemental multiplex/upload probe is not retried or counted as evidence.

## FANBOX core/native contracts and shared SDK context foundation

Go solver124/context41/native6 are freshly frozen and independently reviewed. The repaired native six-scenario suite uses owned child trust and synthetic peers only; exact Chrome146 TLS/H2, header-stall, certificate and idle/active cancellation observations are actual Go evidence. Preliminary weaker proof is superseded, not silently reused. Supplemental multiplex/HEAD/upload denial remains respected.

Only shared SDK context/diagnostics is implemented so far: eight fixture tests map 20 frozen rows, with four additional paused-clock regressions; app public paths reexport the same production types. Pixiv Facade remains caller-context forwarding. FANBOX identity/options/raw body/solver/native client, its nine SDK-forwarding and six lifecycle contract rows, saved-account/CLI/MCP/content/resource/download workflows remain unimplemented. Runtime-free constructor/monotonic lazy-or-awaited deadlines differ from independent Go Done timers; arbitrary Go keys/interface/nil panic/timestamp cases remain named. No FANBOX feature or platform is marked verified. Continue genuine public SDK identity, solver and native transport integration against the unchanged captures, then saved CLI/MCP/content/download and remaining migration debts.

Final unchanged full Rust script passes all stages: `/tmp/pixiv-context-bridge-final-full-gates.log`, exit0, 311 seconds, optimized release47.56s. Actual272 summaries comprise266 Running, five doc suites and one unchanged child summary; raw667/meaningful666 pass, zero fail and existing11 ignored. No assertion/ignored or Go production/module/native/vendor/Cargo/full-script changes were used. Final migration validator/vet pass; independent Go-first and Rust foundation reviews remain scoped to the recorded evidence, not full FANBOX/migration completion. [Gate provenance](provenance/fanbox-context-foundation-gates.json) preserves genuine compile/runtime RED, format failure, owned interrupted gate and final rerun separately.

## FANBOX public SDK identity, solver and native integration

This bounded implementation adds the real public FANBOX Client, normalized Options/FlareSolverrOptions, identity DTO and CurrentUser/ValidateSession, redacted credentials/debug output, shared RequestContext forwarding, private Session and private solver. Rust's RawTransport/RawBody are genuine fallible dependency boundaries. There is no exported private Session, fake HttpClient wrapper solely for Go nil-Transport cases, or public solver-control test seam. The ordinary direct solver HTTP control client is deliberately distinct from the real Chrome-fingerprint native transport.

The comparison reuses 156 representable public rows from the original309 identity capture, plus actual public CurrentUser67 HTML observations and eight representable SDK context rows. Two Go constructor typed-nil Transport cases and the nil-context case remain Go-only. Private parser errors are not guessed into public SDK error strings. Actual malformed redirect Location is validated before the no-follow boundary; malformed Location closes once, ignores that close error and yields the Go request failure, while valid/absent Location retains response-close precedence. Three additional truncated-markup ownership regressions guard against a Rust panic without altering the frozen fixtures.

The actual public solver93 capture precedes Rust execution. It records challenge/replay/control payloads, first-complete streamed JSON, duplicate object fields, quoted and extreme expiry, clearance/native-header bytes, cache invalidation, detached owner scope and independent waiter cancellation. The original private solver124 and redirect3 remain intact and are evidence of explicitly separate internal boundaries. One public native_future row was added to the preliminary92 before Rust comparison, followed by complete recapture/replay. No private expected SDK prefixes or test-only public client option are substituted for real public behavior.

The native factory uses pinned official wreq/btls/btls-sys/wreq-proto/http2 source. Narrow reviewed patches retain real BoringSSL verification and genuine certificate causes, Chrome146 TLS/H2 profile, ordered request headers, physical cancellation, synchronous idle-retirement ownership and graceful frame drainage. Imported BoringSSL source blobs remain byte-exact; the build applies the recorded patch series rather than disabling patches. Native runtime is limited to the same six approved synthetic scenarios with child-only trust. Raw request inputs are captured from actual Go Session, then fed to public Rust NativeTransport; this is a raw-boundary comparison, not complete Session or public native API equivalence.

Initial identity/solver and native missing-API/module compilation failures establish RED before their corresponding implementation. First native runtime failed because an owned JSON projection inserted absent frame fields as null; the correction uses get_mut without changing expected values. The second run passed sequential reuse/idle, the31.2-second active response-header stall and 300ms deadline, then stopped on [] versus Go's comparable nil frames. The raw peer schema already matched Go's empty slice; the projection now allocates only after a retained non-ACK frame, exactly matching the original Go comparison. Both failed logs and raw witnesses remain separate evidence. No supplemental multiplex/unfinished HEAD/upload probe was reconstructed or retried.

Media/body80 is fresh Go-only evidence from Session/OpenMedia and actual fhttp decoders with synthetic in-memory bodies. Rust media/decompression integration is still pending: non-zlib deflate consumes its two sniff bytes, eager recognized deflate and repeated Close behaviors are not silently normalized. Native compressed response decoding, resource/content/download, saved-account app facade and CLI/MCP wiring, browser extraction, TLS1.2/resumption/HRR/HTTP1/proxy, concurrent read/close and non-Linux native execution remain unfinished. Existing SDK HTTP2/Accept/ownership, pacing, signal restoration and destructive Windows repeated-cleanup gaps remain visible. This checkpoint does not mark FANBOX or any platform verified.

Independent source review additionally identifies remaining compatibility boundaries outside the captured SDK rows: Go JSON Unicode folding of non-ASCII aliases, replacement of lone UTF-16 surrogate escapes with U+FFFD, the Go scanner's 10,000 nesting limit, Go numeric optional-port validation versus WHATWG URL limits, RFC3339 leap-second rejection and HTTP-date weekday handling. Bounded tokenizer/Unicode identity/decimal-port regressions and solver Unicode aliases are being captured separately and repaired tests-first in this checkpoint. Surrogate/depth/date parsing requires the immediately continued broader parser slice; no arbitrary JSON, URL or date grammar equivalence is claimed here.

Git checkout attributes preserve all imported FANBOX source and mixed-line-ending reconstruction patch bytes. A dedicated Rust regression first fails on unspecified text conversion, then checks the actual repository Git attributes after the -text rules are added; it does not execute a Windows checkout or establish native Windows support.

The unchanged default `go run ./scripts/cmd/licensebundle -check` passes for its actual manifest, `internal/media/ugoira/rust/Cargo.toml`. It does not cover the root Rust CLI/native dependency graph. The imported family licenses and native notices are preserved and independently byte-verified; complete future Rust CLI release license/package integration remains an explicit distribution gap. Cargo.lock retains all prior package/version/source selections and checksums, adding 37 packages for the native family and its dependencies.

Independent source review's bounded regressions are now mapped separately: 14 actual public Go rows cover abrupt comments/raw-text duplicate attributes, identity Unicode aliases and decimal optional ports; seven actual public solver rows cover its Unicode aliases. Rust records three failing identity tests and one failing solver test before the production fixes. Final focused formatter, strict workspace/all-target Clippy, nine identity tests, four solver tests and one actual Git attribute test pass. Duplicate attributes now derive spans from real html5ever structural-token callbacks rather than a document-wide tag guess. The private Go ASCII-schema fold helper handles long-s and Kelvin classes consistently. Port validation preserves original decimal authority even when WHATWG's bounded port parser cannot represent it. No prior fixture expectations were edited. The alias fixture's later prose clarification does not change any of its seven input/observation objects.

Final unchanged scripts/check-rust.ps1 finishes with exit 0 in 419 seconds. Formatter, strict workspace/all-target Clippy, all workspace tests and optimized release (1m 48s) succeed. Actual 282 summaries comprise 270 Running suites, five doc suites and seven child summaries. Raw 689 / meaningful 687 pass, zero fail and the existing 11 ignored tests remain. Meaningful counts exclude only the existing terminal_prompt_child no-mode scaffold and new fanbox_native_child no-mode scaffold, one each; all six owned native children actually execute and pass. No assertion or ignored expectation is removed. The pre-run snapshot confirms every production source, test, fixture, dependency and script byte remains unchanged through the gate; only result documents change.

Final related Go tests, vet, empty gofmt and migration validator pass: 32 top-level and 589 nested named pass events, zero failure or skip, scoped to all SDK FANBOX tests and media-body protocol comparison. Separate manifests preserve all Go-first/race evidence and the earlier broad native pass/interrupted broad race accurately. All 434 frozen Go production/module files remain exact. Independent SDK, native idle and current certificate-policy source approvals are persisted beside the final gate record. Git byte-style diagnostics for untouched upstream sources and valid reconstruction patches are publication hygiene evidence, not compiler/runtime failures or a reason to rewrite official blobs. FANBOX and all platform entries remain in_progress with no new verified platform.


## FANBOX bounded JSON/date continuation

Fresh identity 47 and public solver 79 Go rows are mapped to the real SDK, with genuine semantic Rust RED before repair. Exact-byte Unicode/surrogate handling, nesting 10000, selected literal-NUL metadata and the two actual solver expiry layouts are now covered by focused tests; final aggregate evidence follows after completion. First-root syntax/depth normalization excludes ignored later bytes. Scalar-root streamed completion timing, unrestricted JSON/HTML/URL/date grammar, native media decoding and resource/content/download, saved app/CLI/MCP, browser extraction and non-Linux execution remain open. Earlier SDK/native summaries are historical bounded checkpoints, not full parity. Existing HTTP2/Accept/SDK ownership/pacing, signal restoration, repeated Windows cleanup and release license-bundle debts remain explicit.


The final parser/date full script passes all stages in 294 seconds, raw 707/meaningful 705 tests, failed 0, existing ignored 11 and optimized release 48.78s. The initial concurrent test-cleanup run remains interrupted/inconclusive; all stages rerun on final bytes. Fresh related Go/vet/gofmt/validator and independent source review pass. The next complete functional scope is saved-session content/resource reads: all eight content SDK endpoints, ResolveURL/OpenResource, six CLI content leaves and eleven MCP tools, including required native decoded-body integration using unchanged media 80. No pending auth/download/browser or native/platform boundary is silently removed.


## FANBOX connected read contracts, implementation still pending

Fresh Go-first SDK content416/resource243/CLI172/MCP126 captures preserve actual saved selection, endpoints, opaque reopening, distinct projections and real stdio cancellation/reuse/EOF. This durable checkpoint records contracts and provenance only; the connected Rust content/resource/facade/CLI/MCP implementation remains pending. See [read contract boundaries](fanbox-read-contracts.md). Fixture/schema counts do not increase verified platforms or establish feature completion. Existing media80/saved321/help22/parser/native rows remain unchanged.

## FANBOX connected read continuation (2026-10-10)

Saved-session SDK content/reference/resource, actual saved-account app composition, six CLI read leaves and eleven MCP tools are now connected in the candidate described by [fanbox-connected-reads.md](fanbox-connected-reads.md). Actual Linux binary tests cover help, argument/config/store stops, missing-account paths and MCP stdio/schema/errors; successful authenticated native reads remain unverified. The candidate's bounded scope does not complete FANBOX auth/browser session extraction, download/save/replay, all public lifecycle behavior, native compressed-wire/HEAD/Range/concurrent decoding, arbitrary abort-body cleanup or non-Linux runtime. These operations remain required for the whole migration. No pending feature is replaced with empty success.


## FANBOX bounded auth continuation (2026-10-10)

The current candidate connects all five FANBOX auth leaves through real service/SDK/SQLite, sparse config file ports and normal root/main ownership; [fanbox-auth.md](fanbox-auth.md) records the exact mappings and sealed Go 233-scenario/255-execution scope. Final corrected focused verification passes 61 top-level tests plus one genuine CLI child, including the actual byte-input SDK 40, service trim 24/saved SQLite 24 and 30 real Linux binary help/parser/offline-saved observations. The initial 37-case SDK API RED, three supplementary first-GREEN rows and actual service one-failure RED remain separately recorded; native import is excluded. Fresh related Go/vet/empty gofmt, supplementary SDK Go and 434-production/93-fixture source guards pass. The unchanged full gate, final validator and 8,486 executable input hashes pass; independent final source/evidence review is approved with documentation reconciliation. Visible raw passes are 787, failures zero, existing ignored cases 11, and optimized release completes in 53.33 seconds. Native browser extraction, the global automatic update checker, arbitrary custom String factory correspondence, malformed parser/SQLite Scan/nil-slice/other invalid-UTF-8 String fields, download/save/replay and native/platform/public SDK lifecycle/Accept/HTTP/2 relay/pacing/signal/Windows repeated-cleanup/distribution debts remain required. No feature is removed or replaced by Go fallback, no denied supplemental multiplex/unfinished-HEAD/upload probe is retried and no platform is promoted to verified. Earlier summaries are historical bounded evidence, not whole-FANBOX completion.

FANBOX auth five-leaf bounded final checkpoint: unchanged full-script exit 0, raw 787 pass/0 fail/11 existing ignored, release 53.33 seconds; actual SDK raw 40、service invalid-byte trim 24/real SQLite 24、CLI 233 scenario/255 execution と実 Linux binary 30を比較する。native browser extraction/global automatic checker、FANBOX download、全platform/protocol/lifecycle debt は未完のまま。[auth scope](fanbox-auth.md)を参照し、このgateを全移植完了としない。

## FANBOX connected download continuation (2026-10-10)

The candidate connects `fanbox download SOURCE...` to saved runtime/account selection, all-source/all-page SDK reads and real resource atomic saves. [Download scope](fanbox-download.md) records the Go183/184 CLI and277/285 SDK oracles, all30 comparison fields,17 real offline binary observations and genuine API/runtime failure corrections. Focused comparisons and unchanged full script pass (407 seconds, raw 795 passed/0 failed/11 existing ignored, release 58.81 seconds); all 8,807 protected inputs remain unchanged. Independent approval is recorded separately and required before publication. This is not whole migration completion. Native browser extraction/global automatic updates and all explicitly recorded native/protocol/UTF8/filesystem/lifecycle/platform/distribution debts remain. Frozen Go does not expose a FANBOX MCP download or CLI record/template/pool/replay interface, so their absence is not hidden feature removal.

## Browser cookie contracts and byte foundation (2026-10-10)

[Browser contracts](browser-cookie-contracts.md) preserve six genuine Go fixtures: crypto159, Firefox60, Safari57, SQLite42, native-source54, connected import82cases/84executions/84commands. Native-source tests execute SHA-anchored source with explicit Linux dependency substitutions; they do not verify Darwin/Windows native execution. Actual official SQLite3.54.0 owned-database behavior, Safari malformed-input panic/cancellation, raw Chromium bytes and DPAPI allocation edges remain explicit. Rust adds backward-compatible byte provider methods and auth forwarding, with genuine missing-API RED then three regression tests GREEN; system extraction is still unimplemented and these six fixtures have not been compared to a Rust backend. The unchanged full script passes in338s: raw798/0failed/11existing ignored, release46.12s; all8,831 protected inputs match. Related Go295named passes/0fail/skip and vet/gofmt pass. Independent approval is recorded separately. All673 ledger identities/statuses/platforms and434 Go production/module paths plus97 existing fixtures are preserved. Continue connected backend implementation immediately; no whole migration/native-platform completion claim.

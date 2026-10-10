# 保存済みブラウザー cookie の契約と byte 基盤

参照実装は Go `4b4426487ef18bed276706daec385e0d0a6979f9`、今回の公開済み Rust 基準は `0da302cdebcbcbbeeee558d9b93b9533e4d69662` に固定する。[移植方針](strategy.md) に従い、Go の実処理で期待値を固定してから Rust の入口を変更する。

今回の区切りは Go 契約 fixture の保存と、Rust の byte 型の受け渡しを auth import へ接続するところまでである。Rust の通常 factory は Chrome、Edge、Firefox、Safari のいずれにも `browsercookies: native browser cookie extraction is not implemented` を返す。保存済みブラウザーから取得して import する Rust の機能は、まだ利用できない。

## 証拠の区別と保存

6 個の新しい JSON fixture は、次の Go 実行を固定する。ケース数、root 実行数、記録したコマンド数は別々に数える。これらの数から Rust の抽出機能の進捗率や native OS の検証完了を算出しない。

| fixture（`crates/pixiv-cli/tests/fixtures/`） | Go の観測数 | 実行した境界 | provenance |
| --- | ---: | --- | --- |
| `browser-chromium-crypto.json` | 159 ケース | 元の Go の private crypto、Local State、Linux secret 処理。標準ライブラリー暗号と合成鍵、所有する secret-tool | [crypto](provenance/browser-cookie-crypto.json) |
| `browser-firefox.json` | 60 ケース | 元の Go の discovery、INI、Read。process helper と公式 SQLite shell を区別 | [profiles](provenance/browser-cookie-profiles.json) |
| `browser-safari.json` | 57 ケース | 元の Go の parser、discovery、Read。Linux 上の所有する合成ファイル | [profiles](provenance/browser-cookie-profiles.json) |
| `browser-sqliteio.json` | 42 ケース | 元の Go の Query。33 process-helper ケースと 9 公式 shell ケース | [profiles](provenance/browser-cookie-profiles.json) |
| `browser-native-secret.json` | 54 ケース | Darwin secret 14、Darwin mapping 8、Windows DPAPI 11、Windows routing 21。SHA 確認済み Go を Linux 上の一時 package で実行 | [native](provenance/browser-cookie-native.json) |
| `fanbox-browser-import.json` | 82 ケース、84 root/RunContext 観測、84 コマンド | 元の Go の通常 provider、SQLite shell、secret コマンド、暗号、auth、SDK identity、保存 DB/config/output を接続 | [import](provenance/browser-cookie-import.json) |

native 54 ケースの一時 package は、元ファイルの SHA、変換後の SHA、置換内容を記録する。置換は明示 build tag 9、ファイル名による build 制約 3、Windows native import 1 の計 13 箇所である。Darwin は所有する `security` helper、Windows は所有する native call/allocation mock を使用する。これは元の Go の分岐・引数・copy/free 処理の実行証拠であり、Darwin/Windows 上の native compile、ABI、Keychain、DPAPI の実行証拠にはしない。

保存した provenance は、元 manifest のすべての値を維持し、`_documentation_checkpoint` に今回の状態と追加の証拠を記録する。434 個の基準 Go production/module ファイルと、公開済み fixture 97 ファイル（JSON 95、PEM 2）は変更されていない。新しい producer と fixture は追加ファイルであり、既存期待値の差し替えには使用していない。個別 source、producer、fixture、support source、依存、ログの SHA は各 provenance に残す。

### 公式 SQLite shell と過去の skip

公式 SQLite 3.54.0 の amalgamation を所有する workspace でコンパイルした。archive、4 個の source、compiler、build command、binary、version の根拠は [SQLite source provenance](provenance/browser-cookie-sqlite-source.json) に保存する。binary SHA256 は `a948747fbdbbf3827101254067b7ea480bad8b13732decbf750da75026c3e38d` であり、system install や実データへのアクセスは行っていない。

crypto の初回 related log では sqlite3 がなく、既存 SQL テスト 5 件が skip した。この履歴は元の crypto manifest とログにそのまま残す。その後の別の公式 shell 実行 `/tmp/pixiv-browser-chromium-official-sqlite-related.log` は exit 0、名前付きテスト 19 件 pass、skip 0 であり、5 件の SQL テストも pass した。追加実行の SHA と結果を crypto provenance に記録し、古い skip を成功へ書き換えない。

Firefox の 7 ケース、sqliteio の 9 ケースと接続 import は、公式 shell と所有する実 SQLite DB を使用する。process-helper の CSV/error/argv ケースは SQL 自体を実行しないため、公式 shell の比較と別に記録する。import の sqlite3 wrapper は固定 argv と parameter-map を観測してから、所有する公式 binary を exec する。

## 固定 Go の discovery と profile 選択

browser 名は Go の TrimSpace と ToLower を経て registry から選ぶ。adapter は `.fanbox.cc` と `FANBOXSESSID` だけを問い合わせる。profile 未指定では 0 件が profile-not-found、1 件が自動選択、複数件が ID の一覧付きエラーとなる。指定 ID は完全一致の先頭を選ぶ。エラーに呼び出し側の任意入力や絶対パスを含めない。

| provider | Linux | Darwin | Windows |
| --- | --- | --- | --- |
| Chrome | `$XDG_CONFIG_HOME/google-chrome`、空なら `$HOME/.config/google-chrome` | `$HOME/Library/Application Support/Google/Chrome` | home 由来の `AppData/Local/Google/Chrome/User Data` |
| Edge | `$XDG_CONFIG_HOME/microsoft-edge`、空なら `$HOME/.config/microsoft-edge` | `$HOME/Library/Application Support/Microsoft Edge` | home 由来の `AppData/Local/Microsoft/Edge/User Data` |
| Firefox | XDG または `$HOME/.config` の `mozilla/firefox` | `$HOME/Library/Application Support/Firefox` | home 由来の `AppData/Roaming/Mozilla/Firefox` |

Chromium は root 直下の directory を名前順に列挙し、その直下の `Cookies` が存在して directory でない profile だけを返す。directory entry の symlink は directory とみなさず、cookie ファイルの stat は symlink をたどる。ID は空、`.`、`..`、先頭 dot、対象 OS の basename と不一致なら拒否し、空白や受け入れ可能な非 ASCII は残す。root の permission error と未インストールを区別し、候補 0 件も未インストールに分類する。`Network/Cookies`、Local State の profile-info、ブラウザー起動による探索はしない。

Firefox の `profiles.ini` は TrimSpace 後に、角括弧 section を名前にかかわらず受け入れる。`IsRelative` は省略時 true、指定時は文字列 `1` だけ true となる。宣言された相対・絶対 path を使用し、ID は basename、Name の既定値は ID となる。宣言順と重複を維持し、Read は再探索後の最初の一致 ID を選ぶ。`cookies.sqlite` は存在だけを確認し、directory でも discovery の候補になる。非 permission の stat エラーは候補を飛ばす。Linux root を `~/.mozilla/firefox` へ変更しない。

その他 OS の Chromium/Firefox root は空である。Firefox では空 root から作業 directory の `profiles.ini` を参照する可能性があり、今回 native unsupported guard があると仮定しない。Linux 上で build 制約を除いた Windows source も、path 演算は実行環境の Go filepath を使用する。Windows の drive、UNC、basename、raw-name の実行証拠とは区別する。

Safari は OS guard を持たず、home の `Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies`、次に `Library/Cookies/Cookies.binarycookies` を確認する。最初の既存 non-directory を採用し、permission error では fallback を続けない。唯一の ID は `Default` である。Read はそれ以外の ID を拒否し、再探索後、見つからなければ最初の候補を読む。現代 Safari の named profile や Keychain の探索は含まれない。

いずれも expiry、cookie path、Secure、HttpOnly、新しさ、重複を理由に cookie を選別しない。adapter の一致 cookie 0 件と複数件は、それぞれ `browser profile does not contain a FANBOXSESSID cookie` と `browser profile contains multiple FANBOXSESSID cookies` を返す。

## SQLite コマンド、CSV、エラー順序

Go production の browser 抽出は linked SQLite を使用せず、固定 `sqlite3 -readonly -noheader -csv -newline "\n"` を実行する。許可された parameter ごとに `-cmd ".parameter set NAME VALUE"` を追加し、DB path と provider 所有の固定 SQL を渡す。Go map の列挙順は非決定的なので、fixture は parameter-map の意味と残りの argv を分けて比較する。

CookieQuery の host/name は 1〜256 bytes の ASCII 英数字、dot、hyphen、underscore に制限する。SQLite parameter は同じ制約に `@`、`:`、`$` を加える。空白だけの path/SQL や invalid parameter は query-failed となる。CLI に任意 SQL、DB path、executable、鍵の指定を追加しない。

Chrome/Edge は `cookies` の `host_key,value,hex(encrypted_value)`、Firefox は `moz_cookies` の `value` を読む。host は指定値または先頭 dot を一つ除いた値、name は完全一致である。ORDER BY はない。通常処理は元の DB path を readonly query し、snapshot や WAL copy を作る処理へ変更しない。

process 失敗時は、まず context error、次に exec.ErrNotFound、stderr の case-insensitive `permission denied`/`not authorized`、ExitError の code 5 または `locked`、その他 query-failed の順に分類する。stdout CSV が malformed の場合も query-failed となる。path、SQL、parameter、stderr、鍵、cookie を診断へ含めない。

Go encoding/csv は variable row width、引用符、field 内改行、CRLF、末尾空 field、任意 bytes を扱う。明示 `-newline "\n"` は Windows の CRCRLF を避けるための固定 argv である。helper が返した NUL/invalid UTF-8 の CSV と、公式 shell が実 DB から出力した CSV は別の観測である。公式 SQLite の TEXT `x\x00y` は CSV 出力時に `x` へ切り詰められ、Go もその値を読む。暗号復号後の raw NUL/invalid UTF-8 を同じ理由で切り詰めない。

Go の fixture hook を使った snapshot ケースは、新規 private directory 0700、file 0600 を作り、同じ Query に渡して成功・失敗時に削除する。この hook は test の境界であり、通常抽出の実装とは区別する。Rust production にこの test 専用 hook を移植する必要はない。

## Chromium の byte、暗号、鍵取得

非空の encrypted 列は plaintext より優先する。invalid hex は format-unknown となり、plaintext は鍵を取得しない。`decryptEncrypted` は format/鍵処理の前に context を確認する。encrypted row ごとに鍵を取得し、profile 全体の鍵 cache は持たない。

v10/v11 は candidate ごとに AES-GCM を先に試し、失敗後に CBC を試す。GCM は nonce 12 bytes、tag 16 bytes、AAD なし、鍵長 16/24/32 bytes である。CBC は space IV と厳密な PKCS#7 を使用する。元の CBC は `key[:min(len(key),32)]` を使うため、32 bytes を超える raw key は先頭 32 bytes に切り詰める。GCM と CBC の鍵長処理を同一と仮定しない。candidate を使い切った場合は malformed となる。

Unix legacy blob は 32 bytes 以上かつ block-aligned を要求し、先頭 16 bytes の鍵で zero-IV AES-128 CBC を実行する。unpadding はせず、平文の v10/v11 prefix を確認して先頭 32 bytes を除く。legacy candidate は SHA256(key || `peanuts`) の 1000 回反復、PBKDF2-HMAC-SHA1(`saltysalt`,1003,16)、PBKDF2-HMAC-SHA1(`peanuts`,1,16) の順で、重複を除く。この固定 source の値を一般的な Chromium recipe で置き換えない。

復号後の先頭 32 bytes は、実際の row の host_key の SHA256 と一致する場合だけ除く。不一致なら opaque 値の一部として残し、新しいエラーを加えない。metadata DB version は参照しない。未知 version の非 Windows 処理、新しい app-bound format などを v10/v11 として受け入れない。

Local State は missing なら鍵なし、permission なら permission-denied、その他 IO、不正 JSON/base64 なら format-unknown となる。encoded key は trim し、standard の padded/raw-unpadded base64 を受け入れる。absent/empty encrypted_key は鍵なしである。Unix は password と legacy candidate の取得後に State を読み、v10/v11 State key は GCM だけで unwrap する。DPAPI prefix は未知形式、その他の非空 key bytes はそのまま候補にする。Windows は State を先に読み、missing なら encrypted-value-unsupported、必要なら文字列 `DPAPI` prefix を除いて unprotect する。

### Secret Service と Keychain

Linux は LookPath 後に固定 `secret-tool lookup xdg:schema chrome_libsecret application {chrome,microsoft-edge}` を使用する。tool missing は SecretService-unavailable、lookup failure、空 password、invalid item は access error に分類する。pre-cancel/command-cancel は context を維持する。password の末尾 CR/LF だけを除く。keyring の変更やブラウザーへの fallback はしない。

Darwin は固定 `security find-generic-password -w -s {Chrome Safe Storage,Microsoft Edge Safe Storage} -a {Chrome,Microsoft Edge}` を使用する。低位 Keychain wrapper は command cancellation を context error に、ExitError stderr の case-sensitive `could not be found` を item-not-found に、それ以外を command failure に分類する。provider 層では item-not-found を維持し、それ以外の password error は context error も含めて keychain-access に写す。低位 wrapper と provider の分類を混同しない。末尾 CR/LF を除いた空 password は拒否する。

### Windows DPAPI と output 所有権

Windows の legacy encrypted_value は、非空かつ v10/v11 でなければ DPAPI に渡す。version に似た新しい bytes も DPAPI error となることがある。input は 1〜uint32::MAX bytes、input/output DATA_BLOB、optional pointer は null、flags は 0 である。borrowed input を native call 中も保持する。

元の Go は pre-context、input 判定、native call、output 判定、copy、post-copy context の順に処理する。正常な非空 output は copy 後に LocalFree を一度実行し、copy 後の cancellation でも deferred LocalFree を実行する。native call は非割込みであり、mock 成功から native interruption や timeout の保証を導かない。

native failure と nonnull zero-size output は、free defer を設定する前に DPAPI error を返す。したがって元の Go の LocalFree 観測数は 0 である。mock の残余 allocation の test teardown を、本番 LocalFree の実行として数えない。uint32::MAX を超える input は source の条件を確認しただけであり、巨大 allocation や偽 slice による実行はしていない。

## Safari/Firefox の UTF-8 と cancellation

Safari fixture は固定 source の `cook`、BE page count、交互の BE page-size と page、BE page header/count/start/offset、u16-BE record-size、version、4 個の u8 length-prefixed field、timestamp 12 bytes、flags という grammar を使用する。page count 0 は空成功、末尾 bytes、version、timestamp は無視する。host 比較は両側の leading dot を一つ除き、name は完全一致となる。

Safari の宣言 page count が 2、最初の page が完全で次の size が欠ける入力は、元の Go の unchecked slice で panic する。fixture は実際の panic を捕捉した。invalid-format に期待値を差し替えない。Rust の parser と blocking adapter での同じ panic の扱いは、まだ実装・比較されていない。この grammar の成功は、現代 Safari binarycookies の全形式対応を意味しない。

Firefox と Safari は一致 cookie の invalid UTF-8 を encrypted-cookie-unsupported に分類し、NSS/Keychain の復号は行わない。Safari の不一致 cookie は invalid UTF-8 でも parse できる。Chromium は復号/CSV bytes を opaque のまま返すため、provider 一般で UTF-8 化や lossy conversion を行うと契約が変わる。

Discover は IO 前に context を一度確認する。Safari Read 自体には cancellation check がなく、pre-canceled context でも所有するファイルの正常値を返した。parser/crypto loop に polling があると仮定しない。各 provider の通常 Close は nil だが、上位 adapter は結果にかかわらず close し、read/close 両方のエラーを join する。

## Go の接続 import で確認した順序

Go fixture は登録済み通常 SystemBrowserProvider から AccountService、SDK CurrentUser、保存 SQLite/config/output まで実行する。browser error は service factory/proxy より先に返す。取得した raw bytes は factory、proxy/options を通り、SDK の実 cookie validation で拒否される。たとえば raw `78ff0079` は browser 層で書き換えず、SDK の malformed-cookie に到達する。公式 shell 由来の `78` は正常 session として保存する。

reimport は明示 default を `--default` 指定まで維持し、既存 config comment、creation time、sort order を残し、credential revision を増やす。SDK HTTP dependency には合成 response だけを与え、実ネットワークは拒否する。各 root child は HOME、USERPROFILE、XDG、APPDATA、LOCALAPPDATA、TMPDIR、PATH を所有し、process-wide seccomp TSYNC で socket/socketpair/connect と x32 syscall を拒否する。exec 自体は許可しているため、executable allowlist sandbox として説明しない。

この接続 fixture は registry override、DB read hook、encryption-key override、password-provider override を使用しない。secret-tool は合成 password を返す所有する executable である。Linux Secret Service の DBus/libsecret、Darwin Keychain、Windows DPAPI、実 browser/account/credential の確認は含まれない。

## 今回の Rust byte 基盤

Rust の追加 API は `BrowserBytesFuture`/`BrowserProvider::read_session_bytes` と `BrowserCookieBytesFuture`/`CookieProvider::read_bytes` である。既存 String 実装には `String::into_bytes` の default を追加し、既存呼び出し側との互換を保つ。SystemBrowserProvider の byte override は discovery、既存 profile selection、read_bytes、exactly-one、close/join を通る。`ProviderClose::finish<T>` は String/bytes の両方を扱い、Drop の close 所有権も維持する。

AuthCommand の browser import は byte method を使い、既存 `import_session_bytes_with_proxy` へ直接渡す。SDK の cookie validation、identity validation、proxy/options、保存規則を変更しない。byte を保持する対象は SDK が拒否するまでの入力であり、invalid session を受け入れたり保存したりする変更ではない。

旧 `read_session`/`read` の String adapter は従来どおり別の入口として残っている。この区切りで strict UTF-8 native adapter が追加された、または旧 String method が byte method へ委譲するようになったとは記録しない。native backend の byte read と String compatibility method の接続は後続実装に残る。

新しい `fanbox_browser_bytes.rs` の API compile RED は exit 101 で、欠落していた byte 型/trait method が原因である。その後の GREEN は exit 0 となり、新規 3 テスト（String 実装の default、opaque bytes の exact-one adapter、cardinality/once-close）と既存 auth 5 テストが pass した。ログの SHA と現在の source/test SHA は import provenance に記録する。

この GREEN は、6 個の新 fixture 全体の Rust 比較、通常 native factory の接続、実 crypto、SQLite/process、OS secret の実装成功を示すものではない。変更していない `scripts/check-rust.ps1` は Formatter、strict Clippy、workspace test、release build の全段階で成功した。338 秒、raw 798 passed / 0 failed / 既存 11 ignored、release 46.12 秒である。[最終 gate](provenance/browser-cookie-foundation-gates.json) と独立 approval artifact を別に記録し、fixture 数や個別 GREEN を機能完成の根拠にしない。

関連 Go の最初の grouped 実行は、import producer に必要な `MIGRATION_BROWSER_SQLITE_SOURCE` が未設定のため exit 1 となった。ログ `/tmp/pixiv-browser-foundation-related-go-attempt1-env.log` と exit file を保存し、確認済み公式 binary の絶対 path を設定した同じコマンドの再実行は成功した。名前付き Go テスト 295 件（top-level 63、nested 232）、fail/skip 0、vet と gofmt は成功である。この失敗を契約差分や semantic RED として数えない。全チェックの前後で 8,831 入力の SHA が一致し、既存期待値を除去していない。

## 残る実装・検証

次の実装の区切りは通常 root/main から 4 provider、固定 query/parser、実 crypto/OS 依存、exactly-one/close、byte import、SDK identity、保存/output までを接続する。その後に今回の Go fixture を本番依存境界で比較し、aggregate gate を実行する。モジュール数や fixture 行数のために helper を公開 API へ追加しない。

- Rust native factory、profile/file/env、SQLite command と private byte CSV parser、Chromium decoder/key store、固定 secret command、DPAPI copy/free、Safari parser、blocking ownership は未実装
- direct PBKDF2 の任意/非正 iteration、汎用 length/multiblock、unpad、prefix、digest、legacy-supported、private error-map 等の helper-only 契約は Go の期待値として残す。到達可能な production interface の観測と別にし、到達不能な組合せや command により除かれる password の末尾 CR/LF を Rust 比較済みとして数えない
- Linux 公式 SQLite shell の今回の所有 DB 比較は完了したが、Rust の shell 実行、Windows の shell/CSV/CRLF、配布先の tool availability/version、全 SQLite schema/type/row-width/locking/permission/cancellation は未検証
- native Darwin/Windows compile/link/run、Windows drive/UNC/raw path、OS の raw-name/非 UTF-8 profile ID、native discovery、ACL/permission、Secret Service/Keychain/DPAPI の許可済み実 integration は未検証
- Safari panic/pre-canceled Read、UTF-8 rejection の位置、Chromium raw bytes、CBC >32-byte truncation、DPAPI failure/zero-size output/free の差分は、Rust の本番入口で未比較
- actual cancellation/drop、join-panic、blocking job 継続、provider close、native output allocation、並行実行と lifecycle/distribution の検証は残る。future drop から native call 停止を保証しない
- 現代 Safari binarycookies、新しい Chromium version/app-bound、Local State/profile grammar の追加対応は固定 Go にない範囲を含むため、今回の parity とは別の調査・機能追加として扱う

実 credential の抽出・送信、live browser/account/keyring、persistent access、system trust/settings の変更は今回の範囲外である。拒否された supplemental probe の再実行は行わない。

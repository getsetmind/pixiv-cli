# FANBOX download の接続比較

固定した Go `4b4426487ef18bed276706daec385e0d0a6979f9` の root RunContext capture
を比較する。期待値の post/resource/save DTO は作らず、実 SQLite と Store、
AccountService、Facade の lease、SDK の endpoint/resource/save、所有済み出力を使う。
本番の prepare_root と execute へ同じ入力を渡し、help も本番 writer を通す。
root と leaf の runtime 読込み、options の runtime 読込みを別に記録する。
成功時には本番 post_success(false) を DB cleanup より前に呼び、forbidden-update canary
が一度も呼ばれないことで development-build の自動更新 skip を検証する。
所有する Database はコマンド終了後に実際に close し、Go の LIFO cleanup 順も比べる。

request、RawRead、body close、writer の要求 bytes・返却 count・error、lease close、
成功 prefix、上書き前後、空 directory、file bytes/hex/SHA256/size/mode を比較する。
Rust std::io::Write は Go の (n,error) を返せないため、error/EPIPE 時の output_writes.n は
owned buffer に実際に追加された prefix の accepted-byte count として記録し、その後に
Rust Err を返す。Go の tuple を Rust の戻り値として主張しない。bytes と error の対応、
呼出し順、CLI がその失敗を無視して進める保存結果は完全比較する。
API decoder の容量も既存実装が固定 Go と一致する範囲では完全比較し、media の
65536-byte producer を含めて黙って落とさない。独自 transport は指定された保存済み
synthetic cookie、許可 host、URL、応答順だけを許す。

以下だけを明示的に投影する。

- SQLite の db_before/db_after は両 driver の生の hash を取得する。物理ページ配置と
  Go の read open による PRAGMA 書込みは cross-driver byte 一致を主張しない。
  corrupt seed の生 bytes/hash は完全比較し、全行の保存済み rows と config は完全比較する
- database-corrupt の stderr と corrupt/missing-table の query 診断は、Go modernc と
  Rust rusqlite の表示差だけを検証・記録する。実 query の SQLite code 26/1 を確認し、
  残る行、exit、trace、request 不在、cleanup は完全比較する
- EPIPE writer 診断は Rust の生 OS error 表示と Go の `broken pipe` を明示的に対応付ける。
  writer の count、bytes、呼び出し順、無視された失敗後の保存は完全比較する
- diagnostics の randomized owned HOME だけを `<HOME>` に置換する。
  相対 saved path と media/file bytes に追加の正規化はしない

integration child は Linux x86_64 に限定し、umask 0022 と隔離 HOME/TMP/XDG/cwd を
子の開始時に固定する。socket/socketpair/connect/execve/execveat の process-wide TSYNC
seccomp を実際に設定し、socket/exec の EPERM probe を確認する。親の環境は変更しない。
60秒の owned child deadline は kill/wait と diagnostics 回収を含む。

17行の actual Rust binary 比較は rejected flags 11、help 4、stdin-empty と未認証選択に
限定する。credential を持たない同じ schema/Pixiv canary の owned DB を seed し、
stdout/stderr/exit、保存済み rows/config の不変、media/temp/native directory 不在を確認する。
pre-exec では物理 socket/connect deny を設定し、AF_INET/AF_INET6 の EPERM を確認する。
Tokio signal に必要な process-local AF_UNIX socketpair だけを許し、実 pair を閉じる positive
probe と、実 outbound connection を作らない invalid-fd connect の negative probe も確認する。
起動に必要な exec を許可するため、actual binary の exec denial は主張しない。

native 成功 networking、native Windows/Darwin startup、SIGPIPE、symlink containment、
non-Linux file modes、sync/chmod/disk-full、実 external media/accounts は証明範囲外である。
wire から到達不能な zero ResourceRef、空 asset ID、任意 asset kind を DTO 注入で補わない。
SDK SaveResource のより深い producer 比較は SDK の既存 public fixture を使う。

本番 API の未実装 RED は focused parser 契約で取得している。この追加 observer の全行が
本番 API より前に compile して失敗したという証拠として扱わない。

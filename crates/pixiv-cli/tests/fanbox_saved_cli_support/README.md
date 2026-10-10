# 保存済み FANBOX CLI の接続比較

`fanbox_saved_content_reads.rs` は SHA-256
`14eb6414dc0694598bbb85992bf134338e93c3cdc7e391bc5ac3f9e2e3b4ca1b`
で固定した Go の172行を使う。期待値と Go ソースを変更しない。

167行は本番の `ReadCommand::parse`、`resolve_source`、`execute`、
`finish_command` を、実際の SQLite、設定 `Store`、`AccountService`、
`Facade`、SDK `Client` に接続する。注入するのは通常の options loader、
account opener、close policy、RawTransport/RawBody、stdin/stdout の境界だけ。
戻り値の DTO を偽造しない。独立した反復実行は同じ transport 応答列を使い、
client 自体は保存済みアカウントから毎回開く。client を保持して、allocator の
アドレス再利用を unique-client と誤認しない。

全25 observation field はテストに列挙し、未分類の追加を失敗にする。
stdout/stderr、終了コード、errors/reasons、trace、requests/options/proxy、
stdin の呼出回数、write の呼出サイズ、body/lease/idle close、client の個数、
出力モード、論理 DB 行、設定 bytes、残存 temp は原則完全一致で比較する。
異常系でも後続行の比較を続け、差分をまとめて報告する。

以下は Go 固有の証拠として明示的に分離する。fixture 全体と159 source hash、
434 frozen production hash を検証するため、これらの値も黙って捨てたり変更したり
しない。

- `db_before` / `db_after`: Go sqlite driver の物理ページ配置と、read open 時の
  PRAGMA user_version 書込み。Rust の実 hash も取得するが、cross-driver の bytes
  一致や物理 read-only を主張しない。実データ行の before/after は別に完全比較する
- `socket_denied` / `exec_denied`: Go helper のプロセス全体への seccomp 設定。
  Rust child は所有済みファイル・stdio と注入した transport を使う。
  Go の denial を実行したと主張せず、この2 field の Rust 値は false とする
- `database-corrupt-before-options` の error/stderr と3 query error、
  `database-missing-table-before-options` の先頭 query error:
  modernc sqlite と rusqlite の driver 診断文。実 error の corrupt/missing-table
  実 SQLite error code 26 / 1 と診断を検証し、残る DB 行、exit、順序、
  診断の reason などは完全比較する。command error の配列長・順序と、stderr の
  JSON envelope、command_failed code、改行は完全一致を要求し、message payload
  だけを明示的に投影する。生の Rust / Go 診断は実行ログに記録する
- `json-temp-create-failure` の error/stderr: Go PathError と Rust io::Error の
  物理 OS 診断と CreateTemp 命名。実 TMPDIR を所有済みの file に置き換え、
  実 io::ErrorKind::NotADirectory と OS error code 20、request 不在、lease 解放を
  検証する。配列順序・JSON envelope/code/改行を完全比較し、message payload だけ
  を投影して、両言語の生の診断を記録する
- root-startup 4行、root-stop 1行: 固定 Go root の安全な stop canary。
  source/hash/inventory で保持する。167行の command/composition 実行によって
  この5行の Rust root 実行や成功した Go cmd/pixiv bootstrap を証明しない

OS 環境変数は子プロセスの起動時に設定する。テスト本体の環境を変更しない。
pipe-output 行は実際の所有済み Unix pipe に書き、reader thread で排出する。
ネットワーク、ブラウザ、認証の変更、native handler、更新、media download は
使わない。RawTransport は選択された synthetic cookie と endpoint host を検査し、
fixture 列以外の送信を許さない。

この追加比較は本番 API 実装後に書いた。元の focused RED と本番 API 実装より
先に、この167行すべてが compile して失敗したという証拠ではない。

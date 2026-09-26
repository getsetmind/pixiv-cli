<div align="center">

# pixiv-cli

**Pixiv CLI · MCP stdio server · Go SDK**

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

<p><a href="https://github.com/FlanChanXwO/pixiv-cli/actions/workflows/ci.yml"><img alt="Quality gate" src="https://github.com/FlanChanXwO/pixiv-cli/actions/workflows/ci.yml/badge.svg?event=push"></a> <a href="https://github.com/FlanChanXwO/pixiv-cli/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/FlanChanXwO/pixiv-cli?style=flat-square"></a> <a href="go.mod"><img alt="Go" src="https://img.shields.io/github/go-mod/go-version/FlanChanXwO/pixiv-cli?style=flat-square"></a> <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/FlanChanXwO/pixiv-cli?style=flat-square"></a> <img alt="Views" src="https://hits.sh/github.com/FlanChanXwO/pixiv-cli.svg?style=flat-square&amp;label=views"></p>

[インストール](#インストール) · [クイックスタート](#60秒クイックスタート) · [インターフェース](#インターフェースの選択) · [ドキュメント](#ドキュメント)

</div>

`pixiv-cli` は Pixiv のエコシステムをターミナルに持ち込みます。作品と作者の探索、アカウントとコレクションの管理、作者のフォロー、作品のブックマーク、ビジュアル作品のダウンロードに対応します。独立した非公式のサードパーティ製 CLI・MCP サーバー・公開 Go SDK であり、Pixiv 株式会社との提携や承認関係はありません。CLI と MCP サーバーは同じ公開 Go SDK を呼び出し、認証済みの情報源として Pixiv App API を使用します。Pixiv の規約と適用法に従って使用してください。

## なぜ pixiv-cli か

- **一貫した機能セット** — CLI・MCP・SDK のいずれからも、キーワード検索、詳細、ランキング、おすすめ、ユーザー、ブックマーク、フォロー、ダウンロード、うごイラを扱えます。逆画像検索は CLI/MCP の機能セットに組み込まれています。
- **読み取り専用の FANBOX アクセス** — `FANBOXSESSID` で認証すると、CLI・MCP・`sdk/fanbox` から、作者、投稿、ホーム／支援中フィード、タグ、提供元のファイルリソースを閲覧できます。
- **組み合わせて使えるビジュアル処理** — ビジュアル作品の一覧は、パイプに渡すと正規の NDJSON を自動で出力します。`--filter` で型付きのローカル作品フィルタを指定でき、条件に一致したレコードをそのまま `download` に渡せます。
- **ローカルアカウントプール** — `pixiv auth pool status|enable|disable` で、読み取り処理にデータベース管理のアカウント割り当てを有効にできます。割り当て時は Pixiv の `Retry-After` 応答に従い、認証情報は外部に出しません。
- **案内に沿ったアカウント登録** — `pixiv auth login` でブラウザ OAuth を完了し、その後は `auth list`、`auth use`、`auth check` でローカルの複数アカウントを管理できます。
- **うごイラの出力形式** — GIF または APNG を選べます。ファイル名テンプレートが不正または空の場合は安定した既定値を使用し、その旨を警告として出力します。
- **ダウンロード結果の明示** — 画像品質と閉区間のページ範囲を指定でき、許可リストにある Pixiv CDN URL を直リンク元として利用できます。完了したファイル、警告、失敗はいずれも確認できます。
- **認証済み App API による探索** — App API 経由で R18 の詳細、ページ、うごイラのメタデータ、16 種すべてのランキングモードを読み取れます。
- **実用的な検索フィルタ** — レーティング、コンテンツ種別、AI モード、アスペクト比、解像度、バージョン付きの描画ツールカタログを扱えます。逆画像検索は、ローカルファイルまたは URL から SauceNAO や ascii2d に問い合わせられます。
- **Pixiv の URL を直接指定** — 対応する作品 URL をそのまま `detail` や `download` に貼り付けられます。認証済みのプロフィール URL と作品一覧 URL は、その作者のビジュアル作品に展開されます。
- **ローカルでの複数アカウント OAuth** — ブラウザログイン、アカウントの選択、リフレッシュトークンのローテーション、任意でマシン間コールバックリレーを利用できます。
- **自動化にそのまま使える統合** — 型付きの SDK エラー、JSON 出力、クリーンな MCP stdio、署名付きリリース更新、完全な結果報告を備えます。

## インストール

### インストーラスクリプト（Windows・Linux・macOS）

Linux/macOS（`sh`）:

```bash
curl -fsSLo /tmp/pixiv-install.sh https://github.com/FlanChanXwO/pixiv-cli/releases/latest/download/install.sh && sh /tmp/pixiv-install.sh --add-to-path
```

Windows コマンドプロンプト（`cmd.exe`、PowerShell 不要）:

```bat
curl.exe -fsSLo "%TEMP%\pixiv-install.cmd" https://raw.githubusercontent.com/FlanChanXwO/pixiv-cli/main/scripts/install.cmd && call "%TEMP%\pixiv-install.cmd" --add-to-path
```

どちらのスクリプトも AMD64/ARM64 を検出し、最新の安定版公式リリースアーカイブを選択して、公開された SHA-256 を検証し、ステージングしたバイナリを事前確認したうえで、PATH を変更する前にユーザー単位でインストールします。PATH を変更しない場合は `--no-path` を、別のインストール先を指定する場合は `--install-dir DIR` を使います。実行前に、ダウンロードしたスクリプトを確認できます。

バージョン付きインストーラは、公式 GitHub の HTTPS パスにある `checksums.txt` を使用します。組み込みの無償候補ソースはプラットフォーム別アーカイブの取得にのみ使用し、返されるチェックサムの内容が公式取得分と一致していなければなりません。さらに、ダウンロードしたアーカイブは SHA-256 検証を通過する必要があります。ここで変わるのは配布経路の到達性だけで、リリースの同一性や完全性は変わりません。

### Docker（Linux amd64/arm64）

公式イメージは GHCR の `ghcr.io/flanchanxwo/pixiv-cli` と Docker Hub の `docker.io/flanchanxwo/pixiv-cli` に公開されています。どちらのレジストリも、同じネイティブビルドの `linux/amd64` と `linux/arm64` リリースイメージを提供します。再現性が重要な場合は、いずれかのレジストリから正確なリリースを取得してください:

```bash
docker pull ghcr.io/flanchanxwo/pixiv-cli:v1.2.3
docker pull docker.io/flanchanxwo/pixiv-cli:v1.2.3
```

`latest` は安定版リリースのみを追跡し、プレリリースタグが `latest` を動かすことはありません。現在の安定版を追跡するには、`ghcr.io/flanchanxwo/pixiv-cli:latest` または `docker.io/flanchanxwo/pixiv-cli:latest` を取得します。イメージは `linux/amd64` と `linux/arm64` 向けにネイティブビルドされています。コンテナは他のインストール方法と同じ `pixiv` バイナリを実行し、同じ `~/.pixiv-cli` 状態名前空間を使用します。

アカウント状態を永続化し、ダウンロード用ワークスペースを公開するには:

```bash
docker run --rm \
  -v pixiv-cli-state:/home/pixiv/.pixiv-cli \
  -v "$PWD:/work" \
  ghcr.io/flanchanxwo/pixiv-cli:v1.2.3 \
  --version
```

ダウンロードしたファイルは、明示的な出力パスを指定して `/work` に置きます。`/work` はコンテナの作業ディレクトリであり、別の製品モードではありません。

バインドマウントはホスト側の所有権を保持します。ホストのディレクトリがイメージの UID 1000 から書き込めない場合は、ホストのユーザーとして実行し、コンテナに一時的な `HOME` を与え、出力パスを明示します:

```bash
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/pixiv-cli \
  -v "$PWD:/work" \
  ghcr.io/flanchanxwo/pixiv-cli:v1.2.3 \
  download URL --output /work/downloads
```

同じホストユーザーで状態を永続化する場合は、既定の名前付きボリュームではなく、自分が所有するホストディレクトリをバインドします:

```bash
mkdir -p "$PWD/pixiv-cli-state"
docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e HOME=/tmp/pixiv-cli \
  -v "$PWD/pixiv-cli-state:/home/pixiv/.pixiv-cli" \
  ghcr.io/flanchanxwo/pixiv-cli:v1.2.3 \
  auth list
```

リフレッシュトークンは、argv の値として渡さず stdin からインポートします。手動でインポートするときは TTY を割り当て、非表示の入力プロンプトでエコーを抑止します。コマンドを実行して不透明なトークンを貼り付け、EOF（`Ctrl-D`）を送ります。

```bash
docker run --rm -it \
  -v pixiv-cli-state:/home/pixiv/.pixiv-cli \
  ghcr.io/flanchanxwo/pixiv-cli:v1.2.3 \
  auth import
```

自動化では、シークレットマネージャーの stdout を `-it` ではなく `-i` で直接 stdin にパイプします。コンテナが書き込むのは永続状態ボリュームだけです:

```bash
secret-manager print-token | docker run --rm -i \
  -v pixiv-cli-state:/home/pixiv/.pixiv-cli \
  ghcr.io/flanchanxwo/pixiv-cli:v1.2.3 \
  auth import
```

これは既存の `pixiv auth import` の動作を利用するもので、`auth login` に Docker 専用の OAuth コールバックフローを追加するものではありません。

MCP のトランスポートは `docker run --rm -i ghcr.io/flanchanxwo/pixiv-cli mcp` のままで、stdout は MCP JSON-RPC 用に確保されます。リリースを固定するには次のようにします:

```bash
docker run --rm -i ghcr.io/flanchanxwo/pixiv-cli:v1.2.3 mcp
```

MCP サーバーで保存済みアカウントを再利用する場合は、`-v pixiv-cli-state:/home/pixiv/.pixiv-cli` を追加します。

アップグレードは、新しいイメージを取得し、同じ状態ボリュームで再デプロイします。この手順によって `pixiv update` がコンテナ対応になるわけではありません。

### AI エージェントにインストールさせる

ターミナルを操作できる Codex、Claude Code、Cursor などのローカル AI エージェントに、次のプロンプトをそのまま貼り付けます:

```text
https://github.com/FlanChanXwO/pixiv-cli の最新安定版を、このマシンにインストールしてください。まずリポジトリの scripts/install.sh または scripts/install.cmd を確認し、検出した OS とアーキテクチャに合うスクリプトを選びます（Windows では cmd.exe を使い、PowerShell を呼び出してはいけません）。ダウンロードするのは公式 GitHub Release のアセットだけに限定し、公開された SHA-256 検証に合格しなければ既存のファイルを置き換えないでください。管理者権限や root 権限を使わずユーザー単位でインストールし、選択したインストールディレクトリだけをユーザー PATH に追加してください。不足している前提ツールがあれば、インストール前に確認を取ってください。Pixiv の認証情報は決して読み取らず、出力もしないでください。最後に pixiv --version で確認し、インストールしたバージョンと、変更したすべてのファイルおよび PATH の変更を報告してください。
```

### Homebrew（macOS と Linux で推奨）

```bash
brew install FlanChanXwO/tap/pixiv-cli
```

後で更新するには:

```bash
brew update
brew upgrade pixiv-cli
```

### Go

公開済みの正確なタグを指定します。ソースからのインストールには、Go、cgo、C リンカー、および対象プラットフォームに対応するコミット済みの Rust 静的ライブラリが必要です。

```bash
go install github.com/FlanChanXwO/pixiv-cli/cmd/pixiv@vX.Y.Z
```

### リリースアーカイブまたはソースからのビルド

[GitHub Releases](https://github.com/FlanChanXwO/pixiv-cli/releases) から対応するアーカイブをダウンロードするか、チェックアウトをビルドします:

```bash
sh scripts/build.sh
```

直接ダウンロードした成果物には、チェックサムと署名済みマニフェストが含まれます。対応プラットフォームと信頼性の詳細は [CLI リファレンス](docs/en/cli-reference.md#installation) を参照してください。

## 60秒クイックスタート

```bash
# ブラウザ OAuth で Pixiv アカウントを保存します。
pixiv auth login

# 非表示入力で FANBOX セッションを保存し、FANBOX のコンテンツを確認します。
pixiv fanbox auth import
pixiv fanbox post 123456

# App 側のフィルタで検索します。
pixiv search "初音ミク" --type illust --ai-mode exclude --resolution high
pixiv novel search "初音ミク" --rating sfw --min-text-length 1000

# ローカル画像または HTTP(S) の画像 URL を逆画像検索します。結果は JSON か NDJSON で出力できます。
pixiv search ./image.png --provider ascii2d-color --json
pixiv search https://your-image-url.example/image.png --provider all --ndjson

# 作者をフォローしてコレクションを作ります。
pixiv follow add 12345678
pixiv bookmark add 123456

# 詳細を確認し、おすすめを見つけてダウンロードします。
pixiv detail https://www.pixiv.net/artworks/123456
pixiv recommended all --limit 10
pixiv timeline latest --type illust --limit 20
pixiv download https://www.pixiv.net/artworks/123456 --pages 1,3-5 --quality regular
pixiv download 123456 https://i.pximg.net/img-original/example.jpg

# 作者のビジュアル作品をまとめてダウンロードします。
pixiv download https://www.pixiv.net/users/12345678/artworks
```

すべてのコマンド、フラグ、設定キー、環境変数、フォールバック規則、更新動作については、`pixiv --help` を実行するか、[CLI リファレンス全文](docs/en/cli-reference.md) を参照してください。

## インターフェースの選択

### CLI

対話的に使うときは既定のテキスト出力を、機械可読な出力が必要なときは、コマンドが対応していれば `--json` を使います:

```bash
pixiv ranking --mode day --json
pixiv user search "miku" --limit 10 --json
pixiv user detail 12345678
pixiv timeline latest --type illust --limit 10 --json
```

### MCP

逆画像検索は CLI/MCP の統合から利用できます。公開 Go SDK も、型付きの作品・小説のブックマーク操作とコメント操作を公開しています。

stdio サーバーは明示的に起動します。stdout は JSON-RPC 用に確保され、ツールの失敗は `isError=true` を付けた構造化結果として返されます。既定では、プロジェクト単位のログファイルや日次ログファイルは作成されません。

```bash
pixiv mcp
# FANBOX のツールは独自の実行時認証情報の選択を使用します。
pixiv fanbox mcp
```

ツール、パラメータ、構造化出力、認証動作については、[MCP ツール契約](docs/en/mcp-tools.md) を参照してください。MCP の固定ステータス、エラー、表示テキストは英語です。Pixiv のメタデータとユーザーが指定したテキストはそのまま保持されます。

`reverse_search` ツールは通常のローカルファイルまたは HTTP(S) URL を受け取り、そのソースをサードパーティのプロバイダーにアップロードする場合があります。信頼されたローカル MCP クライアントは、プライベートファイルやプライベート／ループバック／リンクローカル URL を要求できるため、信頼できるクライアントからだけ実行してください。詳細は [逆画像検索の MCP 契約](docs/en/mcp-tools.md#reverse-image-search) を参照してください。逆画像検索のプロキシ、User-Agent、チャレンジ回復の詳細設定は、[CLI リファレンス](docs/en/cli-reference.md) にあります。FlareSolverr は JSON チャレンジ回復の制御経路であり、ネイティブの ascii2d 画像アップロードを受け取ることはありません。

### Go SDK

**`go.mod` で宣言されている Go バージョンが、ソースビルドと SDK 開発の公式な基準です。** リポジトリ、CI、リリースビルド、ネイティブパッケージングは、いずれもこの一つのツールチェーン宣言を使用します。

公開 SDK は認証情報を明示的に受け取り、CLI のローカルアカウントストアやプロセス環境変数を読み取りません。認証情報はアプリケーションのシークレットストアから取得し、`Open` が返すローテーション後の認証情報を保存してください:

```go
package main

import (
	"context"
	"fmt"
	"log"

	pixiv "github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

func main() {
	ctx := context.Background()
	refreshToken := "replace-with-a-refresh-token-from-your-secret-store"
	client, _, err := pixiv.Open(ctx, refreshToken)
	if err != nil {
		log.Fatal(err)
	}

	result, err := client.SearchArtworks(ctx, pixiv.SearchArtworksRequest{Word: "初音ミク"})
	if err != nil {
		log.Fatal(err)
	}
	for _, artwork := range result.Items {
		fmt.Printf("%d %s\\n", artwork.ID, artwork.URL)
	}
}
```

インポートパスは `github.com/FlanChanXwO/pixiv-cli/sdk/pixiv` です。`sdk/fanbox` は `FANBOXSESSID` を明示的に受け取り、Chrome 146 の TLS ルーティングと、組み込みの Firefox 148 HTTP User-Agent ベースラインを使うネイティブ経路をサポートします。サービス単位のプロキシ、User-Agent、チャレンジ回復専用の FlareSolverr も任意で指定できます。公開 SDK が提供するのは、型付きの Pixiv/FANBOX クライアントと不透明なリソース API であり、CLI の一括ダウンロード用ヘルパーではありません。`SaveResource` は `ResourceRef` を受け取り、Pixiv の `SaveResourceURL` は許可された Pixiv メディアホストの HTTPS URL を受け取ります。どちらもアトミックに保存し、保存先パス、バイト数、レスポンスの Content-Type を返します。[SDK ガイド](docs/en/sdk.md) には、モデル、カーソル、リソース、エラー、呼び出し側の責務、および CLI/MCP の JSON 出力が使う明示的な DTO 境界が記載されています。メディアリソースがこの境界を越えるときは、常に不透明な参照として扱われます。

## 認証とトークンの安全性

`pixiv auth login` が推奨の設定方法です。Pixiv App OAuth の生のリフレッシュトークンを、UID ごとにローカルアカウントストアへ保存します。

### アカウント情報に関する注意

アカウント名、ID、会員状態の示唆、現在選択されているローカルアカウントは、ローカルストアと Pixiv の応答から得られる便宜的な情報です。アカウントの所有権、権利、現在の Pixiv での状態を証明するものではないため、参考情報として扱ってください。重要なアカウント情報は Pixiv 上で確認し、管理を許可されたアカウントだけを使用してください。

macOS、Windows、デスクトップ Linux では、`pixiv` がインストール済みバイナリ用のカレントユーザー `pixiv://` コールバックハンドラーを準備します。サーバーに `login_relay_public_url` と `login_relay_listen_addr` が設定されている場合、`pixiv auth login` は 1 回限りのリモートハンドオフ URL を表示します。この URL を開くと、セッションはインストール済みのデスクトップハンドラーへ直接引き渡され、ハンドラーが OAuth を開始してコールバックをサーバーに返します。したがってリモートログインには、pixiv-cli をインストールしたデスクトップが必要です。確認ページやコールバックをコピーするフォームはありません。詳細は [CLI リファレンス](docs/en/cli-reference.md#getting-a-refresh-token) を参照してください。

```bash
pixiv auth list
pixiv auth pool status
pixiv auth use 12345678
pixiv auth check
```

## ドキュメント

| ガイド | 用途 |
| --- | --- |
| [CLI リファレンス](docs/en/cli-reference.md) | コマンド、フラグ、認証、設定、フォールバック、ダウンロード、更新 |
| [Go SDK](docs/en/sdk.md) | 公開クライアント、モデル、ページネーション、リソース、型付きエラー |
| [MCP ツール](docs/en/mcp-tools.md) | ツールスキーマと出力の意味 |
| [変更履歴](changelog/README.md) | ユーザーに影響する変更 |

日本語で用意しているのはこの README だけです。CLI・SDK・MCP のリファレンスは英語版を参照してください。公開インターフェースの正本は英語ドキュメントです。

## ライセンス

[MIT](LICENSE) © FlanChanXwO

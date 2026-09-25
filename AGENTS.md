# pixiv-cli エージェント契約

このリポジトリは `pixiv` CLI、2 つの MCP stdio サーバー、公開 Go パッケージ `sdk`・`sdk/pixiv`・`sdk/fanbox` を提供する。

## まずここから

タスクに着手する前に、下記の該当ローカルスキルを読むこと。これらはチェックイン済みの指示であり、個人の設定・CCS インストール・グローバルスキルへの依存ではない。クライアントが `.agents/skills` を発見できない場合は、リンクされた `SKILL.md` を直接開く。変更に必要なルートだけを読む。

| タスク | ローカル指示 |
| --- | --- |
| Go の実装・デバッグ・リファクタ・設計 | [pixiv-cli-develop](.agents/skills/pixiv-cli-develop/SKILL.md) |
| テストの選択、Red/Green の実行、変更の検証 | [pixiv-cli-test](.agents/skills/pixiv-cli-test/SKILL.md) |
| コードコメント・API ドキュメント・番号付き段階の作成やレビュー | [pixiv-cli-code-commenting](.agents/skills/pixiv-cli-code-commenting/SKILL.md) |
| MCP ツールの追加・変更 | [pixiv-cli-mcp-tool](.agents/skills/pixiv-cli-mcp-tool/SKILL.md) |
| Rust・cgo・ネイティブライブラリ・プラットフォーム証拠の変更 | [pixiv-cli-native](.agents/skills/pixiv-cli-native/SKILL.md) |
| ドキュメントまたはどちらの種類のスキルの編集 | [pixiv-cli-docs](.agents/skills/pixiv-cli-docs/SKILL.md) |
| コードレビューや PR の評価 | [pixiv-cli-review](.agents/skills/pixiv-cli-review/SKILL.md) |
| PR の準備・更新・検証 | [pixiv-cli-pr](.agents/skills/pixiv-cli-pr/SKILL.md) |
| チェックの診断や承認済みワークフロー実行の操作 | [pixiv-cli-ci](.agents/skills/pixiv-cli-ci/SKILL.md) |
| リリースの準備や publisher の復旧 | [pixiv-cli-release-notes](.agents/skills/pixiv-cli-release-notes/SKILL.md) |
| ステージ済み変更からのコミットメッセージ作成 | [pixiv-cli-commit-message](.agents/skills/pixiv-cli-commit-message/SKILL.md) |

別配布の[プロダクトスキル](skills/pixiv-cli/SKILL.md)は、インストール済みバイナリの利用を教えるもので、リポジトリの開発ワークフローではない。メンテナンススキル名は `pixiv-cli-` 接頭辞を、プロダクト名は `pixiv-cli` を維持する。

## 交渉不可の境界

- `cmd/pixiv` は薄く保つ。`internal/cli/root.go` がコマンドツリーと本番依存を組み立て、コマンドのオーナーは `internal/cli/commands` 配下に置く。グローバルなサービスロケータや削除済みの bootstrap/resource グラフを復活させない。
- CLI/MCP の Pixiv・FANBOX 操作は公開 SDK とオーナー固有の狭いポートを使い、プロトコルアダプターを使わない。MCP ツールは `internal/mcpserver/{pixiv,fanbox}/tools/<tool>` に属し、その stdio ランタイムは CLI コマンドが起動する。
- リバースサーチは明示的な例外である。`internal/services/reversesearch/assembly` を import できるのは CLI のコンポジションルートだけ。コマンドと MCP のオーナーはトップレベルの `internal/services/reversesearch` 契約を使えるが、そのプロバイダーサブパッケージは決して使わない。
- 共有機構は既存のオーナーに置く: record、pagination、traversal、lifecycle、configuration、file persistence、downloader。汎用ユーティリティにプロダクトのプロトコルやアカウント意味論を持たせない。所有権の詳細は [architecture](docs/en/maintainers/architecture.md) を参照。
- App-only 境界を守る。コンテンツには認証済みのローカルアカウントか、適格なデータベース管理プールアカウントが必要であり、エラーが匿名 Web 経路を選ぶことはない。データコマンドは `--uid` も `--refresh-token` も受け付けない。公開 SDK と MCP はそれぞれ独自の明示的な資格情報契約を維持する。
- secret をログ・エラー・fixture・PR・成果物に出さない。secret を stdout に出せるのは、明示的に要求された素の `auth export [UID]` または `auth export --all` だけ。それ以外は文書化された private-output/transfer 経路を使う。SQLite アカウントストアは secret を保持するため、デバッグの近道として実資格情報を覗かない。
- CLI の機械可読出力と MCP の JSON-RPC stdout を汚さない。MCP のランタイム失敗は `isError=true` を伴う構造化出力を保つ。キャンセル・トランスポート・認証・上流・永続化の失敗を、成功形の空データではなく報告する。
- limit・timeout・retry・truncation・fallback は、検証済みの要件・プラットフォーム制約・確立された契約・再現可能な失敗があるときにだけ導入する。有効なデータを黙って捨てず、トリガーを説明してテストする。

## 作業合意

- 編集の前に、求められた振る舞いと受け入れ証拠を特定する。小さな変更は小さく保ち、大きな作業では結果を左右する未知点を明確にする。意思決定には issue・PR・会話を再利用し、プロセス文書を自動で作らない。
- ブランチと既存の変更を確認し、無関係な作業を保持する。利用可能なツールだけを使い、作業ディレクトリを明示し、入力を安全に引用する。シンボルや呼び出し元には利用可能な意味的ナビゲーションを優先し、対象を絞った検索やコンパイラ確認しかできない場合はそう開示する。
- テストを足す前に既存のカバレッジを再利用または拡張する。新しい関数やファイルはテストのノルマではない。純粋な構造変更は before/after の特性評価を使う。適用可能な failing-test 要件を満たせない場合は明示的な例外を得る。通常のコメント・文書編集には、でっち上げの実行時テストではなく関連する文書とツールの確認を要する。消費されるディレクティブや例は振る舞いの確認を要することがある。
- 言語・所有権・検証の規則は develop/test スキルに従う。ネイティブのビルド要件を維持する。Linux fixture の pass は全プラットフォームの証拠ではない。
- 標準ライブラリ・プラットフォーム・既存プロジェクトの機能を再利用する。依存の追加や未導入ツールのインストールの前に承認を得て、必要性・代替案・lockfile/ライセンス/セキュリティ/配備への実質的影響を説明する。
- 実行の前にネットワークアクセスと実質的な副作用を説明する。実際の Pixiv/FANBOX 呼び出し、ブラウザー資格情報アクセス、画像アップロード、アカウント書き込みには明示的なスコープと承認を要し、日常的なオフライン検証ではない。
- 複数ステップの進捗を、利用可能ならクライアントの plan ツールで、なければ簡潔なチェックリストで可視化する。継続時は実際の状態を復元する。委譲は明確な所有権を持つ限定的な作業に限り、統合検証はメインエージェントの責務のままとする。
- `AGENTS.md` は日本語で書く。メンテナンススキルとプロダクトスキル、その参照、UI メタデータは英語または日本語のどちらでもよいが、1 つの文書内で言語を混在させない。名前・frontmatter・ルートは機械可読な既存形式を維持する。ソースコメントは英語または中国語でよく、このファイルの言語ではなくローカルの読者と [commenting rules](.agents/skills/pixiv-cli-code-commenting/SKILL.md) に従う。公開の英語/簡体字中国語ドキュメントは行動的に整合させ、会話にはユーザーが要求した言語を使う。
- 引き渡しの前に意味のある変更をレビューする。実際の diff、実行したチェックと結果、残るリスク、PR/worktree の場所を報告する。ローカルレビューは GitHub の承認ではなく、pending や skipped のチェックは pass ではない。
- PR の作成は、マージ・タグ移動・公開・ブランチ保護の変更・本番 secret の使用を許可しない。リリース承認は別個でバージョン固有である。

## 正式参照

[Development and test layout](docs/en/maintainers/development.md)、[CLI contract](docs/en/cli-reference.md)、[MCP contract](docs/en/mcp-tools.md) が、チェックイン済みの振る舞いを説明する。Workflow YAML、`ci/platforms.json`、ツールマニフェストが実行可能な設定を所有する。散文と実装が食い違うときは、争点の挙動を検証して影響する契約を更新する。推測したり無関係な履歴を書き換えたりしない。

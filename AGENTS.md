# pixiv-cli エージェント契約

このリポジトリは、`pixiv` CLI、二つの MCP stdio サーバー、公開 Go パッケージ `sdk`・`sdk/pixiv`・`sdk/fanbox` を提供する。

## まずここから

タスクに着手する前に、下表から該当するローカルスキルを読む。これらの指示はリポジトリに含まれており、個人の設定、CCS のインストール、グローバルスキルを必要としない。クライアントが `.agents/skills` を検出できない場合は、リンク先の `SKILL.md` を直接開く。変更に関係するスキルだけを読む。

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

別配布の[プロダクトスキル](skills/pixiv-cli/SKILL.md)は、インストール済みバイナリの使い方を説明する。リポジトリの開発には、上表のメンテナンススキルを使う。メンテナンススキル名の接頭辞 `pixiv-cli-` と、プロダクト名 `pixiv-cli` を維持する。

## 交渉不可の境界

- `cmd/pixiv` は薄く保つ。`internal/cli/root.go` でコマンドツリーと本番用の依存関係を組み立て、各コマンドは `internal/cli/commands` 配下に置く。グローバルなサービスロケータや、削除済みの bootstrap/resource グラフを復活させない。
- CLI/MCP の Pixiv・FANBOX 操作は公開 SDK とオーナー固有の狭いポートを使い、プロトコルアダプターを使わない。MCP ツールは `internal/mcpserver/{pixiv,fanbox}/tools/<tool>` に属し、その stdio ランタイムは CLI コマンドが起動する。
- リバースサーチには例外を設ける。`internal/services/reversesearch/assembly` を import できるのは CLI のコンポジションルートだけとする。コマンドと MCP のオーナーはトップレベルの `internal/services/reversesearch` 契約を使えるが、プロバイダーのサブパッケージは使わない。
- record、pagination、traversal、lifecycle、configuration、file persistence、downloader の共有機構は、それぞれ既存のオーナーに置く。汎用ユーティリティに、プロダクト固有のプロトコルやアカウントの意味づけを持たせない。所有権の詳細は [architecture](docs/en/maintainers/architecture.md) を参照する。
- App-only 境界を守る。コンテンツの取得には、認証済みのローカルアカウントか、利用条件を満たすデータベース管理下のプールアカウントが必要となる。エラー時に匿名 Web 経路へ切り替えない。データコマンドは `--uid` と `--refresh-token` を受け付けない。公開 SDK と MCP は、それぞれの明示的な資格情報契約を維持する。
- secret をログ、エラー、fixture、PR、成果物に含めない。secret を stdout に出せるのは、明示的に要求された、オプションを付けない `auth export [UID]` または `auth export --all` だけとする。それ以外は文書化された private-output/transfer 経路を使う。SQLite アカウントストアには secret が保存されるため、デバッグ目的でも実際の資格情報を覗かない。
- CLI の機械可読出力と MCP の JSON-RPC stdout に余分な情報を出さない。MCP の実行時エラーは、`isError=true` を伴う構造化出力で報告する。キャンセル、トランスポート、認証、上流サービス、永続化の失敗を、成功を示す空データに置き換えない。
- limit、timeout、retry、truncation、fallback は、検証済みの要件、プラットフォームの制約、確立された契約、再現可能な失敗のいずれかが根拠となる場合にだけ導入する。有効なデータを黙って捨てず、適用条件を説明してテストする。

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

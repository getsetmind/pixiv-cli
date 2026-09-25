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
- secret をログ、エラー、fixture、PR、成果物に含めない。secret を stdout に出せるのは、明示的に要求された、追加オプションなしの `auth export [UID]` または `auth export --all` だけとする。それ以外は文書化された private-output/transfer 経路を使う。SQLite アカウントストアには secret が保存されるため、デバッグ目的でも実際の資格情報を覗かない。
- CLI の機械可読出力と MCP の JSON-RPC stdout に余分な情報を出さない。MCP の実行時エラーは、`isError=true` を伴う構造化出力で報告する。キャンセル、トランスポート、認証、上流サービス、永続化の失敗を、成功を示す空データに置き換えない。
- limit、timeout、retry、truncation、fallback は、検証済みの要件、プラットフォームの制約、確立された契約、再現可能な失敗のいずれかが根拠となる場合にだけ導入する。有効なデータを黙って捨てず、適用条件を説明してテストする。

## 作業合意

- 編集前に、求められる振る舞いと、その達成を示す証拠を特定する。小さな変更は小さく保ち、大きな作業では結果を左右する不明点を明らかにする。判断には既存の issue、PR、会話を利用し、作業手順などの文書を機械的に作らない。
- ブランチと既存の変更を確認し、無関係な作業を保持する。利用可能なツールを使い、作業ディレクトリを明示し、入力を安全に引用する。シンボルや呼び出し元の調査では、利用可能な場合は意味的なナビゲーションを優先する。対象を絞った検索やコンパイラでの確認しかできない場合は、その範囲を伝える。
- テストを追加する前に、既存のテストを再利用または拡張できないか確認する。新しい関数やファイルを作るたびにテストを増やす必要はない。動作を変えない構造変更では、変更前後に同じ特性評価を実施する。適用される failing-test 要件を満たせない場合は、明示的な例外を得る。通常のコメントや文書の編集では、架空の実行時テストを作らず、関連する文書とツールを確認する。ツールが解釈するディレクティブや例は、動作確認が必要になることもある。
- 言語、所有権、検証の規則は develop/test スキルに従う。ネイティブのビルド要件を維持する。Linux の fixture テストが通っても、全プラットフォームでの動作を確認したことにはならない。
- 標準ライブラリ、プラットフォーム、既存プロジェクトの機能を再利用する。依存の追加や未導入ツールのインストールは、必要性、代替案、lockfile・ライセンス・セキュリティ・配備への実質的な影響を説明し、事前に承認を得る。
- ネットワークアクセスや実質的な副作用を伴う操作は、実行前にその内容を説明する。実際の Pixiv/FANBOX 呼び出し、ブラウザーの資格情報へのアクセス、画像のアップロード、アカウントへの書き込みには、対象範囲を明示して承認を得る。これらは日常的なオフライン検証には含まれない。
- 複数ステップの進捗は、利用可能ならクライアントの plan ツールで、なければ簡潔なチェックリストで示す。作業を再開するときは、実際の状態を確認する。委譲する作業は担当範囲を明確にして限定し、統合検証はメインエージェントが担当する。

## 正式参照

[Development and test layout](docs/en/maintainers/development.md)、[CLI contract](docs/en/cli-reference.md)、[MCP contract](docs/en/mcp-tools.md) は、リポジトリに記録された振る舞いを説明する。Workflow YAML、`ci/platforms.json`、ツールマニフェストには実行に使う設定が記されている。文書と実装が食い違う場合は、該当する動作を検証し、影響を受ける契約を更新する。推測で変更したり、無関係な履歴を書き換えたりしない。

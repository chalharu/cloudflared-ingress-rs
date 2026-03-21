# postToolUse hook

Copilot CLI の `postToolUse` hook をこのリポジトリ向けに最適化した設定です。

## Copilot hooks

`.github/hooks/hooks.json` では `postToolUse` を 1 つだけ定義し、`.github/hooks/postToolUse/main.mjs` を起動します。`main.mjs` は標準入力を 1 回だけ読み、Git 差分を 1 回だけ取得し、対象ファイルの判定と linter の切り替えをまとめて行います。

実行対象は `.github/hooks/postToolUse/linters.json` で定義します。この JSON は `tools` と `pipelines` の 2 セクションだけを持ちます。`tools` は実行コマンドと引数を含む concrete な定義で、必要に応じて対象ファイルを引数に付けないコマンドも表現できます。`pipelines` は regex matcher 配列と実行順、各 step の fallback tools をまとめて表します。

この構成により、`postToolUse` が複数コマンドへ分かれている場合に発生するプロセス起動、stdin 解析、Git 差分取得の重複を避けられます。dirty な対象ファイルは `.git/.copilot-hooks/post-tool-use-state.json` の署名と比較し、今回変わったものだけを処理します。pipeline は上から評価され、各ファイルは最初にマッチした pipeline にだけ割り当てられます。matcher は regex 配列のみを受け付け、内部では `|` で結合した 1 つの `RegExp` として扱います。

Markdown は `markdownlint-cli2 --fix` → 再lint、JS/TS 系 (`.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.mts`, `.cts`, `.tsx`) は `Biome check --write` → `Oxlint --fix` → `Biome check --write` → 再check、Python は `ruff format` → `ruff check --fix` → `ruff format` → `ruff check` の順で動きます。Rust は `.rs` の変更をトリガーに `cargo fmt --all` と `cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets` を実行し、Dockerfile は `hadolint` で検査します。自動修正で解決した内容は表示せず、fix 後も残った違反だけを表示します。

各 step はローカル CLI を優先し、必要なら別ツールや `npx` 実行へ順に fallback できます。たとえば JS/TS lint は `oxlint -> eslint -> eslint-npx -> oxlint-npx` の順で試せるようにしています。

フック自体を変更したときは、`node --test .github/hooks/postToolUse/main.test.mjs` で回帰確認を行ってください。`.github/hooks/hooks.json` の timeout は Rust の `cargo clippy` まで許容できるよう 90 秒に設定しています。

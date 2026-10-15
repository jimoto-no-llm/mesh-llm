<!--
JIMOTO.md の「運用ルール」に基づき、このフォークの PR 説明は日本語4行（概要・変更・検証・注意点）で記述してください。
大きなアーキテクチャ変更は「## Architecture」節、プロトコルや互換性への影響は「## Protocol」節に分けて記載してください。
CLI の挙動を変更するときは実行例と出力を、UI を変更するときはスクリーンショットを添付してください。
-->
概要：
変更：
検証：
注意点：

<!--
検証（`検証：` 行）には、実行したコマンドと結果をそのまま書いてください。

Rust を変更したときは、まず次の1本を実行すれば affected-crates.sh が算出した対象クレートに対して fmt・clippy（警告拒否）・crate テストがまとめて走ります:

    just check-pr            # base を変えるときは just check-pr <base>

個別に回す場合：変更したクレートごとに cargo fmt、cargo check、cargo clippy --all-targets -- -D warnings を実行し（mesh-llm のバイナリに影響する変更は -p mesh-llm でも実行）、関連テストを実行してください。

コミット件名は just check-commits、PR タイトルは python3 scripts/check-conventional-commit.py --message "<タイトル>" で確認できます。詳しくは AGENTS.md の Pre-Commit Checklist を参照してください。
-->

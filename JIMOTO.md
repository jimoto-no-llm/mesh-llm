# JIMOTO — 地元フォーク案内

jimoto-no-llm/mesh-llm の独自案内です。上流の使い方（[README](README.md)）に加えて、このフォークでできること・やることをまとめています。英語版（参考訳）は [JIMOTO.en.md](JIMOTO.en.md) です。

## このフォークの方針

- 日本語で開発・運用する。ドキュメントと PR 説明は日本語第一です。
- 速くする。推論ホットパスの確保削減など、計測できる改善から入れます。
- 壊さない。メッシュのプロトコル・プラグイン規約の互換を維持します。
- 見える化する。main 保護・ラベル・CODEOWNERS で運用を固定します。
- 上流に追従する。 upstream の main を定期的に取り込み、差分は小さく保ちます。

## はじめ方（最短）

```bash
git clone https://github.com/jimoto-no-llm/mesh-llm
cd mesh-llm
just build
./target/debug/mesh-llm setup
./target/debug/mesh-llm serve --auto
```

ブラウザで `http://localhost:3131` を開くと Web コンソール、API は `http://localhost:9337/v1` です。フォーク情報の確認は `mesh-llm jimoto`（`--json` 付きで機械可読）です。

## 動作確認対象の主要環境

- Linux（x86_64）・macOS・Windows を主要環境とします。
- 各環境の手順は CONTRIBUTING と `install.sh` / `install.ps1` が基準です。
- 実際に手順どおり動いた記録は Issue #52 に集めます。

## 動作確認済み環境一覧

| 環境 | 版 | 確認日 | 記録 |
|---|---|---|---|
| Linux x86_64（Fedora 44, kernel 6.19） | 0.78.0 | 2026-10-16 | ビルドと CLI のみ（下記） |
| macOS | — | — | 未取得 |
| Windows | — | — | 未取得 |

### Linux x86_64（2026-10-16）

検証した範囲:

1. ビルド: `cargo build -p mesh-llm`（バックエンド非依存のホスト）が成功
2. `./target/debug/mesh-llm --version` → `mesh-llm 0.78.0`
3. `--help` に `serve` / `client` / `doctor` / `rdsh` などのサブコマンドが表示される
4. `./target/debug/mesh-llm jimoto --json` がフォーク情報を返す
5. `./target/debug/mesh-llm doctor split --help` が表示される
6. テスト: `cargo test -p mesh-llm-host-runtime --lib` 全件通過、`cargo clippy --all-targets -- -D warnings` クリーン

未検証（この環境では実行できない）:

- `just build` によるネイティブランタイム込みのビルド。この環境には `just` と `sccache` が無いため、`cargo build`（ホストのみ）で代用しています
- `mesh-llm setup` / `serve` / `join` と推論。ネイティブランタイムが無いため未実施です

再現手順は上記のコマンドのとおりです。記録の追加・訂正は Issue #52 へお願いします。

## スマホで使う

### 今日できること：スマホはクライアントにする

自宅サーバで mesh-llm を出し、スマホのブラウザやアプリから使います。サーバ側で LAN 公開します：

```bash
./target/debug/mesh-llm serve --auto --listen-all --port 9337 --console 3131
```

スマホのブラウザで `http://<サーバのLANアドレス>:3131` を開くと Web コンソールが使えます。OpenAI 互換 API（`:9337/v1`）にアプリから直接投げることもできます。推論の重い処理はサーバ側で動くので、スマホの電池と発熱の心配はありません。

注意点が2つあります：

- `--listen-all` は API と Web コンソールを全インターフェース（`0.0.0.0`）で待ち受けます。利用者の認証はないため、同じネットワークにいる端末なら誰でも使えます。自宅 LAN など信頼できるネットワークに限って使ってください。
- 設定の取得・適用やモデルのロード／アンロードなどのランタイム操作 API は、localhost からの接続しか受け付けません（403 になります）。これらの操作はサーバ側で行ってください。

### Android 端末で直接動かす（実験的）

上流は Linux ARM64（`aarch64-unknown-linux-gnu`）の配布物を出しており、Android の Termux＋proot Ubuntu 環境で Linux バイナリが動く可能性があります。条件と注意点は次の通りです：

- RAM 8GB 以上推奨。小さい量子化モデル（Q4 の 1B〜8B 級）から試します。
- `just build` フルセットは端末では重いため、まず小さいモデル＋ `--local-model-only` で単体サーブから確認します。
- 発熱と電池消費が大きいため、常用サーバ用途には向きません。常時動かすなら自宅サーバ＋スマホはクライアント構成がおすすめです。

### iOS で動かす（将来）

上流の ROADMAP では iOS（Swift＋XCFramework）が先で、Android が後続です。`mesh/sdk/swift` に XCFramework ビルド手順があり、組込みクライアント（iOS・macOS・Android）は正式にサポート対象です。このフォークで iOS アプリを出す段階になったら、ここに手順を足します。それまではブラウザ経由の利用が確実です。

## 画像生成（実験的）

手元で稼働している OpenAI 互換の画像生成サーバーを、メッシュの画像生成モデルとして共有できます。共有したモデルは、メッシュ内のどのノードからでも `POST /v1/images/generations` で利用可能です。生成はサーバーを動かしているノードでのみ行われ、複数ノードへの分散処理には未対応です。

[stable-diffusion.cpp](https://github.com/leejet/stable-diffusion.cpp) の `sd-server` を使用する例です。

```bash
# 1. 画像生成サーバーを起動する（モデルの指定方法は sd-server の README を参照）
sd-server <モデルの指定> --listen-ip 127.0.0.1 --listen-port 1234

# 2. メッシュに sd-turbo という名前で共有する
./target/debug/mesh-llm serve --image-endpoint sd-turbo=http://127.0.0.1:1234/v1

# 3. メッシュ内の任意のノードから呼び出す
curl -s http://localhost:9337/v1/images/generations \
  -H 'Content-Type: application/json' \
  -d '{"model":"sd-turbo","prompt":"a lighthouse at dusk","size":"512x512"}'
```

- `名前=URL` の名前は、メッシュに広告するモデル名です。リクエストの `model` と照合されるため、サーバーごとに別の名前を付けてください。`--image-endpoint` は複数回指定できます。
- サーバーのモデル一覧（`/v1/models`）が応答している間のみモデルを広告し、2回連続で応答がなければ取り下げます。取り下げる前に転送が失敗した場合は、同じモデルを共有しているほかのノードがあればそちらへ転送します（`x-mesh-target` で自ノードを指定した要求と、ほかのノードから転送されてきた要求は再転送しません）。mesh-llm 自体はサーバーの起動・停止やモデルの読み込みを行いません。
- `/v1/models` では、共有したモデルの `metadata.workload_class` が `image_generation` になります。
- 画像は `data[].b64_json` で返却されます。`response_format` を省略した要求も、`b64_json` を指定してサーバーへ転送します。URL 形式で返る画像はサーバーのあるノード内部を指し、ほかのノードからは参照できないため、`response_format: "url"` は使用できません。`n` は 1〜10 を指定できます。
- メッシュが最初の応答を待つ時間は最大5分です。`sd-server` は生成が完了してから応答するため、1回の生成に5分以上かかる設定ではエラーになります。
- URL は `http://` のみ対応しています。認証のないサーバーは、ループバックまたは信頼できるネットワーク内でのみ稼働させてください。
- `--image-endpoint` は `mesh-llm serve` 専用であり、クライアント（`mesh-llm client`）では使用できません。
- 画像の編集（`/v1/images/edits`）と、mesh-llm 自身がモデルを読み込んで生成する方式（Skippy での実行）は、今後対応予定です。

## リリースについて

### 現状

- フォーク独自のリリースバイナリはまだありません。
- 試すだけならソースビルド（[クイックスタート](README.md#クイックスタート)）か、上流の配布バイナリを使います。
- `install.sh` / `install.ps1` は `MESH_LLM_INSTALL_REPO` 環境変数で取得元を切替えられます。フォークがリリースを出し始めたら `MESH_LLM_INSTALL_REPO=jimoto-no-llm/mesh-llm` で取れます。

### フォークでリリースを出す条件

上流の `just release vX.Y.Z` は Release ワークフローを起動し、版上げ・SDK 資材・タグ打ち・全基盤ビルド・公開まで自動化します。フォークで回すには次が要ります：

1. GPU・macOS・Windows のビルド実行環境（上流は Depot と専用ランナー利用。本フォークの Actions は現在未稼働です）。
2. リリース署名鍵（`release-attestation` の鍵ペア）と公開先の整理。
3. 版番号の方針（上流追従かフォーク独自版か）。

環境が整い次第、小さい版（CPU 系のみなど）から出して手順を固めます。

## 運用ルール

- main は保護されています（承認1名必須・stale 却下・スレッド解決必須・squash のみ）。Organization 管理者のみ PR 要件をバイパスできます。
- コミット件名と PR タイトルは Conventional Commits です。コミットは `just check-commits`、PR タイトルは `python3 scripts/check-conventional-commit.py --message "<タイトル>"` で確認できます。説明は日本語4行（概要・変更・検証・注意点）＋必要に応じ Architecture 節です。
- ラベルは `perf`・`skippy`・`scheduler`・`cache`・`maintenance`・`documentation`・`ci:canary` に加え、フォーク独自の `jimoto`・`upstream-sync`・`mobile` を使います。
- Rust 変更は対象クレートの check・clippy（警告拒否）・関連テストが必須です。ホット4クレートと workspace には `clippy::perf` ゲートがあります。

## 自動検証（CI）

- PR 用のワークフロー（`PR · Quality` / `PR · Linux` / `PR · macOS` / `PR · Windows` など）はリポジトリに定義されています。ただし **このフォークでは 2026-10-16 時点で `pull_request` / `push` を契機に実行されていません**（実行履歴は 0 件のまま。PR を実際に作成して確認）。Organization 側の Actions ポリシーが原因と考えられ、確認・変更にはオーナー権限（`admin:org`）が必要です。
- 手動実行は動作します。`gh workflow run "CI · Manual Full"` でプラットフォーム別レーン（`CI · Linux` / `CI · macOS` / `CI · Windows` / `CI · Quality` / `CI · Website`）が起動することを確認済みです。
- したがって **PR を出す前に手元で最低限のゲートを回してください**。自動検証が動かない現状では、これが唯一の定型チェックです。

  ```bash
  just check-pr            # origin/main との差分から対象クレートを判定して fmt / clippy / test
  just check-pr <base>     # 比較先を変える場合
  ```

  `just` が無い環境では `bash scripts/check-pr.sh origin/main` を直接実行してください。作業ツリーの未コミット変更と未追跡ファイルも対象に含まれます。

## 上流への追従

- 定期的に `Mesh-LLM/mesh-llm` の main を取り込みます。
- フォーク独自差分（日本語化・高速化・ルール）は小さく保ち、衝突時はフォーク側で解消します。
- 上流へ返せる改善は上流へ PR します。

## ロードマップ

全体方針は Issue #48 で議論しています。進捗はマイルストーンで管理します。

- [フェーズ0：全体方針の確定](https://github.com/jimoto-no-llm/mesh-llm/milestone/1)
- [フェーズ1：手順・リリース準備](https://github.com/jimoto-no-llm/mesh-llm/milestone/2)
- [フェーズ2：運用開始](https://github.com/jimoto-no-llm/mesh-llm/milestone/3)
- [フェーズ3：対応環境の拡大](https://github.com/jimoto-no-llm/mesh-llm/milestone/4)
- [フェーズ4：英語対応とSDK整備](https://github.com/jimoto-no-llm/mesh-llm/milestone/5)
- [フェーズ5：分散推論と報酬設計](https://github.com/jimoto-no-llm/mesh-llm/milestone/6)


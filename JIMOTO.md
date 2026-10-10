# JIMOTO — 地元フォーク案内

jimoto-no-llm/mesh-llm の独自案内です。上流の使い方（[README](README.md)）に加えて、このフォークでできること・やることをまとめています。

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

ブラウザで `http://localhost:3131` を開くと Web コンソール、API は `http://localhost:9337/v1` です。

## スマホで使う

### 今日できること：スマホはクライアントにする

自宅サーバで mesh-llm を出し、スマホのブラウザやアプリから使います。サーバ側で LAN 公開します：

```bash
mesh-llm serve --auto --listen-all --port 9337 --console 3131
```

スマホのブラウザで `http://<サーバのLANアドレス>:3131` を開くと Web コンソールが使えます。OpenAI 互換 API（`:9337/v1`）にアプリから直接投げることもできます。推論の重い処理はサーバ側で動くので、スマホの電池と発熱の心配はありません。

### Android 端末で直接動かす（実験的）

上流は Linux ARM64（`aarch64-unknown-linux-gnu`）の配布物を出しており、Android の Termux＋proot Ubuntu 環境で Linux バイナリが動く可能性があります。条件と注意点は次の通りです：

- RAM 8GB 以上推奨。小さい量子化モデル（Q4 の 1B〜8B 級）から試します。
- `just build` フルセットは端末では重いため、まず小さいモデル＋ `--local-model-only` で単体サーブから確認します。
- 発熱と電池消費が大きいため、常用サーバ用途には向きません。常時動かすなら自宅サーバ＋スマホはクライアント構成がおすすめです。

### iOS で動かす（将来）

上流の ROADMAP では iOS（Swift＋XCFramework）が先で、Android が後続です。`mesh/sdk/swift` に XCFramework ビルド手順があり、組込みクライアント（iOS・macOS・Android）は正式にサポート対象です。このフォークで iOS アプリを出す段階になったら、ここに手順を足します。それまではブラウザ経由の利用が確実です。

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
- PR タイトルは Conventional Commits（`just check-commits` が検査）。説明は日本語4行（概要・変更・検証・注意点）＋必要に応じ Architecture 節です。
- ラベルは `perf`・`skippy`・`scheduler`・`cache`・`maintenance`・`documentation`・`ci:canary` を使います。
- Rust 変更は対象クレートの check・clippy（警告拒否）・関連テストが必須です。ホット4クレートと workspace には `clippy::perf` ゲートがあります。

## 上流への追従

- 定期的に `Mesh-LLM/mesh-llm` の main を取り込みます。
- フォーク独自差分（日本語化・高速化・ルール）は小さく保ち、衝突時はフォーク側で解消します。
- 上流へ返せる改善は上流へ PR します。


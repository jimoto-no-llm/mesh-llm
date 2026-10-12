# 主要環境別 日本語手順（ビルド・参加・モデル提供）

[JIMOTO.md](JIMOTO.md)の補足です。このフォークはソースビルドが基本です。ビルドは `just build`、起動は `./target/debug/mesh-llm setup` と `serve` 系だけを使います。参加・提供の疎通確認は未実行のため「※未検証」を付けています。

## 共通の前提

1. リポジトリを取ります。
```bash
git clone https://github.com/jimoto-no-llm/mesh-llm
cd mesh-llm
```
2. ビルド済みバイナリのパスを読み替えます。例の `mesh-llm` は `./target/debug/mesh-llm`（Windowsは `.\target\debug\mesh-llm.exe`）です。
3. 本フォークのGitHub Actionsは現在未稼働です。対応OSの裏付けは上流のLinux・macOS・Windows用ワークフローで確認しています。
4. 「※未検証」は、文献の裏付けはあるものの実機疎通していない手順です。

## Linux

### ビルド

1. 必要なものを用意します。`just`・`cmake`・Rust・`sccache`・高速リンカ（`mold`と`lld`）・Node.js 24・`pnpm 10`です。
2. 初回だけ導入用レシピを実行します。
```bash
just bootstrap-build-tools
```
3. ビルドします。
```bash
just build
```
4. sccacheを入れられない環境では、そのシェルで次を設定します。
```bash
export RUSTC_WRAPPER=""
```

### 参加（join）

1. セットアップを仕上げます。
```bash
./target/debug/mesh-llm setup
```
実験用にサービス登録を省くときは次を使います。
```bash
./target/debug/mesh-llm setup --yes --no-service
```
2. 公開メッシュに参加します。※未検証
```bash
./target/debug/mesh-llm serve --auto
```
3. 招待トークンで参加します。※未検証
```bash
./target/debug/mesh-llm serve --join '<token>'
```

### モデル提供（serve）

1. 1モデルのみ単体で出します（デバッグ・開発用）。※未検証
```bash
./target/debug/mesh-llm serve --local-model-only --gguf /models/model.gguf --port 9337
```
`--gguf`・`--model`には絶対パスを渡し、シンボリックリンクは使いません。カタログ名はこの形では動きません。
2. LAN公開します。※未検証
```bash
./target/debug/mesh-llm serve --auto --listen-all --port 9337 --console 3131
```
`--listen-all`は認証なしで全インターフェースに公開します。信頼できるLANでのみ使います。
3. サーバ用途でWeb UIを隠すときは次を付けます。
```bash
./target/debug/mesh-llm serve --auto --headless
```

## macOS

### ビルド

1. 必要なものを用意します。`just`・`cmake`・Rust・`sccache`・lld・Node.js 24・`pnpm 10`です。Apple Siliconを使います。Metalは自動です。
2. 初回だけ次を実行します。
```bash
just bootstrap-build-tools
```
リンカの診断が出たら `brew install lld` を使います。
3. ビルドします。
```bash
just build
```
4. sccacheを入れられない環境では、そのシェルで次を設定します。
```bash
export RUSTC_WRAPPER=""
```

### 参加（join）

1. セットアップを仕上げます。
```bash
./target/debug/mesh-llm setup
```
実験用にサービス登録を省くときは次を使います。
```bash
./target/debug/mesh-llm setup --yes --no-service
```
2. 公開メッシュに参加します。※未検証
```bash
./target/debug/mesh-llm serve --auto
```
3. 招待トークンで参加します。※未検証
```bash
./target/debug/mesh-llm serve --join '<token>'
```

### モデル提供（serve）

1. 1モデルのみ単体で出します。※未検証
```bash
./target/debug/mesh-llm serve --local-model-only --gguf /models/model.gguf --port 9337
```
絶対パス・シンボリックリンク禁止・カタログ名不可の注意はLinuxと同じです。
2. LAN公開します。※未検証
```bash
./target/debug/mesh-llm serve --auto --listen-all --port 9337 --console 3131
```
信頼できるLANでのみ使います。
3. サーバ用途でWeb UIを隠すときは次を付けます。
```bash
./target/debug/mesh-llm serve --auto --headless
```

## Windows

### ビルド

1. PowerShellを使います。必要なものは`just`・`cmake`・Rust（MSVC・C++ワークロード付きBuild Tools）・`sccache`・lld系・Node.js 24・`pnpm 10`です。
2. 初回だけ次を実行します。
```powershell
just bootstrap-build-tools
```
3. ビルドします。
```powershell
just build
```
4. sccacheを入れられない環境では `just bootstrap-build-tools` で導入します（`$env:RUSTC_WRAPPER = ""` はPowerShell 7.4以前では回避策になりません）。

### 参加（join）

1. セットアップを仕上げます。
```powershell
.\target\debug\mesh-llm.exe setup
```
実験用にサービス登録を省くときは次を使います。
```powershell
.\target\debug\mesh-llm.exe setup --yes --no-service
```
2. 公開メッシュに参加します。※未検証
```powershell
.\target\debug\mesh-llm.exe serve --auto
```
3. 招待トークンで参加します。※未検証
```powershell
.\target\debug\mesh-llm.exe serve --join "<token>"
```

### モデル提供（serve）

1. 1モデルのみ単体で出します。※未検証
```powershell
.\target\debug\mesh-llm.exe serve --local-model-only --model "C:/path/to/model.gguf"
```
ローカル`.gguf`の絶対パスが必須です。カタログとHugging Face参照はこの形では使いません。
2. LAN公開はLinuxと同じ `--listen-all --port 9337 --console 3131` を付けます。信頼できるLANでのみ使います。※未検証
3. WSL2を使う代替構成もあります。WSL2側ではLinux手順で `just build`・`setup`・`serve` を使います。※未検証

## 動作確認

1. 管理APIとモデル一覧を見ます。
```bash
curl -fsS http://127.0.0.1:3131/api/status
curl -fsS http://127.0.0.1:9337/v1/models
```
※`--local-model-only` では `/api/status` が通らないため、`/v1/models` で確認します。
2. 小さなチャット完了を投げます。使うモデルIDは上の一覧の値に置き換えます。
```bash
curl http://localhost:9337/v1/chat/completions -H "Content-Type: application/json" -d '{"model":"<model-id>","messages":[{"role":"user","content":"hello"}]}'
```

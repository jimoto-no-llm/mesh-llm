<p align="center">
  <img src="mesh/docs/mesh-llm-wordmark.png" alt="Mesh LLM" width="420">
</p>

![Mesh LLM web console](mesh.png)

> **地元フォーク（jimoto-no-llm/mesh-llm）です。**
> 上流の [Mesh-LLM/mesh-llm](https://github.com/Mesh-LLM/mesh-llm) をもとに、日本語での開発と運用を前提に整備しているフォークです。プロトコルやワイヤ形式の互換は壊さない方針です。フォーク独自の案内は [JIMOTO.md](JIMOTO.md) にまとめています。

Mesh LLM は複数マシンの GPU とメモリを束ね、1つの OpenAI 互換 API（`http://localhost:9337/v1`）として公開します。まず1ノードで始め、後からノードを足せます。モデルをローカル実行するか、ピアに回すか、1台に載らないモデルを Skippy のステージ分割で動かすかはメッシュが決めます。

## Mesh と単体 Skippy

[Skippy](skippy/README.md) がモデル管理・ネイティブ推論・分割実行・OpenAI 互換サーブを担います。[Mesh](mesh/README.md) は同じ Skippy 実装のまわりにピア探索・転送・ルーティング・プラグイン・コンソールを足したものです。

ソースはこの2つのプロダクトツリーにあります。1つのルート Cargo ワークスペースとロックファイルを共有します。プロダクトのクレート・自作ドキュメント・スクリプト・評価は持ち主の側に置きます。Mesh はウェブサイトと SDK も持ちます。ルートの `docs/` は生成されたウェブサイト出力です。

素の `just`（または `just build`）は、先に単体の Skippy 製品（パッチ済み llama.cpp ランタイムと `skippy` CLI）をビルドし、次に MeshLLM ホストとコンソールをビルドします。Skippy だけなら `just skippy`、MeshLLM だけ作り直すなら `just mesh`、Skippy CLI だけなら `just skippy-cli-build` です。Skippy の個別パッケージ化したネイティブ ランタイムは `--runtime-bundle` で渡せます。[Skippy の設定](skippy/docs/CONFIGURATION.md)と[分割サーブ](skippy/docs/SKIPPY_SPLITS.md)も参照してください。

## 上流との違い

- **日本語第一のドキュメント。** この README とフォーク案内 [JIMOTO.md](JIMOTO.md) を日本語で整備します。
- **推論の高速化。** スケジューラのホットループの確保削減など、効くところから手を入れています。
- **開発ルールの整備。** Clippy の perf ゲート、ツールチェイン手順、PR テンプレートを固定しています。
- **レビュー必須の運用。** main は承認必須・squash のみ・線形履歴で保護し、ラベル体系と CODEOWNERS を備えています。
- **互換維持。** メッシュのプロトコルやプラグイン規約を壊す変更はしません。壊れる場合は事前に明示します。

## クイックスタート

このフォークはソースビルドが基本です。

```bash
git clone https://github.com/jimoto-no-llm/mesh-llm
cd mesh-llm
just build
```

必要なものは `just`・`cmake`・Rust・`sccache`・高速リンカ（Linux は `mold`、macOS/Windows は lld）・Node.js 24 です。`just bootstrap-build-tools` で sccache とリンカを入れられます。sccache を入れられない環境では、そのシェルで `export RUSTC_WRAPPER=""` とします。詳しくは [CONTRIBUTING.md](CONTRIBUTING.md) をどうぞ。

セットアップを仕上げます：

```bash
mesh-llm setup
```

Windows PowerShell では `mesh-llm.exe setup` です（ネイティブ Windows の注意と、任意の WSL2 構成・複数ノード LAN クラスタは[こちら](#-windows--wsl2-トラブルシュート)）。

実行形式のインストールを後で消すときは、先に確認してから消します：

```bash
mesh-llm uninstall --dry-run
mesh-llm uninstall --yes
```

`~/.mesh-llm` の設定と ID データは、`--purge-config` を付けない限り残ります。

公開メッシュに参加してサーブを開始します：

```bash
mesh-llm serve --auto
```

この1行でバックエンド種別を選び、必要ならモデルを落とし、見つかった最良の公開メッシュに参加し、ローカル API（ポート `9337`）と Web コンソール（ポート `3131`）を起動します。

モデル一覧の確認：

```bash
curl -s http://localhost:9337/v1/models | jq ".data[].id"
```

OpenAI 互換リクエストの送信：

```bash
curl http://localhost:9337/v1/chat/completions -H "Content-Type: application/json" -d '{"model":"GLM-4.7-Flash-Q4_K_M","messages":[{"role":"user","content":"hello"}]}'
```

サーバ用途では `--headless` を付けると Web UI を隠し、`--console` ポートの管理 API だけ残します：

```bash
mesh-llm serve --auto --headless
```

## 目的別の使い方

| 目的 | コマンド | 詳しい手順 |
|---|---|---|
| 公開メッシュを試す | `mesh-llm serve --auto` | [docs/MESHES.md](mesh/docs/MESHES.md) |
| プライベートメッシュを立てる | `mesh-llm serve --model Qwen3-8B-Q4_K_M` | [docs/MESHES.md](mesh/docs/MESHES.md) |
| メッシュ通信なしで1モデルだけ出す（デバッグ・開発用） | `mesh-llm serve --local-model-only --gguf /models/model.gguf` | OpenAI API は既定で `127.0.0.1:9337`（`--port` と `--listen-all` で変更可） |
| 自分のメッシュを公開する | `mesh-llm serve --model Qwen3-8B-Q4_K_M --publish` | [docs/MESHES.md](mesh/docs/MESHES.md) |
| 招待トークンで参加する | `mesh-llm serve --join <token>` | [docs/MESHES.md](mesh/docs/MESHES.md) |
| API 専用クライアントで動く | `mesh-llm client --auto` | [docs/MESHES.md](mesh/docs/MESHES.md) |
| 大きいモデルを分割で動かす | `mesh-llm serve --model hf://meshllm/<repo>@<rev> --split` | [docs/SKIPPY_SPLITS.md](skippy/docs/SKIPPY_SPLITS.md) |
| DeepSeek V4 などの ds4 モデルを DwarfStar で動かす | `mesh-llm plugins install Mesh-LLM/ds4-plugin` | [セットアップと設定](mesh/docs/plugins/dwarfstar.md) |
| Flash-MoE の SSD バックエンドを付ける | `[[plugin]] name = "flash-moe"` 付きで `mesh-llm serve` | [docs/plugins/flash-moe.md](mesh/docs/plugins/flash-moe.md) |
| 1プロンプトをメッシュ内全モデルに振る | モデルに `mesh` を指定 | [docs/design/MOA_GATEWAY.md](mesh/docs/design/MOA_GATEWAY.md) |
| Goose・OpenCode・Claude Code・Pi をつなぐ | `mesh-llm goose`・`mesh-llm opencode`・`mesh-llm claude`・`mesh-llm pi` | [docs/AGENTS.md](mesh/docs/AGENTS.md) |
| ビルド・開発に参加する | `just build` | [CONTRIBUTING.md](CONTRIBUTING.md) |

## 仕組み

- **載るものは単機優先。** 1ノードでモデル全体を載せられるなら、ステージ間通信なしでローカルにサーブします。
- **メッシュルーティング。** 全ノードが同じ `/v1` API を出します。リクエストは `model` 欄で、そのモデルを出せるピアへ振られます。
- **暗号化されたピア転送。** Mesh ノード間は QUIC でエンドツーエンド暗号化します（推論の要求・応答・分割モデルの活性値ふくむ）。Iroh リレーは暗号化パケットを中身を読まずに運びます。
- **オーナー管理面。** 運用者の設定・棚卸し操作は、明示的な立上げ付きの追加 `mesh-llm-control/1` レーンを使います。公開メッシュ参加・ゴシップ・ルーティング・推論は、混在バージョン互換のため公開メッシュ面に残します。
- **Skippy ステージ分割。** 大きい密モデルはパッケージ裏付けの層ステージとして読めます。コーディネータが連続した層範囲を割り当て、下流ステージから先に起動し、準備ができてから stage-0 ルートを公開します。
- **層パッケージ。** パッケージリポジトリは `model-package.json` と GGUF 断片を持ち、ピアは担当ステージに要る分だけ取ります。
- **公開探索。** 公開メッシュは Nostr 探索で宣伝します。プライベートメッシュは招待トークン方式のままです。

運用の詳しい手順は [docs/USAGE.md](mesh/docs/USAGE.md) へ。全 CLI コマンドとスイッチは [docs/CLI.md](mesh/docs/CLI.md) です。

### ローカル単体サーブ

> **デバッグ・開発用の口です。** `--local-model-only` は OpenAI フロントエンドを1つのローカル Skippy に直結し、メッシュ通信をまるごと省きます。ツール呼び出しふくむ本番の OpenAI クライアント向けは、ホストランタイムの正規化が入る通常のメッシュ推論ポートが正規の経路です。メッシュに入らず軽く1プロセスで出したいときに使い、フル機能の置換にはしないでください。

1つの完全なローカルモデルを、メッシュノードにならず OpenAI API で出すときの形です：

```bash
mesh-llm serve --local-model-only --gguf /models/model.gguf --port 9337
```

特定の GGUF を出すには `--gguf <絶対パス>` です。`--model <絶対パス>` も通ります。ただし `--model <カタログ名>` はここでは動きません。カタログ解決と HF 参照解決はメッシュ側の機能で、この直結形にはありません。出るエラーも分かりにくいので注意してください。

この形では OpenAI フロントエンドと1つのローカル Skippy だけ起動します。QUIC・探索・ピア維持・分割計画・プラグイン・リリース参照・Web コンソール・管理 API は起動しません。OpenAI 口を loopback の外に出すときだけ `--listen-all` を付けます。モデル全体が検出容量（または `--max-vram`）に入らないと起動失敗します。分散サーブへのフォールバックはしません。

`--model`・`--gguf`・`--mmproj` に渡す値は絶対パスで、シンボリックリンク禁止です。Hugging Face から落としたモデルは、キャッシュの symlink を解決してから渡します。

## Mixture-of-Agents（`model: "mesh"`）— 実験的

> 実験的です。MoA ゲートウェイは新しい機能です。調整が進む間、挙動・ルーティング指針・エラー形・調整つまみは変わる可能性があります。安定した意味が必要ならモデル ID を指定してください。

`"model": "mesh"` で投げると、プロキシがメッシュ内の全モデルへ並列に振り、決定的な logic で調停して1つの OpenAI 互換応答を返します。調停器はコードで動き（モデル呼び出しを重ねません）、本当の衝突のときだけ reducer LLM に上げます。ツール呼び出しも全パイプラインを通ります。

```bash
curl http://localhost:9337/v1/chat/completions -H "Content-Type: application/json" -d '{"model":"mesh","messages":[{"role":"user","content":"What is the capital of Japan?"}]}'
```

メッシュ内に2つ以上の異なるモデルが必要です。仕組み・調停則・調整つまみは [docs/design/MOA_GATEWAY.md](mesh/docs/design/MOA_GATEWAY.md) です。

## 対応モデルファミリ

Skippy ランタイムは llama.cpp とのファミリ parity を、査読済み GGUF 代表で追っています。現状の査読済み対応は P0/P1 で72行、parity 台帳全体では89行が certified で、Qwen・Llama・Gemma・Mistral・DeepSeek・GLM・MiniMax・Phi・Granite・Hunyuan・EXAONE・Cohere・Falcon・RWKV などをふくみます。

分割マルチモーダルは Qwen2-VL・Qwen3-VL・Qwen3-VL-MoE・HunyuanOCR/Hunyuan-VL・DeepSeek-OCR が実 GGUF＋projector fixture で certified です。DeepSeek3 と EXAONE-MoE はフル GGUF が手元の基準機に載らないためパッケージ裏付けステージを使います。

全体の artifact・分割・wire dtype・キャッシュ方針・例外表は [docs/skippy/FAMILY_STATUS.md](skippy/docs/FAMILY_STATUS.md) です。残りの llama.cpp parity キューは [docs/skippy/LLAMA_PARITY.md](skippy/docs/LLAMA_PARITY.md) です。

## インストールとビルドの注意

タグ付きリリースは macOS バンドルと、Linux CPU・Linux ARM64 CPU・Linux ARM64 CUDA・Linux CUDA・Linux CUDA Blackwell・Linux ROCm・Linux Vulkan・Windows CPU・Windows CUDA・Windows ROCm・Windows Vulkan バンドルを出します（上流の配布物。フォーク独自のリリースは [JIMOTO.md](JIMOTO.md) の計画を見てください）。Metal は macOS 専用です。全種別とも、OS/アーキテクチャごとの素のホスト1つ＋版付きネイティブ ランタイム1つの組合せです。Linux ARM64 CPU は `mesh-llm-aarch64-unknown-linux-gnu.tar.gz`、Linux ARM64 CUDA は `mesh-llm-aarch64-unknown-linux-gnu-cuda.tar.gz` です。`arm64` と `aarch64` は同じ64bit ARM です。可搬アーカイブはオフラインで動きます。ホストはユーザーキャッシュを見る前に隣の `native-runtimes/<runtime-id>` 配下を探します。

ソースから `just` でビルドします：

```bash
git clone https://github.com/jimoto-no-llm/mesh-llm
cd mesh-llm
just build
```

ソースビルドには `just`・`cmake`・Rust・`sccache`・高速リンカ（Linux は `mold`、macOS/Windows は lld）・Node.js 24＋npm が要ります。Cargo は素の `cargo build` ふくめ、リポジトリ設定の既定を使います。導入コマンドとリンカのフォールバックは [CONTRIBUTING.md](CONTRIBUTING.md) です。リリース境界だけ試すなら、素のホストとランタイム1つをビルドします：

```bash
just release-host-build
just release-runtime-build metal
MESH_LLM_NATIVE_RUNTIME_BUNDLE_DIR="$PWD/dist/native-runtimes" MESH_LLM_NATIVE_RUNTIME_CACHE_DIR="$(mktemp -d)" ./target/release/mesh-llm runtime list
```

CUDA ランタイムには `nvcc`、ROCm には ROCm/HIP、Vulkan には開発ファイルと `glslc` が要ります。[docs/design/NATIVE_RUNTIMES.md](skippy/docs/design/NATIVE_RUNTIMES.md) に manifest・探索・互換契約があります。

配布 `mesh-llm` 実行形式は、来歴と admission 強化のための組込みリリース認証を使います。SDK など他の成果物には適用されず、実行時 integrity 証明でもありません。詳しい検証手順は上流 README の該当節と `cargo run -p xtask -- release-attestation` のヘルプを見てください。

## Windows と WSL2 のトラブルシュート

### ネイティブ Windows（NVIDIA）で動かす

ネイティブ Windows CUDA は動きます。CUDA 13 系ドライバふくめ、GPU 検出（`mesh-llm gpus`）とフル速度 CUDA 推論は driver 610.74（CUDA UMD 13.3）＋RTX 4070 Ti で確認済みです。`mesh-llm` が CUDA 13 ドライバで `0 GPUs` と出すときに WSL2 へ逃がす旧アドバイスは、cuda12-runtime 互換修正より前の話で、今のリリースには当てはまりません。

v0.76.0-rc8 時点で、配布・読込みのバグ3件がそのままの素通しを塞いでいます。修正が出るまで、この手順なら端まで通ります（番号は上流 Issues #1510・#1511・#1512 に対応します）：

1. **プレリリースを入れる** — 安定版 v0.75.1 の Windows バンドルは `install.ps1` 検証で落ちます：

   ```powershell
   irm https://raw.githubusercontent.com/Mesh-LLM/mesh-llm/main/install.ps1 -OutFile install.ps1
   ./install.ps1 -PreRelease
   ```

2. **製品バンドルから CUDA ランタイムを入れる** — `mesh-llm runtime install cuda` はリリース manifest に windows/x86_64 ランタイムを見つけません。使っている版の `mesh-llm-x86_64-pc-windows-msvc-cuda-12.zip`（RTX 50番台など Blackwell GPU は CUDA 13 ドライバの要る `-cuda-13.zip`）を上流の[リリース頁](https://github.com/Mesh-LLM/mesh-llm/releases)から落として展開し、次を実行します：

   ```powershell
   mesh-llm runtime install --bundle-dir "<展開先>/mesh-bundle" cuda
   ```

3. **サーブ前にランタイムの `lib` を `PATH` に入れる** — ランタイム DLL がエラー126で読めません。手順2のバンドルに合わせ、`<cuda>` を `cuda12` か `cuda13` に置き換えます：

   ```powershell
   $env:PATH = "$env:LOCALAPPDATA/mesh-llm/native-runtimes/<version>/meshllm-native-runtime-windows-x86_64-<cuda>/lib;" + $env:PATH
   mesh-llm serve --local-model-only --model "C:/path/to/model.gguf"
   ```

ネイティブ Windows の覚書：

- `--local-model-only` はローカル `.gguf` の絶対パス必須です。カタログと Hugging Face 参照はこの形では弾きます。
- モデルは `%LOCALAPPDATA%/huggingface/hub` に落ちます（Rust の `hf-hub` 流儀）。Python 系の `~/.cache/huggingface` とは別キャッシュです。
- `mesh-llm.exe` は未署名のため、手動起動で SmartScreen が出ることがあります。

### WSL2 で動かす（代替案）

Linux CUDA ランタイム派や、複数ノード LAN 構成をやりたいなら WSL2 も堅い選択肢です。

#### 1. WSL2 内に CUDA 13.0 Toolkit を入れる
WSL2 の Ubuntu 端末で `cuda-toolkit-13-0` を入れます：

```bash
wget https://developer.download.nvidia.com/compute/cuda/repos/wsl-ubuntu/x86_64/cuda-wsl-ubuntu.pin
sudo mv cuda-wsl-ubuntu.pin /etc/apt/preferences.d/cuda-repository-pin-600
wget https://developer.download.nvidia.com/compute/cuda/13.0.0/local_installers/cuda-repo-wsl-ubuntu-13-0-local_13.0.0-1_amd64.deb
sudo dpkg -i cuda-repo-wsl-ubuntu-13-0-local_13.0.0-1_amd64.deb
sudo cp /var/cuda-repo-wsl-ubuntu-13-0-local/cuda-*-keyring.gpg /usr/share/keyrings/
sudo apt-get update && sudo apt-get -y install cuda-toolkit-13-0
```

#### 2. WSL2 ミラーモード向けにファイアウォールを整える
WSL2 の `.wslconfig` で `networkingMode=mirrored` のとき、Windows 11 側の Hyper-V VM ファイアウォールが別にあり、サードパーティ製セキュリティソフト入りだと既定で inbound を塞ぎます。
Windows 側で `C:/Users/<username>/.wslconfig` に次を書きます：

```ini
[wsl2]
networkingMode=mirrored
autoProxy=true
```

PowerShell で `wsl --shutdown` して入れ直し、**管理者 PowerShell** で次を実行します：

```powershell
$vmCreatorId = '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}'
Set-NetFirewallHyperVVMSetting -Name $vmCreatorId -DefaultInboundAction Allow
New-NetFirewallRule -DisplayName "MeshLLM TCP In" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 3131,9337 -Profile Any
New-NetFirewallRule -DisplayName "MeshLLM UDP In" -Direction Inbound -Action Allow -Protocol UDP -LocalPort 9337,5353 -Profile Any
```

#### 3. 直接 LAN 読み用にモデルパスを揃える
ワーカーが GGUF 断片をネットワーク越しでなく手元の NVMe/SSD から直接読むため、`--gguf` のパス文字列を全ノードで揃えます：

```bash
sudo mkdir -p /mnt/d/models/
sudo mount --bind /path/to/local/fast/nvme/ /mnt/d/models/
mesh-llm serve --gguf "/mnt/d/models/model.gguf" --listen-all --auto
```

## ドキュメントハブ

| 資料 | 用途 |
|---|---|
| [docs/MESHES.md](mesh/docs/MESHES.md) | プライベートメッシュ・公開探索・公開・招待トークン・API専用クライアント |
| [docs/SKIPPY_SPLITS.md](skippy/docs/SKIPPY_SPLITS.md) | パッケージ裏付け Skippy ステージ分割で大きいモデルを動かす |
| [docs/LAYER_PACKAGE_REPOS.md](skippy/docs/LAYER_PACKAGE_REPOS.md) | 層パッケージリポジトリの寄稿と公開 |
| [docs/AGENTS.md](mesh/docs/AGENTS.md) | Goose・Claude Code・OpenCode・Pi・curl・blackboard |
| [docs/EXO_COMPARISON.md](mesh/docs/EXO_COMPARISON.md) | Exo との冷静な比較 |
| [docs/CLI.md](mesh/docs/CLI.md) | コマンドリファレンスと JSON 自動化 |
| [docs/USAGE.md](mesh/docs/USAGE.md) | 長めの運用手順・ランタイム制御・オーナー管理フロー |
| [docs/design/TESTING.md](mesh/docs/design/TESTING.md) | テスト手順・混在バージョン QA・リモート配備チェック |
| [docs/plugins/flash-moe.md](mesh/docs/plugins/flash-moe.md) | 任意の Flash-MoE SSD エキスパート streaming バックエンド設定 |
| [docs/plugins/dwarfstar.md](mesh/docs/plugins/dwarfstar.md) | Apple Silicon 向け DwarfStar（ds4）代替エンジン設定 |
| [docs/skippy/FAMILY_STATUS.md](skippy/docs/FAMILY_STATUS.md) | 認定済み Skippy モデルファミリ状況 |
| [JIMOTO.md](JIMOTO.md) | フォーク独自の案内（スマホ・リリース・運用） |

## CI 基盤

上流にならい、信頼済み `main` の Linux ジョブは Depot のマネージドランナーを使えます（リポジトリのランナー方針経由）。PR はキャッシュ隔離と保護ワークフローゲートが通るまで GitHub ホステッドのままです。詳しくは [`ci/DEPOT_MIGRATION.md`](ci/DEPOT_MIGRATION.md) です。

## コミュニティ

Mesh LLM は実験的な分散システムソフトです。バグ報告には、実行コマンド・基盤とバックエンド種別・あれば `/api/status` 出力・プライベート/公開/`--auto`参加の別を添えてください。報告先は[フォークの Issues](https://github.com/jimoto-no-llm/mesh-llm/issues) です。上流への報告は[上流リポジトリ](https://github.com/Mesh-LLM/mesh-llm)の流儀に従ってください。

## 謝辞

このフォークは上流の [Mesh-LLM/mesh-llm](https://github.com/Mesh-LLM/mesh-llm) の成果の上にあります。上流プロジェクトと支援者に感謝します。



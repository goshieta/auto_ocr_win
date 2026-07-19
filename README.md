# オンデバイスOCRバックグラウンド画面監視システム

Windows 11上で完全バックグラウンド動作し、定期的に画面キャプチャを取得・オンデバイスOCR（文字認識）を行い、抽出したテキストデータを指定のサーバーへ非同期送信するRust製軽量サービスです。

## 特徴

- **低負荷**: Windows.Media.Ocr APIを使用しNPU/GPUアクセラレーションで省電力動作
- **ディスク寿命保護（I/Oレス）**: 画面キャプチャからOCR、送信までの全プロセスをメモリ（RAM）上の`Vec<u8>`で完結、SSDに一切書き込まない
- **差分検知**: xxHash (XXH3) で画面変化がない時はOCR処理をスキップし負荷を最小化
- **バックグラウンド動作**: `#![windows_subsystem = "windows"]` によりコンソールウィンドウなしで起動

## 動作環境

- **OS**: Windows 11 (64-bit)
- **CPU**: Intel Core Ultra 7 255H 以上 (NPU/GPUアクセラレーション対応推奨)
- **RAM**: 16 GB 以上
- **OCR言語パック**: Windowsの設定からOCR対象言語の言語パックがインストールされていること

## 技術スタック

| 役割                 | 採用技術                                      |
| -------------------- | --------------------------------------------- |
| 言語・ランタイム     | Rust (v1.75+) + `tokio`                       |
| バックグラウンド制御 | `#![windows_subsystem = "windows"]`           |
| 画面キャプチャ       | GDI API (`BitBlt` + `GetDIBits`) — メモリ完結 |
| OCRエンジン          | `Windows.Media.Ocr.OcrEngine` (WinRT)         |
| 差分検知             | `xxhash-rust` (XXH3, 64bit)                   |
| HTTPクライアント     | `reqwest` + `serde_json`                      |
| 設定管理             | `dotenvy` (.envファイル)                      |
| 二重起動防止         | Named Mutex (`CreateMutexW`)                  |

## プロジェクト構成

```
auto_ocr_win/
├── Cargo.toml          # プロジェクト定義・依存クレート
├── .env.example        # 環境変数サンプル
├── README.md           # 本ドキュメント
└── src/
    ├── main.rs         # エントリポイント・メインループ
    ├── config.rs       # .env ファイル読み込み
    ├── capture.rs      # GDI 画面キャプチャ（メモリ完結）
    ├── ocr.rs          # Windows.Media.Ocr 連携
    ├── sender.rs       # HTTP POST 送信
    └── startup.rs      # スタートアップ自動起動登録
```

## セットアップ

### 1. Rustのインストール

```powershell
# RustupでRustをインストール（未インストールの場合）
# https://rustup.rs からインストーラをダウンロードするか、以下を実行:
winget install Rustlang.Rustup
```

### 2. リポジトリのクローンとビルド

```powershell
git clone <repository-url> auto_ocr_win
cd auto_ocr_win
cargo build --release
```

### 3. 設定ファイル (.env) の作成

`.env.example` をコピーして `.env` を作成し、送信先URLを設定します。

```powershell
copy .env.example .env
```

`.env` ファイルの内容:

```env
# OCR Server URL（必須）
# OCR結果の送信先URL
OCR_SERVER_URL=http://localhost:8080/api/ocr

# HTTPリクエストのタイムアウト秒数（任意、デフォルト: 3）
OCR_REQUEST_TIMEOUT_SECS=3
```

| フィールド                 | 必須     | デフォルト | 説明                                   |
| -------------------------- | -------- | ---------- | -------------------------------------- |
| `OCR_SERVER_URL`           | **必須** | —          | OCR結果をPOST送信するエンドポイントURL |
| `OCR_REQUEST_TIMEOUT_SECS` | 任意     | `3`        | HTTPリクエストのタイムアウト秒数       |

### 4. 実行

```powershell
# リリースビルドを実行（バックグラウンド起動、コンソールなし）
.\target\release\auto_ocr_win.exe
```

起動時に以下の処理が自動で行われます:

- `.env` ファイルの読み込み
- 二重起動チェック
- スタートアップ自動起動のレジストリ登録（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\GoshietaOcrService`）
- 以後5秒間隔で画面キャプチャ→差分検知→OCR→送信のループを開始

## 処理フロー

```
[起動]
  ├── .env ファイル読み込み
  ├── 二重起動チェック (Named Mutex)
  ├── スタートアップ自動起動登録 (レジストリ)
  └── メインループ (5秒間隔)
       │
       ├── 1. GDI BitBlt で画面キャプチャ (メモリ上)
       ├── 2. XXH3 ハッシュを計算し前回と比較
       │    ├── 変化なし → スキップ
       │    └── 変化あり → OCRへ
       ├── 3. Windows.Media.Ocr でテキスト抽出
       ├── 4. テキストが空でなければ JSON にパッキング
       └── 5. HTTP POST で送信 (並列・非同期)
            ├── リトライバッファ内の全ペイロードを並列送信
            ├── 現在のペイロードも同時に並列送信
            ├── 成功 → 完了
            └── 失敗 → リトライバッファに再格納 (最大10件)
```

## 送信データ仕様

- **Method**: `POST`
- **Content-Type**: `application/json`

```json
{
  "source": "ocr",
  "content": "OCRで読み取られたテキスト\n2行目のテキスト..."
}
```

## エラーハンドリング

- **設定ファイル不在・読み込み失敗**: エラーログを出力しアプリケーションを終了
- **サーバー不通・ネットワーク切断**: 送信失敗したペイロードはメモリ上のリトライバッファ（最大10件）に保持し、次回送信成功時にフラッシュ
- **HTTP タイムアウト**: デフォルト3秒（`.env` で変更可）
- **メモリ使用量**: 稼働中50MB以下を目標
- **ログ**: `%TEMP%\auto_ocr_win.log` に出力

## アンインストール

スタートアップ登録を解除するには、レジストリエディタで以下を削除してください:

```
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run\GoshietaOcrService
```

または管理者PowerShellで:

```powershell
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "GoshietaOcrService"
```

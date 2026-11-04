#[macro_use]
pub mod shiori_log;
pub mod error_codes;
pub mod hub;
mod audio;
mod self_test;
mod notify;
mod pet;
mod sidebar_layout;
mod claude_stats;
mod claude_status;
mod db;
pub mod eject;
pub mod diagnose;
mod repair;
#[cfg(windows)]
mod disk_io;
mod llm_client;
mod models;
mod piper_client;
mod prompts;
pub mod rag_client;
pub mod shared_daemon;
mod handoff_memory;
mod source_path;
mod shelf;
mod system_docs;
mod system_info;
mod text_transform;
mod whisper_client;

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::Manager;

// Windowsでは、コンソールを持たないGUIサブシステムのshiori-folio.exeから
// コンソールサブシステムの子プロセス(llama-server.exe/whisper-server.exe/
// python.exe/piper-plus-cli.exe/nvidia-smi等)をCommand::spawn()すると、
// デフォルトでは子プロセス用に新しいコンソール窓が生成され、一瞬黒い窓が
// 表示されてしまう(2026-08-14、外付けSSD運用で「コマンドプロンプトが
// 繰り返し開いては閉じる」不具合として実機発覚。原因はget_system_infoが
// 1〜2秒間隔でnvidia-smiを呼ぶたびに窓が生成されていたことだった)。
// CREATE_NO_WINDOW(0x08000000)を子プロセスの起動フラグに立てることで
// この窓の生成自体を止める。Windows専用のプロセス起動オプションのため、
// 他OSでは何もしない。
#[cfg(windows)]
pub fn no_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn no_console_window(_cmd: &mut Command) {}

// config.json のうちバックエンド起動に必要な部分のみを読む
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LlmConfig {
    model_path: String,
    port: u16,
    #[serde(default)]
    context_size: Option<u32>,
    #[serde(default)]
    gpu_layers: Option<u32>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EmbeddingConfig {
    model_path: String,
    port: u16,
    // 【2026-09-10追加】未指定時はllama-serverのデフォルト並列スロット数(4)で
    // --ctx-sizeが均等分割され実効512トークンになり、約500文字を超える日本語
    // チャンクの埋め込みで500エラーが発生し、それを起点にRAGサーバーの起動時
    // 全件再インデックス(_initial_index_sync)ごと巻き込んでクラッシュする
    // 不具合があった(search_libraryがタイムアウトする形で発覚)。
    // nomic-embed-textの訓練時コンテキスト(2048)を超えても無意味なため
    // デフォルトは2048(build_embedding_command側で--parallel 1も指定し、
    // 1スロットで全量を使う)。llm.contextSizeと同じパターンでconfig化し、
    // 明示的に指定する。
    #[serde(default)]
    context_size: Option<u32>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SttConfig {
    model_path: String,
    port: u16,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RagConfig {
    port: u16,
    // 常時軽量検索(自発的想起)の類似度距離しきい値。実データ移行に伴い
    // 再調整が必要になる想定のため設定変更UIから調整できるようにしている。
    #[serde(default = "default_passive_recall_threshold")]
    passive_recall_threshold: f64,
}

fn default_passive_recall_threshold() -> f64 {
    0.55
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TtsConfig {
    model_path: String,
    config_path: String,
    #[serde(default = "default_length_scale")]
    length_scale: f64,
    #[serde(default = "default_noise_scale")]
    noise_scale: f64,
    #[serde(default = "default_noise_w")]
    noise_w: f64,
}

fn default_length_scale() -> f64 {
    1.0
}
fn default_noise_scale() -> f64 {
    0.667
}
fn default_noise_w() -> f64 {
    0.8
}

// ヘッダーの日付表示(2026-08-12、UI/UX改善指示書2章)。既存のconfig.jsonに
// キー自体が無くても壊れないよう、struct全体・各フィールドにデフォルトを持たせる。
// std::default::Defaultはserdeのフィールドデフォルトとは独立のため、
// 意図した初期値(年/月/日=表示・曜日=非表示)に合わせて手動でimplする。
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UiConfig {
    #[serde(default = "default_true")]
    show_year: bool,
    #[serde(default = "default_true")]
    show_month: bool,
    #[serde(default = "default_true")]
    show_day: bool,
    #[serde(default)]
    show_weekday: bool,
    // 時刻の秒の表示(詩織Ver3.9)。
    #[serde(default)]
    show_seconds: bool,
    // 水紋と星の動きを抑えるか(詩織Ver6.0の段階2)。未設定ならOSの「視差効果を減らす」に従う
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reduce_motion: Option<bool>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_year: true,
            show_month: true,
            show_day: true,
            show_weekday: false,
            show_seconds: false,
            reduce_motion: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Clone)]
struct AppConfig {
    llm: LlmConfig,
    embedding: EmbeddingConfig,
    stt: SttConfig,
    rag: RagConfig,
    tts: TtsConfig,
    hotkey: String,
    #[serde(default)]
    ui: UiConfig,
    #[serde(default)]
    mode: AppMode,
    #[serde(default)]
    notify: NotifyConfig,
    #[serde(default)]
    pet: PetConfig,
}

/// ペットの形の設定(詩織Ver6.0の段階4)。ホットキーの変更は、録音のホットキーと同じく次の起動から。
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PetConfig {
    #[serde(default = "default_pet_hotkey")]
    hotkey: String,
    /// 最後の入力から、眠るまでの分
    #[serde(default = "default_sleep_minutes")]
    sleep_minutes: u32,
    /// 本と小道具の枠の大きさ(%)
    #[serde(default = "default_panel_percent")]
    panel_percent: u32,
}

fn default_panel_percent() -> u32 {
    100
}

fn default_pet_hotkey() -> String {
    "ctrl+alt+d".to_string()
}

fn default_sleep_minutes() -> u32 {
    5
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            hotkey: default_pet_hotkey(),
            sleep_minutes: default_sleep_minutes(),
            panel_percent: default_panel_percent(),
        }
    }
}

/// 本と小道具の枠の大きさ(%、本を開くたびに読み直す)
fn pet_panel_percent() -> u32 {
    app_config().map(|c| c.pet.panel_percent).unwrap_or_else(|_| default_panel_percent())
}

/// 眠るまでの分(ペットの形にするたびに読み直す)
fn pet_sleep_minutes() -> u32 {
    app_config().map(|c| c.pet.sleep_minutes).unwrap_or_else(|_| default_sleep_minutes())
}

/// 知らせの設定(詩織Ver6.0の段階3)。音を鳴らすかを、知らせの種類ごとに持つ。
#[derive(Deserialize, Serialize, Clone, Default)]
struct NotifyConfig {
    #[serde(default)]
    sound: NotifySound,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
struct NotifySound {
    #[serde(default = "default_true")]
    timer: bool,
    #[serde(default)]
    claude: bool,
    #[serde(default)]
    pending: bool,
    #[serde(default)]
    extension: bool,
}

/// 既定はタイマーの終わりだけ鳴らす
impl Default for NotifySound {
    fn default() -> Self {
        Self { timer: true, claude: false, pending: false, extension: false }
    }
}

/// 詩織の使い方(詩織Ver4.0)。外部AIモードは、Claudeなど外部のAIと詩織のlibraryを
/// 使うためのモードで、VRAMを最も使う会話用LLMを起動しない。検索に要る埋め込みと
/// RAGは起動する(MCPの`search_library`が使うのもこの2つだけ)。config.jsonに保存し、
/// 再起動しても保たれる。
#[derive(Deserialize, Serialize, Clone, Copy, Default, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
enum AppMode {
    #[default]
    Conversation,
    External,
}

const EXTERNAL_MODE_MESSAGE: &str = "外部AIモードでは、詩織との会話は使えません。ヘッダーで会話モードに切り替えてください。";

fn llm_skipped_status(config: &AppConfig) -> ServiceStatus {
    ServiceStatus {
        name: "llm".into(),
        port: config.llm.port,
        started: false,
        healthy: false,
        error: None,
        skipped: true,
    }
}

fn app_config() -> Result<AppConfig, String> {
    load_config(&project_root())
}

fn now_iso8601() -> String {
    chrono::Local::now().to_rfc3339()
}

fn log_conversation(mode: &str, user_text: &str, assistant_text: &str) -> Result<(), String> {
    let conn = db::open(&project_root())?;
    let now = now_iso8601();
    conn.execute(
        "INSERT INTO conversations (mode, role, content, created_at) VALUES (?1, 'user', ?2, ?3)",
        rusqlite::params![mode, user_text, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO conversations (mode, role, content, created_at) VALUES (?1, 'assistant', ?2, ?3)",
        rusqlite::params![mode, assistant_text, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// TTS再生失敗(既知の不具合により発生することがある)は、これまで警告ログに
// 留めるだけで記録が残らなかった。デバッグ画面(優先度: v1.0スコープ機能2)
// から確認できるようにSQLiteに記録する。記録自体の失敗は握って警告ログのみに
// 留める(会話フロー継続を優先する既存方針を崩さないため)。
fn log_tts_failure(error: &str) {
    let record = || -> Result<(), String> {
        let conn = db::open(&project_root())?;
        conn.execute(
            "INSERT INTO tts_failures (error, created_at) VALUES (?1, ?2)",
            rusqlite::params![error, now_iso8601()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    };
    if let Err(e) = record() {
        log_error!("TTS失敗ログの記録に失敗しました: {e}");
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsFailureDto {
    error: String,
    created_at: String,
}

// デバッグ画面向け。直近の失敗を新しい順に返す。
#[tauri::command]
async fn get_tts_failures() -> Result<Vec<TtsFailureDto>, String> {
    run_blocking(move || get_tts_failures_blocking()).await
}

fn get_tts_failures_blocking() -> Result<Vec<TtsFailureDto>, String> {
    let conn = db::open(&project_root())?;
    let mut stmt = conn
        .prepare("SELECT error, created_at FROM tts_failures ORDER BY id DESC LIMIT 20")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TtsFailureDto {
                error: row.get(0)?,
                created_at: row.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
pub struct ServiceStatus {
    name: String,
    port: u16,
    started: bool,
    healthy: bool,
    error: Option<String>,
    // 外部AIモード(詩織Ver4.0)で、意図して起動しなかった。失敗とは区別する。
    skipped: bool,
}

// 起動した子プロセスを保持する。Drop してもプロセスは止まらないが、
// ハンドルを持たないと再起動や終了処理から参照できなくなるため保持する。
#[derive(Default)]
struct BackendProcesses {
    llm: Option<Child>,
    embedding: Option<Child>,
    rag: Option<Child>,
}

struct BackendState(Mutex<BackendProcesses>);

// third_party/ 配下のビルド済みexeを使う（開発時はsrc-tauriの親をプロジェクトルートとする）
// third_party/models/services/data/config.jsonが並ぶ「ポータブルフォルダ」を、
// 実行ファイル自身の場所を基準に実行時に求める。以前はenv!("CARGO_MANIFEST_DIR")で
// ビルド時の絶対パスを焼き込んでいたため、.appを別の場所(USB上の別ドライブや
// 別フォルダ)に移動すると見つからなくなる問題があった(2026-08-13、Mac移植の
// 過程で発覚)。
//
// 実行ファイルの位置から「固定の階層数だけ上へ」という決め打ちにすると、
// 開発時(cargo run、<root>/src-tauri/target/debug/<exe>)・リリース版(Windows/Linux、
// <root>/src-tauri/target/release/<exe>)・macOSの.appバンドル(<root>/<name>.app/
// Contents/MacOS/<exe>、third_party/models等と同じ階層に.appを置く配布形態を想定)
// で必要な階層数がそれぞれ異なるだけでなく、`cargo test`のテストバイナリ
// (target/debug/deps/配下、通常の実行ファイルより1階層深い)でも簡単に狂う。
// そこで、実行ファイルの場所から上に向かって`config.json`が見つかるところまで
// 辿る方式にする(git/npmがリポジトリルートを探すのと同じ考え方)。
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .expect("実行ファイル自身のパス取得に失敗")
        .parent()
        .expect("実行ファイルの親ディレクトリ取得に失敗")
        .to_path_buf()
}

pub fn project_root() -> PathBuf {
    let mut dir = exe_dir();
    loop {
        if dir.join("config.json").is_file() {
            return dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return exe_dir(),
        }
    }
}

// third_party配下のエンジンビルド出力(llama-server/whisper-server等)の実行ファイルパスを解決する。
// - 拡張子: Windowsのみ`.exe`を付与する
// - 出力先ディレクトリ: CMakeのジェネレータによって`bin/Release/`配下(Visual Studio等の
//   マルチコンフィグ)と`bin/`直下(Ninja/Unix Makefiles等のシングルコンフィグ、macOSのMetal
//   ビルドはこちら)のどちらになるか変わるため、両方試して実在する方を採用する。
// どちらも存在しない場合(未ビルド等)は`bin/Release/`側のパスを返す(エラーメッセージ表示用)。
// 実行ファイル名にOSごとの拡張子(Windowsのみ`.exe`)を付与する。third_party配下の
// サイドカーexeを参照する箇所(resolve_engine_exe、piper_client)で共通利用する
// (2026-08-13、`.exe`のハードコードが複数箇所に分散していたための共通化)。
pub fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

// 段階G: portable/パッケージではthird_party/のソース・ビルド中間生成物を
// 含めず、コンパイル済みのエンジンバイナリのみをbin/<os>/にフラットに
// 配置する。ここが存在すればdevツリー(third_party/.../build/bin)より
// 優先して使う(USBポータブル版の実行ファイルは常にこちらを見る)。
pub fn portable_bin_dir() -> PathBuf {
    let os_dir = if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(windows) {
        "win"
    } else {
        "linux"
    };
    project_root().join("bin").join(os_dir)
}

pub fn resolve_engine_exe(build_dir: &Path, exe_base_name: &str) -> PathBuf {
    let exe_name = exe_name(exe_base_name);

    let portable = portable_bin_dir().join(&exe_name);
    if portable.exists() {
        return portable;
    }

    let with_release = build_dir.join("bin/Release").join(&exe_name);
    if with_release.exists() {
        return with_release;
    }
    let without_release = build_dir.join("bin").join(&exe_name);
    if without_release.exists() {
        return without_release;
    }
    with_release
}

// フォルダ構成の再編(system/library分離)により、知識データ(library/)は
// project_root()(system/)の外、その兄弟ディレクトリに置かれている。
//
// 【2026-08-13追加】ポータブル版(build-portable.ps1)はlibrary/の実データを
// project_root()(portable/)直下にコピーする。当初はportable_bin_dir()等と
// 揃えず、常に「project_root()の親 + library」という開発ツリー前提の解決方法
// のままになっていたため、portable/をUSB等の別ドライブへ単体で移動すると
// (system/やlibrary/という兄弟ディレクトリが存在しなくなり)ライブラリ機能・
// メモ保存機能が壊れることを実機確認した(project_root()自体はconfig.json探索で
// 正しくportable/を指すが、その親には何も無いため)。project_root()直下に
// library/が存在する場合(ポータブル版)はそちらを優先し、無い場合(開発ツリー)は
// 従来通り親ディレクトリを見るフォールバックにする。
pub fn library_root() -> PathBuf {
    let root = project_root();
    let portable_library = root.join("library");
    if portable_library.is_dir() {
        return portable_library;
    }
    root.parent()
        .expect("system/ should have a parent directory")
        .join("library")
}

/// 知識の棚(knowledge/、Ver5.0候補)の場所。library_root()と同じ判定で、ポータブル版は
/// project_root()直下、開発ツリーは親ディレクトリのknowledge/。
pub fn shelf_root() -> PathBuf {
    let root = project_root();
    let portable_shelf = root.join("knowledge");
    if portable_shelf.is_dir() {
        return portable_shelf;
    }
    root.parent()
        .expect("system/ should have a parent directory")
        .join("knowledge")
}

pub(crate) fn load_config(root: &Path) -> Result<AppConfig, String> {
    let text = std::fs::read_to_string(root.join("config.json"))
        .map_err(|e| format!("config.jsonの読み込みに失敗: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("config.jsonの解析に失敗: {e}"))
}

// CUDA_PATH/binを見つけてPATHに追加する。未インストール環境でもCPUフォールバックできるよう
// 見つからない場合はNoneを返すだけでエラーにしない。CUDAはWindows+NVIDIA環境専用のため、
// それ以外のOSでは常にNone(探索コード自体をコンパイルしない)。
#[cfg(windows)]
fn cuda_bin_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CUDA_PATH") {
        let bin = PathBuf::from(p).join("bin");
        if bin.exists() {
            return Some(bin);
        }
    }
    let default = PathBuf::from(r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8\bin");
    default.exists().then_some(default)
}

#[cfg(not(windows))]
fn cuda_bin_dir() -> Option<PathBuf> {
    None
}

// エンジンexeのDLL探索用PATHを組み立てる。
// - ポータブル版(bin/<os>/libs-llama, bin/<os>/libs-whisper)が存在する場合はそちらを
//   最優先でPATH先頭に追加する。libs-*にはCUDA本体のDLL(cudart64_12.dll等)も
//   同梱済みの前提(build-portable.ps1参照)なので、これだけで自己完結する
//   (2026-08-13、Windows実機でPATH方式によるDLL解決を確認済み。exeとDLLを
//   同一フォルダに展開する必要はなく、libs-llama/libs-whisperで別々のまま
//   問題ない)。
// - libs_dir_nameがNone、またはポータブル版が存在しない(開発ツリーでの実行)場合は
//   従来通りcuda_bin_dir()で開発機にグローバルインストールされたCUDA Toolkitを探す。
pub fn extended_path(libs_dir_name: Option<&str>) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };

    if let Some(name) = libs_dir_name {
        let portable_libs = portable_bin_dir().join(name);
        if portable_libs.exists() {
            return format!("{}{sep}{existing}", portable_libs.display());
        }
    }

    match cuda_bin_dir() {
        Some(cuda_bin) => format!("{}{sep}{existing}", cuda_bin.display()),
        None => existing,
    }
}

// whisper-server(whisper.cpp)は非ASCIIパスのコマンドライン引数を正しく扱えないバグがあるため、
// モデルファイルのディレクトリをcwdにして相対ファイル名だけを渡す(llama-serverは問題ないが同じ方式で統一)。
// libs_dir_nameは"libs-llama"/"libs-whisper"のように呼び出し元のエンジンに応じて渡す
// (extended_path参照)。
//
// quiet_stdioは子プロセスの標準出力・標準エラー出力を破棄するかどうか。GUI
// (会話UI)は開発時にターミナルでログを見られると便利なため通常falseで呼ぶが、
// MCPサーバー(mcp_server.rs)はこの子プロセスの標準出力を継承すると、MCP
// プロトコル通信に使っている自分自身の標準出力にログが混入してJSON-RPCの
// パースが壊れる(2026-08-14、実機のstdio疎通テストで発覚)。そのためMCP
// サーバー経由での起動時はtrueを渡し、破棄する。
pub fn spawn_server(
    exe_path: &Path,
    model_path: &Path,
    extra_args: &[&str],
    libs_dir_name: &str,
    quiet_stdio: bool,
) -> std::io::Result<Child> {
    let model_dir = model_path.parent().unwrap_or_else(|| Path::new("."));
    let model_filename = model_path
        .file_name()
        .expect("model path must have a filename");

    let mut cmd = Command::new(exe_path);
    cmd.current_dir(model_dir)
        .arg("--model")
        .arg(model_filename)
        .args(extra_args)
        .env("PATH", extended_path(Some(libs_dir_name)));
    if quiet_stdio {
        // 【2026-09-10追加】stdinを明示指定しないとCommandはデフォルトで
        // 親プロセスの標準入力を継承する。MCPサーバー(mcp_server.rs)は
        // 標準入出力をClaude CodeとのJSON-RPC通信に使っているstdio型
        // サーバーのため、それを継承した子プロセスが標準入力からの読み取りで
        // 稀にブロックし、search_libraryが数分単位でタイムアウトする不具合が
        // あった(実機調査でRAGサーバー側に顕著に再現。詳細はbuild_rag_command
        // 側のコメント参照)。子サーバーはいずれも対話的な標準入力を必要と
        // しないため、常にnullにしてよい。
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    no_console_window(&mut cmd);
    cmd.spawn()
}

pub fn wait_for_health(port: u16, attempts: u32) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    for _ in 0..attempts {
        if let Ok(resp) = ureq::get(&url).timeout(Duration::from_secs(2)).call() {
            if resp.status() < 500 {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

// 起動画面(StartupScreen)に「現在何をしているか」を表示するための進捗通知。
// フロントエンドはこのイベントを購読して、各段階のラベルをそのまま表示する
// (2026-08-13、起動画面のタイミング修正に続く指示書で追加)。
fn emit_startup_stage(app: Option<&tauri::AppHandle>, label: &str) {
    use tauri::Emitter;
    if let Some(app) = app {
        let _ = app.emit("shiori:startup-stage", serde_json::json!({ "label": label }));
    }
}

// 起動画面(StartupScreen)に、個々のサービス(LLM/embedding/RAG)の起動状況を
// 都度通知するためのイベント。2026-10-02(バックエンド起動の並列化)で新設。
// 単一ラベルのshiori:startup-stageと異なり、サービスごとに「今どの段階か」
// (起動中/成功/失敗)を個別に伝えられるため、フロントエンドは複数サービスの
// 進捗を同時に表示できる。
fn emit_service_status(app: Option<&tauri::AppHandle>, status: &ServiceStatus) {
    use tauri::Emitter;
    log_service_status(status);
    if let Some(app) = app {
        let _ = app.emit("shiori:service-status", status);
    }
}

/// サービスの状態の移り変わりをログに残す(詩織Ver6.0の段階1)。起動画面への通知と同じ時に書く。
fn log_service_status(status: &ServiceStatus) {
    let part = match status.name.as_str() {
        "llm" => "LLM",
        "embedding" => "EMB",
        "rag" => "RAG",
        "stt" => "STT",
        _ => "APP",
    };
    let fields: [(&str, &dyn std::fmt::Display); 2] = [("名前", &status.name), ("ポート", &status.port)];
    if let Some(error) = &status.error {
        shiori_log::error(part, "サービスの起動に失敗した", &[fields[0], fields[1], ("理由", error)]);
    } else if status.skipped {
        shiori_log::info(part, "サービスを起動しなかった(外部AIモード)", &fields);
    } else if status.healthy {
        shiori_log::info(part, "サービスが動き始めた", &fields);
    } else if status.started {
        shiori_log::info(part, "サービスを起動した(応答待ち)", &fields);
    } else {
        shiori_log::debug(part, "サービスの起動を始めた", &fields);
    }
}

// LLM(Qwen3-8B、思考モード無効化での運用が前提)を起動する。2026-10-02
// (バックエンド起動の並列化)でstart_backend_services_implから切り出した。
// embeddingと独立に別スレッドで並列実行するための分離で、起動ロジック自体は
// 変更していない。
fn start_llm_service(
    root: &Path,
    config: &AppConfig,
    app: Option<&tauri::AppHandle>,
) -> (ServiceStatus, Option<Child>) {
    emit_startup_stage(app, "会話用のAIモデルを読み込んでいます");
    let llama_server_exe =
        resolve_engine_exe(&root.join("third_party/llama.cpp/build"), "llama-server");
    let model_path = root.join(&config.llm.model_path);
    let ctx = config.llm.context_size.unwrap_or(8192).to_string();
    let gpu_layers = config.llm.gpu_layers.unwrap_or(999).to_string();
    let port_str = config.llm.port.to_string();
    // --jinja: GGUF埋め込みのチャットテンプレートをMinjaで評価させるためのフラグ。
    // llm_client.rsが送るchat_template_kwargs(enable_thinking: false)はこのフラグが
    // ないと無視される。Qwen2.5等、思考モードを持たないモデルのテンプレートは
    // このキーワード引数を単に参照しないだけなので、フラグ自体を常時付けても
    // 無害であることを実機確認済み(2026-08-13)。
    let args = [
        "--port",
        &port_str,
        "--host",
        "127.0.0.1",
        "--n-gpu-layers",
        &gpu_layers,
        "--ctx-size",
        &ctx,
        "--jinja",
    ];
    let (status, child) = match spawn_server(&llama_server_exe, &model_path, &args, "libs-llama", false) {
        Ok(child) => {
            let healthy = wait_for_health(config.llm.port, 30);
            (
                ServiceStatus { skipped: false, name: "llm".into(), port: config.llm.port, started: true, healthy, error: None },
                Some(child),
            )
        }
        Err(e) => (
            ServiceStatus {
                skipped: false,
                name: "llm".into(),
                port: config.llm.port,
                started: false,
                healthy: false,
                error: Some(e.to_string()),
            },
            None,
        ),
    };
    emit_service_status(app, &status);
    (status, child)
}

// embedding用llama-server(nomic-embed-text)を起動する。2026-10-02で
// start_backend_services_implから切り出した(LLMと同様、並列実行のため)。
fn start_embedding_service(
    root: &Path,
    config: &AppConfig,
    app: Option<&tauri::AppHandle>,
) -> (ServiceStatus, Option<Child>) {
    emit_startup_stage(app, "検索用の埋め込みモデルを読み込んでいます");
    // 詩織Ver2.0: embedding用llama-serverは会話UI・MCPサーバー・保存CLI等から
    // 共有されるデーモンになったため(設計指示書v3、4章)、ヘルスチェック→ロック→
    // 起動の共通ロジック(shared_daemon)を経由する。既に他プロセスが起動済みの
    // 場合はOk(None)が返り、呼び出し元のprocs.embeddingはNoneのままになる
    // (=「自分の管理下には無いが動いている」ことを表す。詳細はshared_daemon.rsの
    // コメント参照)。
    let (status, child) = match shared_daemon::ensure_daemon_running(
        root,
        config.embedding.port,
        ".shiori-embed.lock",
        30,
        || {
            build_embedding_command(
                root,
                &config.embedding.model_path,
                config.embedding.port,
                config.embedding.context_size,
                false,
            )
        },
    ) {
        Ok(child) => (
            ServiceStatus {
                skipped: false,
                name: "embedding".into(),
                port: config.embedding.port,
                started: true,
                healthy: true,
                error: None,
            },
            child,
        ),
        Err(e) => (
            ServiceStatus {
                skipped: false,
                name: "embedding".into(),
                port: config.embedding.port,
                started: false,
                healthy: false,
                error: Some(e),
            },
            None,
        ),
    };
    emit_service_status(app, &status);
    (status, child)
}

// LLM/embedding/RAGサーバーを起動する。
//
// 【2026-10-02変更】以前はこの関数全体(=呼び出し元が握るBackendStateのロック)の
// 中でLLM→embedding→RAGを順番に起動し、各サービスのヘルスチェック完了を
// 待ってから次へ進んでいた。RAGサーバーの初回起動(ライブラリ全件のembedding
// 計算で数十秒〜数分かかりうる)の間、get_system_info等の他コマンドが
// BackendStateのロック待ちでブロックされ続け、アプリ全体が応答なしに見える
// 不具合が実機で繰り返し発生した(2026-09-17・2026-10-02)。
//
// 【重要】最初の対策として各サービスをstd::thread::spawnするだけに留めた版を
// 実機確認したところ、起動画面のカーソルがビーチボールになりボタンも反応しない
// (=メインスレッドがブロックされたまま)という、修正前と同じ症状が再発した。
// 原因は、tauriの同期コマンド(async fnでない#[tauri::command])はIPCを処理する
// メインスレッド上でそのまま実行され、その中でスレッドのjoin()を待っても
// メインスレッドのブロックは一切解消しない、という認識の誤りだった。この関数と
// 呼び出し元のコマンド自体をasync fnにし、重い処理はtauri::async_runtime::
// spawn_blocking(tokioのブロッキングスレッドプール)に積んで.awaitする形に
// 変更している(これならメインスレッドは他のIPCメッセージを処理し続けられる)。
//
// LLMとembeddingは互いに独立なため並列に起動し、両方の起動処理(ヘルスチェック
// 待ちを含む)が完了してから、結果(Child)を格納する瞬間だけ短時間ロックを取る
// 設計にした。RAGサーバーはembedding-server起動後でないと起動できない(起動時の
// _initial_index_syncがembeddingへ問い合わせるため)ため、LLM/embeddingの完了を
// 待ってから続けて起動する。
async fn start_backend_services_impl(
    state: &BackendState,
    app: Option<&tauri::AppHandle>,
) -> Result<Vec<ServiceStatus>, String> {
    let root = project_root();
    let config = load_config(&root)?;

    let llm_app = app.cloned();
    let llm_root = root.clone();
    let llm_config = config.clone();
    let llm_task = tauri::async_runtime::spawn_blocking(move || {
        if llm_config.mode == AppMode::External {
            let status = llm_skipped_status(&llm_config);
            emit_service_status(llm_app.as_ref(), &status);
            return (status, None);
        }
        start_llm_service(&llm_root, &llm_config, llm_app.as_ref())
    });

    let embedding_app = app.cloned();
    let embedding_root = root.clone();
    let embedding_config = config.clone();
    let embedding_task = tauri::async_runtime::spawn_blocking(move || {
        start_embedding_service(&embedding_root, &embedding_config, embedding_app.as_ref())
    });

    let (llm_status, llm_child) =
        llm_task.await.map_err(|_| "LLM起動タスクが異常終了しました".to_string())?;
    let (embedding_status, embedding_child) = embedding_task
        .await
        .map_err(|_| "embedding起動タスクが異常終了しました".to_string())?;

    {
        let mut procs = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(child) = llm_child {
            procs.llm = Some(child);
        }
        if let Some(child) = embedding_child {
            procs.embedding = Some(child);
        }
    }

    // RAG検索サーバー(services/rag、Python/uvicorn)。embeddingの起動完了後に
    // 続けて起動する(依存関係)。restart_llm_services経由でこの関数が再度
    // 呼ばれることがあるが、RAGはllm/embeddingの設定変更とは無関係なので、
    // 既に起動済みなら再起動しない(procs.rag.is_none()で判定、元の挙動のまま
    // 結果リストにも含めない)。
    let rag_needs_start = {
        let procs = state.0.lock().map_err(|e| e.to_string())?;
        procs.rag.is_none()
    };
    let rag_status = if rag_needs_start {
        let rag_app = app.cloned();
        let rag_root = root.clone();
        let rag_config = config.clone();
        let (status, child) = tauri::async_runtime::spawn_blocking(move || {
            start_rag_service_core(rag_app.as_ref(), &rag_root, &rag_config)
        })
        .await
        .map_err(|_| "RAG起動タスクが異常終了しました".to_string())?;
        {
            let mut procs = state.0.lock().map_err(|e| e.to_string())?;
            if procs.rag.is_none() {
                if let Some(child) = child {
                    procs.rag = Some(child);
                }
            }
        }
        Some(status)
    } else {
        None
    };
    if let Some(status) = &rag_status {
        emit_service_status(app, status);
    }

    // STT(whisper.cpp)はVRAM軽量化のため常時起動せず、録音開始時にオンデマンドで
    // 起動する(SttManager経由。start_recording/handle_hotkey_toggle参照)。

    // TTS(piper-plus)は現時点でspeaker_embedding次元不一致のアップストリーム不具合により
    // 音声合成が失敗するため、このフェーズでは起動対象に含めない。

    emit_startup_stage(app, "準備が整いました");
    let mut results = vec![llm_status, embedding_status];
    if let Some(status) = rag_status {
        results.push(status);
    }
    Ok(results)
}

// embedding用llama-serverを起動するCommandを組み立てる(spawnまで行う)。
// GUI(start_backend_services_impl)・MCPサーバー(mcp_server.rs)双方から
// 共有デーモンとして起動できるよう、Tauri固有の型に依存しない形で切り出した。
// quiet_stdioはspawn_server参照(MCPサーバーからの起動時はtrueを渡すこと)。
pub fn build_embedding_command(
    root: &Path,
    model_path_rel: &str,
    port: u16,
    context_size: Option<u32>,
    quiet_stdio: bool,
) -> std::io::Result<Child> {
    let llama_server_exe = resolve_engine_exe(&root.join("third_party/llama.cpp/build"), "llama-server");
    let model_path = root.join(model_path_rel);
    let port_str = port.to_string();
    // 【2026-09-10追加】未指定時はllama-serverのデフォルト(--parallelのデフォルト
    // スロット数4で--ctx-sizeを均等分割するため実効512トークン)になり、約500文字を
    // 超える日本語チャンクの埋め込みで500エラーが発生する不具合があったため明示的に
    // 指定する(詳細はEmbeddingConfigのコメント参照)。nomic-embed-textの訓練時
    // コンテキストが2048のため、--ctx-sizeをこれより大きくしてもcapされて無意味
    // (実機調査で確認済み)。RAGサーバーは埋め込みを1件ずつ逐次リクエストするため
    // 並列スロットは不要と判断し、--parallel 1で1スロットの全量(2048)を
    // 使えるようにする。
    //
    // さらに--ctx-size/--parallelだけでは不十分で、実機のlibrary/には2000
    // トークン近い巨大チャンクが実在し、「input (N tokens) is too large to
    // process. increase the physical batch size」というエラーで--ubatch-size
    // (物理バッチサイズ、デフォルト512)にも同時に阻まれることが実測で判明した。
    // ubatch-sizeはctx-sizeを超えられない制約があるため、同じ値を指定する。
    let ctx = context_size.unwrap_or(2048).to_string();
    // VRAM軽量化のためCPU実行にする(埋め込みモデルは軽量なためCPUでも実用速度が出る想定)
    let args = [
        "--port",
        &port_str,
        "--host",
        "127.0.0.1",
        "--n-gpu-layers",
        "0",
        "--embedding",
        "--ubatch-size",
        &ctx,
        "--ctx-size",
        &ctx,
        "--parallel",
        "1",
    ];
    spawn_server(&llama_server_exe, &model_path, &args, "libs-llama", quiet_stdio)
}

// RAG Pythonサーバー(services/rag、uvicorn)を起動するCommandを組み立てる
// (spawnまで行う)。build_embedding_commandと同じく、GUI・MCPサーバー双方から
// 共有デーモンとして起動できるようTauri固有の型に依存しない形で切り出した。
// quiet_stdioはspawn_server参照(MCPサーバーからの起動時はtrueを渡すこと。
// 理由はこのファイル内のquiet_stdioの説明コメントを参照)。
// ChromaDBのRust版SQLiteバインディングが、外部SSD(ExFAT、macOSの新しいfskit
// 経由マウント)上で新規DB作成の最初の接続から「attempt to write a readonly
// database」で失敗する不具合が実機で見つかった(2026-10-02)。Python標準の
// sqlite3モジュール(WALモード含む)では同じボリュームで問題なく書き込める
// ことを確認済みで、ファイルシステムそのものではなくChromaDB側のSQLite実装が
// 使うロック機構とfskitの組み合わせ固有の非互換と見られる。Windows側では
// 同条件で問題が出ていないため、macOS限定でvectordbをMacのローカルディスク
// (~/Library/Application Support/Shiori/vectordb)に逃がす。vectordbは
// library/から再構築可能な派生データなので、OSごとに別々に持っても実害はない。
#[cfg(target_os = "macos")]
fn macos_local_vectordb_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        Path::new(&home)
            .join("Library")
            .join("Application Support")
            .join("Shiori")
            .join("vectordb"),
    )
}

// Rust版のRAG(shiori-rag、詩織Ver6.0の段階5)の実行ファイル。いま動いている実行ファイルと
// 同じフォルダ(開発の版のbin/<os>/ver6-dev/など)を先に見て、無ければbin/<os>/を見る。
// どちらにも無ければNoneで、いままでどおりPython版(services/rag)を起動する。
// 実行ファイルを置くだけで切り替わり、外せば戻る(切り替えの途中で、版ごとに試せるように)。
// Macでは、CLI(shiori-doctorなど)が.appの隣にいるとき、その.appの中(本体の隣)も見る。
pub fn rust_rag_exe() -> Option<PathBuf> {
    let name = exe_name("shiori-rag");
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let beside = exe_dir.as_ref().map(|d| d.join(&name));
    let in_app = exe_dir
        .filter(|_| cfg!(target_os = "macos"))
        .map(|d| d.join("shiori-folio.app").join("Contents").join("MacOS").join(&name));
    beside
        .into_iter()
        .chain(in_app)
        .chain(std::iter::once(portable_bin_dir().join(&name)))
        .find(|p| p.is_file())
}

// RAGサーバーが索引(と進捗ファイル)を置くディレクトリ。
// - Rust版: data/rag-index(WindowsとMacで共有する。SQLiteを使わないので、Macのexfatでも
//   ロックの問題に当たらない見込み。段階5の企画書)
// - Python版: services/rag/library_path.pyのresolve_vectordb_dirと同じ場所(macOSは
//   SHIORI_VECTORDB_DIRで上のローカルディスクへ逃がし、それ以外はdata/vectordb)
pub fn rag_vectordb_dir(root: &Path) -> PathBuf {
    if rust_rag_exe().is_some() {
        return root.join("data").join("rag-index");
    }
    #[cfg(target_os = "macos")]
    if let Some(dir) = macos_local_vectordb_dir() {
        return dir;
    }
    root.join("data").join("vectordb")
}

pub fn build_rag_command(root: &Path, port: u16, quiet_stdio: bool) -> std::io::Result<Child> {
    if let Some(exe) = rust_rag_exe() {
        return build_rust_rag_command(&exe, root, port, quiet_stdio);
    }
    let rag_dir = root.join("services/rag");
    // 【2026-08-13修正】ポータブル版(bin/<os>/rag-venv)はuv python installで
    // 取得した「venvではない可搬版Python本体」で、Windowsではpython.exeが
    // 直下に配置される(Scripts/配下にはuv pip installしたパッケージの
    // コンソールスクリプト(uvicorn.exe等)は入るが、python.exe自体は入らない)。
    // 一方、開発ツリーのservices/rag/.venvは通常のvenv(python -m venv/uv venv)
    // なのでScripts/python.exeが正しい。この2つは配置規約が異なるため、
    // 従来1つの定数(python_rel)を両方に流用していたのが原因で、ポータブル版の
    // python_exeが存在しないパスに解決され、RAGサーバーがCommand::spawn()の
    // 段階で静かに起動失敗する不具合があった(実機のportable/移動テストで再現・
    // 特定)。Windowsのみ両者を分け、Mac/Linuxは元々どちらもbin/python配下で
    // 一致するため据え置く。
    #[cfg(windows)]
    let portable_python_rel = "python.exe";
    #[cfg(not(windows))]
    let portable_python_rel = "bin/python";
    #[cfg(windows)]
    let dev_python_rel = "Scripts/python.exe";
    #[cfg(not(windows))]
    let dev_python_rel = "bin/python";
    // 段階G: portable/パッケージではRAG用のPython環境もbin/<os>/rag-venv/に
    // OS別に配置する(エンジンバイナリのbin/<os>/配置と同じ考え方。venvは
    // コンパイル済みバイナリを含むためOSをまたいで共有できない)。存在すれば
    // こちらを優先し、無ければ開発ツリーのservices/rag/.venvにフォールバックする。
    let portable_python = portable_bin_dir().join("rag-venv").join(portable_python_rel);
    let python_exe = if portable_python.exists() {
        portable_python
    } else {
        rag_dir.join(".venv").join(dev_python_rel)
    };
    let port_str = port.to_string();
    let args = [
        "-m",
        "uvicorn",
        "app:app",
        "--port",
        &port_str,
        "--host",
        "127.0.0.1",
    ];
    let mut cmd = Command::new(&python_exe);
    cmd.current_dir(&rag_dir)
        .args(args)
        .env("PATH", extended_path(None))
        // 起動時のreranker.warmup()(CrossEncoderロード)がhuggingface_hubの
        // オンライン検証(バージョン確認等)を試みる。モデルは
        // ~/.cache/huggingfaceに既にキャッシュ済みで確認自体が不要なため、
        // ネットワーク状態に左右されず安定した起動時間にするためオフライン
        // 強制する(2026-09-10追加)。
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1");
    #[cfg(target_os = "macos")]
    if let Some(dir) = macos_local_vectordb_dir() {
        if std::fs::create_dir_all(&dir).is_ok() {
            cmd.env("SHIORI_VECTORDB_DIR", &dir);
        }
    }
    if quiet_stdio {
        // 【2026-09-10追加】真因はこちら。stdinを明示指定しないとCommandは
        // デフォルトで親プロセスの標準入力を継承する。MCPサーバー
        // (mcp_server.rs)は標準入出力をClaude CodeとのJSON-RPC通信に
        // 使っているため、それを継承したRAGサーバー(uvicorn)がまれに
        // 標準入力からの読み取りでブロックし、search_libraryが数分単位で
        // タイムアウトする不具合があった(実機調査で、embedding用
        // llama-serverは影響を受けずRAGサーバー側だけが毎回ハングすることから
        // 特定。HF_HUB_OFFLINE単体では解消しなかった)。RAGサーバーは対話的な
        // 標準入力を必要としないため常にnullにしてよい。
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    }
    no_console_window(&mut cmd);
    cmd.spawn()
}

// Rust版のRAG(shiori-rag)を起動する。モデルは<root>/models/rerank/、索引は<root>/data/rag-index/、
// ログは<root>/data/logs/rag-…(shiori-ragの既定)。Python版と違い、PCのキャッシュもPATHも使わない。
fn build_rust_rag_command(exe: &Path, root: &Path, port: u16, quiet_stdio: bool) -> std::io::Result<Child> {
    let port_str = port.to_string();
    let mut cmd = Command::new(exe);
    cmd.current_dir(root)
        .arg("serve")
        .arg("--root")
        .arg(root)
        .arg("--port")
        .arg(&port_str);
    // MCPサーバーから起動するときに、標準入力を引き継がない理由はPython版と同じ(下のbuild_rag_commandの説明)
    cmd.stdin(std::process::Stdio::null());
    if quiet_stdio {
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    }
    no_console_window(&mut cmd);
    shiori_log::info("RAG", "Rust版のRAGを起動する", &[("実行ファイル", &exe.display()), ("ポート", &port)]);
    cmd.spawn()
}

// MCPサーバー(mcp_server.rs)・保存CLI等、shiori_folio_lib外の別バイナリ向けに
// config.jsonのうち検索バックエンドの起動に必要な最小限の情報だけを公開する。
// AppConfig本体(プライベート)を丸ごとpublicにすると変更の影響範囲が広がり
// すぎるため、専用の薄い構造体に絞って公開する。
#[derive(Clone)]
pub struct SearchBackendConfig {
    pub embedding_port: u16,
    pub embedding_model_path: String,
    pub embedding_context_size: Option<u32>,
    pub rag_port: u16,
}

pub fn load_search_backend_config(root: &Path) -> Result<SearchBackendConfig, String> {
    let config = load_config(root)?;
    Ok(SearchBackendConfig {
        embedding_port: config.embedding.port,
        embedding_model_path: config.embedding.model_path.clone(),
        embedding_context_size: config.embedding.context_size,
        rag_port: config.rag.port,
    })
}

// RAGサーバー(services/rag、uvicorn)を起動し、ヘルスチェックの結果を含めて返す。
// BackendProcessesのロックを取らない版(2026-10-02追加)。start_backend_services_impl
// からspawn_blockingで(ロックの外で)呼べるようにするための分離で、起動ロジック
// 自体は変更していない。
fn start_rag_service_core(
    app: Option<&tauri::AppHandle>,
    root: &Path,
    config: &AppConfig,
) -> (ServiceStatus, Option<Child>) {
    emit_startup_stage(app, "詩織の図書館(検索インデックス)を読み込んでいます");
    // 詩織Ver2.0: RAG Pythonサーバーも会話UI・MCPサーバー・保存CLI等から共有
    // されるデーモンになったため(設計指示書v3、4章)、embedding用llama-serverと
    // 同じくshared_daemon経由で起動する。リランカーのwarmup(services/rag/app.py)
    // 込みで起動に時間がかかりうるため、ヘルスチェックの試行回数はembeddingより
    // 多めに取る。
    // 索引の版が上がった初回など、起動時の同期が長引くときは、進捗ファイルを見て
    // killせずに待ち、起動画面に進み具合を出す(shared_daemon::DaemonProgress参照)。
    let on_label = |label: &str| emit_startup_stage(app, label);
    let progress = shared_daemon::DaemonProgress {
        file: rag_vectordb_dir(root).join(".index_progress.json"),
        on_label: &on_label,
    };
    match shared_daemon::ensure_daemon_running_with_progress(
        root,
        config.rag.port,
        ".shiori-rag.lock",
        60,
        || build_rag_command(root, config.rag.port, false),
        Some(&progress),
    ) {
        Ok(child) => (
            ServiceStatus { skipped: false, name: "rag".into(), port: config.rag.port, started: true, healthy: true, error: None },
            child,
        ),
        Err(e) => (
            ServiceStatus { skipped: false, name: "rag".into(), port: config.rag.port, started: false, healthy: false, error: Some(e) },
            None,
        ),
    }
}

// start_rag_service_coreを呼び、結果のChildをBackendProcessesへ格納するところまで
// 行う薄いラッパー。本体の起動経路は非同期化でstart_rag_service_coreを直接呼ぶ形に
// なったため、今は既存テストだけが使う(製品ビルドでは未使用警告になるのでテスト限定)。
#[cfg(test)]
fn start_rag_service(
    procs: &mut BackendProcesses,
    app: Option<&tauri::AppHandle>,
    root: &Path,
    config: &AppConfig,
) -> ServiceStatus {
    let (status, child) = start_rag_service_core(app, root, config);
    if let Some(child) = child {
        procs.rag = Some(child);
    }
    status
}

// RAGサーバーが起動失敗、またはヘルスチェックが通らないまま残ってしまった場合の
// 再試行専用コマンド。起動画面の「再試行」ボタンから呼ばれる想定
// (start_backend_services自体はhealthy:falseでもOk(...)を返す設計のため、
// 呼び出し元が結果を見て明示的にこちらを呼ぶ必要がある)。
// プロセスが残っていれば(起動はしたがヘルスチェックが通らなかった場合)一旦
// 終了してから再度起動を試みる。
// 【2026-10-02 async化】旧実装はこの関数全体(#[tauri::command]のasyncでない
// fn)がtauriのIPC処理スレッド上でそのまま実行され、ensure_daemon_running内の
// ヘルスチェックループ(最大60回×2.5秒)をメインスレッドで直接ブロックしていた。
// 「再試行」ボタンを押した後アプリ全体が応答なしになる不具合の原因の一つだった
// ため、重い処理をtauri::async_runtime::spawn_blockingへ逃がすasync fnに変更した。
#[tauri::command]
async fn retry_rag_service(
    state: tauri::State<'_, BackendState>,
    sys: tauri::State<'_, Mutex<sysinfo::System>>,
    app: tauri::AppHandle,
) -> Result<ServiceStatus, String> {
    let root = project_root();
    let config = load_config(&root)?;
    {
        let mut procs = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(mut c) = procs.rag.take() {
            let _ = c.kill();
            let _ = c.wait();
        } else {
            // 共有デーモン化により、RAGがこのプロセスの管理下に無い(procsに
            // 保持されていない、外部プロセスが起動した)場合がある。その場合は
            // ロックファイルに記録されたPIDを頼りにkillする
            // (2026-08-14、Ver2.0 Phase 2フォローアップ)。
            let mut sys = sys.lock().map_err(|e| e.to_string())?;
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            shared_daemon::kill_daemon(&sys, &root, ".shiori-rag.lock");
        }
    }
    let rag_app = Some(app);
    let rag_root = root;
    let rag_config = config;
    let (status, child) = tauri::async_runtime::spawn_blocking(move || {
        start_rag_service_core(rag_app.as_ref(), &rag_root, &rag_config)
    })
    .await
    .map_err(|e| format!("RAG再試行タスクが異常終了しました: {e}"))?;
    {
        let mut procs = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(child) = child {
            procs.rag = Some(child);
        }
    }
    Ok(status)
}

// 診断の画面(詩織Ver6.0の段階6)。中身はCLIのshiori-doctorと同じ(diagnose.rs)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnoseDto {
    text: String,
    ng: usize,
}

// ポートへの問い合わせで数秒かかることがあるので、画面を止めないよう別のスレッドで作る。
#[tauri::command]
async fn diagnose_report() -> Result<DiagnoseDto, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let r = diagnose::build(&project_root(), "shiori-folio");
        DiagnoseDto { text: r.text, ng: r.ng }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn diagnose_copy(app: tauri::AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_backend_services(
    state: tauri::State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<Vec<ServiceStatus>, String> {
    let statuses = start_backend_services_impl(&state, Some(&app)).await?;
    // 全部が起動できた(意図して起動しなかったものを除く)ら、いまの設定を「前回正常に起動したときの写し」にする(段階6)
    if statuses.iter().all(|s| s.healthy || s.skipped) {
        repair::save_last_good_config(&project_root());
    }
    Ok(statuses)
}

// ディスクの読み書き速度は瞬間値ではなく「前回ポーリングからの差分」から算出するため、
// sysinfo::Disksと直前のポーリング時刻をアプリ全体で使い回す状態として保持する。
struct DiskMonitor {
    disks: sysinfo::Disks,
    // IOCTL_DISK_PERFORMANCEで取れるのは累積の読み書きバイト数のため、
    // 前回ポーリング時点の値と経過時間を保持して差分からMB/sを算出する。
    prev_counters: Option<(i64, i64)>,
    last_refresh: Instant,
}

// コントロールパネルの「システム状態」(優先度1、読み取り専用)向け。
// フロントエンドから1〜2秒間隔でポーリングされる想定。
#[tauri::command]
async fn get_system_info(app: tauri::AppHandle) -> Result<system_info::SystemInfoDto, String> {
    run_blocking(move || get_system_info_blocking(app.state(), app.state(), app.state(), app.state())).await
}

fn get_system_info_blocking(
    backend: tauri::State<BackendState>,
    stt: tauri::State<Arc<SttManager>>,
    sys: tauri::State<Mutex<sysinfo::System>>,
    disk_monitor: tauri::State<Mutex<DiskMonitor>>,
) -> Result<system_info::SystemInfoDto, String> {
    let config = app_config().ok();

    let mut sys = sys.lock().map_err(|e| e.to_string())?;
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let cpu_usage_percent = sys.global_cpu_usage();
    let ram_used_mb = sys.used_memory() / (1024 * 1024);
    let ram_total_mb = sys.total_memory() / (1024 * 1024);
    let os = sysinfo::System::long_os_version().unwrap_or_else(|| "不明".to_string());
    let platform = std::env::consts::OS.to_string();
    let cpu_model = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_else(|| "不明".to_string());
    let frequency_mhz = sys.cpus().first().map(|c| c.frequency()).unwrap_or(0);
    let cpu = system_info::CpuDetail {
        model: cpu_model,
        physical_cores: sys.physical_core_count().unwrap_or(0),
        logical_cores: sys.cpus().len(),
        frequency_mhz,
    };

    let ram_mb_of = |pid: Option<u32>| -> Option<u64> {
        let pid = pid?;
        sys.process(sysinfo::Pid::from_u32(pid))
            .map(|p| p.memory() / (1024 * 1024))
    };

    let root = project_root();

    let llm_pid = {
        let mut procs = backend.0.lock().map_err(|e| e.to_string())?;
        procs.llm.as_mut().and_then(|c| {
            matches!(c.try_wait(), Ok(None)).then(|| c.id())
        })
    };
    let llm_running = llm_pid.is_some();

    // embedding用llama-server・RAG Pythonサーバーは共有デーモン化により
    // BackendState(procs)に保持されているとは限らない(外部プロセス、例えば
    // 将来のMCPサーバーが先に起動した場合はNoneのまま)。そのため稼働判定・
    // PID取得はどちらもshared_daemonのロックファイルに記録されたPIDの生存
    // 確認を正とする(2026-08-14、Ver2.0 Phase 2フォローアップ)。
    let embedding_pid = shared_daemon::daemon_pid_if_alive(&sys, &root, ".shiori-embed.lock");
    let embedding_running = embedding_pid.is_some();
    let rag_pid = shared_daemon::daemon_pid_if_alive(&sys, &root, ".shiori-rag.lock");
    let rag_running = rag_pid.is_some();

    let stt_pid = stt.pid();

    let vram_by_pid = system_info::query_process_vram_map();
    let vram_mb_of = |pid: Option<u32>| -> Option<u64> { pid.and_then(|p| vram_by_pid.get(&p).copied()) };

    let model_name = config.as_ref().map(|c| {
        Path::new(&c.llm.model_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| c.llm.model_path.clone())
    });

    // Windows(WDDM)のnvidia-smiはプロセス別VRAM使用量を[N/A]としか返さない既知の
    // 制約があり、vram_by_pidが取れないことが多い。LLMについては優先度3の
    // モデル切替で使っている実測/概算値(models/vram_estimates.json)を代用する。
    // Mac(統合メモリ)はそもそもVRAM専用の概念が無くnvidia-smi代替も存在しない
    // ため、この概算フォールバックは行わずサービス一覧のVRAM欄は常に空欄にする
    // (RAM欄のみで実態を表す)。
    #[cfg(windows)]
    let llm_vram_mb = vram_mb_of(llm_pid).or_else(|| {
        let c = config.as_ref()?;
        let root = project_root();
        let path = root.join(&c.llm.model_path);
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let size_mb = std::fs::metadata(&path).ok()?.len() / (1024 * 1024);
        let (gb, _) = models::estimate_for(&root, &file_name, size_mb);
        Some((gb * 1024.0) as u64)
    });
    #[cfg(not(windows))]
    let llm_vram_mb = vram_mb_of(llm_pid);

    let services = vec![
        system_info::ServiceInfo {
            name: "llama-server (LLM)".to_string(),
            running: llm_running,
            vram_mb: llm_vram_mb,
            ram_mb: ram_mb_of(llm_pid),
            model_name,
        },
        system_info::ServiceInfo {
            name: "llama-server (embedding)".to_string(),
            running: embedding_running,
            vram_mb: vram_mb_of(embedding_pid),
            ram_mb: ram_mb_of(embedding_pid),
            model_name: None,
        },
        system_info::ServiceInfo {
            name: "uvicorn (RAG検索)".to_string(),
            running: rag_running,
            vram_mb: None,
            ram_mb: ram_mb_of(rag_pid),
            model_name: None,
        },
        system_info::ServiceInfo {
            name: "whisper-server (STT)".to_string(),
            running: stt.is_running(),
            vram_mb: vram_mb_of(stt_pid),
            ram_mb: ram_mb_of(stt_pid),
            model_name: None,
        },
        system_info::ServiceInfo {
            name: "piper-plus (TTS)".to_string(),
            // piper-plusは常駐サーバーではなく、発話のたびに都度起動して終了するため
            // 「稼働中/停止中」という概念がなく、常にオンデマンド呼び出しである旨を示す。
            running: false,
            vram_mb: None,
            ram_mb: None,
            model_name: None,
        },
    ];

    // 詩織が動作しているドライブ(project_root()のドライブ)の空き容量・種類・
    // 読み書き速度(前回ポーリングからの差分)を取得する。
    let (disk_throughput, storage) = {
        let mut monitor = disk_monitor.lock().map_err(|e| e.to_string())?;
        let elapsed = monitor.last_refresh.elapsed().as_secs_f64().max(0.05);
        monitor.disks.refresh_list();
        monitor.last_refresh = Instant::now();

        let root = project_root();
        let storage = monitor
            .disks
            .list()
            .iter()
            .filter(|d| root.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(|disk| {
                let kind = match disk.kind() {
                    sysinfo::DiskKind::SSD => "SSD".to_string(),
                    sysinfo::DiskKind::HDD => "HDD".to_string(),
                    sysinfo::DiskKind::Unknown(_) => "不明".to_string(),
                };
                system_info::StorageInfo {
                    drive: disk.mount_point().to_string_lossy().to_string(),
                    kind,
                    file_system: disk.file_system().to_string_lossy().to_string(),
                    total_gb: disk.total_space() as f64 / (1024.0 * 1024.0 * 1024.0),
                    free_gb: disk.available_space() as f64 / (1024.0 * 1024.0 * 1024.0),
                    is_removable: disk.is_removable(),
                }
            });

        let drive_letter = root.to_string_lossy().chars().next().unwrap_or('C');
        let throughput = system_info::query_disk_counters(drive_letter).and_then(|(read, written)| {
            let prev = monitor.prev_counters.replace((read, written));
            prev.map(|(prev_read, prev_written)| system_info::DiskThroughput {
                read_mb_per_sec: (read - prev_read).max(0) as f64 / elapsed / (1024.0 * 1024.0),
                write_mb_per_sec: (written - prev_written).max(0) as f64 / elapsed / (1024.0 * 1024.0),
            })
        });

        (throughput, storage)
    };

    Ok(system_info::SystemInfoDto {
        gpu: system_info::query_gpu_info(),
        ram_used_mb,
        ram_total_mb,
        cpu_usage_percent,
        os,
        platform,
        cpu,
        services,
        disk_throughput,
        storage,
        power: system_info::query_power_info(),
    })
}

// コントロールパネルの「詳細設定」向け。config.jsonのうち調整可能な項目のみを
// やり取りする(modelPath等の触ってほしくない項目は含めない)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigDto {
    hotkey: String,
    llm_port: u16,
    llm_context_size: Option<u32>,
    embedding_port: u16,
    stt_port: u16,
    rag_port: u16,
    passive_recall_threshold: f64,
    length_scale: f64,
    noise_scale: f64,
    noise_w: f64,
    show_year: bool,
    show_month: bool,
    show_day: bool,
    show_weekday: bool,
    show_seconds: bool,
    mode: AppMode,
    // ログに「詳細」の重さも書くか(詩織Ver6.0の段階1)
    log_detail: bool,
    // 水紋と星の動きを抑えるか(詩織Ver6.0の段階2)。未設定ならnull
    reduce_motion: Option<bool>,
    // 知らせの種類ごとに音を鳴らすか(詩織Ver6.0の段階3)
    notify_sound: NotifySound,
    // ペットの形の切り替えのホットキーと、眠るまでの分(詩織Ver6.0の段階4)
    pet_hotkey: String,
    pet_sleep_minutes: u32,
    pet_panel_percent: u32,
}

// 実機SSDを取り外せる状態にする(取り外し機能)。止める対象の選び方は`eject.rs`を参照。
// 外付けSSD上のプロセス一覧・lsofを使うため、tauriの同期コマンドがUIを固める問題
// (2026-10-02)を踏まえ、asyncにしてspawn_blockingで実行する。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EjectPreview {
    plan: eject::Plan,
    /// 止める予定のうち、詩織を閉じても残るもの(詩織の子プロセスではないもの)。詩織は終了時に
    /// 自分の子だけを止めるため、MCPや別のセッションが起動した共有デーモンがここに入る。
    /// 閉じるときの確認を出すかどうかの判定に使う。
    leftover_after_exit: Vec<eject::Entry>,
    /// `lsof`で見つかった、ほかにSSDを開いているプロセス(pid, 名前)。止めはしない。
    other_holders: Vec<(u32, String)>,
}

#[tauri::command]
async fn eject_preview() -> Result<EjectPreview, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = project_root();
        let (plan, _sys) = eject::current_plan(&root);
        let skip: Vec<u32> = plan.to_stop.iter().map(|e| e.pid).collect();
        let other_holders = eject::other_holders(&root, &skip);
        let me = std::process::id();
        let leftover_after_exit = plan.to_stop.iter().filter(|e| e.parent_pid != Some(me)).cloned().collect();
        EjectPreview { plan, leftover_after_exit, other_holders }
    })
    .await
    .map_err(|e| format!("取り外しの確認に失敗: {e}"))
}

/// SSD上のほかのプロセスを止めてから、詩織自身も終了する。結果を返した少し後に終了するので、
/// フロントは結果を表示できる。止められなかったものがあっても、詩織は終了する(再度の確認は
/// `shiori-eject`か、再起動後にこの画面で行う)。
#[tauri::command]
async fn eject_execute(app: tauri::AppHandle) -> Result<eject::StopOutcome, String> {
    // 先に、ハブで拡張機能の別のプログラムを止める(止める印を付けてから止めるので、再起動しない)。
    // 外から一斉に止めると、ハブが「落ちた」と見て再起動し、SSDをつかみ直す恐れがあった(段階6で見つけた)
    if let Some(programs) = app.try_state::<hub::process::ProgramsState>() {
        programs.0.stop_all();
    }
    let outcome = tauri::async_runtime::spawn_blocking(|| {
        // 止めたプログラムが一覧に残らないよう、少し待ってから一覧を作る
        std::thread::sleep(Duration::from_millis(300));
        let (plan, _sys) = eject::current_plan(&project_root());
        let pids: Vec<u32> = plan.to_stop.iter().map(|e| e.pid).collect();
        eject::stop_processes(&pids)
    })
    .await
    .map_err(|e| format!("プロセスの停止に失敗: {e}"))?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        app.exit(0);
    });
    Ok(outcome)
}

// Claudeモニター(詩織Ver3.7)。Claude Code側のフックが書いた`data/claude-status/`を
// 読んで返す。外付けSSD(exFAT)上のディレクトリを数秒ごとに読むため、tauriの
// 同期コマンドがUIを固める問題(2026-10-02)を踏まえ、asyncにしてspawn_blockingで
// 読む。
#[tauri::command]
async fn list_claude_sessions() -> Result<Vec<claude_status::ClaudeSession>, String> {
    let dir = project_root().join("data").join("claude-status");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("現在時刻の取得に失敗: {e}"))?
        .as_millis() as u64;
    let local_host = if cfg!(windows) { "win" } else { "mac" };
    tauri::async_runtime::spawn_blocking(move || {
        claude_status::read_sessions(&dir, now_ms, local_host, &claude_status::is_claude_alive)
    })
        .await
        .map_err(|e| format!("Claudeの状況の読み取りに失敗: {e}"))
}

// Claudeの知らせの札を押したとき(詩織Ver6.0)。作業が終わったセッションのアプリを前に出し、
// 出せたかを返す。出せなければ画面の側が「Claude」の画面を開く。プロセスの一覧を読むのでasync。
#[tauri::command]
async fn open_claude_session(session_id: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || claude_status::focus_session(&session_id))
        .await
        .map_err(|e| format!("Claudeのセッションを前に出せなかった: {e}"))
}

// Claudeの利用統計(詩織Ver3.9)。このPCのClaude Codeの履歴を`data/claude-stats/`の
// 台帳へ取り込み、全端末の台帳を合算して返す。起動時の取り込みと、統計タブを開いた
// ときの取り込みが重ならないよう、取り込みは1つずつ行う。
static CLAUDE_STATS_REFRESH: Mutex<Option<u64>> = Mutex::new(None);
/// 強制でない取り込みを省く間隔。統計タブを開くたびにSSDへ書き直さないため。
const CLAUDE_STATS_REFRESH_INTERVAL_MS: u64 = 5 * 60 * 1000;

/// SSD(exFAT)の読み書きやRAGへの問い合わせを行うコマンド向け。Tauri 2の同期コマンドは
/// UIのスレッドで動き、終わるまで画面全体が固まる(図書館で「応答なし」になっていた)ため、
/// 本体は裏のスレッドで動かす。
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("処理の実行に失敗: {e}"))?
}

fn claude_stats_dir() -> PathBuf {
    project_root().join("data").join("claude-stats")
}

fn unix_now_ms() -> Result<u64, String> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("現在時刻の取得に失敗: {e}"))?
        .as_millis() as u64)
}

/// 会話モードと外部AIモードを切り替える(詩織Ver4.0)。config.jsonに保存し、
/// 再起動せずに会話用LLMを止める・起動する。返すのは切り替え後の会話用LLMの状態。
#[tauri::command]
async fn set_app_mode(app: tauri::AppHandle, mode: AppMode) -> Result<ServiceStatus, String> {
    run_blocking(move || set_app_mode_blocking(&app, mode)).await
}

fn set_app_mode_blocking(app: &tauri::AppHandle, mode: AppMode) -> Result<ServiceStatus, String> {
    let path = project_root().join("config.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("config.jsonの読み込みに失敗: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("config.jsonの解析に失敗: {e}"))?;
    value["mode"] = serde_json::json!(mode);
    let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| format!("config.jsonの書き込みに失敗: {e}"))?;

    let config = app_config()?;
    let state = app.state::<BackendState>();
    match mode {
        AppMode::External => {
            let child = state.0.lock().map_err(|e| e.to_string())?.llm.take();
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
            let status = llm_skipped_status(&config);
            emit_service_status(Some(app), &status);
            Ok(status)
        }
        AppMode::Conversation => {
            if state.0.lock().map_err(|e| e.to_string())?.llm.is_some() {
                return Ok(ServiceStatus {
                    name: "llm".into(),
                    port: config.llm.port,
                    started: true,
                    healthy: true,
                    error: None,
                    skipped: false,
                });
            }
            // 起動は数十秒かかりうるので、待つ間はロックを握らない。
            let (status, child) = start_llm_service(&project_root(), &config, Some(app));
            if let Some(child) = child {
                state.0.lock().map_err(|e| e.to_string())?.llm = Some(child);
            }
            emit_service_status(Some(app), &status);
            Ok(status)
        }
    }
}

/// 画面の操作(タブの移動など)を記録する(詩織Ver6.0の段階1)。部品名はUIに固定し、
/// 画面から渡せるのは出来事と、キーと値の組だけにする。
#[tauri::command]
async fn log_ui_event(event: String, fields: Option<std::collections::BTreeMap<String, String>>) -> Result<(), String> {
    let fields = fields.unwrap_or_default();
    let pairs: Vec<(&str, &dyn std::fmt::Display)> =
        fields.iter().map(|(k, v)| (k.as_str(), v as &dyn std::fmt::Display)).collect();
    shiori_log::info("UI", &event, &pairs);
    Ok(())
}

/// 画面側(フロント)の未処理のエラーを記録する(詩織Ver4.0)。
#[tauri::command]
async fn log_frontend_error(message: String) -> Result<(), String> {
    run_blocking(move || {
        shiori_log::error("UI", "画面の未処理のエラー", &[("内容", &message)]);
        Ok(())
    })
    .await
}

/// 利用統計の台帳に使う端末名。macOSの`hostname`はつないだネットワークが割り当てる名前
/// (`ip-10-…compute.internal`など)に変わり、同じMacが別の端末として増えるため、
/// ネットワークで変わらないLocalHostNameに`.local`を付けて使う。
fn stats_host_name() -> String {
    #[cfg(target_os = "macos")]
    {
        let local = std::process::Command::new("scutil")
            .args(["--get", "LocalHostName"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(name) = local {
            return format!("{name}.local");
        }
    }
    sysinfo::System::host_name().unwrap_or_else(|| "unknown".to_string())
}

/// 取り込みを省いたときは`None`を返す。
fn refresh_claude_stats_blocking(force: bool) -> Result<Option<claude_stats::RefreshOutcome>, String> {
    let mut last = CLAUDE_STATS_REFRESH
        .lock()
        .map_err(|_| "利用統計の取り込みの状態が壊れています".to_string())?;
    let now_ms = unix_now_ms()?;
    if !force && last.is_some_and(|t| now_ms.saturating_sub(t) < CLAUDE_STATS_REFRESH_INTERVAL_MS) {
        return Ok(None);
    }
    let projects = claude_stats::default_projects_dir()
        .ok_or("Claude Codeの履歴の場所(ホームフォルダ)が分かりません")?;
    let host = stats_host_name();
    let outcome = claude_stats::refresh(&projects, &claude_stats_dir(), &host, now_ms)?;
    *last = Some(now_ms);
    Ok(Some(outcome))
}

#[tauri::command]
async fn refresh_claude_stats(force: bool) -> Result<Option<claude_stats::RefreshOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || refresh_claude_stats_blocking(force))
        .await
        .map_err(|e| format!("利用統計の取り込みに失敗: {e}"))?
}

#[tauri::command]
async fn get_claude_stats(
    range: claude_stats::StatsRange,
    host: Option<String>,
) -> Result<claude_stats::ClaudeStats, String> {
    let now_ms = unix_now_ms()?;
    tauri::async_runtime::spawn_blocking(move || {
        claude_stats::summarize_local(&claude_stats_dir(), range, host.as_deref(), now_ms)
    })
    .await
    .map_err(|e| format!("利用統計の集計に失敗: {e}"))
}

#[tauri::command]
async fn get_config() -> Result<ConfigDto, String> {
    run_blocking(move || get_config_blocking()).await
}

fn get_config_blocking() -> Result<ConfigDto, String> {
    let config = app_config()?;
    Ok(ConfigDto {
        hotkey: config.hotkey,
        llm_port: config.llm.port,
        llm_context_size: config.llm.context_size,
        embedding_port: config.embedding.port,
        stt_port: config.stt.port,
        rag_port: config.rag.port,
        passive_recall_threshold: config.rag.passive_recall_threshold,
        length_scale: config.tts.length_scale,
        noise_scale: config.tts.noise_scale,
        noise_w: config.tts.noise_w,
        show_year: config.ui.show_year,
        show_month: config.ui.show_month,
        show_day: config.ui.show_day,
        show_weekday: config.ui.show_weekday,
        show_seconds: config.ui.show_seconds,
        mode: config.mode,
        log_detail: shiori_log::LogSettings::from_config(&project_root()).detail,
        reduce_motion: config.ui.reduce_motion,
        notify_sound: config.notify.sound,
        pet_hotkey: config.pet.hotkey,
        pet_sleep_minutes: config.pet.sleep_minutes,
        pet_panel_percent: config.pet.panel_percent,
    })
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConfigUpdate {
    hotkey: Option<String>,
    llm_port: Option<u16>,
    llm_context_size: Option<u32>,
    embedding_port: Option<u16>,
    stt_port: Option<u16>,
    rag_port: Option<u16>,
    passive_recall_threshold: Option<f64>,
    length_scale: Option<f64>,
    noise_scale: Option<f64>,
    noise_w: Option<f64>,
    show_year: Option<bool>,
    show_month: Option<bool>,
    show_day: Option<bool>,
    show_weekday: Option<bool>,
    show_seconds: Option<bool>,
    log_detail: Option<bool>,
    reduce_motion: Option<bool>,
    notify_sound: Option<NotifySound>,
    pet_hotkey: Option<String>,
    pet_sleep_minutes: Option<u32>,
    pet_panel_percent: Option<u32>,
}

fn validate_config_update(update: &ConfigUpdate) -> Result<(), String> {
    for port in [update.llm_port, update.embedding_port, update.stt_port, update.rag_port]
        .into_iter()
        .flatten()
    {
        if port < 1024 {
            return Err(format!(
                "ポート番号は1024以上を指定してください(指定値: {port})"
            ));
        }
    }
    if let Some(t) = update.passive_recall_threshold {
        if !(0.0..=2.0).contains(&t) {
            return Err("類似度しきい値は0.0〜2.0の範囲で指定してください".to_string());
        }
    }
    if let Some(cs) = update.llm_context_size {
        if !(512..=32768).contains(&cs) {
            return Err("コンテキストサイズは512〜32768の範囲で指定してください".to_string());
        }
    }
    for (label, value) in [
        ("発話速度(length-scale)", update.length_scale),
        ("ノイズスケール(noise-scale)", update.noise_scale),
        ("音素幅ノイズ(noise-w)", update.noise_w),
    ] {
        if let Some(v) = value {
            if !(0.1..=3.0).contains(&v) {
                return Err(format!("{label}は0.1〜3.0の範囲で指定してください"));
            }
        }
    }
    if let Some(hotkey) = &update.hotkey {
        if hotkey.trim().is_empty() {
            return Err("ホットキーを空にはできません".to_string());
        }
    }
    if let Some(hotkey) = &update.pet_hotkey {
        if hotkey.trim().is_empty() {
            return Err("形の切り替えのホットキーを空にはできません".to_string());
        }
        if hotkey.parse::<tauri_plugin_global_shortcut::Shortcut>().is_err() {
            return Err(format!("形の切り替えのホットキーとして読めません(例: ctrl+alt+d): {hotkey}"));
        }
    }
    if let Some(m) = update.pet_sleep_minutes {
        if !(1..=120).contains(&m) {
            return Err("眠るまでの時間は1〜120分で指定してください".to_string());
        }
    }
    if let Some(p) = update.pet_panel_percent {
        if !(60..=150).contains(&p) {
            return Err("本と小道具の枠の大きさは60〜150%で指定してください".to_string());
        }
    }
    Ok(())
}

#[tauri::command]
async fn set_config(update: ConfigUpdate) -> Result<(), String> {
    run_blocking(move || set_config_blocking(update)).await
}

fn set_config_blocking(update: ConfigUpdate) -> Result<(), String> {
    validate_config_update(&update)?;

    let path = project_root().join("config.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("config.jsonの読み込みに失敗: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("config.jsonの解析に失敗: {e}"))?;

    if let Some(v) = update.hotkey {
        value["hotkey"] = serde_json::json!(v);
    }
    if let Some(v) = update.llm_port {
        value["llm"]["port"] = serde_json::json!(v);
    }
    if let Some(v) = update.llm_context_size {
        value["llm"]["contextSize"] = serde_json::json!(v);
    }
    if let Some(v) = update.embedding_port {
        value["embedding"]["port"] = serde_json::json!(v);
    }
    if let Some(v) = update.stt_port {
        value["stt"]["port"] = serde_json::json!(v);
    }
    if let Some(v) = update.rag_port {
        value["rag"]["port"] = serde_json::json!(v);
    }
    if let Some(v) = update.passive_recall_threshold {
        value["rag"]["passiveRecallThreshold"] = serde_json::json!(v);
    }
    if let Some(v) = update.length_scale {
        value["tts"]["lengthScale"] = serde_json::json!(v);
    }
    if let Some(v) = update.noise_scale {
        value["tts"]["noiseScale"] = serde_json::json!(v);
    }
    if let Some(v) = update.noise_w {
        value["tts"]["noiseW"] = serde_json::json!(v);
    }
    if let Some(v) = update.show_year {
        value["ui"]["showYear"] = serde_json::json!(v);
    }
    if let Some(v) = update.show_month {
        value["ui"]["showMonth"] = serde_json::json!(v);
    }
    if let Some(v) = update.show_day {
        value["ui"]["showDay"] = serde_json::json!(v);
    }
    if let Some(v) = update.show_weekday {
        value["ui"]["showWeekday"] = serde_json::json!(v);
    }
    if let Some(v) = update.show_seconds {
        value["ui"]["showSeconds"] = serde_json::json!(v);
    }
    if let Some(v) = update.reduce_motion {
        value["ui"]["reduceMotion"] = serde_json::json!(v);
    }
    if let Some(v) = update.notify_sound {
        value["notify"]["sound"] = serde_json::json!(v);
    }
    if let Some(v) = &update.pet_hotkey {
        value["pet"]["hotkey"] = serde_json::json!(v);
    }
    if let Some(v) = update.pet_sleep_minutes {
        value["pet"]["sleepMinutes"] = serde_json::json!(v);
    }
    if let Some(v) = update.pet_panel_percent {
        value["pet"]["panelPercent"] = serde_json::json!(v);
    }
    let log_detail = update.log_detail;
    if let Some(v) = log_detail {
        value["log"]["detail"] = serde_json::json!(v);
    }

    let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| format!("config.jsonの書き込みに失敗: {e}"))?;
    // ログの「詳細」は、再起動を待たずに切り替える
    if let Some(v) = log_detail {
        shiori_log::set_detail(v);
        shiori_log::info("CFG", "ログの詳細を切り替えた", &[("詳細", &v)]);
    }
    Ok(())
}

// ポート番号やコンテキストサイズの変更を反映する。既存のllm/embeddingプロセスを
// 止めてから、新しいconfig.jsonの値で起動し直す。ユーザーの明示操作(ボタン)からのみ
// 呼ばれ、set_config成功後に自動では走らない(意図せずサービスが落ちるのを防ぐため)。
// 【2026-10-02 async化】start_backend_services_impl/retry_rag_serviceと同じ理由
// (旧実装はIPC処理スレッドをブロックしていた)でasync fnに変更した。
#[tauri::command]
async fn restart_llm_services(
    state: tauri::State<'_, BackendState>,
    sys: tauri::State<'_, Mutex<sysinfo::System>>,
    app: tauri::AppHandle,
) -> Result<Vec<ServiceStatus>, String> {
    {
        let mut procs = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(mut c) = procs.llm.take() {
            let _ = c.kill();
        }
        if let Some(mut c) = procs.embedding.take() {
            let _ = c.kill();
        } else {
            // 共有デーモン化により、embeddingがこのプロセスの管理下に無い
            // (procsに保持されていない、外部プロセスが起動した)場合がある。
            // その場合はロックファイルに記録されたPIDを頼りにkillする
            // (2026-08-14、Ver2.0 Phase 2フォローアップ)。
            let root = project_root();
            let mut sys = sys.lock().map_err(|e| e.to_string())?;
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            shared_daemon::kill_daemon(&sys, &root, ".shiori-embed.lock");
        }
        // start_backend_services_impl自身が必要なタイミングでロックを取り直す
        // ため、ここでロックを解放してから呼ぶ(握ったまま渡すとデッドロックする)。
    }
    start_backend_services_impl(&state, Some(&app)).await
}

// ホットキーの変更はtauri-plugin-global-shortcutの動的な再登録より、アプリ全体を
// 再起動する方が単純で確実なため、そちらを採用する。RunEvent::ExitRequestedの
// クリーンアップ処理を経由して子プロセスも正しく終了してから再起動される。
#[tauri::command]
fn restart_app(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .spawn()
        .map_err(|e| format!("新しいプロセスの起動に失敗: {e}"))?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn list_available_models() -> Result<Vec<models::ModelInfo>, String> {
    run_blocking(move || list_available_models_blocking()).await
}

fn list_available_models_blocking() -> Result<Vec<models::ModelInfo>, String> {
    let root = project_root();
    let config = app_config()?;
    let current = Path::new(&config.llm.model_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    models::list_models(&root, &current)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelSwitchEstimate {
    projected_vram_percent: f64,
    // 危険水域(優先度1のシステムモニターと同じ80%を採用)の見込みかどうか。
    // trueの場合、フロントエンド側で明示確認を挟んでからswitch_modelを呼ぶ想定。
    is_risky: bool,
}

// 実際に2つのモデルを同時にロードして計測することは「避けたい危険な状態」を
// 自ら作ることになるため行わない。現在ロード中のモデルの実測/概算値を使用量
// から差し引いた「モデル以外の使用量」に、切替先モデルの実測/概算値を足して見積もる。
// 「使用量/総量」はWindowsはVRAM、Mac(統合メモリ)は物理メモリ全体を指す
// (system_info::query_memory_headroom参照)。
#[tauri::command]
async fn estimate_model_switch(app: tauri::AppHandle, file_name: String) -> Result<ModelSwitchEstimate, String> {
    run_blocking(move || estimate_model_switch_blocking(app.state(), file_name)).await
}

fn estimate_model_switch_blocking(
    sys: tauri::State<Mutex<sysinfo::System>>,
    file_name: String,
) -> Result<ModelSwitchEstimate, String> {
    let mut sys = sys.lock().map_err(|e| e.to_string())?;
    sys.refresh_memory();
    estimate_model_switch_impl(&sys, file_name)
}

fn estimate_model_switch_impl(sys: &sysinfo::System, file_name: String) -> Result<ModelSwitchEstimate, String> {
    let root = project_root();
    let config = app_config()?;
    let headroom = system_info::query_memory_headroom(sys)
        .ok_or_else(|| "メモリ使用状況を取得できませんでした(Windowsではnvidia-smi未検出の可能性があります)".to_string())?;

    let current_file_name = Path::new(&config.llm.model_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let current_size_mb = std::fs::metadata(root.join(&config.llm.model_path))
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    let (current_estimate_gb, _) = models::estimate_for(&root, &current_file_name, current_size_mb);

    let target_path = root.join("models/llm").join(&file_name);
    let target_size_mb = std::fs::metadata(&target_path)
        .map_err(|e| format!("モデルファイルが見つかりません: {e}"))?
        .len()
        / (1024 * 1024);
    let (target_estimate_gb, _) = models::estimate_for(&root, &file_name, target_size_mb);

    let baseline_other_mb = (headroom.used_mb as f64 - current_estimate_gb * 1024.0).max(0.0);
    let projected_used_mb = baseline_other_mb + target_estimate_gb * 1024.0;
    let projected_vram_percent = projected_used_mb / headroom.total_mb as f64 * 100.0;

    Ok(ModelSwitchEstimate {
        projected_vram_percent,
        is_risky: projected_vram_percent >= 80.0,
    })
}

fn update_llm_model_path(root: &Path, file_name: &str) -> Result<(), String> {
    let path = root.join("config.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("config.jsonの読み込みに失敗: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("config.jsonの解析に失敗: {e}"))?;
    value["llm"]["modelPath"] = serde_json::json!(format!("./models/llm/{file_name}"));
    let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| format!("config.jsonの書き込みに失敗: {e}"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelSwitchResult {
    success: bool,
    measured_vram_gb: Option<f64>,
    rolled_back: bool,
    error: Option<String>,
}

// 「現在のllama-serverを停止 → 新モデルで起動 → ヘルスチェック」の手順で切り替える。
// 起動またはヘルスチェックに失敗した場合は、元のモデルで自動的に再起動を試みる
// (起動できない状態のまま放置しない)。成功時は切替前後のVRAM差分(Windows)、
// または新モデルプロセス自体のRSS(Mac、統合メモリのためVRAM差分が取れない)を
// 実測値としてmodels/vram_estimates.jsonに書き戻し、次回以降の見積もり精度を上げる。
#[tauri::command]
async fn switch_model(app: tauri::AppHandle, file_name: String) -> Result<ModelSwitchResult, String> {
    run_blocking(move || switch_model_blocking(app.state(), app.state(), file_name)).await
}

// sysはMacのRSS実測でのみ使う(Windowsは未使用)
#[cfg_attr(windows, allow(unused_variables))]
fn switch_model_blocking(
    state: tauri::State<BackendState>,
    sys: tauri::State<Mutex<sysinfo::System>>,
    file_name: String,
) -> Result<ModelSwitchResult, String> {
    let root = project_root();
    let config = app_config()?;
    let llama_server_exe =
        resolve_engine_exe(&root.join("third_party/llama.cpp/build"), "llama-server");
    let target_path = root.join("models/llm").join(&file_name);
    if !target_path.exists() {
        return Err(format!("モデルファイルが見つかりません: {}", target_path.display()));
    }

    let old_model_path = config.llm.model_path.clone();
    let ctx = config.llm.context_size.unwrap_or(8192).to_string();
    let gpu_layers = config.llm.gpu_layers.unwrap_or(999).to_string();
    let port_str = config.llm.port.to_string();
    let args = [
        "--port",
        &port_str,
        "--host",
        "127.0.0.1",
        "--n-gpu-layers",
        &gpu_layers,
        "--ctx-size",
        &ctx,
        "--jinja",
    ];

    let mut procs = state.0.lock().map_err(|e| e.to_string())?;

    // 1. 旧llama-serverを停止し、VRAMが解放されるのを少し待つ
    if let Some(mut c) = procs.llm.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    std::thread::sleep(Duration::from_millis(800));
    #[cfg(windows)]
    let floor_mb = system_info::query_gpu_info()
        .map(|g| g.vram_used_mb)
        .unwrap_or(0);

    // 2. 新モデルで起動
    match spawn_server(&llama_server_exe, &target_path, &args, "libs-llama", false) {
        Ok(child) => {
            procs.llm = Some(child);
            if wait_for_health(config.llm.port, 30) {
                #[cfg(windows)]
                let measured_gb = {
                    let after_mb = system_info::query_gpu_info()
                        .map(|g| g.vram_used_mb)
                        .unwrap_or(floor_mb);
                    (after_mb.saturating_sub(floor_mb) as f64 / 1024.0).max(0.1)
                };
                // Mac(統合メモリ)はVRAM専用プールが無くWindowsと同じ差分計測が
                // できないため、切替後のllama-serverプロセス自体のRSS(常駐メモリ)
                // を実測値として使う。
                #[cfg(not(windows))]
                let measured_gb = {
                    let pid = procs.llm.as_ref().map(|c| c.id());
                    let mut s = sys.lock().map_err(|e| e.to_string())?;
                    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                    let rss_mb = pid
                        .and_then(|p| s.process(sysinfo::Pid::from_u32(p)))
                        .map(|p| p.memory() / (1024 * 1024))
                        .unwrap_or(0);
                    (rss_mb as f64 / 1024.0).max(0.1)
                };
                drop(procs);
                let _ = models::record_measurement(&root, &file_name, measured_gb);
                update_llm_model_path(&root, &file_name)?;
                Ok(ModelSwitchResult {
                    success: true,
                    measured_vram_gb: Some(measured_gb),
                    rolled_back: false,
                    error: None,
                })
            } else {
                if let Some(mut c) = procs.llm.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                let rolled_back = match spawn_server(&llama_server_exe, &root.join(&old_model_path), &args, "libs-llama", false) {
                    Ok(child) => {
                        procs.llm = Some(child);
                        wait_for_health(config.llm.port, 30)
                    }
                    Err(_) => false,
                };
                Ok(ModelSwitchResult {
                    success: false,
                    measured_vram_gb: None,
                    rolled_back,
                    error: Some("新モデルの起動に失敗しました(ヘルスチェックがタイムアウトしました)".to_string()),
                })
            }
        }
        Err(e) => {
            let rolled_back = match spawn_server(&llama_server_exe, &root.join(&old_model_path), &args, "libs-llama", false) {
                Ok(child) => {
                    procs.llm = Some(child);
                    wait_for_health(config.llm.port, 30)
                }
                Err(_) => false,
            };
            Ok(ModelSwitchResult {
                success: false,
                measured_vram_gb: None,
                rolled_back,
                error: Some(format!("新モデルの起動に失敗: {e}")),
            })
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeResultDto {
    id: String,
    text: String,
    source: String,
    heading: String,
    source_category: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFileDto {
    source: String,
    source_category: String,
    headings: Vec<String>,
    // frontmatterのtitle・実ファイルへの絶対パス(2026-09-16追加、
    // エクスプローラー風UI刷新向け)。
    title: String,
    path: String,
    // .metaのとき対応する実体の絶対パス(無ければ空文字列、詩織Ver3.5)。
    asset_path: String,
    // library_rootからの相対パス(/区切り)。GUIのフォルダツリー構築に使う。
    relative_path: String,
    // ファイルの更新日時(Unixタイムスタンプ)。
    mtime: f64,
}

// KnowledgePanelの参照情報をクリックした際、元のMarkdownファイル全文を返す
// (v1.0スコープ機能1: 参照資料の表示モーダル)。source_category/sourceから
// library/配下のパスを組み立てるが、ユーザー入力(というよりLLM経由の
// 値だが、フロント側も経由するため信頼しない)を直接パス結合に使うと
// パストラバーサル(例: source_category="..", source="../../config.json")の
// リスクがあるため、組み立てた絶対パスをcanonicalize()した上で、
// library/ディレクトリ自体をcanonicalize()した結果の配下に収まって
// いるかを必ず確認する。
#[tauri::command]
async fn get_source_document(source_category: String, source: String) -> Result<String, String> {
    run_blocking(move || get_source_document_blocking(source_category, source)).await
}

fn get_source_document_blocking(source_category: String, source: String) -> Result<String, String> {
    let knowledge_root = library_root();
    let search_root = if source_category == "uncategorized" {
        knowledge_root.clone()
    } else {
        knowledge_root.join(&source_category)
    };
    // sourceは通常ファイル名。memoryだけ「project/memory.md」(source_path.rs参照)。
    let candidate = source_path::resolve_source_file(&search_root, &source)
        .ok_or_else(|| "指定された参照資料が見つかりませんでした。".to_string())?;

    let canonical_root = knowledge_root
        .canonicalize()
        .map_err(|e| format!("library/の解決に失敗: {e}"))?;
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|_| "指定された参照資料が見つかりませんでした。".to_string())?;

    if !canonical_candidate.starts_with(&canonical_root) {
        return Err("不正なパスが指定されました。".to_string());
    }

    std::fs::read_to_string(&canonical_candidate)
        .map_err(|e| format!("参照資料の読み込みに失敗: {e}"))
}

// 全件閲覧の素材行(詩織Ver3.5)から、素材の実体を既定のアプリで開く/
// フォルダで表示する。フロントから渡されるパスは信頼せず、canonicalize()した
// 結果がlibrary/配下に収まり、かつ通常ファイルであることを確認してから使う
// (get_source_documentと同じ方針)。
fn resolve_library_asset(path: &str) -> Result<PathBuf, String> {
    let canonical_root = library_root()
        .canonicalize()
        .map_err(|e| format!("library/の解決に失敗: {e}"))?;
    let canonical = Path::new(path)
        .canonicalize()
        .map_err(|_| "指定された素材が見つかりませんでした。".to_string())?;
    if !canonical.starts_with(&canonical_root) {
        return Err("library/の外のパスは開けません。".to_string());
    }
    if !canonical.is_file() {
        return Err("指定されたパスはファイルではありません。".to_string());
    }
    Ok(canonical)
}

// 既定のアプリで「開く」と、そのまま実行されてしまう拡張子。libraryはMCP経由で
// 外部AIも読み書きしうるため、素材として置かれた実行形式をワンクリックで
// 起動できないようにする。これらは「フォルダで表示」のみ許可する。
const NON_OPENABLE_EXTENSIONS: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "vbe", "js", "jse", "wsf", "lnk",
    "sh", "command", "app", "jar", "pkg", "dmg", "apk", "reg", "hta",
];

#[tauri::command]
async fn open_library_asset(app: tauri::AppHandle, path: String) -> Result<(), String> {
    run_blocking(move || open_library_asset_blocking(app, path)).await
}

fn open_library_asset_blocking(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let target = resolve_library_asset(&path)?;
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if NON_OPENABLE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "実行形式(.{ext})は安全のため直接開けません。「フォルダで表示」を使ってください。"
        ));
    }
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("素材を開けませんでした: {e}"))
}

#[tauri::command]
async fn reveal_library_asset(app: tauri::AppHandle, path: String) -> Result<(), String> {
    run_blocking(move || reveal_library_asset_blocking(app, path)).await
}

fn reveal_library_asset_blocking(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let target = resolve_library_asset(&path)?;
    app.opener()
        .reveal_item_in_dir(&target)
        .map_err(|e| format!("フォルダを表示できませんでした: {e}"))
}

// 規約画面(詩織Ver4.1)向け。libraryの保存規約・台帳・プロンプト・設定など、詩織の
// システムを決めている書類を読み取り専用で返す。読める書類は許可リストだけ
// (system_docs.rs参照)。
#[tauri::command]
async fn list_system_docs() -> Result<Vec<system_docs::SystemDocDto>, String> {
    run_blocking(move || Ok(system_docs::list(&library_root(), &project_root()))).await
}

#[tauri::command]
async fn read_system_doc(id: String) -> Result<String, String> {
    run_blocking(move || system_docs::read(&library_root(), &project_root(), &id)).await
}

// 知識の棚タブ(Ver5.0候補)向け。一覧と本文は`knowledge/`を直接読み(shelf.rs参照)、
// 検索だけRAGの`/search_shelf`を使う。
#[tauri::command]
async fn list_shelf_docs() -> Result<Vec<shelf::ShelfDocDto>, String> {
    run_blocking(move || Ok(shelf::list(&shelf_root()))).await
}

#[tauri::command]
async fn read_shelf_doc(name: String) -> Result<String, String> {
    run_blocking(move || shelf::read(&shelf_root(), &name)).await
}

// 棚の検索。関連のあるファイル名を、関連度の高い順で返す(リランカーの閾値は/searchと同じ0.3)。
#[tauri::command]
async fn search_shelf(query: String) -> Result<Vec<String>, String> {
    run_blocking(move || {
        let config = app_config()?;
        let hits = rag_client::search_shelf(config.rag.port, &query, 50, 0, None)
            .inspect_err(|e| shiori_log::error("RAG", "棚の検索に失敗した", &[("語", &query), ("理由", e)]))?;
        shiori_log::info(
            "SHELF",
            "棚で検索した",
            &[
                ("語", &query),
                ("件数", &hits.len()),
                ("上位", &shiori_log::summarize_hits(hits.iter().map(|h| (h.title.as_str(), h.source.as_str(), h.best_score)), 3)),
            ],
        );
        Ok(hits.into_iter().filter(|h| h.best_score >= 0.3).map(|h| h.source).collect())
    })
    .await
}

// スタンドアロン図書館UI(Phase 7、詩織Ver2.0設計指示書v3、10章)向け。検索を
// 経由せず、蔵書をファイル単位(1冊=1ファイル)に集約して一覧として返す。
#[tauri::command]
async fn list_all_knowledge() -> Result<Vec<LibraryFileDto>, String> {
    run_blocking(move || list_all_knowledge_blocking()).await
}

fn list_all_knowledge_blocking() -> Result<Vec<LibraryFileDto>, String> {
    let config = app_config()?;
    let files = rag_client::list_all_library(config.rag.port)?;
    Ok(files
        .into_iter()
        .map(|f| LibraryFileDto {
            source: f.source,
            source_category: f.source_category,
            headings: f.headings.into_iter().map(|h| h.heading).collect(),
            title: f.title,
            path: f.path,
            asset_path: f.asset_path,
            relative_path: f.relative_path,
            mtime: f.mtime,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLibraryHeadingDto {
    heading: String,
    rerank_score: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLibraryResultDto {
    source: String,
    source_category: String,
    headings: Vec<SearchLibraryHeadingDto>,
    best_score: f64,
    // 実ファイルへの絶対パス・frontmatterのtitle(2026-09-16追加)。
    path: String,
    asset_path: String,
    title: String,
    // 一覧に添える属性と冒頭の文(詩織Ver3.9)。記事を読めなかったときは空。
    summary: String,
    project: String,
    entry_kind: String,
    status: String,
    date: String,
}

// ライブラリウィンドウの検索結果ベースUI(詩織Ver3.0、UI改善4-2節)向け。
// MCPサーバー(mcp_server.rs)のsearch_libraryツールと同じ共有ロジック
// (rag_client::search_library)をGUI側からも呼べるようにするだけの薄いラッパー。
// 詩織Ver3.9で、図書館の絞り込み(アーカイブと古い記録の除外・棚・project・種類)を足した。
#[tauri::command]
async fn search_library(
    query: String,
    limit: u32,
    offset: u32,
    filter: LibraryFilterDto,
) -> Result<Vec<SearchLibraryResultDto>, String> {
    run_blocking(move || search_library_blocking(query, limit, offset, filter)).await
}

/// 図書館の検索の絞り込み(詩織Ver3.9)。空の項目は絞り込まない。
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFilterDto {
    #[serde(default)]
    include_stale: bool,
    #[serde(default)]
    source_categories: Vec<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

fn search_library_blocking(
    query: String,
    limit: u32,
    offset: u32,
    filter: LibraryFilterDto,
) -> Result<Vec<SearchLibraryResultDto>, String> {
    let config = app_config()?;
    let rag_filter = rag_client::LibrarySearchFilter {
        project: filter.project.as_deref().filter(|p| !p.is_empty()),
        entry_kind: filter.kind.as_deref().filter(|k| !k.is_empty()),
        source_categories: (!filter.source_categories.is_empty()).then_some(filter.source_categories.as_slice()),
        exclude_stale: !filter.include_stale,
        ..Default::default()
    };
    let results = rag_client::search_library(config.rag.port, &query, limit, offset, Some(rag_filter), false)
        .inspect_err(|e| shiori_log::error("RAG", "図書館の検索に失敗した", &[("語", &query), ("理由", e)]))?;
    shiori_log::info(
        "LIB",
        "図書館で検索した",
        &[
            ("語", &query),
            ("offset", &offset),
            ("件数", &results.len()),
            ("上位", &shiori_log::summarize_hits(results.iter().map(|r| (r.title.as_str(), r.source.as_str(), r.best_score)), 3)),
        ],
    );
    Ok(results
        .into_iter()
        .map(|r| {
            let preview = read_article_preview(&r.path);
            SearchLibraryResultDto {
            source: r.source,
            source_category: r.source_category,
            headings: r
                .headings
                .into_iter()
                .map(|h| SearchLibraryHeadingDto { heading: h.heading, rerank_score: h.rerank_score })
                .collect(),
            best_score: r.best_score,
            path: r.path,
            asset_path: r.asset_path,
            title: r.title,
            summary: preview.summary,
            project: preview.project,
            entry_kind: preview.entry_kind,
            status: preview.status,
            date: preview.date,
            }
        })
        .collect())
}

/// 検索結果の一覧に添える、記事の属性と冒頭の文(詩織Ver3.9)。
#[derive(Default)]
struct ArticlePreview {
    summary: String,
    project: String,
    entry_kind: String,
    status: String,
    date: String,
}

/// 冒頭の文として切り出す最大の文字数。一覧では2〜3行に収めて省略表示する。
const PREVIEW_MAX_CHARS: usize = 200;

/// `path`の記事(.mdまたは素材の.meta)のfrontmatterと本文から、一覧用の属性と
/// 冒頭の文を作る。冒頭の文はfrontmatterの`summary`、無ければ本文の最初の数行。
/// 読めない・frontmatterが壊れている記事は、分かる範囲だけ埋めて返す(一覧の
/// 表示を止めないため)。
fn read_article_preview(path: &str) -> ArticlePreview {
    if !(path.ends_with(".md") || path.ends_with(".meta")) {
        return ArticlePreview::default();
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return ArticlePreview::default();
    };
    let frontmatter = parse_display_frontmatter(&content).unwrap_or_default();
    let summary = frontmatter
        .summary
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| body_excerpt(&content));
    ArticlePreview {
        summary: summary.chars().take(PREVIEW_MAX_CHARS).collect(),
        project: frontmatter.project.unwrap_or_default(),
        entry_kind: frontmatter.entry_kind.unwrap_or_default(),
        status: frontmatter.status,
        date: frontmatter.date.as_ref().map(display_date).unwrap_or_default(),
    }
}

/// frontmatterの日付の値(日付だけ、日時、引用符の有無がまちまち)を、前後の空白を除いた文字列にする。
fn yaml_value_text(value: &serde_yaml::Value) -> String {
    let text = match value {
        serde_yaml::Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other).unwrap_or_default(),
    };
    text.trim().to_string()
}

/// frontmatterの`date`(日付だけ、日時、引用符の有無がまちまち)を`YYYY-MM-DD`にする。
fn display_date(value: &serde_yaml::Value) -> String {
    yaml_value_text(value).chars().take(10).collect()
}

/// frontmatterより後の本文から、見出し・表・コードなどを除いた最初の文を取り出す。
fn body_excerpt(content: &str) -> String {
    let body = content
        .strip_prefix("---")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[end + 4..]))
        .unwrap_or(content);
    let mut excerpt = String::new();
    let mut in_code = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.is_empty() || line.starts_with(['#', '|', '<']) || line.starts_with("---") {
            continue;
        }
        let line = line.trim_start_matches(['-', '*', '>', ' ']);
        let line = line.replace("**", "").replace('`', "").replace("[[", "").replace("]]", "");
        if !excerpt.is_empty() {
            excerpt.push(' ');
        }
        excerpt.push_str(&line);
        if excerpt.chars().count() >= PREVIEW_MAX_CHARS {
            break;
        }
    }
    excerpt
}

// frontmatterの必要フィールドのみを読み取る表示専用構造体。書き込み側
// (shiori_save.rs)のFrontmatter構造体とは目的が異なる(こちらは表示専用の
// 読み取りのみ)ため、意図的に別構造体として持つ。
#[derive(Debug, Default, Deserialize)]
struct DisplayFrontmatter {
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    // `type`(棚)とは別の、記事の種類(journal・resource・decisionなど)。
    #[serde(rename = "kind", default)]
    entry_kind: Option<String>,
    #[serde(default)]
    date: Option<serde_yaml::Value>,
    // 共通記憶MD(kind: memory)の最終更新日時(詩織Ver4.1)。他の記事には無い。
    #[serde(default)]
    updated: Option<serde_yaml::Value>,
    #[serde(default = "default_display_index")]
    index: bool,
    #[serde(default = "default_display_status")]
    status: String,
    #[serde(default)]
    related: Vec<String>,
}

fn default_display_index() -> bool {
    true
}

fn default_display_status() -> String {
    "new".to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFrontmatterDto {
    title: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    tags: Vec<String>,
    project: Option<String>,
    summary: Option<String>,
    index: bool,
    status: String,
    related: Vec<String>,
}

// 先頭`---`〜次の`---`をfrontmatterとしてパースする。shiori_save.rsの
// split_frontmatterと同種のロジックだが、bin側のためlib.rsから直接
// 呼べず、表示専用の軽量版としてここに個別実装している。
fn parse_display_frontmatter(content: &str) -> Result<DisplayFrontmatter, String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))
        .ok_or_else(|| "frontmatter(先頭の---)が見つかりません".to_string())?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| "frontmatterの終端(---)が見つかりません".to_string())?;
    serde_yaml::from_str(&rest[..end]).map_err(|e| format!("frontmatterの解析に失敗: {e}"))
}

// 記事詳細画面(詩織Ver3.0、UI改善4-2節)向け。get_source_documentは生
// Markdown全体(frontmatter込み)を返すのみのため、frontmatterだけを
// 構造化JSONで返す専用コマンドを別途用意する。
#[tauri::command]
async fn get_source_frontmatter(source_category: String, source: String) -> Result<SourceFrontmatterDto, String> {
    run_blocking(move || get_source_frontmatter_blocking(source_category, source)).await
}

fn get_source_frontmatter_blocking(source_category: String, source: String) -> Result<SourceFrontmatterDto, String> {
    let content = get_source_document_blocking(source_category, source)?;
    let fm = parse_display_frontmatter(&content)?;
    Ok(SourceFrontmatterDto {
        title: fm.title,
        kind: fm.kind,
        tags: fm.tags,
        project: fm.project,
        summary: fm.summary,
        index: fm.index,
        status: fm.status,
        related: fm.related,
    })
}

// 要確認UI(Phase 8、詩織Ver2.0設計指示書v3)向け。tags.yaml/projects.yamlの
// status(pending/confirmed/deferredの3値)、およびinboxファイルのfrontmatter
// review_status(confirmed/deferred、未設定=未着手)を人間がレビューするための
// 一覧取得・承認/却下/保留コマンド。
//
// tags.yaml/projects.yamlは先頭のコメントを人間が書いているため、serde_yamlで
// パース→丸ごと再シリアライズすると失われてしまう。そのため読み取りはパースで
// 行うが、書き込みは対象エントリのブロックをテキスト上で探して該当行だけを
// 書き換える(削除の場合はブロックごと除去する)方式にしている
// (shiori_save.rsの追記専用方針と同じ思想)。

/// ホームの申し送り(詩織Ver3.9。Ver4.1でmemoryの「やること」「申し送り・待ち」を優先)。
/// memoryから作ったとき(`from_memory`)は`todos`・`notes`を、journalから作ったときは`items`を使う。
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HandoffDto {
    title: String,
    project: String,
    date: String,
    source: String,
    source_category: String,
    items: Vec<String>,
    from_memory: bool,
    todos: Vec<String>,
    notes: Vec<String>,
}

/// 申し送りを探すとき、更新日時の新しい順に読むjournalの数。整理などで古い記事の
/// 更新日時が変わることがあるため、少し多めに読んでからfrontmatterのdateで選ぶ。
const HANDOFF_CANDIDATES: usize = 40;

/// ホームの申し送りを返す。プロジェクトのmemory(`updated`が最も新しいもの)の「やること」
/// 「申し送り・待ち」を優先する。memoryが1つも無い(または両方の節が空)ときは、従来どおり、
/// 最も新しいjournal(frontmatterのdate)のうち、「申し送り」を含む見出しの節を持つものから、
/// その箇条書きを返す。どちらも無ければNone。
#[tauri::command]
async fn get_latest_handoff() -> Result<Option<HandoffDto>, String> {
    run_blocking(|| {
        let config = app_config()?;
        let mut files = rag_client::list_all_library(config.rag.port)?;

        // 新しさは、shiori-saveが保存の都度書く`updated`で比べる(無ければdate)。どれも同じ形の
        // ISO8601(同じタイムゾーン)なので、文字列の比較で足りる。
        let memory = files
            .iter()
            .filter(|f| handoff_memory::is_project_memory_path(&f.relative_path))
            .filter_map(|f| {
                let content = std::fs::read_to_string(&f.path).ok()?;
                let frontmatter = parse_display_frontmatter(&content).ok()?;
                let sections = handoff_memory::memory_sections(&content);
                if sections.todos.is_empty() && sections.notes.is_empty() {
                    return None;
                }
                let stamp_value = frontmatter.updated.as_ref().or(frontmatter.date.as_ref());
                Some((
                    stamp_value.map(yaml_value_text).unwrap_or_default(),
                    HandoffDto {
                        title: frontmatter.title.unwrap_or_default(),
                        project: frontmatter.project.unwrap_or_default(),
                        date: stamp_value.map(display_date).unwrap_or_default(),
                        source: f.source.clone(),
                        source_category: f.source_category.clone(),
                        from_memory: true,
                        todos: sections.todos,
                        notes: sections.notes,
                        ..Default::default()
                    },
                ))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, dto)| dto);
        if memory.is_some() {
            return Ok(memory);
        }

        files.retain(|f| f.source.ends_with(".md"));
        files.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
        let latest = files
            .iter()
            .take(HANDOFF_CANDIDATES)
            .filter_map(|f| {
                let content = std::fs::read_to_string(&f.path).ok()?;
                let frontmatter = parse_display_frontmatter(&content).ok()?;
                if frontmatter.entry_kind.as_deref() != Some("journal") {
                    return None;
                }
                let items = handoff_items(&content);
                if items.is_empty() {
                    return None;
                }
                Some(HandoffDto {
                    title: frontmatter.title.unwrap_or_default(),
                    project: frontmatter.project.unwrap_or_default(),
                    date: frontmatter.date.as_ref().map(display_date).unwrap_or_default(),
                    source: f.source.clone(),
                    source_category: f.source_category.clone(),
                    items,
                    ..Default::default()
                })
            })
            .max_by(|a, b| a.date.cmp(&b.date));
        Ok(latest)
    })
    .await
}

/// 「申し送り」を含む見出しの節から、箇条書きの各項目を飾り(太字・コード・リンク)を
/// 外した1行ずつにして返す。入れ子の項目は親にまとめず、そのまま1行にする。
fn handoff_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            in_section = trimmed.contains("申し送り");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let item = item.replace("**", "").replace('`', "").replace("[[", "").replace("]]", "");
            items.push(item);
        }
    }
    items
}

/// ホームの状態行に出す、SSD(portable)の空き容量と全体の容量(GB)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageSpaceDto {
    free_gb: f64,
    total_gb: f64,
}

#[tauri::command]
async fn get_portable_storage() -> Result<StorageSpaceDto, String> {
    run_blocking(|| {
        let root = project_root();
        let gb = 1024.0 * 1024.0 * 1024.0;
        let free = fs4::available_space(&root).map_err(|e| format!("空き容量の取得に失敗: {e}"))?;
        let total = fs4::total_space(&root).map_err(|e| format!("容量の取得に失敗: {e}"))?;
        Ok(StorageSpaceDto { free_gb: free as f64 / gb, total_gb: total as f64 / gb })
    })
    .await
}

/// 図書館の絞り込みで選べるproject(詩織Ver3.9)。projects.yamlの登録順。
#[tauri::command]
async fn list_library_projects() -> Result<Vec<String>, String> {
    run_blocking(|| {
        let path = library_root().join("_system").join("projects.yaml");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("projects.yamlの読み込みに失敗: {e}"))?;
        let file: YamlProjectsFile =
            serde_yaml::from_str(&text).map_err(|e| format!("projects.yamlの解析に失敗: {e}"))?;
        Ok(file.projects.into_iter().map(|p| p.id).collect())
    })
    .await
}

#[derive(Deserialize)]
struct YamlProjectEntry {
    id: String,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct YamlProjectsFile {
    #[serde(default)]
    projects: Vec<YamlProjectEntry>,
}

#[derive(Deserialize)]
struct YamlTagEntry {
    canonical: String,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct YamlTagsFile {
    #[serde(default)]
    tags: Vec<YamlTagEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTagDto {
    canonical: String,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingProjectDto {
    id: String,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInboxItemDto {
    filename: String,
    title: Option<String>,
    reason: Option<String>,
    review_status: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingItemsDto {
    tags: Vec<PendingTagDto>,
    projects: Vec<PendingProjectDto>,
    inbox: Vec<PendingInboxItemDto>,
}

// inboxファイルのfrontmatterから、要確認UIの表示に必要な最小限の3項目
// (title/reason/review_status)だけを読み取る。他のfrontmatterフィールド
// (type/tags/project/author等)は要確認UIの範囲外(人間が直接編集する前提)
// のため、ここでは扱わない。
#[derive(Deserialize, Default)]
struct InboxFrontmatterFields {
    title: Option<String>,
    reason: Option<String>,
    review_status: Option<String>,
}

fn parse_inbox_frontmatter(content: &str) -> InboxFrontmatterFields {
    let trimmed = content.trim_start_matches('\u{feff}');
    let after_first = match trimmed.strip_prefix("---") {
        Some(rest) => rest,
        None => return InboxFrontmatterFields::default(),
    };
    let end = match after_first.find("\n---") {
        Some(i) => i,
        None => return InboxFrontmatterFields::default(),
    };
    let yaml_block = &after_first[..end];
    serde_yaml::from_str(yaml_block).unwrap_or_default()
}

#[tauri::command]
async fn list_pending_items() -> Result<PendingItemsDto, String> {
    run_blocking(move || list_pending_items_blocking()).await
}

/// 要確認の件数(画面の `pendingBadgeCount` と同じく、未着手だけを数える)。知らせの見張りが使う。
fn pending_badge_count() -> Option<usize> {
    let items = list_pending_items_blocking().ok()?;
    Some(
        items.tags.iter().filter(|t| t.status == "pending").count()
            + items.projects.iter().filter(|p| p.status == "pending").count()
            + items.inbox.iter().filter(|i| i.review_status.is_none()).count(),
    )
}

fn list_pending_items_blocking() -> Result<PendingItemsDto, String> {
    let root = library_root();

    let tags_path = root.join("_system").join("tags.yaml");
    let tags_text = std::fs::read_to_string(&tags_path)
        .map_err(|e| format!("tags.yamlの読み込みに失敗: {e}"))?;
    let tags_file: YamlTagsFile =
        serde_yaml::from_str(&tags_text).map_err(|e| format!("tags.yamlの解析に失敗: {e}"))?;
    let tags = tags_file
        .tags
        .into_iter()
        .filter(|t| t.status == "pending" || t.status == "deferred")
        .map(|t| PendingTagDto { canonical: t.canonical, status: t.status })
        .collect();

    let projects_path = root.join("_system").join("projects.yaml");
    let projects_text = std::fs::read_to_string(&projects_path)
        .map_err(|e| format!("projects.yamlの読み込みに失敗: {e}"))?;
    let projects_file: YamlProjectsFile = serde_yaml::from_str(&projects_text)
        .map_err(|e| format!("projects.yamlの解析に失敗: {e}"))?;
    let projects = projects_file
        .projects
        .into_iter()
        .filter(|p| p.status == "pending" || p.status == "deferred")
        .map(|p| PendingProjectDto { id: p.id, status: p.status })
        .collect();

    let inbox_dir = root.join("00-inbox");
    let mut inbox = Vec::new();
    if inbox_dir.is_dir() {
        let entries = std::fs::read_dir(&inbox_dir)
            .map_err(|e| format!("00-inbox/の読み込みに失敗: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("00-inbox/の読み込みに失敗: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("{filename}の読み込みに失敗: {e}"))?;
            let fields = parse_inbox_frontmatter(&content);
            if fields.review_status.as_deref() == Some("confirmed") {
                continue;
            }
            inbox.push(PendingInboxItemDto {
                filename,
                title: fields.title,
                reason: fields.reason,
                review_status: fields.review_status,
            });
        }
    }

    Ok(PendingItemsDto { tags, projects, inbox })
}

// tags.yaml/projects.yamlの、`  - {key_field}: {key_value}`で始まるブロックの
// 行範囲([開始行, 終了行))を返す。終了行は次の`  - `行、または末尾。
fn find_yaml_list_item_range(lines: &[String], key_field: &str, key_value: &str) -> Option<(usize, usize)> {
    let marker = format!("- {key_field}:");
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(&marker) {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if value == key_value {
                start = Some(i);
                break;
            }
        }
    }
    let start = start?;
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim_start().starts_with("- ") && line.starts_with("  -") {
            end = i;
            break;
        }
    }
    Some((start, end))
}

fn rewrite_yaml_status(path: &Path, key_field: &str, key_value: &str, new_status: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("{}の読み込みに失敗: {e}", path.display()))?;
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let (start, end) = find_yaml_list_item_range(&lines, key_field, key_value)
        .ok_or_else(|| "対象のエントリが見つかりませんでした。".to_string())?;

    let status_line_idx = (start..end)
        .find(|&i| lines[i].trim_start().starts_with("status:"))
        .ok_or_else(|| "statusフィールドが見つかりませんでした。".to_string())?;
    let indent_len = lines[status_line_idx].len() - lines[status_line_idx].trim_start().len();
    lines[status_line_idx] = format!("{}status: {new_status}", " ".repeat(indent_len));

    let mut new_text = lines.join("\n");
    if text.ends_with('\n') {
        new_text.push('\n');
    }
    std::fs::write(path, new_text).map_err(|e| format!("{}への書き込みに失敗: {e}", path.display()))
}

fn remove_yaml_list_item(path: &Path, key_field: &str, key_value: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("{}の読み込みに失敗: {e}", path.display()))?;
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let (start, end) = find_yaml_list_item_range(&lines, key_field, key_value)
        .ok_or_else(|| "対象のエントリが見つかりませんでした。".to_string())?;

    let mut remaining: Vec<String> = lines[..start].to_vec();
    remaining.extend_from_slice(&lines[end..]);

    let mut new_text = remaining.join("\n");
    if !remaining.is_empty() && text.ends_with('\n') {
        new_text.push('\n');
    }
    std::fs::write(path, new_text).map_err(|e| format!("{}への書き込みに失敗: {e}", path.display()))
}

fn pending_action_to_status(action: &str) -> Result<&'static str, String> {
    match action {
        "confirm" => Ok("confirmed"),
        "defer" => Ok("deferred"),
        _ => Err(format!("不明なaction: {action}")),
    }
}

#[tauri::command]
async fn resolve_pending_tag(canonical: String, action: String) -> Result<(), String> {
    run_blocking(move || resolve_pending_tag_blocking(canonical, action)).await
}

fn resolve_pending_tag_blocking(canonical: String, action: String) -> Result<(), String> {
    let path = library_root().join("_system").join("tags.yaml");
    if action == "reject" {
        return remove_yaml_list_item(&path, "canonical", &canonical);
    }
    let status = pending_action_to_status(&action)?;
    rewrite_yaml_status(&path, "canonical", &canonical, status)
}

#[tauri::command]
async fn resolve_pending_project(id: String, action: String) -> Result<(), String> {
    run_blocking(move || resolve_pending_project_blocking(id, action)).await
}

fn resolve_pending_project_blocking(id: String, action: String) -> Result<(), String> {
    let path = library_root().join("_system").join("projects.yaml");
    if action == "reject" {
        return remove_yaml_list_item(&path, "id", &id);
    }
    let status = pending_action_to_status(&action)?;
    rewrite_yaml_status(&path, "id", &id, status)
}

// inboxファイルのfrontmatter内、`review_status:`行を書き換える(無ければ
// frontmatter終端の直前に追加する)。frontmatter以外の本文には触れない。
fn rewrite_inbox_review_status(path: &Path, new_status: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("{}の読み込みに失敗: {e}", path.display()))?;
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("frontmatterが見つかりませんでした。".to_string());
    }
    let closing_idx = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim() == "---")
        .map(|(i, _)| i)
        .ok_or_else(|| "frontmatterの終端が見つかりませんでした。".to_string())?;

    if let Some(status_idx) = (1..closing_idx).find(|&i| lines[i].starts_with("review_status:")) {
        lines[status_idx] = format!("review_status: {new_status}");
    } else {
        lines.insert(closing_idx, format!("review_status: {new_status}"));
    }

    let mut new_text = lines.join("\n");
    if text.ends_with('\n') {
        new_text.push('\n');
    }
    std::fs::write(path, new_text).map_err(|e| format!("{}への書き込みに失敗: {e}", path.display()))
}

// filenameはUI側がlist_pending_itemsで取得した一覧由来の値のみを渡す前提だが、
// パストラバーサル対策としてファイル名部分のみであること(区切り文字を含まない)を
// 念のため検証する。
fn validate_plain_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') || filename == ".." {
        return Err("不正なファイル名です。".to_string());
    }
    Ok(())
}

#[tauri::command]
async fn resolve_pending_inbox_item(filename: String, action: String) -> Result<(), String> {
    run_blocking(move || resolve_pending_inbox_item_blocking(filename, action)).await
}

fn resolve_pending_inbox_item_blocking(filename: String, action: String) -> Result<(), String> {
    validate_plain_filename(&filename)?;
    let path = library_root().join("00-inbox").join(&filename);

    if action == "reject" {
        return std::fs::remove_file(&path).map_err(|e| format!("{filename}の削除に失敗: {e}"));
    }
    let status = pending_action_to_status(&action)?;
    rewrite_inbox_review_status(&path, status)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageReply {
    reply: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sources: Option<Vec<KnowledgeResultDto>>,
    detected_mode: String,
}


// 「棚を教える」方針(2026-08-12、図書館ビジョン統合仕様書2章)により、LLMには
// チャンク本文を一切渡さない。見出し・出典・カテゴリのみを伝え、「この話題の
// 記録はこの棚にある」という所在情報だけを渡す。本文の要約・解釈をLLMにさせない
// ことを、プロンプトの指示ではなく構造的に不可能にするのが狙い。UI向けの
// KnowledgeResultDto(sources_out)は本文込みのまま渡す点に注意(人間はUI上で
// 全文閲覧できる。制限されるのはLLMに渡す側のみ)。
fn format_shelf_reference(r: &rag_client::SearchResultItem) -> String {
    format!("[{}] {}(分類: {})", r.source, r.heading, r.source_category)
}

// ツール名 -> 引数(JSON) を受け取り実行し、LLMに返すためのツール結果文字列(JSON)を返す。
// search_knowledge実行時はナレッジパネル表示用にsources_outへ検索結果を書き込む。
fn execute_tool(
    name: &str,
    args: &serde_json::Value,
    config: &AppConfig,
    sources_out: &mut Option<Vec<KnowledgeResultDto>>,
) -> Result<String, String> {
    match name {
        "search_knowledge" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let normalized_query = normalize_search_query(query);
            let relevant =
                rag_client::search(config.rag.port, &normalized_query, 5).unwrap_or_default();
            // 以前はここで生のembedding距離(config.rag.passive_recall_threshold)
            // による再フィルタをかけていたが、/search側にリランカー(2026-08-07
            // 導入)が入り、rerank_score>=0.3で既に関連性フィルタ済みの結果が
            // 返るようになった。生距離での再フィルタは「リランカーが関連ありと
            // 判定したものを、後からより粗い指標で棄却するだけ」の経路になり、
            // 良い結果を減らす方向にしか働かないため撤去した(2026-08-08、
            // メモ機能の想起調査で発見。docs/voice-consistency-policy.md参照)。
            if relevant.is_empty() {
                return Ok("該当する記録は見つかりませんでした。".to_string());
            }
            let context = relevant
                .iter()
                .map(format_shelf_reference)
                .collect::<Vec<_>>()
                .join("\n\n");
            *sources_out = Some(
                relevant
                    .into_iter()
                    .map(|r| KnowledgeResultDto {
                        id: r.id,
                        text: r.text,
                        source: r.source,
                        heading: r.heading,
                        source_category: r.source_category,
                    })
                    .collect(),
            );
            Ok(context)
        }
        other => Err(format!("未知のツール: {other}")),
    }
}

const TOOL_CALL_MAX_ITERATIONS: u32 = 4;

const KNOWN_TOOL_NAMES: [&str; 1] = ["search_knowledge"];

// 量子化モデルがtool_callsを正しく発行せず、代わりに応答本文へ関数呼び出し風の
// 生文字列を書いてしまうことがある(表示ガード)。実測で確認できている漏れ方は
// 2パターン:
// 1. 「search_knowledge {"query": "..."}」のように、ツール名がそのまま
//    文頭に書かれる形式
// 2. 「{"name": "search_knowledge", "arguments": {...}}」のように、OpenAI
//    形式のtool_calls本体をJSONのまま丸ごと書いてしまう形式(前後に無関係な
//    文字や`</tool_call>`のような特殊トークンの断片が混ざることもある)。
//    10回に1回程度の頻度で実機投入後に発見された(docs/voice-consistency-
//    policy.md参照。当時はupdate_taskで発見されたが、仕組み自体は
//    ツールの種類に依存しない)
// どちらの形式でも、検知できたら実際にツールを実行する「救済実行」につなげる
// (execute_tool呼び出し側は形式を区別しない)。
fn parse_leaked_tool_call(content: &str) -> Option<(String, serde_json::Value)> {
    let trimmed = content.trim();

    // パターン1: ツール名で始まる形式
    for name in KNOWN_TOOL_NAMES {
        let Some(rest) = trimmed.strip_prefix(name) else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('(').unwrap_or(rest.trim_start());
        let (Some(json_start), Some(json_end)) = (rest.find('{'), rest.rfind('}')) else {
            continue;
        };
        if json_end < json_start {
            continue;
        }
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(&rest[json_start..=json_end]) {
            return Some((name.to_string(), args));
        }
    }

    // パターン2: {"name": "...", "arguments": {...}}形式のJSON全体。
    // 前後に混入したゴミ文字(特殊トークンの断片等)を無視するため、最初の
    // '{'から最後の'}'までを取り出してパースする。
    let json_start = trimmed.find('{')?;
    let json_end = trimmed.rfind('}')?;
    if json_end < json_start {
        return None;
    }
    let candidate: serde_json::Value =
        serde_json::from_str(&trimmed[json_start..=json_end]).ok()?;
    let name = candidate.get("name")?.as_str()?;
    if !KNOWN_TOOL_NAMES.contains(&name) {
        return None;
    }
    let args = candidate.get("arguments")?.clone();
    Some((name.to_string(), args))
}

// 自発的想起を「LLMに検索すべきか判断させる」のではなく「毎ターン軽量に検索し、
// 使うかどうかは応答生成そのものに委ねる」方式に置き換えたもの。明示的な検索依頼
// (search_knowledgeツール)とは完全に独立した経路で、互いに干渉しない。
const PASSIVE_RECALL_TOP_K: u32 = 3;

// しきい値は0.75を仮値として試したところ、雑談の挨拶や無関係な話題でも0.65〜0.70
// 程度で偶然ヒットしてしまうことが実測で分かった(ダミーデータが4件と少なく、
// 意味的にほぼ無関係でもこの範囲に収まりやすいため)。実際に関連した話題
// (例: CUDAビルドの雑談)は0.40程度とはっきり低く、その間に十分な余地が
// あったため0.55に厳格化した経緯がある。実データ移行後に再調整が必要になる
//想定のため、定数ではなくconfig.jsonの値(コントロールパネルから変更可能)を使う。
// 検索クエリからフィラー的な口語表現(「〜んだっけ」「〜かな」等)を軽く取り除く。
// ルールの中身はprompts/transforms/query-normalization.jsonにある。
// 「〜って」は当初含めていたが、「使ってる」→「使る」のように文中の活用形を
// 壊すことが判明したため、ルールファイル側から除外している
// (経緯はdocs/voice-consistency-policy.md参照)。
fn normalize_search_query(query: &str) -> String {
    match text_transform::load_rules("query-normalization.json") {
        Ok(rules) => text_transform::apply_rules(query, &rules).0,
        Err(e) => {
            log_error!("[query_normalize] ルール読み込みに失敗、正規化をスキップ: {e}");
            query.to_string()
        }
    }
}

// デバッグ画面向け、常時バックグラウンド検索(passive recall)のヒット率集計
// (v1.0スコープ機能2)。「直近N件、またはセッション単位の集計で構わない」
// という要件のため、永続化はせずプロセス内のin-memoryカウンタのみで良いと
// 判断した(アプリ再起動でリセットされる)。
static PASSIVE_RECALL_HIT_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PASSIVE_RECALL_TOTAL_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveRecallStatsDto {
    hits: u32,
    total: u32,
}

#[tauri::command]
fn get_passive_recall_stats() -> PassiveRecallStatsDto {
    use std::sync::atomic::Ordering;
    PassiveRecallStatsDto {
        hits: PASSIVE_RECALL_HIT_COUNT.load(Ordering::Relaxed),
        total: PASSIVE_RECALL_TOTAL_COUNT.load(Ordering::Relaxed),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagasHistoryDto {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

// デバッグ画面向け、services/rag/eval/results/history.csvの内容をそのまま
// 表形式で返す(v1.0スコープ機能2)。run_eval.py側の値はカンマを含まない
// (タグ・ISO日時・数値のみ)ため、外部crateを追加せず単純なsplit(',')で
// パースしている。ファイルが無い場合(RAGAS評価をまだ実行していない場合)は
// エラーにせず空の表を返す。
#[tauri::command]
async fn get_ragas_history() -> Result<RagasHistoryDto, String> {
    run_blocking(move || get_ragas_history_blocking()).await
}

fn get_ragas_history_blocking() -> Result<RagasHistoryDto, String> {
    let csv_path = project_root()
        .join("services")
        .join("rag")
        .join("eval")
        .join("results")
        .join("history.csv");

    if !csv_path.exists() {
        return Ok(RagasHistoryDto { headers: vec![], rows: vec![] });
    }

    let text =
        std::fs::read_to_string(&csv_path).map_err(|e| format!("history.csvの読み込みに失敗: {e}"))?;
    let mut lines = text.lines();
    let headers = lines
        .next()
        .map(|h| h.split(',').map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let rows = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(',').map(|s| s.to_string()).collect())
        .collect();
    Ok(RagasHistoryDto { headers, rows })
}

// 自発的想起がヒットした場合、そのチャンクを`sources_out`にも書き込む。
// これによりKnowledgePanel(UI)が「明示的検索」「identity_guard」だけでなく
// passive recallでも参考情報の痕跡を表示できるようになる(2026-08-07、
// UI/UX改善の優先度A-1対応)。ヒットしなかったターンでは何もセットしない
// (「検索しましたが見つかりませんでした」等を無理に出さない既存方針と一貫させる)。
//
// 以前はここで生のembedding距離による再フィルタをかけていたが、/search側の
// リランカー(2026-08-07導入)が既にrerank_score>=0.3で関連性フィルタ済みの
// 結果を返すため撤去した(execute_toolのsearch_knowledge分岐と同じ理由。
// 詳細はそちらのコメント・docs/voice-consistency-policy.md参照)。
fn build_passive_recall_context(
    port: u16,
    text: &str,
    sources_out: &mut Option<Vec<KnowledgeResultDto>>,
) -> Option<String> {
    let normalized = normalize_search_query(text);
    let relevant = rag_client::search(port, &normalized, PASSIVE_RECALL_TOP_K).ok()?;
    if relevant.is_empty() {
        return None;
    }
    let context = relevant
        .iter()
        .map(format_shelf_reference)
        .collect::<Vec<_>>()
        .join("\n\n");
    *sources_out = Some(
        relevant
            .into_iter()
            .map(|r| KnowledgeResultDto {
                id: r.id,
                text: r.text,
                source: r.source,
                heading: r.heading,
                source_category: r.source_category,
            })
            .collect(),
    );
    Some(format!(
        "以下は、今の会話に関連するかもしれない記録の所在です(本文はここには\n\
         含まれていません)。関連がありそうなら、「そういえば、そのことについての\n\
         記録がありますよ」のように、記録があることだけを自然に伝えてください。\n\
         中身は自分でも読んでいないので、聞かれても内容までは答えず、「開いて\n\
         確認してみてください」と案内してください。関連が薄い、あるいは話の腰を\n\
         折ると感じたら、無理に触れる必要はありません。\n\n\
         [参考情報]\n{context}"
    ))
}

// 詩織が自分自身のことを三人称(「詩織は〜」)で語ってしまう不具合の保険。
// ルールの中身はprompts/transforms/person-correction.jsonにあり、ここでは
// text_transformエンジンを呼び出すだけの薄いラッパーにしている。
// 各ルールの追加経緯・リスク判断はdocs/voice-consistency-policy.mdを参照。
// 応答後の事実確認ガード(詩織Ver3.6)。Ver3.6の測定で、確かめていないことを
// 「ある/やった」と言う作り話が見つかった(参照情報が無いのに「宇宙旅行の記録が
// ありますね。開いてみますか?」、保存の処理が動いていないのに「メモしておき
// ましたよ」)。プロンプトの指示だけではQwen3-8Bが守りきれないため、identity_guard
// と同じく、LLMの協力に頼らずRust側で検知して正直な文に置き換える。
//
// 否定形(「記録がありません」「残っていません」)に当たらないよう、肯定形の
// 言い回しだけを並べている。
const RECORD_EXISTENCE_CLAIMS: [&str; 8] = [
    // 「2つ目の記録は〜」のように、前のターンで案内した記録を指す言い方。
    // 前のターンに記録がないのにこう答える作り話が再測定で見つかった。
    "目の記録",
    "記録があります",
    "記録があるかも",
    "記録が残っています",
    "メモがあります",
    "メモがあるかも",
    "メモが残っています",
    "開いてみますか",
];
const SAVE_CLAIMS: [&str; 8] = [
    "メモしておきました",
    "メモしました",
    "保存しておきました",
    "保存しました",
    "記録しておきました",
    "記録しました",
    "覚えておきました",
    "覚えました",
];
const NO_RECORD_REPLY: &str =
    "それについての記録は、見当たらないみたいです。";
const NOT_SAVED_REPLY: &str =
    "すみません、まだ保存はしていません。残しておきたいときは「〜ってメモして」のように言ってもらえれば、記録しておきますね。";

const APP_STATE_REPLY: &str =
    "すみません、それは私からは確認できないんです。要確認の画面で見てもらえますか？";

fn correct_unverified_claims(
    reply: &str,
    user_text: &str,
    memo_executed: bool,
    has_references: bool,
    app_state_question: bool,
) -> String {
    // app_state_guardの指示を受けても「記録は見当たらない」のように答えることが
    // 再測定で見つかったため、「確認できない」と言っていなければ置き換える。
    let corrected = if app_state_question && !reply.contains("確認でき") {
        Some(APP_STATE_REPLY)
    } else if !memo_executed && SAVE_CLAIMS.iter().any(|p| reply.contains(p)) {
        Some(NOT_SAVED_REPLY)
    } else if !has_references && RECORD_EXISTENCE_CLAIMS.iter().any(|p| reply.contains(p)) {
        Some(NO_RECORD_REPLY)
    } else {
        None
    };
    match corrected {
        Some(replacement) => {
            // 会話の中身は残さず、字数だけを残す(Ver6.0のログの決まり)
            shiori_log::info(
                "LLM",
                "確かめていない内容の応答を置き換えた",
                &[
                    ("発話の字数", &user_text.chars().count()),
                    ("置き換え前の字数", &reply.chars().count()),
                    ("置き換え後の字数", &replacement.chars().count()),
                ],
            );
            replacement.to_string()
        }
        None => reply.to_string(),
    }
}

// 要確認リスト(タグ・プロジェクト・inbox)のように、詩織が会話の中で確かめる手段を
// 持たないアプリの状態を尋ねる発言。測定(Ver3.6、S9)では「要確認のタグは
// 見かけませんね」と断定する作り話が見つかった。読み取りの道具を持たせるのは
// Ver3.7(管理機能)の範囲のため、ここでは「確かめられない」と正直に答えさせる。
const APP_STATE_KEYWORDS: [&str; 3] = ["要確認", "未承認", "pending"];

fn is_app_state_question(text: &str) -> bool {
    APP_STATE_KEYWORDS.iter().any(|k| text.contains(k))
}

const APP_STATE_GUARD_CONTEXT: &str = "ふらるさんは、要確認リスト(承認待ちのタグ・プロジェクト・inboxの項目)など、\n\
     アプリの状態について尋ねています。今のあなたには、それを会話の中で確かめる手段が\n\
     ありません。「ある」とも「ない」とも断定せず、「私からは確認できないので、\n\
     要確認の画面で見てもらえますか」のように、正直に伝えてください。";

fn correct_third_person_self_reference(text: &str) -> String {
    let rules = match text_transform::load_rules("person-correction.json") {
        Ok(rules) => rules,
        Err(e) => {
            log_error!("[self_reference_guard] ルール読み込みに失敗、補正をスキップ: {e}");
            return text.to_string();
        }
    };
    let (corrected, triggered) = text_transform::apply_rules(text, &rules);

    if !triggered.is_empty() {
        shiori_log::info("LLM", "三人称を一人称に補正した", &[("規則", &format!("{triggered:?}"))]);
    }

    corrected
}

// 「詩織の由来」「詩織はどのLLMを使ってるの」等、詩織自身のアイデンティティ・
// 技術的な仕組みに関わる質問はpassive recallのヒット率が不安定で、外れると
// 作り話(『源氏物語』由来、LLMを使っていない等)が発生することが実測で分かっている。
//
// 当初はOpenAI互換のtool_choiceでsearch_knowledgeの呼び出しをLLMに強制する
// 方式を試みたが、system-prompt.mdのような長いsystemメッセージと組み合わせると
// llama-serverがtool_choiceを無視し、finish_reason=stopの通常応答を返す
// ことが実測で判明した(短いsystemメッセージ単体では機能するため、モデル/
// サーバー側の実装が長いプロンプトでは強制ルーティングに対応しきれていない
// と見られる)。LLMの協力に依存する方式は信頼できないため、該当キーワードを
// 検知したらRust側でsearch_knowledgeを直接実行し、その結果を強い調子の
// system contextとして注入する方式に切り替えた。
//
// 単独で判定してよい語(標準キーワード)と、「詩織」との組み合わせでのみ
// 判定する語(汎用語すぎて単独判定だと無関係な会話で誤爆するもの、例:
// 「モデル」「仕組み」「使って」)を分けている。漏れが見つかった場合は
// ここに追加していく。
//
// 「なんで」「どうして」は元々「なんで詩織」「どうして詩織」という固定語順の
// 標準キーワードだったが、単純な部分文字列一致のため「詩織ってなんでその
// 名前なの」のように語順が逆(詩織が先)だと検知できないバグがあった
// (2026-08-13、Qwen3-8B移行検証のphase_identity_guard_checkで発覚)。
// コンボキーワード側(「詩織」の有無と個別に判定)に移すことで、語順に
// 依存せず検知できるようにしている。
const IDENTITY_STANDALONE_KEYWORDS: [&str; 7] = [
    "由来", "名前の意味", "LLM", "頭脳", "音声認識", "STT", "TTS",
];
const IDENTITY_COMBO_KEYWORDS: [&str; 7] =
    ["仕組み", "モデル", "記憶", "声", "使って", "なんで", "どうして"];

fn is_identity_question(text: &str) -> bool {
    if IDENTITY_STANDALONE_KEYWORDS.iter().any(|k| text.contains(k)) {
        return true;
    }
    text.contains("詩織") && IDENTITY_COMBO_KEYWORDS.iter().any(|k| text.contains(k))
}

// 距離(embedding距離)による足切りだけでは、この規模のデータでは正しい
// チャンクと無関係なチャンクの距離が逆転することがあると実測で判明した
// (例:「詩織のキャラクター名の由来って何」で、正しい「詩織の名前の由来」
// チャンクが距離0.742なのに対し、無関係な「基本情報・目指すもの」(ふらる
// 自身についての記述)チャンクが距離0.6803とより近い判定になった)。
// 距離のみでの閾値調整では、正しいチャンクを含めようとすると無関係な
// チャンクも一緒に混入してしまい、閾値を絞ると今度は正しいチャンクごと
// 除外されてしまう、というトレードオフが解消できなかった。
//
// そこで、`heading`メタデータを使った絞り込みに切り替えた。
// docs/voice-consistency-policy.mdの方針により、見出しは検索性のために
// 意図的に「詩織」という語を残す設計になっている(例: `詩織の頭脳（LLM）`
// `詩織の名前の由来`)。アイデンティティ質問はキーワードで既に「詩織自身に
// ついての質問」と確定しているため、見出しに「詩織」を含むチャンクだけに
// 絞り込むことで、距離の逆転に影響されずに正しいチャンクを一意に特定できる。
// 「詩織」は自己紹介・将来の目標・日記機能等、幅広い見出しに登場するため、
// 上位3件程度だと本当に聞かれている話題(例:「詩織の名前の由来」)が
// 他の「詩織」関連見出しに押し出されて漏れることがあった(実測で5位相当)。
// 候補プールと採用件数を広めに取り、複数の関連チャンクをLLMに渡した上で
// 「実際に質問に答えているものだけ使う」判断はLLM自身に委ねる。
const IDENTITY_GUARD_CANDIDATE_TOP_K: u32 = 10;
const IDENTITY_GUARD_MAX_RESULTS: usize = 5;
// 見出し絞り込みが効いているぶん、ここでの距離上限はノイズ除去程度の
// 緩い値でよい(通常のpassive_recall/search_knowledgeの0.61より緩い)。
const IDENTITY_GUARD_DISTANCE_CEILING: f64 = 0.9;

// アイデンティティ質問と判定された場合に、search_knowledgeをRust側で直接
// 実行して結果をcontextとして組み立てる。passive_recallと違い「関連が薄ければ
// 触れなくてよい」ではなく、「この情報を優先して使い、なければ正直に分からないと
// 言う」という強い指示にすることで、作り話の発生を抑える。
fn build_identity_guard_context(
    config: &AppConfig,
    text: &str,
    sources_out: &mut Option<Vec<KnowledgeResultDto>>,
) -> Option<String> {
    let normalized = normalize_search_query(text);
    let results =
        rag_client::search(config.rag.port, &normalized, IDENTITY_GUARD_CANDIDATE_TOP_K).ok()?;
    let mut relevant: Vec<_> = results
        .into_iter()
        .filter(|r| r.heading.contains("詩織"))
        .filter(|r| r.distance <= IDENTITY_GUARD_DISTANCE_CEILING)
        .collect();
    relevant.truncate(IDENTITY_GUARD_MAX_RESULTS);

    let tool_result = if relevant.is_empty() {
        "該当する記録は見つかりませんでした。".to_string()
    } else {
        let context = relevant
            .iter()
            .map(format_shelf_reference)
            .collect::<Vec<_>>()
            .join("\n\n");
        *sources_out = Some(
            relevant
                .into_iter()
                .map(|r| KnowledgeResultDto {
                    id: r.id,
                    text: r.text,
                    source: r.source,
                    heading: r.heading,
                    source_category: r.source_category,
                })
                .collect(),
        );
        context
    };

    Some(format!(
        "ふらるさんは詩織自身のアイデンティティや技術的な仕組み(名前の由来、\n\
         使用しているLLM・音声認識・記憶の探し方など)について質問しています。\n\
         以下の[参考情報]は、その質問に対してsearch_knowledgeを実行した結果の\n\
         所在情報です(本文はここには含まれていません)。複数の話題の見出しが\n\
         混ざっていることがあるので、その中から実際にこの質問に関係していそうな\n\
         ものだけを使ってください。中身を答えるのではなく、「その話についての\n\
         記録がありますよ、開いて確認してみてください」のように、記録の場所を\n\
         案内してください。想像で由来や仕組みを作り出すことは絶対にしないで\n\
         ください。関係していそうな見出しが見当たらない場合や、「該当する記録は\n\
         見つかりませんでした」と書かれている場合は、「まだ決まっていないようです」\n\
         ではなく、正直に「それは、ちょっと分からないですね」のように伝えてください。\n\n\
         [参考情報]\n{tool_result}"
    ))
}

// 「前はどうだったか」「以前と何が変わったか」のような、経緯・変遷を尋ねる
// 質問はidentity_guardと同じ理由(LLMの協力に依存する強制ルーティングは
// 信頼できない)でRust側のキーワード検知に切り替える(詩織Ver3.3、時間
// 認識検索)。この種の質問はRAGの通常モード(現在モード、新しさ優先+
// deprecated除外)では過去の情報が不利に扱われ、「前はどうだったか」に
// 正しく答えられない構造的な弱さがあるため、経緯モード(deprecated/
// archive込み、date降順)での検索を強制する。
//
// 「変わった」は「気持ちが変わった」等、時間の経緯を尋ねる質問以外でも
// 頻出するため誤爆しやすく、標準キーワードから外している。
const HISTORY_GUARD_KEYWORDS: [&str; 4] = ["前は", "以前は", "昔は", "かつて"];

fn is_history_question(text: &str) -> bool {
    HISTORY_GUARD_KEYWORDS.iter().any(|k| text.contains(k))
}

// identity_guardのIDENTITY_GUARD_CANDIDATE_TOP_K/MAX_RESULTSと同じ考え方。
// 経緯モードはheadingでの絞り込みを行わない(アイデンティティ質問と違い
// 話題が「詩織自身」に限定されないため)ぶん、候補・採用件数はidentity_guard
// と揃えている。
const HISTORY_GUARD_CANDIDATE_TOP_K: u32 = 10;
const HISTORY_GUARD_MAX_RESULTS: usize = 5;

// 経緯を尋ねる質問と判定された場合に、search_knowledgeを経緯モード
// (rag_client::search_history)でRust側が直接実行し、結果をcontextとして
// 組み立てる(build_identity_guard_contextと同じ構造)。
fn build_history_guard_context(
    config: &AppConfig,
    text: &str,
    sources_out: &mut Option<Vec<KnowledgeResultDto>>,
) -> Option<String> {
    let normalized = normalize_search_query(text);
    let mut relevant =
        rag_client::search_history(config.rag.port, &normalized, HISTORY_GUARD_CANDIDATE_TOP_K)
            .ok()?;
    relevant.truncate(HISTORY_GUARD_MAX_RESULTS);

    let tool_result = if relevant.is_empty() {
        "該当する記録は見つかりませんでした。".to_string()
    } else {
        let context = relevant
            .iter()
            .map(format_shelf_reference)
            .collect::<Vec<_>>()
            .join("\n\n");
        *sources_out = Some(
            relevant
                .into_iter()
                .map(|r| KnowledgeResultDto {
                    id: r.id,
                    text: r.text,
                    source: r.source,
                    heading: r.heading,
                    source_category: r.source_category,
                })
                .collect(),
        );
        context
    };

    Some(format!(
        "ふらるさんは過去と現在の経緯・変遷について質問しています(「前は」「以前は」\n\
         のような言い回し)。以下の[参考情報]は、deprecated・アーカイブ済みの記録も\n\
         含めて新しい順に並べた検索結果の所在情報です(本文はここには含まれていません)。\n\
         古い記録と新しい記録が混在していることを踏まえ、何がいつ変わったのかを\n\
         見比べられるように案内してください。中身を答えるのではなく、「その経緯に\n\
         ついての記録がありますよ、開いて確認してみてください」のように、記録の場所を\n\
         案内してください。想像で経緯を作り出すことは絶対にしないでください。\n\
         関係していそうな記録が見当たらない場合や、「該当する記録は見つかりませんでした」\n\
         と書かれている場合は、正直に「それは、ちょっと分からないですね」のように\n\
         伝えてください。\n\n\
         [参考情報]\n{tool_result}"
    ))
}

// 「メモして」「記録して」「覚えておいて」という明確な保存意図をRust側で検知し、
// LLMの判断を経由せず直接メモを保存する(identity_guardと同じ考え方。以前の
// task_guardと同様、「保存すべきかどうか」をLLMの判断に委ねると、呼び出しを
// 試みない・形式が漏れる等、信頼できないことがこれまでの検証で分かっている)。
//
// Ver3.6(2026-10-06)で、口語の言い回しを追加した。「これ忘れないようにしといて:〜」
// がキーワードに当たらず保存されないまま、LLMが「メモしておきましたよ」と答える
// 虚偽の報告が測定で見つかったため(保存をツールにしてLLMに判断させる案は、上記の
// 理由で採らなかった)。先頭から順に照合するため、長い言い回しを先に置く。
const MEMO_TRIGGER_KEYWORDS: [&str; 12] = [
    "忘れないようにしておいて",
    "忘れないようにしといて",
    "メモしておいて",
    "メモしといて",
    "メモっといて",
    "メモして",
    "記録しておいて",
    "記録しといて",
    "記録して",
    "覚えておいて",
    "覚えといて",
    "忘れないで",
];

fn parse_memo_intent(text: &str) -> Option<String> {
    let trigger = MEMO_TRIGGER_KEYWORDS.iter().copied().find(|k| text.contains(k))?;
    let content = text.replacen(trigger, "", 1);
    let content = content.trim();
    // 「これ忘れないようにしといて:〜」のように、指示語+区切り記号で内容が
    // 後ろに続く形は、指示語を取り除く(「これから〜」のような語は残す)。
    let content = ["これ", "それ"]
        .iter()
        .find_map(|p| {
            content
                .strip_prefix(p)
                .filter(|rest| rest.starts_with([':', '：']))
        })
        .unwrap_or(content);
    let content = content
        .trim_start_matches([':', '：', '、', ' ', '　'])
        .trim_end_matches(['。', '、'])
        .trim()
        .to_string();
    // 指示語だけが残った場合(「これ忘れないようにしといて」)は、何を保存すべきか
    // 分からないため保存しない。
    if content.is_empty() || content == "これ" || content == "それ" {
        None
    } else {
        Some(content)
    }
}

// 検知したメモ内容を実際に保存する。library/00-inbox/へMarkdownファイルとして
// 書き出し(ChromaDBが壊れても元データが残るように)、続けてRAGサーバーの
// /add_documentで即座に埋め込み・ChromaDBへ登録する(passive recallの対象に
// 即時反映するため、ingest.pyの再実行は不要にしている)。結果をLLMへの強い
// 指示付きcontextとして組み立てる(build_identity_guard_contextと同じ構造)。
fn build_memo_guard_context(
    config: &AppConfig,
    content: &str,
    sources_out: &mut Option<Vec<KnowledgeResultDto>>,
) -> String {
    let now = chrono::Local::now();
    let file_stem = format!("memo-{}", now.timestamp());
    let file_name = format!("{file_stem}.md");
    let chunk_id = format!("memo-{file_stem}-0");
    // 見出しは検索性のため内容の要点を含む形が望ましいが、自動要約は行わず
    // 本文の先頭部分をそのまま使う(要件通り)。
    let heading: String = content.chars().take(40).collect();

    let markdown = format!(
        "---\ntype: メモ\ndate: {}\ntags: []\n---\n\n## {heading}\n\n{content}\n",
        now.format("%Y-%m-%d")
    );

    // Ver2.0(00-inbox〜90-archive体系)ではメモは「インボックス」に対応するため
    // 00-inboxへ保存する(2026-08-17、旧"memo"カテゴリのままだと図書館UI側の
    // CATEGORY_META(library.ts)に存在せず「未分類」表示になる不整合が発覚)。
    let memo_dir = library_root().join("00-inbox");
    let write_result = std::fs::create_dir_all(&memo_dir)
        .map_err(|e| format!("メモ保存先ディレクトリの作成に失敗: {e}"))
        .and_then(|_| {
            std::fs::write(memo_dir.join(&file_name), &markdown)
                .map_err(|e| format!("メモファイルの書き込みに失敗: {e}"))
        });

    let tool_result = match write_result {
        Ok(()) => {
            match rag_client::add_document(
                config.rag.port,
                &chunk_id,
                content,
                &heading,
                &file_name,
                "00-inbox",
            ) {
                Ok(()) => {
                    *sources_out = Some(vec![KnowledgeResultDto {
                        id: chunk_id,
                        text: content.to_string(),
                        source: file_name,
                        heading,
                        source_category: "00-inbox".to_string(),
                    }]);
                    format!("「{content}」という内容をメモとして保存しました。")
                }
                Err(e) => format!(
                    "メモファイル({file_name})への保存はできましたが、検索用データベースへの即時登録に失敗しました: {e}"
                ),
            }
        }
        Err(e) => format!("メモの保存に失敗しました: {e}"),
    };

    format!(
        "ふらるさんはメモの保存を頼んでいます。以下の[実行結果]は、実際に\n\
         メモを保存した結果です。この操作はすでに実行済みなので、改めて保存を\n\
         試みる必要はありません。結果をそのまま素直に伝えてください。失敗して\n\
         いる場合は、成功したかのように装わず、正直にその内容を伝えてください。\n\n\
         [実行結果]\n{tool_result}"
    )
}

// UI(ActivityIndicator)向けの「ツール実行中」通知。send_messageは同期的な
// Tauriコマンドで、完了まで結果を返せないため、実行の最中にフロントエンドへ
// 一時的な状態を伝える手段としてTauriイベントを使う(既存のvoice:*イベントと
// 同じ仕組み)。UI/UX改善の優先度B-1対応。
fn emit_activity_started(app: &tauri::AppHandle, tool: &str) {
    use tauri::Emitter;
    let _ = app.emit("shiori:activity", serde_json::json!({ "tool": tool }));
}

// 会話履歴(詩織Ver3.6)。以前はsend_messageのたびに「システムプロンプト+参照情報+
// 今回の発言」だけをLLMに渡しており、「さっきの件」のような前の発言を指す話が
// 通じなかった。直近のターンだけをプロセス内に保持し、アプリを再起動すると消える
// (会話DBのconversationsには従来どおり全件残る)。コンテキスト(config.llm.
// contextSize=8192)を圧迫しないよう、ターン数と1発言あたりの文字数に上限を設ける。
const HISTORY_MAX_TURNS: usize = 4;
const HISTORY_MAX_CHARS_PER_MESSAGE: usize = 400;

struct HistoryTurn {
    user: String,
    assistant: String,
    // このターンで記録を案内したか。次のターンで「その2つ目のやつ」のように
    // 前のターンの記録を指す応答を、事実確認ガードが作り話と誤判定しないために使う。
    had_references: bool,
}

static CONVERSATION_HISTORY: Mutex<std::collections::VecDeque<HistoryTurn>> =
    Mutex::new(std::collections::VecDeque::new());

const HISTORY_CONTINUATION_NOTE: &str = "これは上のやり取りの続きです。ふらるさんの次の発言に「さっき」「それ」「その〇つ目」のような\n\
     言葉があれば、上のやり取りの内容を指しています。聞き返す前に、上のやり取りを\n\
     踏まえて答えてください。";

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}

// 履歴に積むassistant側の内容。案内した記録の見出しも添えておくことで、次の
// ターンの「その2つ目のやつ」のような指し示しに答えられるようにする(応答本文は
// 記録の一部にしか触れないことが多いため)。
fn history_assistant_content(reply: &str, sources: Option<&Vec<KnowledgeResultDto>>) -> String {
    let reply = truncate_chars(reply, HISTORY_MAX_CHARS_PER_MESSAGE);
    match sources {
        Some(list) if !list.is_empty() => {
            let shelf = list
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {}({})", i + 1, s.heading, s.source))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{reply}\n\n(このとき案内した記録)\n{shelf}")
        }
        _ => reply,
    }
}

fn history_messages() -> Vec<serde_json::Value> {
    let Ok(history) = CONVERSATION_HISTORY.lock() else {
        return Vec::new();
    };
    history
        .iter()
        .flat_map(|turn| {
            [
                serde_json::json!({ "role": "user", "content": turn.user }),
                serde_json::json!({ "role": "assistant", "content": turn.assistant }),
            ]
        })
        .collect()
}

fn last_turn_had_references() -> bool {
    CONVERSATION_HISTORY
        .lock()
        .ok()
        .and_then(|h| h.back().map(|t| t.had_references))
        .unwrap_or(false)
}

fn push_history(user: &str, reply: &str, sources: Option<&Vec<KnowledgeResultDto>>) {
    let Ok(mut history) = CONVERSATION_HISTORY.lock() else {
        return;
    };
    history.push_back(HistoryTurn {
        user: truncate_chars(user, HISTORY_MAX_CHARS_PER_MESSAGE),
        assistant: history_assistant_content(reply, sources),
        had_references: sources.is_some_and(|s| !s.is_empty()),
    });
    while history.len() > HISTORY_MAX_TURNS {
        history.pop_front();
    }
}

// 同期コマンドはメインスレッドで実行されるため、以前は応答を待つ数秒〜十数秒の間
// UIが固まっていた。spawn_blockingで専用スレッドに追い出す(2026-10-06、Ver3.6)。
#[tauri::command]
async fn send_message(app: tauri::AppHandle, text: String) -> Result<SendMessageReply, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if app_config()?.mode == AppMode::External {
            return Err(EXTERNAL_MODE_MESSAGE.to_string());
        }
        send_message_impl(text, |tool| emit_activity_started(&app, tool))
    })
    .await
    .map_err(|e| format!("応答生成タスクの実行に失敗: {e}"))?
}

// 本体。activity通知はコールバックとして受け取ることで、AppHandleを必要と
// せずテストから直接呼べるようにしている(テストではno-opのコールバックを渡す)。
fn send_message_impl(
    text: String,
    on_activity: impl Fn(&str),
) -> Result<SendMessageReply, String> {
    let config = app_config()?;
    let tools = prompts::load_tool_definitions()?;
    let system_prompt = prompts::load_system_prompt()?;

    let mut sources: Option<Vec<KnowledgeResultDto>> = None;
    let mut active_tool: Option<String> = None;

    // アイデンティティ質問の場合は、passive recallより先に確実な検索を
    // 実行する。この場合search_knowledgeが実質的に既に実行されているので、
    // active_toolもこの時点でsearchにしておく(UIのソース表示にも使う)。
    let identity_guard_context = if is_identity_question(&text) {
        shiori_log::info("LLM", "自分についての質問を検知し、直接検索する", &[]);
        active_tool = Some("search_knowledge".to_string());
        on_activity("search_knowledge");
        build_identity_guard_context(&config, &text, &mut sources)
    } else {
        None
    };

    // アイデンティティ質問でない場合、明確なメモ保存意図がないか確認する。
    // こちらも検知したら確実に実行してしまう(上記コメント参照)。
    let memo_guard_context = if identity_guard_context.is_none() {
        parse_memo_intent(&text).map(|content| {
            shiori_log::info("LLM", "メモの保存の意図を検知し、直接保存する", &[("字数", &content.chars().count())]);
            active_tool = Some("memo".to_string());
            on_activity("memo");
            build_memo_guard_context(&config, &content, &mut sources)
        })
    } else {
        None
    };

    // identity_guard/memo_guardのどちらでもない場合、経緯・変遷を尋ねる
    // 質問でないか確認する(詩織Ver3.3、時間認識検索)。
    let history_guard_context = if identity_guard_context.is_none() && memo_guard_context.is_none()
    {
        if is_history_question(&text) {
            shiori_log::info("LLM", "経緯についての質問を検知し、経緯モードで検索する", &[]);
            active_tool = Some("search_knowledge".to_string());
            on_activity("search_knowledge");
            build_history_guard_context(&config, &text, &mut sources)
        } else {
            None
        }
    } else {
        None
    };

    // 上記のどれでもない場合、確かめる手段のないアプリの状態(要確認リスト等)を
    // 尋ねる発言でないか確認する(詩織Ver3.6)。
    let app_state_guard_context = if identity_guard_context.is_none()
        && memo_guard_context.is_none()
        && history_guard_context.is_none()
        && is_app_state_question(&text)
    {
        shiori_log::info("LLM", "確かめる手段のない状態についての質問を検知した", &[]);
        Some(APP_STATE_GUARD_CONTEXT.to_string())
    } else {
        None
    };

    let passive_recall_start = std::time::Instant::now();
    let passive_recall = if identity_guard_context.is_some()
        || memo_guard_context.is_some()
        || history_guard_context.is_some()
        || app_state_guard_context.is_some()
    {
        None
    } else {
        let context = build_passive_recall_context(config.rag.port, &text, &mut sources);
        // identity_guard/memo_guard/history_guardで代替された場合は「常時検索」を
        // 試みてすらいないため集計対象に含めない。実際に試みたときだけ数える。
        use std::sync::atomic::Ordering;
        PASSIVE_RECALL_TOTAL_COUNT.fetch_add(1, Ordering::Relaxed);
        if context.is_some() {
            PASSIVE_RECALL_HIT_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        context
    };
    shiori_log::debug(
        "LLM",
        "受け身の想起",
        &[
            ("秒", &format!("{:.2}", passive_recall_start.elapsed().as_secs_f32())),
            ("当たり", &passive_recall.is_some()),
        ],
    );

    let mut messages_vec = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt,
    })];
    // 履歴は参照情報(ガード/passive recall)より前に置く。参照情報は今回の発言に
    // ついてのものなので、今回の発言の直前にあるほうがLLMが取り違えにくい。
    let history = history_messages();
    let has_history = !history.is_empty();
    messages_vec.extend(history);
    if let Some(context) = &identity_guard_context {
        messages_vec.push(serde_json::json!({ "role": "system", "content": context }));
    } else if let Some(context) = &memo_guard_context {
        messages_vec.push(serde_json::json!({ "role": "system", "content": context }));
    } else if let Some(context) = &history_guard_context {
        messages_vec.push(serde_json::json!({ "role": "system", "content": context }));
    } else if let Some(context) = &app_state_guard_context {
        messages_vec.push(serde_json::json!({ "role": "system", "content": context }));
    } else if let Some(context) = &passive_recall {
        messages_vec.push(serde_json::json!({ "role": "system", "content": context }));
    }
    // 履歴があるときは、今回の発言の直前に「会話の続きである」ことを短く念押しする。
    // 履歴を渡すだけでは、Qwen3-8Bが「さっきの件」を前のやり取りと結びつけず、
    // プロンプトの例文をなぞった応答を返すことが再測定で分かったため(Ver3.6)。
    // 小さいモデルは直前に置かれた指示に最も強く従う。
    if has_history {
        messages_vec.push(serde_json::json!({ "role": "system", "content": HISTORY_CONTINUATION_NOTE }));
    }
    messages_vec.push(serde_json::json!({ "role": "user", "content": text }));
    let mut messages = serde_json::Value::Array(messages_vec);

    let mut final_content = String::new();

    for _ in 0..TOOL_CALL_MAX_ITERATIONS {
        let response = llm_client::chat_with_tools(config.llm.port, &messages, &tools)?;
        let message = response
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| "LLM応答の形式が不正です".to_string())?;

        let tool_calls = message
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        messages
            .as_array_mut()
            .expect("messagesは配列")
            .push(message.clone());

        if tool_calls.is_empty() {
            let content = message
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if let Some((name, args)) = parse_leaked_tool_call(&content) {
                shiori_log::warn("LLM", "応答に混ざったツール呼び出しの文字列を、呼び出しとして処理した", &[("道具", &name)]);
                active_tool = Some(name.clone());
                on_activity(&name);
                let tool_result = match execute_tool(&name, &args, &config, &mut sources) {
                    Ok(r) => r,
                    Err(e) => format!("エラー: {e}"),
                };

                // 生文字列のまま履歴に残すと次の応答でも同じ書式を繰り返しやすいため、
                // 正常なtool_calls形式に差し替えてから積む。
                let arr = messages.as_array_mut().expect("messagesは配列");
                arr.pop();
                let fixed_call_id = "leaked-call-0";
                arr.push(serde_json::json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": fixed_call_id,
                        "type": "function",
                        "function": { "name": name, "arguments": args.to_string() }
                    }]
                }));
                arr.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": fixed_call_id,
                    "content": tool_result,
                }));
                continue;
            }

            final_content = content;
            break;
        }

        for call in &tool_calls {
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args_str = call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let call_id = call.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let args: serde_json::Value =
                serde_json::from_str(args_str).unwrap_or_else(|_| serde_json::json!({}));

            active_tool = Some(name.to_string());
            on_activity(name);
            let tool_result = match execute_tool(name, &args, &config, &mut sources) {
                Ok(r) => r,
                Err(e) => format!("エラー: {e}"),
            };

            messages.as_array_mut().expect("messagesは配列").push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_result,
            }));
        }
    }

    // 最大反復数まで正常な最終応答を得られなかった場合(表示ガードでの
    // 差し替えが繰り返された場合など)のフォールバック。
    if final_content.trim().is_empty() {
        final_content = match active_tool.as_deref() {
            Some("search_knowledge") => "調べてみたよ。".to_string(),
            Some("memo") => "メモしておいたよ。".to_string(),
            _ => "うまく応答できなかったみたい、もう一度言ってもらえる?".to_string(),
        };
    }

    let detected_mode = match active_tool.as_deref() {
        Some("search_knowledge") => "search",
        Some("memo") => "memo",
        _ => "chat",
    }
    .to_string();

    // 直前のターンで案内した記録を指して話を続けている場合も「参照情報あり」とみなす。
    let has_references =
        sources.as_ref().is_some_and(|s| !s.is_empty()) || last_turn_had_references();
    final_content = correct_unverified_claims(
        &final_content,
        &text,
        memo_guard_context.is_some(),
        has_references,
        app_state_guard_context.is_some(),
    );
    final_content = correct_third_person_self_reference(&final_content);

    log_conversation(&detected_mode, &text, &final_content)?;
    push_history(&text, &final_content, sources.as_ref());

    Ok(SendMessageReply {
        reply: final_content,
        sources,
        detected_mode,
    })
}

// whisper-server(STT)はVRAM軽量化のため常時起動せず、録音のたびにオンデマンドで
// 起動し、一定時間(IDLE_TIMEOUT)使われなければバックグラウンドスレッドが自動停止する。
// 文字起こし完了直後にstop_now()で即座にアンロードするのが基本経路。
// これは異常終了等でstop_now()が呼ばれなかった場合の保険(短め)。
const STT_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const STT_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

pub struct SttManager {
    child: Mutex<Option<Child>>,
    last_used: Mutex<Instant>,
    // 起動待ち〜文字起こしの最中はtrue。この間はアイドル監視で停止させない。
    // 以前は起動待ち(wait_healthy)の間にlast_usedが更新されず、モデルの読み込みが
    // 15秒を超えると、まだ起動中のwhisper-serverをアイドル監視が止めてしまい、
    // 起動待ちがタイムアウトする不具合があった(2026-10-06、Ver3.6の測定で発見)。
    in_use: std::sync::atomic::AtomicBool,
}

impl SttManager {
    fn is_running(&self) -> bool {
        matches!(self.child.lock(), Ok(guard) if guard.is_some())
    }

    fn pid(&self) -> Option<u32> {
        self.child.lock().ok()?.as_ref().map(|c| c.id())
    }
}

impl SttManager {
    fn new() -> Arc<Self> {
        let manager = Arc::new(Self {
            child: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            in_use: std::sync::atomic::AtomicBool::new(false),
        });
        let watcher = manager.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(STT_IDLE_CHECK_INTERVAL);
            watcher.stop_if_idle();
        });
        manager
    }

    // 録音開始時に呼ぶ。既に起動済み/起動中ならすぐ返る。
    fn ensure_started(&self, exe: &Path, model_path: &Path, port: u16) -> Result<(), String> {
        *self.last_used.lock().map_err(|e| e.to_string())? = Instant::now();
        let mut guard = self.child.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Ok(());
        }
        let port_str = port.to_string();
        // whisper-serverの--languageはデフォルトが"en"(autoではない)。指定を忘れると
        // 日本語の音声でも「英語のはず」という前提でデコードされ、内容が支離滅裂な
        // (それでいて文法的には自然な)英文になってしまう不具合があったため明示指定する。
        // このアプリは日本語のみを想定しているため"ja"を固定する。
        // --prompt はよく出てくる固有名詞(詩織自身の名前、サイト名等)を初期プロンプトとして
        // 与えることで、同音異義語への誤変換(例:「詩織」→「仕寄り」)を減らすためのヒント。
        // --carry-initial-prompt を付けないと最初の1回しか効かない(録音のたびに
        // 新しいリクエストとして扱われるため、毎回効かせる必要がある)。
        let args = [
            "--port",
            &port_str,
            "--host",
            "127.0.0.1",
            "--language",
            "ja",
            "--prompt",
            "詩織、ふらる、huraru.com、ポートフォリオ、lab.huraru.com",
            "--carry-initial-prompt",
        ];
        let child = spawn_server(exe, model_path, &args, "libs-whisper", false).map_err(|e| e.to_string())?;
        *guard = Some(child);
        Ok(())
    }

    // 録音停止後に呼ぶ。whisper-serverの起動を待って文字起こしし、成否にかかわらず
    // 即座にアンロードしてVRAMピークを短くする。起動待ち・文字起こしに数十秒かかる
    // ことがあるため、メインスレッド(UIスレッド)から直接呼ばないこと。
    // マイクボタン経路(stop_recording_and_transcribe)とホットキー経路
    // (handle_hotkey_toggle)で共有する。
    fn transcribe_when_ready(&self, port: u16, wav: &[u8]) -> Result<String, String> {
        use std::sync::atomic::Ordering;
        self.in_use.store(true, Ordering::SeqCst);
        let result = if wait_for_health(port, 30) {
            whisper_client::transcribe(port, wav)
        } else {
            Err("STT(whisper-server)の起動待ちでタイムアウトしました".to_string())
        };
        self.in_use.store(false, Ordering::SeqCst);
        self.stop_now("文字起こし完了のため");
        result
    }

    fn stop_if_idle(&self) {
        if self.in_use.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let idle_for = match self.last_used.lock() {
            Ok(t) => t.elapsed(),
            Err(_) => return,
        };
        if idle_for < STT_IDLE_TIMEOUT {
            return;
        }
        self.stop_now("アイドルタイムアウトのため");
    }

    // 文字起こし完了直後に即座に呼ぶ。録音〜応答生成の間、LLM+embedding+whisperの
    // 3つが同時に載る時間(VRAMピーク)を最小化するため、アイドルタイムアウト(保険)を
    // 待たずにここで明示的にアンロードする。
    fn stop_now(&self, reason: &str) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                shiori_log::info("STT", "whisper-serverを止めた", &[("理由", &reason)]);
            }
        }
    }
}

fn whisper_server_exe_path() -> PathBuf {
    resolve_engine_exe(
        &project_root().join("third_party/whisper.cpp/build"),
        "whisper-server",
    )
}

#[tauri::command]
async fn start_recording(app: tauri::AppHandle) -> Result<(), String> {
    run_blocking(move || start_recording_blocking(app.state(), app.state())).await
}

fn start_recording_blocking(
    state: tauri::State<audio::RecordingState>,
    stt: tauri::State<Arc<SttManager>>,
) -> Result<(), String> {
    if app_config()?.mode == AppMode::External {
        return Err(EXTERNAL_MODE_MESSAGE.to_string());
    }
    audio::start(&state)?;

    // 録音(発話)と並行してwhisper-serverを起動しておき、起動待ちを発話時間の裏に隠す
    let stt = stt.inner().clone();
    std::thread::spawn(move || {
        let exe = whisper_server_exe_path();
        match app_config() {
            Ok(config) => {
                let model_path = project_root().join(&config.stt.model_path);
                if let Err(e) = stt.ensure_started(&exe, &model_path, config.stt.port) {
                    log_error!("whisper-serverの起動に失敗: {e}");
                }
            }
            Err(e) => log_error!("config読み込みに失敗: {e}"),
        }
    });

    Ok(())
}

#[derive(Serialize, Clone)]
pub struct TranscribeResult {
    text: String,
}

// 同期コマンドはメインスレッドで実行されるため、以前は起動待ち・文字起こしの間
// UIが「応答なし」になっていた(2026-10-06、Ver3.6の測定で発見)。
// synthesize_speechと同じくspawn_blockingで専用スレッドに追い出す。
#[tauri::command]
async fn stop_recording_and_transcribe(
    state: tauri::State<'_, audio::RecordingState>,
    stt: tauri::State<'_, Arc<SttManager>>,
) -> Result<TranscribeResult, String> {
    let wav = audio::stop_and_encode_wav(&state)?;
    let config = app_config()?;
    let stt = stt.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let text = stt.transcribe_when_ready(config.stt.port, &wav)?;
        Ok(TranscribeResult { text })
    })
    .await
    .map_err(|e| format!("文字起こしタスクの実行に失敗: {e}"))?
}

// TTSはspeaker_embedding次元不一致の既知の不具合により失敗することがあるが、
// 会話フロー自体は継続させたいため、エラーは警告ログに留め呼び出し元には常にOkを返す。
//
// synthesize_and_play内部はpiper-plus-cliサブプロセスの完了待ち(Command::status)
// と音声再生の完了待ち(rodioのsink.sleep_until_end)という2つの長時間ブロッキング
// 処理を直列に行う。以前はこの関数を同期(非async)コマンドから直接呼んでおり、
// 読み上げ中はUIが「応答なし」になり、読み上げ終了と同時にUIが一気に更新される、
// という不具合があった(2026-08-08、実機確認で発見)。spawn_blockingで専用スレッドに
// 追い出すことで、読み上げ中もUIスレッドが塞がれないようにしている。
#[tauri::command]
async fn synthesize_speech(text: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = project_root();
        let config = app_config()?;
        let voice = piper_client::VoiceParams {
            length_scale: config.tts.length_scale,
            noise_scale: config.tts.noise_scale,
            noise_w: config.tts.noise_w,
        };
        if let Err(e) = piper_client::synthesize_and_play(
            &root,
            &text,
            &root.join(&config.tts.model_path),
            &root.join(&config.tts.config_path),
            &voice,
        ) {
            log_error!("TTS再生に失敗しました(既知の不具合により発生する場合があります): {e}");
            log_tts_failure(&e);
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("TTS処理タスクの実行に失敗: {e}"))?
}

// コントロールパネルの音声設定「試し読み」ボタン用。config.jsonへの保存を伴わず、
// 今その場でスライダーの値を試せるようにする(保存は別途set_configで行う)。
// synthesize_speechと同じ理由でspawn_blockingを使う(下記コメント参照)。
#[tauri::command]
async fn preview_voice(
    text: String,
    length_scale: f64,
    noise_scale: f64,
    noise_w: f64,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = project_root();
        let config = app_config()?;
        let voice = piper_client::VoiceParams {
            length_scale,
            noise_scale,
            noise_w,
        };
        piper_client::synthesize_and_play(
            &root,
            &text,
            &root.join(&config.tts.model_path),
            &root.join(&config.tts.config_path),
            &voice,
        )
    })
    .await
    .map_err(|e| format!("TTS処理タスクの実行に失敗: {e}"))?
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // history_guard(詩織Ver3.3)のキーワード検知が狙い通りに発火し、かつ
    // 明示的に除外した「変わった」では誤爆しないことを確認する軽量テスト
    // (RAGサーバー不要)。
    // cargo test --lib history_guard_keyword_detection
    #[test]
    fn history_guard_keyword_detection() {
        for text in [
            "詩織の声、前はどんな感じだったっけ",
            "以前はどういう構成だったの",
            "昔はもっとシンプルだった気がする",
            "かつての設計はどうなってたの",
        ] {
            assert!(is_history_question(text), "発話=\"{text}\": history_guardが発火していない");
        }
        for text in ["気持ちが変わった", "今日はいい天気だね", "詩織って何?"] {
            assert!(
                !is_history_question(text),
                "発話=\"{text}\": history_guardが誤爆している"
            );
        }
    }

    // resolve_engine_exeが「Release/あり」「Release/なし」どちらのビルド出力構成でも
    // 実在するパスを見つけられることを確認する軽量テスト(実バイナリ不要、tempdirで代用)。
    // cargo test --lib resolve_engine_exe
    #[test]
    fn resolve_engine_exe_prefers_existing_release_dir() {
        let dir = std::env::temp_dir().join(format!(
            "shiori_test_resolve_engine_exe_release_{}",
            std::process::id()
        ));
        let bin_release = dir.join("bin/Release");
        std::fs::create_dir_all(&bin_release).unwrap();
        std::fs::write(bin_release.join(exe_name_for_test("dummy-server")), b"").unwrap();

        let resolved = resolve_engine_exe(&dir, "dummy-server");
        assert_eq!(resolved, bin_release.join(exe_name_for_test("dummy-server")));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_engine_exe_falls_back_to_bin_without_release() {
        let dir = std::env::temp_dir().join(format!(
            "shiori_test_resolve_engine_exe_norelease_{}",
            std::process::id()
        ));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(exe_name_for_test("dummy-server")), b"").unwrap();

        let resolved = resolve_engine_exe(&dir, "dummy-server");
        assert_eq!(resolved, bin.join(exe_name_for_test("dummy-server")));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn exe_name_for_test(base: &str) -> String {
        if cfg!(windows) {
            format!("{base}.exe")
        } else {
            base.to_string()
        }
    }

    // parse_memo_intentの検知ロジックを確認する軽量テスト(LLM不要)。
    // cargo test --lib parse_memo_intent
    #[test]
    fn parse_memo_intent_extracts_content() {
        assert_eq!(
            parse_memo_intent("huraru.comのベースカラーは#080C14ってメモして"),
            Some("huraru.comのベースカラーは#080C14って".to_string())
        );
        assert_eq!(
            parse_memo_intent("次のミーティングは来週水曜って記録して"),
            Some("次のミーティングは来週水曜って".to_string())
        );
        assert_eq!(
            parse_memo_intent("この設計判断の理由を覚えておいて"),
            Some("この設計判断の理由を".to_string())
        );
        // トリガー語だけで内容が空の場合は検知しない
        assert_eq!(parse_memo_intent("メモして"), None);
        // 無関係な文章は検知しない
        assert_eq!(parse_memo_intent("こんにちは、調子はどう？"), None);
    }

    // 事実確認ガード(Ver3.6、測定シナリオS4・S6)。
    #[test]
    fn correct_unverified_claims_replaces_only_unbacked_claims() {
        // 参照情報なしで記録の存在を言う → 置き換え(S4)
        assert_eq!(
            correct_unverified_claims(
                "そういえば、宇宙旅行についての記録があるかもしれません。開いてみますか？",
                "前に宇宙旅行の計画を立てたっけ?",
                false,
                false,
                false,
            ),
            NO_RECORD_REPLY
        );
        // 参照情報ありなら、そのまま
        let backed = "huraru.comのベースカラーについてのメモがありますね。開いてみますか？";
        assert_eq!(correct_unverified_claims(backed, "", false, true, false), backed);
        // 否定形は置き換えない
        let negative = "記録にはポートフォリオの色についての記述が見つかりませんでした。記録がありません。";
        assert_eq!(correct_unverified_claims(negative, "", false, false, false), negative);
        // 保存の処理なしで保存したと言う → 置き換え(S6)
        assert_eq!(
            correct_unverified_claims("今、メモしておきましたよ。", "", false, false, false),
            NOT_SAVED_REPLY
        );
        // 保存の処理が動いていれば、そのまま
        let saved = "今、メモしておきましたよ。";
        assert_eq!(correct_unverified_claims(saved, "", true, true, false), saved);
        // 前のターンに記録がないのに「2つ目の記録は〜」と言う → 置き換え(S8)
        assert_eq!(
            correct_unverified_claims(
                "2つ目の記録は、背景についてのものです。",
                "その2つ目のやつ、いつの?",
                false,
                false,
                false,
            ),
            NO_RECORD_REPLY
        );
        // 要確認の質問に「確認できない」と答えていなければ置き換え(S9)
        assert_eq!(
            correct_unverified_claims("それについての記録は、見当たらないみたいです。", "", false, false, true),
            APP_STATE_REPLY
        );
        let honest = "私からは確認できないので、要確認の画面で見てもらえますか。";
        assert_eq!(correct_unverified_claims(honest, "", false, false, true), honest);
    }

    #[test]
    fn history_assistant_content_lists_shelf_and_truncates() {
        let sources = vec![
            KnowledgeResultDto {
                id: "a".to_string(),
                text: String::new(),
                source: "a.md".to_string(),
                heading: "見出しA".to_string(),
                source_category: "20-areas".to_string(),
            },
            KnowledgeResultDto {
                id: "b".to_string(),
                text: String::new(),
                source: "b.md".to_string(),
                heading: "見出しB".to_string(),
                source_category: "20-areas".to_string(),
            },
        ];
        let content = history_assistant_content("記録が2つあります。", Some(&sources));
        assert!(content.contains("1. 見出しA(a.md)"));
        assert!(content.contains("2. 見出しB(b.md)"));

        let long = "あ".repeat(HISTORY_MAX_CHARS_PER_MESSAGE + 10);
        let truncated = history_assistant_content(&long, None);
        assert_eq!(truncated.chars().count(), HISTORY_MAX_CHARS_PER_MESSAGE + 1);
    }

    #[test]
    fn app_state_question_detection() {
        assert!(is_app_state_question("要確認のタグってある?"));
        assert!(!is_app_state_question("今日は何しようかな"));
    }

    // Ver3.6で追加した口語の言い回し(測定シナリオS6)。
    #[test]
    fn parse_memo_intent_handles_colloquial_phrases() {
        assert_eq!(
            parse_memo_intent("これ忘れないようにしといて:Ver3.6の測定結果はシナリオ記事に追記する"),
            Some("Ver3.6の測定結果はシナリオ記事に追記する".to_string())
        );
        assert_eq!(
            parse_memo_intent("それ忘れないようにしておいて：明日は休み"),
            Some("明日は休み".to_string())
        );
        assert_eq!(
            parse_memo_intent("これから本を読むって覚えといて"),
            Some("これから本を読むって".to_string())
        );
        assert_eq!(
            parse_memo_intent("明日の予定をメモしといて"),
            Some("明日の予定を".to_string())
        );
        assert_eq!(parse_memo_intent("これ忘れないようにしといて"), None);
    }

    // parse_leaked_tool_callの2つの漏れ形式を確認する軽量テスト(LLM不要)。
    // 実機で発見された「{"name":..., "arguments":{...}}」形式(前後にゴミ文字・
    // </tool_call>断片が混ざるケースを含む)を、この修正でも検知できることを
    // 固定する。cargo test --lib parse_leaked_tool_call
    #[test]
    fn parse_leaked_tool_call_handles_both_formats() {
        // パターン1: ツール名で始まる形式(従来から対応)
        let (name, args) = parse_leaked_tool_call(
            r#"search_knowledge {"query": "詩織の由来"}"#,
        )
        .expect("パターン1を検知できていない");
        assert_eq!(name, "search_knowledge");
        assert_eq!(args["query"], "詩織の由来");

        // パターン2: OpenAI形式のtool_calls本体をそのまま書いてしまうケース
        // (実機で実際に観測された、前後にゴミ文字・</tool_call>断片つき)
        let leaked = "啉\n{\"name\": \"search_knowledge\", \"arguments\": {\"query\": \"詩織の由来\"}}\n</tool_call>";
        let (name, args) = parse_leaked_tool_call(leaked).expect("パターン2を検知できていない");
        assert_eq!(name, "search_knowledge");
        assert_eq!(args["query"], "詩織の由来");

        // 無関係な文章は検知しない(誤検知しないことの確認)
        assert!(parse_leaked_tool_call("こんにちは、調子はどうですか？").is_none());
    }

    // GPU上で実モデルを読み込むため重い。手動実行用: cargo test --lib -- --ignored --nocapture
    #[test]
    #[ignore]
    fn start_backend_services_starts_and_health_checks_all_three() {
        let state = BackendState(Mutex::new(BackendProcesses::default()));
        let results = tauri::async_runtime::block_on(start_backend_services_impl(&state, None))
            .expect("should return status list");
        for status in &results {
            assert!(status.started, "{} failed to start: {:?}", status.name, status.error);
            assert!(status.healthy, "{} did not become healthy", status.name);
        }
        let mut procs = state.0.lock().expect("lock poisoned");
        if let Some(mut c) = procs.llm.take() {
            let _ = c.kill();
        }
        if let Some(mut c) = procs.embedding.take() {
            let _ = c.kill();
        }
        if let Some(mut c) = procs.rag.take() {
            let _ = c.kill();
        }
    }

    // TTS(piper-plus-cli, WavLM版モデルへのピン留め後)の動作確認用の手動実行テスト。
    // 実際にスピーカーから再生されるところまで確認する。
    // cargo test --lib -- --ignored --nocapture phase_tts_playback
    #[test]
    #[ignore]
    fn phase_tts_playback() {
        let result = tauri::async_runtime::block_on(synthesize_speech(
            "こんにちは、詩織です。テストがうまくいくといいな。".to_string(),
        ));
        result.unwrap_or_else(|e| panic!("TTS再生に失敗: {e}"));
        println!("TTS再生完了(エラーなし)");
    }

    // コントロールパネルのget_config/set_config/preview_voiceの動作確認用。
    // config.jsonが壊れないこと(他の項目が消えないこと)を特に確認する。
    // cargo test --lib -- --ignored --nocapture phase_control_panel_config
    #[test]
    #[ignore]
    fn phase_control_panel_config() {
        let before = get_config_blocking().expect("get_config失敗");
        println!(
            "取得: hotkey={} llmPort={} threshold={} lengthScale={}",
            before.hotkey, before.llm_port, before.passive_recall_threshold, before.length_scale
        );

        // 元の値に戻すだけの無害な更新で、書き込み→再読み込みの往復を確認する
        set_config_blocking(ConfigUpdate {
            passive_recall_threshold: Some(before.passive_recall_threshold),
            length_scale: Some(before.length_scale),
            ..Default::default()
        })
        .expect("set_config失敗");

        let after = get_config_blocking().expect("再取得失敗");
        assert_eq!(after.hotkey, before.hotkey, "無関係な項目(hotkey)が変化してはいけない");
        assert_eq!(after.llm_port, before.llm_port, "無関係な項目(llmPort)が変化してはいけない");
        assert_eq!(after.rag_port, before.rag_port, "無関係な項目(ragPort)が変化してはいけない");
        assert_eq!(after.passive_recall_threshold, before.passive_recall_threshold);

        // 不正値のバリデーションも確認する
        let invalid = set_config_blocking(ConfigUpdate {
            passive_recall_threshold: Some(99.0),
            ..Default::default()
        });
        assert!(invalid.is_err(), "範囲外の値はエラーになるべき");
        println!("バリデーションエラー(想定通り): {}", invalid.unwrap_err());

        println!("get_config/set_configの往復・バリデーションともにOK");
    }

    // RAGサーバーの起動失敗検知・再試行の動作確認用(2026-08-13、起動画面が
    // healthy:falseを握りつぶしてホーム画面へ進んでしまう退行の修正確認)。
    // 事前にservices/rag/.venv/bin/pythonを一時的に別名へ退避させてから実行すると、
    // start_rag_service(=start_backend_services_impl/retry_rag_serviceの両方が
    // 使う共通処理)がhealthy:false(かつプロセス自体が起動しない)を返すことを
    // 確認できる。pythonを元に戻してから再度実行すると、同じ関数呼び出しだけで
    // healthy:trueに回復すること(=リトライボタンの動作)を確認できる。
    // cargo test --lib -- --ignored --nocapture phase_rag_startup_failure_and_retry
    #[test]
    #[ignore]
    fn phase_rag_startup_failure_and_retry() {
        let root = project_root();
        let config = load_config(&root).expect("config.jsonの読み込みに失敗");
        let mut procs = BackendProcesses::default();

        let status = start_rag_service(&mut procs, None, &root, &config);
        println!(
            "started={} healthy={} error={:?}",
            status.started, status.healthy, status.error
        );

        if let Some(mut c) = procs.rag.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    // Ver2.0 Phase 2フォローアップ: shared_daemonのロックファイルにPIDが正しく
    // 記録され、「procsにハンドルを保持していない(=外部プロセスが起動した)」
    // 想定でもkill_daemonでプロセスを止められることを確認する用。
    // start_rag_serviceでRAGを起動した直後、procs.ragを意図的に空にして
    // (外部プロセスが起動した状態を模擬する)、shared_daemon::kill_daemonが
    // ロックファイルのPID経由で実際にプロセスを終了できるか確認する。
    // cargo test --lib -- --ignored --nocapture phase_shared_daemon_pid_kill
    #[test]
    #[ignore]
    fn phase_shared_daemon_pid_kill() {
        let root = project_root();
        let config = load_config(&root).expect("config.jsonの読み込みに失敗");
        let mut procs = BackendProcesses::default();

        let status = start_rag_service(&mut procs, None, &root, &config);
        assert!(status.healthy, "RAGサーバーの起動に失敗: {:?}", status.error);

        // procsのハンドルを手放す(外部プロセスが起動した状態を模擬)。
        // Child自体をdropしてもプロセスはkillされない(Rust標準ライブラリの仕様)。
        let detached_pid = procs.rag.take().map(|c| c.id());
        println!("RAGサーバーPID(detach前): {detached_pid:?}");

        let mut sys = sysinfo::System::new_all();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        let recorded_pid = shared_daemon::daemon_pid_if_alive(&sys, &root, ".shiori-rag.lock");
        println!("ロックファイルから読んだPID: {recorded_pid:?}");
        assert_eq!(
            recorded_pid, detached_pid,
            "ロックファイルのPIDが実際に起動したプロセスのPIDと一致しない"
        );

        let killed = shared_daemon::kill_daemon(&sys, &root, ".shiori-rag.lock");
        assert!(killed, "kill_daemonがプロセスを終了できなかった");

        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !wait_for_health(config.rag.port, 1),
            "kill_daemon後もRAGサーバーが応答している(終了できていない)"
        );
        println!("kill_daemon後、RAGサーバーは正しく終了した");
    }

    // list_available_models/estimate_model_switchの動作確認用(実際の切替は行わない)。
    // cargo test --lib -- --ignored --nocapture phase_model_list_and_estimate
    #[test]
    #[ignore]
    fn phase_model_list_and_estimate() {
        let models = list_available_models_blocking().expect("list_available_models失敗");
        assert!(!models.is_empty(), "modelsが空(models/llm/にggufが無い?)");
        for m in &models {
            println!(
                "{} : {:.1}GB (measured={}) size={}MB current={}",
                m.file_name, m.vram_estimate_gb, m.is_measured, m.size_mb, m.is_current
            );
        }

        let target = models.iter().find(|m| !m.is_current).expect("切替候補が無い");
        let mut sys = sysinfo::System::new_all();
        sys.refresh_memory();
        let estimate =
            estimate_model_switch_impl(&sys, target.file_name.clone()).expect("estimate_model_switch失敗");
        println!(
            "切替先={} 見込み使用率={:.1}% risky={}",
            target.file_name, estimate.projected_vram_percent, estimate.is_risky
        );
    }

    // ディスクI/O・電源情報など、Rust単体で完結する低レベルAPIの動作確認用。
    // cargo test --lib -- --ignored --nocapture phase_system_info_extras
    #[test]
    #[ignore]
    fn phase_system_info_extras() {
        let gpu = system_info::query_gpu_info();
        println!("gpu = {:?}", gpu.map(|g| (g.name, g.vram_used_mb, g.vram_total_mb, g.temperature_c)));

        let vram_map = system_info::query_process_vram_map();
        println!("vram_by_pid = {vram_map:?}");

        let power = system_info::query_power_info();
        println!("power = {:?}", power.map(|p| (p.on_battery, p.battery_percent)));

        let counters1 = system_info::query_disk_counters('C');
        println!("disk_counters(1回目) = {counters1:?}");
        std::thread::sleep(Duration::from_secs(1));
        let counters2 = system_info::query_disk_counters('C');
        println!("disk_counters(2回目) = {counters2:?}");
        assert!(counters1.is_some(), "ディスクカウンタが取得できていない");
    }

    // LLM/RAGサーバーが手動起動済みであることを前提とした手動実行用テスト。
    // cargo test --lib -- --ignored --nocapture phase4
    #[test]
    #[ignore]
    fn phase4_send_message_chat_only() {
        let result = send_message_impl("こんにちは、調子はどう？".to_string(), |_| {});
        let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
        println!("reply = {} (detected_mode={})", reply.reply, reply.detected_mode);
        assert!(!reply.reply.is_empty());
    }

    // UI/UX改善(優先度A-1)の確認用: passive recallがヒットしたターンで、
    // 明示的な検索(search_knowledge)を経由していなくてもsourcesが埋まって
    // いるか(=KnowledgePanelに反映される情報が返っているか)を確認する。
    // cargo test --lib -- --ignored --nocapture phase_passive_recall_sources_check
    #[test]
    #[ignore]
    fn phase_passive_recall_sources_check() {
        let result = send_message_impl(
            "lab.huraru.comをWeb技術特化にするか、全技術実験に広げるか迷ってるんだよね"
                .to_string(),
            |_| {},
        );
        let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
        println!(
            "判定={} 応答={} sources件数={}",
            reply.detected_mode,
            reply.reply,
            reply.sources.as_ref().map(|s| s.len()).unwrap_or(0)
        );
        assert_eq!(reply.detected_mode, "chat", "ツールを経由していないはず");
        assert!(
            reply.sources.is_some(),
            "passive recallでヒットしたはずなのにsourcesが空(A-1未対応の可能性)"
        );
    }

    // プロフィールデータ投入後、明示的な検索依頼が機能するかの確認用。
    // 「棚を教える」方針転換後は、ベースカラーの値そのものを答えず、棚を案内する
    // 応答になっているはず(2026-08-12)。
    // cargo test --lib -- --ignored --nocapture phase_profile_explicit_search
    #[test]
    #[ignore]
    fn phase_profile_explicit_search() {
        for _ in 0..3 {
            let result = send_message_impl("huraru.comのベースカラーって何だっけ、調べて教えて".to_string(), |_| {});
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
            println!("判定={} 応答={}", reply.detected_mode, reply.reply);
            // 「調べて」はsearch_knowledgeの明示呼び出しを強制するキーワードではないため
            // (identity_guard/memo_guardと違いRust側の強制ルーティングが無い)、passive_recallが
            // 先に情報を拾ってdetected_mode="chat"のまま応答することもある。ここで検証すべきは
            // 経路ではなく、どちらの経路でも値そのものを漏らしていないことだけ。
            assert!(
                !reply.reply.contains("#080C14"),
                "「棚を教える」方針に反し、値そのもの(#080C14)を答えてしまっている: {}",
                reply.reply
            );
        }
    }

    // 課題2(a)の確認: 完全に無関係な話題を明示的に「調べて」と頼んだとき、
    // 距離フィルタにより「見つかりませんでした」に正しく到達するか。
    // cargo test --lib -- --ignored --nocapture phase_explicit_search_not_found
    #[test]
    #[ignore]
    fn phase_explicit_search_not_found() {
        for _ in 0..3 {
            let result = send_message_impl("詩織の血液型って何だっけ、調べて教えて".to_string(), |_| {});
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
            println!("判定={} 応答={}", reply.detected_mode, reply.reply);
            // phase_profile_explicit_searchと同じ理由でdetected_modeの経路は問わない。
            // 検証すべきは距離フィルタを通過してsourcesが混入していないことのみ。
            assert!(
                reply.sources.as_ref().map(|s| s.is_empty()).unwrap_or(true),
                "無関係な話題なのに距離フィルタを通過してsourcesが返ってしまっている(ハルシネーション疑い)"
            );
        }
    }

    // shiori-voice-samples.md統合後のsystem-prompt.mdで、人格面(呼び方・トーン・
    // 感情表現・好みの控えめさ)と既存の技術的振る舞い(リグレッション)の両方を
    // 一通り確認する。目視での確認が中心のため出力を読んで判断する。
    // cargo test --lib -- --ignored --nocapture phase_voice_integration_check
    #[test]
    #[ignore]
    fn phase_voice_integration_check() {
        let samples: [(&str, &str); 10] = [
            ("こんにちは、調子はどう？", "呼び方・トーンの確認(雑談)"),
            (
                "実は今日、ずっと詰まってたバグが急に直ったんだよね",
                "感情表現(驚き・喜び)が自然に出るか。毎回ではないはずなので複数回試す価値あり",
            ),
            ("詩織って何が好きなの？", "好みを聞かれたとき、さりげなく答えられるか"),
            (
                "lab.huraru.comをWeb技術特化にするか、全技術実験に広げるか迷ってるんだよね",
                "壁打ち(問い返してくるか)の確認",
            ),
            (
                "詩織ってどういう存在でいたいと思ってる？",
                "ふらるさんとの関係性について、押しつけがましくなく答えられるか",
            ),
            (
                "詩織のキャラクター名の由来って何だっけ",
                "search_knowledge(自発的想起)のリグレッション確認。方針転換後は由来の中身を語らず棚を案内するはず",
            ),
            (
                "huraru.comのベースカラーって何だっけ、調べて教えて",
                "search_knowledge(明示的検索)のリグレッション確認。方針転換後は色の値そのものを答えず棚を案内するはず",
            ),
            ("huraru.comのフォントについてメモして", "memo_guardのリグレッション確認"),
            (
                "詩織の血液型って何だっけ",
                "分からないことを聞かれたとき、正直に答えられるか(創作しないこと)",
            ),
            (
                "さっき言ってたこと、実は違ったかも",
                "間違いを認める場面(直接的に間違いを指摘した場合の反応)",
            ),
        ];

        for (text, expectation) in samples {
            let start = std::time::Instant::now();
            let result = send_message_impl(text.to_string(), |_| {});
            let elapsed = start.elapsed();
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗(発話=\"{text}\"): {e}"));
            println!(
                "[{:>5.2}s] 発話=\"{text}\" 期待={expectation} => 判定={} 応答={}",
                elapsed.as_secs_f32(),
                reply.detected_mode,
                reply.reply
            );
            // サンプル6は自発的想起(passive_recallのキーワードにも「由来」が含まれ、
            // identity_guardの強制ルーティングも重なるため確実にsearchになる)、
            // サンプル8はmemo_guardのリグレッション確認であることがコメントで明記されているため、
            // 判定を機械的に検証する。サンプル7(明示検索)はidentity_guard/memo_guardと違い
            // Rust側の強制ルーティングが無く、passive_recallが先に拾ってchatのまま
            // 応答することもあるため、経路ではなく値の非漏洩だけをこの下のassertで見る。
            match text {
                "詩織のキャラクター名の由来って何だっけ" => {
                    assert_eq!(reply.detected_mode, "search", "発話=\"{text}\": identity_guardのリグレッション");
                }
                "huraru.comのベースカラーって何だっけ、調べて教えて" => {
                    assert!(
                        !reply.reply.contains("#080C14"),
                        "発話=\"{text}\": 値そのもの(#080C14)を答えてしまっている: {}",
                        reply.reply
                    );
                }
                "huraru.comのフォントについてメモして" => {
                    assert_eq!(reply.detected_mode, "memo", "発話=\"{text}\": memo_guardのリグレッション");
                }
                _ => {}
            }
        }
    }

    // アイデンティティ質問安全網の確認: 「由来」等のキーワードでsearch_knowledge
    // への強制ルーティングが働き、作り話(『源氏物語』由来など)が発生しないかを
    // 複数の言い回しで確認する。「棚を教える」方針転換後は、由来の中身を語る
    // のではなく「記録がありますよ、開いてみますか」のように棚の場所を案内する
    // 応答になっているはず(2026-08-12)。detected_modeが"search"になっているか、
    // 応答が中身を語らず所在を案内できているかを目視で確認する。
    // cargo test --lib -- --ignored --nocapture phase_identity_guard_check
    #[test]
    #[ignore]
    fn phase_identity_guard_check() {
        let samples = [
            "詩織の由来って何だっけ",
            "詩織の由来って何？",
            "詩織ってなんでその名前なの",
            "詩織の名前の由来を教えて",
            "どうして詩織っていう名前になったの",
            "詩織の名前の意味って何？",
            "なんで詩織って名前なんだっけ",
            "詩織という名前の由来を知りたい",
            "詩織のキャラクター名の由来って何だっけ",
            "詩織ってどういう由来の名前なの",
            "詩織の名前、なんでこれになったんだっけ",
        ];

        for text in samples {
            let start = std::time::Instant::now();
            let result = send_message_impl(text.to_string(), |_| {});
            let elapsed = start.elapsed();
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗(発話=\"{text}\"): {e}"));
            println!(
                "[{:>5.2}s] 発話=\"{text}\" => 判定={} 応答={}",
                elapsed.as_secs_f32(),
                reply.detected_mode,
                reply.reply
            );
            assert_eq!(
                reply.detected_mode, "search",
                "発話=\"{text}\": identity_guardが発火せずsearch_knowledgeへ強制ルーティングされていない"
            );
        }
    }

    // 技術情報系(LLM・音声認識・記憶の探し方等)へのキーワード安全網拡大の確認。
    // 「詩織はどのLLMを使ってるの？」等は以前hallucination(「特定のLLMを
    // 使用しているわけではない」等の誤答)が確認されていた話題。「棚を教える」
    // 方針転換後は、具体的な仕組みを語るのではなく「記録があります、開いて
    // みますか」という案内になっているはずで、それでもhallucinationしていない
    // ことを目視で確認する(2026-08-12)。
    // cargo test --lib -- --ignored --nocapture phase_identity_guard_tech_check
    #[test]
    #[ignore]
    fn phase_identity_guard_tech_check() {
        let samples = [
            "詩織はどのLLMを使ってるの？",
            "詩織って何のLLM使ってるんだっけ",
            "詩織の音声認識は何？",
            "詩織はどうやって記憶を探すの？",
            "詩織の頭脳って何？",
            "詩織のSTTって何を使ってるの",
            "詩織のTTSは何のモデル？",
            "詩織はどんな仕組みで動いてるの",
            "詩織が使ってるモデルを教えて",
            "詩織の声って何のTTS使ってるんだっけ",
        ];

        for text in samples {
            let start = std::time::Instant::now();
            let result = send_message_impl(text.to_string(), |_| {});
            let elapsed = start.elapsed();
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗(発話=\"{text}\"): {e}"));
            println!(
                "[{:>5.2}s] 発話=\"{text}\" => 判定={} 応答={}",
                elapsed.as_secs_f32(),
                reply.detected_mode,
                reply.reply
            );
            assert_eq!(
                reply.detected_mode, "search",
                "発話=\"{text}\": identity_guard(技術系キーワード)が発火せずsearch_knowledgeへ強制ルーティングされていない"
            );
        }
    }

    // アイデンティティ安全網の誤爆確認: キーワードに関係ない通常の雑談で
    // 強制検索が発動していないか([identity_guard]ログの有無で判定する)。
    // cargo test --lib -- --ignored --nocapture phase_identity_guard_false_positive_check
    #[test]
    #[ignore]
    fn phase_identity_guard_false_positive_check() {
        let samples = [
            "今日の天気はどう？",
            "このライブラリ、まだ使ってる？",
            "そのコード、声に出して読んでみて",
            "最近のモデル、性能上がったよね",
            "この記憶違いかもしれないんだけど",
            "仕組みが複雑すぎて分からない",
            "こんにちは、調子はどう？",
        ];

        for text in samples {
            let start = std::time::Instant::now();
            let result = send_message_impl(text.to_string(), |_| {});
            let elapsed = start.elapsed();
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗(発話=\"{text}\"): {e}"));
            println!(
                "[{:>5.2}s] 発話=\"{text}\" => 判定={} 応答={}",
                elapsed.as_secs_f32(),
                reply.detected_mode,
                reply.reply
            );
            assert_ne!(
                reply.detected_mode, "search",
                "発話=\"{text}\": identity_guardが誤爆してsearch_knowledgeへ強制ルーティングされている"
            );
        }
    }

    // メモ機能(memo_guard)の確認用: 「〇〇についてメモして」で実際に保存が
    // 行われるか(sourcesへの反映、library/00-inbox/へのファイル作成)、
    // 保存直後の別会話でpassive recallとして自然に想起されるか(即時反映)を
    // 確認する。想起の可否はembeddingの距離しきい値に左右される既知の限界
    // (docs/voice-consistency-policy.md参照)があるため、そちらは目視確認に
    // 留める(hard assertはしない)。
    // cargo test --lib -- --ignored --nocapture phase_memo_guard_check
    #[test]
    #[ignore]
    fn phase_memo_guard_check() {
        let unique_marker = format!("テストメモ{}", std::process::id());
        let text = format!("{unique_marker}という単語についてメモして");
        let result = send_message_impl(text.clone(), |_| {});
        let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
        println!("判定={} 応答={}", reply.detected_mode, reply.reply);
        assert_eq!(reply.detected_mode, "memo", "memo_guardが発火していない");
        assert!(reply.sources.is_some(), "保存したメモがsourcesに反映されていない");

        let memo_dir = library_root().join("00-inbox");
        let found = std::fs::read_dir(&memo_dir)
            .expect("library/00-inbox/の読み込みに失敗(ディレクトリが作られていない?)")
            .filter_map(|e| e.ok())
            .any(|e| {
                std::fs::read_to_string(e.path())
                    .map(|content| content.contains(&unique_marker))
                    .unwrap_or(false)
            });
        assert!(found, "library/00-inbox/に保存したメモの内容が見つからない");

        // 即時反映の確認(目視): 保存直後の別会話でpassive recallが拾うか
        let followup = send_message_impl(
            format!("さっき{unique_marker}についてメモしたと思うけど、何て書いた？"),
            |_| {},
        );
        match followup {
            Ok(r) => println!("追撃発話 判定={} 応答={}", r.detected_mode, r.reply),
            Err(e) => println!("追撃発話 エラー: {e}"),
        }
    }

    // メモ想起の診断用: 実際の使い方に近い内容でメモを保存し、直後に複数の
    // 言い回しで質問した際、RAG検索で実際にどの距離・リランクスコアで
    // メモチャンクが返るかを直接確認する。「さっきメモした〜って何だっけ」
    // のように話題を再言及しない参照表現は、話題そのものを言い当てられない
    // (リランカーのスコアが閾値を大きく下回る)という既知の限界がある
    // (docs/voice-consistency-policy.md参照)。将来同種の想起不具合を
    // 調査する際のテンプレートとして残す。
    // cargo test --lib -- --ignored --nocapture phase_memo_recall_diagnostic
    #[test]
    #[ignore]
    fn phase_memo_recall_diagnostic() {
        let config = app_config().expect("config読み込みに失敗");
        let save_text = "今日魚を食べたことをメモして".to_string();
        let result = send_message_impl(save_text, |_| {});
        let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
        println!("保存結果 判定={} 応答={}", reply.detected_mode, reply.reply);
        let memo_id = reply
            .sources
            .as_ref()
            .and_then(|s| s.first())
            .map(|s| s.id.clone())
            .expect("sourcesが空");
        println!("保存されたchunk id: {memo_id}");

        let queries = [
            "さっき言っていた魚の件だけど",
            "今日食べた魚のことなんだけど",
        ];
        for q in queries {
            let normalized = normalize_search_query(q);
            let results = rag_client::search(config.rag.port, &normalized, 10)
                .expect("RAG検索に失敗");
            println!("=== クエリ: \"{q}\" (正規化後: \"{normalized}\") ===");
            for r in &results {
                let marker = if r.id == memo_id { " <-- 保存したメモ" } else { "" };
                println!(
                    "  id={} distance={:.4} rerank_score={:.4}{marker}",
                    r.id, r.distance, r.rerank_score
                );
            }
            if !results.iter().any(|r| r.id == memo_id) {
                println!("  ※ 保存したメモがtop10に入っていません");
            }
        }

        println!("\n=== send_message_impl経由(実際の会話フロー)での確認 ===");
        // 「棚を教える」方針転換後(2026-08-12)は、LLMに渡るのが本文ではなく
        // 見出し(=content先頭40文字)のみになるため、「魚」を含むかどうかは
        // 「本文を語れているか」ではなく「見出しに魚という語が入っていて、それが
        // 案内文に出てきたか」の確認に意味合いが変わる。参考程度に留める。
        for q in queries {
            let r = send_message_impl(q.to_string(), |_| {})
                .unwrap_or_else(|e| panic!("送信に失敗: {e}"));
            let mentioned = r.reply.contains('魚');
            println!(
                "発話=\"{q}\" 判定={} 「魚」を含む={mentioned} 応答={}",
                r.detected_mode, r.reply
            );
        }
    }

    // メモ機能の誤爆確認: トリガーキーワードに関係ない通常の雑談で
    // memo_guardが発動していないか([memo_guard]ログの有無で判定する)。
    // cargo test --lib -- --ignored --nocapture phase_memo_guard_false_positive_check
    #[test]
    #[ignore]
    fn phase_memo_guard_false_positive_check() {
        let samples = [
            "今日の天気はどう？",
            "この設計、覚えるのが大変そう",
            "さっきの記録、見返しておいてもらえる？",
            "テストのメモリ使用量が気になる",
            "こんにちは、調子はどう？",
        ];

        for text in samples {
            let result = send_message_impl(text.to_string(), |_| {});
            match result {
                Ok(reply) => println!("発話=\"{text}\" => 判定={} 応答={}", reply.detected_mode, reply.reply),
                Err(e) => println!("発話=\"{text}\" => エラー: {e}"),
            }
        }
    }

    // 壁打ち・相談場面での「問い返す」振る舞いの発生率確認。会話例追加前は
    // 2/2で直接的な意見提示になっていた。目視で「質問で応答しているか」を判定する。
    // cargo test --lib -- --ignored --nocapture phase_brainstorm_askback_check
    #[test]
    #[ignore]
    fn phase_brainstorm_askback_check() {
        let samples = [
            "lab.huraru.comをWeb技術特化にするか、全技術実験に広げるか迷ってるんだよね",
            "portfolioのデザイン、ミニマルにするか賑やかにするか悩んでる",
            "この機能、今実装するか後回しにするか迷ってる",
            "AとBどっちのライブラリを使うか決めかねてる",
            "リファクタリングを今やるべきか、機能追加を優先すべきか迷う",
            "このバグ、応急処置で直すか根本から直すか悩んでる",
            "新しいプロジェクトを始めようか迷ってるんだよね",
            "UIをシンプルにしようと思ってるんだけど、これでいいのかな",
            "この設計、後で困りそうな気もするけど大丈夫かな",
            "テストを先に書くか、実装を先にするか迷う",
        ];

        let mut askback_count = 0;
        for text in samples {
            let start = std::time::Instant::now();
            let result = send_message_impl(text.to_string(), |_| {});
            let elapsed = start.elapsed();
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗(発話=\"{text}\"): {e}"));
            let asked_back = reply.reply.contains('？') || reply.reply.contains('?');
            if asked_back {
                askback_count += 1;
            }
            println!(
                "[{:>5.2}s] 発話=\"{text}\" 問い返し={asked_back} => 判定={} 応答={}",
                elapsed.as_secs_f32(),
                reply.detected_mode,
                reply.reply
            );
        }
        // 会話例追加前は10件中0件だった。過剰に厳密な閾値にはせず、半数以上が
        // 問い返しになっていることだけを最低ラインとして検証する。
        println!("問い返し発生率: {askback_count}/{}", samples.len());
        assert!(
            askback_count * 2 >= samples.len(),
            "壁打ち発話に対する問い返し率が低すぎる({askback_count}/{})",
            samples.len()
        );
    }

    // 一人称で話す指示の効果確認(自分自身について三人称で語っていないか)。
    // 「棚を教える」方針転換後は、いずれの質問も中身の内容ではなく棚の案内に
    // なるはずだが、その案内自体が一人称で書けているかを確認する(2026-08-12)。
    // cargo test --lib -- --ignored --nocapture phase_first_person_check
    #[test]
    #[ignore]
    fn phase_first_person_check() {
        let topics = [
            "しおりのキャラクター名の由来ってなんだっけ",
            "詩織って何のLLM使ってるんだっけ",
            "詩織はどうやって記憶を探してるんだっけ",
            "詩織の将来的な目標って何だっけ",
            "詩織の声とか耳の部分ってどうなってるんだっけ",
        ];
        for topic in topics {
            for _ in 0..2 {
                let result = send_message_impl(topic.to_string(), |_| {});
                let reply = result.unwrap_or_else(|e| panic!("送信に失敗: {e}"));
                println!("発話=\"{topic}\" 応答={}", reply.reply);
                // self_reference_guardによる三人称→一人称の補正が正しく適用されていれば、
                // 応答本文に「詩織は」「詩織が」という三人称の自己言及は残らないはず。
                assert!(
                    !reply.reply.contains("詩織は") && !reply.reply.contains("詩織が"),
                    "発話=\"{topic}\": 応答に三人称の自己言及が残っている: {}",
                    reply.reply
                );
            }
        }
    }

    // 「道具ではなく居る存在」への再設計後の振る舞いを確認する手動実行用テスト。
    // cargo test --lib -- --ignored --nocapture phase_function_calling
    #[test]
    #[ignore]
    fn phase_function_calling_samples() {
        let samples: [(&str, &str); 6] = [
            ("こんにちは、調子はどう？", "ツール不要(chat想定)"),
            (
                "詩織のキャラクター名の由来って何だっけ",
                "明示的な検索依頼(search_knowledgeが機能するか、プロフィールデータ投入後)。方針転換後は由来の中身を語らず棚を案内するはず",
            ),
            (
                "huraru.comのデザインって結局どんな方向性にしたんだっけ",
                "search_knowledgeの自発的想起想定(profile-huraru.mdの世界観の記述と関連。検索を明示的に頼んでいない雑談)。方針転換後は方向性の中身を語らず棚を案内するはず",
            ),
            (
                "今日は宇宙旅行の予約でも取ろうかな",
                "関連する記憶が無い話題(不自然な「見つかりませんでした」を言わず自然に流れるはず)",
            ),
            ("huraru.comのフォントについてメモして", "memo_guard想定(直接保存されるか)"),
            ("これ、どう思う？", "曖昧な発話(壁打ち相当、ツール不要のはず)"),
        ];

        for (text, expectation) in samples {
            let start = std::time::Instant::now();
            let result = send_message_impl(text.to_string(), |_| {});
            let elapsed = start.elapsed();
            let reply = result.unwrap_or_else(|e| panic!("送信に失敗(発話=\"{text}\"): {e}"));
            println!(
                "[{:>5.2}s] 発話=\"{text}\" 期待={expectation} => 判定={} 応答冒頭={}",
                elapsed.as_secs_f32(),
                reply.detected_mode,
                &reply.reply.chars().take(60).collect::<String>()
            );
            match text {
                "こんにちは、調子はどう？" | "これ、どう思う？" => {
                    assert_eq!(reply.detected_mode, "chat", "発話=\"{text}\": ツール不要のはずが発火している");
                }
                "huraru.comのフォントについてメモして" => {
                    assert_eq!(reply.detected_mode, "memo", "発話=\"{text}\": memo_guardが機能していない");
                }
                _ => {}
            }
        }
    }

    // SttManagerのオンデマンド起動・アイドル自動停止を実際に検証する手動実行用テスト。
    // cargo test --lib -- --ignored --nocapture phase_stt_manager
    #[test]
    #[ignore]
    fn phase_stt_manager_lazy_start_and_idle_stop() {
        let manager = SttManager::new();
        let exe = whisper_server_exe_path();
        let config = app_config().expect("config読み込みに失敗");
        let model_path = project_root().join(&config.stt.model_path);

        println!("起動前にwhisper-serverがhealthyでないことを確認...");
        assert!(
            !wait_for_health(config.stt.port, 1),
            "テスト開始前からwhisper-serverが起動している(他のテスト/プロセスと衝突?)"
        );

        println!("ensure_startedを呼び出し...");
        manager
            .ensure_started(&exe, &model_path, config.stt.port)
            .expect("起動に失敗");
        assert!(
            wait_for_health(config.stt.port, 60),
            "起動後にhealthyにならなかった"
        );
        println!("起動確認OK");

        println!("アイドルタイムアウト(60秒)を待機...");
        std::thread::sleep(STT_IDLE_TIMEOUT + Duration::from_secs(15));

        let still_healthy = ureq::get(&format!("http://127.0.0.1:{}/health", config.stt.port))
            .timeout(Duration::from_secs(2))
            .call()
            .is_ok();
        assert!(!still_healthy, "アイドルタイムアウト後も起動したままだった");
        println!("アイドル自動停止OK");
    }

    // 要確認UI(Phase 8)のtags.yaml/projects.yamlテキスト書き換えが、対象
    // エントリだけを書き換え、先頭コメントや他のエントリを壊さないことを
    // 確認する軽量テスト(実ファイルではなくtempdir上のコピーで検証)。
    fn write_temp_yaml(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "shiori_test_pending_yaml_{}_{}",
            name,
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.yaml"));
        std::fs::write(&path, content).unwrap();
        path
    }

    const SAMPLE_TAGS_YAML: &str = "# ヘッダーコメント1行目\n# ヘッダーコメント2行目\ntags:\n  - canonical: 詩織\n    aliases: [shiori]\n    status: confirmed\n  - canonical: 新タグ\n    aliases: []\n    status: pending\n";

    #[test]
    fn rewrite_yaml_status_updates_only_target_entry() {
        let path = write_temp_yaml("rewrite", SAMPLE_TAGS_YAML);
        rewrite_yaml_status(&path, "canonical", "新タグ", "confirmed").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("# ヘッダーコメント1行目"), "先頭コメントが保持されていない");
        assert!(result.contains("canonical: 詩織\n    aliases: [shiori]\n    status: confirmed"), "既存エントリが変化した");
        assert!(result.contains("canonical: 新タグ\n    aliases: []\n    status: confirmed"), "対象エントリが書き換わっていない");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn remove_yaml_list_item_removes_only_target_entry() {
        let path = write_temp_yaml("remove", SAMPLE_TAGS_YAML);
        remove_yaml_list_item(&path, "canonical", "新タグ").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("# ヘッダーコメント1行目"), "先頭コメントが保持されていない");
        assert!(result.contains("canonical: 詩織"), "既存エントリが消えた");
        assert!(!result.contains("新タグ"), "対象エントリが削除されていない");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn rewrite_inbox_review_status_inserts_when_absent() {
        let content = "---\ntitle: テスト\nreason: タグが空でした\n---\n\n本文\n";
        let path = write_temp_yaml("inbox_insert", content).with_extension("md");
        std::fs::write(&path, content).unwrap();
        rewrite_inbox_review_status(&path, "deferred").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("review_status: deferred"));
        assert!(result.contains("本文"), "本文が保持されていない");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn rewrite_inbox_review_status_replaces_when_present() {
        let content = "---\ntitle: テスト\nreview_status: deferred\n---\n\n本文\n";
        let path = write_temp_yaml("inbox_replace", content).with_extension("md");
        std::fs::write(&path, content).unwrap();
        rewrite_inbox_review_status(&path, "confirmed").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("review_status: confirmed"));
        assert!(!result.contains("review_status: deferred"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

// ホットキー(Ctrl+Alt+S)押下時の録音トグル処理。開始/停止の実装は`audio`モジュールを
// VoiceBarのマイクボタン経由(start_recording/stop_recording_and_transcribeコマンド)と共有し、
// 結果はイベントでフロントに通知する(モードに応じたLLM呼び出しはフロント側で行う)。
fn handle_hotkey_toggle(app: &tauri::AppHandle) {
    use tauri::{Emitter, Manager};

    let recording_state = app.state::<audio::RecordingState>();
    let stt = app.state::<Arc<SttManager>>();
    let is_recording = recording_state.is_recording();

    if !is_recording && app_config().is_ok_and(|c| c.mode == AppMode::External) {
        let _ = app.emit("voice:failed", serde_json::json!({ "message": EXTERNAL_MODE_MESSAGE }));
        return;
    }

    if !is_recording {
        match audio::start(&recording_state) {
            Ok(()) => {
                let _ = app.emit("voice:recording-started", ());
                // 発話中にwhisper-serverの起動待ちを隠す(start_recordingコマンドと同じ考え方)
                let stt = stt.inner().clone();
                std::thread::spawn(move || {
                    let exe = whisper_server_exe_path();
                    match app_config() {
                        Ok(config) => {
                            let model_path = project_root().join(&config.stt.model_path);
                            if let Err(e) = stt.ensure_started(&exe, &model_path, config.stt.port) {
                                log_error!("whisper-serverの起動に失敗: {e}");
                            }
                        }
                        Err(e) => log_error!("config読み込みに失敗: {e}"),
                    }
                });
            }
            Err(e) => log_error!("録音開始に失敗: {e}"),
        }
    } else {
        // 失敗はログだけでなくフロントにも通知する(以前はeprintln!のみで、画面上は
        // 何も起きないように見えていた)。
        let notify_failure = |message: String| {
            log_error!("{message}");
            let _ = app.emit("voice:failed", serde_json::json!({ "message": message }));
        };

        let wav = match audio::stop_and_encode_wav(&recording_state) {
            Ok(w) => w,
            Err(e) => {
                notify_failure(format!("録音停止に失敗: {e}"));
                return;
            }
        };

        let config = match app_config() {
            Ok(c) => c,
            Err(e) => {
                notify_failure(format!("config読み込みに失敗: {e}"));
                return;
            }
        };

        // ホットキーのハンドラはメインスレッドで呼ばれるため、起動待ち・文字起こしは
        // 別スレッドで行う(stop_recording_and_transcribeのコメント参照)。
        let app = app.clone();
        let stt = stt.inner().clone();
        std::thread::spawn(move || match stt.transcribe_when_ready(config.stt.port, &wav) {
            Ok(text) => {
                let _ = app.emit("voice:transcribed", TranscribeResult { text });
            }
            Err(e) => {
                let message = format!("文字起こしに失敗: {e}");
                log_error!("{message}");
                let _ = app.emit("voice:failed", serde_json::json!({ "message": message }));
            }
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri_plugin_global_shortcut::ShortcutState;
    let root = project_root();
    let before = repair::before_log(&root);
    shiori_log::init(
        &root.join("data").join("logs"),
        shiori_log::LogKind::App,
        &shiori_log::LogSettings::from_config(&root),
    );
    shiori_log::info("APP", "起動した", &[("版", &env!("CARGO_PKG_VERSION")), ("場所", &root.display())]);
    // 起動時の自動修復(詩織Ver6.0の段階6)。設定とハブを読む前に、詩織が作ったものを直す
    repair::run_at_startup(&root, before);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        // 形の切り替え(詩織Ver6.0の段階4)と、録音
                        if pet::is_pet_shortcut(shortcut) {
                            pet::toggle_from_hotkey(app.clone());
                        } else {
                            handle_hotkey_toggle(app);
                        }
                    }
                })
                .build(),
        )
        .manage(BackendState(Mutex::new(BackendProcesses::default())))
        // 拡張機能(詩織Ver6.0の段階1)。起動時に1回だけ読み込む
        .manage(hub::HubState(Mutex::new(hub::Hub::load(&root))))
        // 画面の部品の拡張機能を配る専用のURLの仕組み(shiori-ext://)
        .register_uri_scheme_protocol(hub::protocol::SCHEME, hub::protocol::handle)
        .manage(audio::RecordingState::new())
        .manage(SttManager::new())
        .manage(Mutex::new(sysinfo::System::new_all()))
        .manage(Mutex::new(DiskMonitor {
            disks: sysinfo::Disks::new_with_refreshed_list(),
            prev_counters: None,
            last_refresh: Instant::now(),
        }))
        .setup(|app| {
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            // 自己確認の起動(詩織Ver6.0の段階2)なら、打ち切りの見張りを始める
            self_test::start_watchdog(app.handle().clone());
            // 知らせと時刻の予約(詩織Ver6.0の段階3)。Claudeの作業の終わりと要確認も見張る
            notify::start(app.handle().clone());
            notify::watch_pending(pending_badge_count);
            // 別のプログラムの拡張機能(詩織Ver6.0の段階1)。頼みごとは、画面の部品と同じ窓口に通す
            {
                let handle = app.handle().clone();
                let handler: hub::process::Handler = Arc::new(move |name, method, params| {
                    let programs = handle.try_state::<hub::process::ProgramsState>();
                    hub::bridge::call(
                        &handle.state::<hub::HubState>(),
                        programs.as_ref().map(|p| &p.0),
                        name,
                        method,
                        params,
                    )
                });
                let programs = hub::process::Programs::new(handler);
                if let Ok(state) = app.state::<hub::HubState>().0.lock() {
                    hub::start_ready_programs(&state, &programs);
                }
                app.manage(hub::process::ProgramsState(programs));
            }
            // ホットキーの変更はコントロールパネルからconfig.jsonへ保存されるが、
            // 反映にはアプリ再起動が必要(次回起動時にここで読み直される)。
            let hotkey = app_config()
                .map(|c| c.hotkey)
                .unwrap_or_else(|_| "ctrl+alt+s".to_string());
            app.global_shortcut().register(hotkey.as_str())?;
            // 形の切り替えのホットキー(詩織Ver6.0の段階4)。登録できなくても起動は止めない
            let pet_hotkey = app_config().map(|c| c.pet.hotkey).unwrap_or_else(|_| default_pet_hotkey());
            pet::register_hotkey(app.handle(), &pet_hotkey);
            // SSDを挿していなかった間の分も含め、このPCのClaudeの履歴を台帳へ取り込む。
            std::thread::spawn(|| {
                if let Err(e) = refresh_claude_stats_blocking(true) {
                    log_error!("Claudeの利用統計の取り込みに失敗: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            start_backend_services,
            send_message,
            start_recording,
            stop_recording_and_transcribe,
            synthesize_speech,
            preview_voice,
            get_system_info,
            get_config,
            list_claude_sessions,
            open_claude_session,
            refresh_claude_stats,
            log_frontend_error,
            set_app_mode,
            get_claude_stats,
            eject_preview,
            eject_execute,
            set_config,
            restart_llm_services,
            retry_rag_service,
            restart_app,
            list_available_models,
            estimate_model_switch,
            switch_model,
            get_source_document,
            open_library_asset,
            reveal_library_asset,
            list_system_docs,
            read_system_doc,
            list_shelf_docs,
            read_shelf_doc,
            search_shelf,
            list_all_knowledge,
            search_library,
            list_library_projects,
            get_latest_handoff,
            get_portable_storage,
            get_source_frontmatter,
            list_pending_items,
            resolve_pending_tag,
            resolve_pending_project,
            resolve_pending_inbox_item,
            get_tts_failures,
            get_passive_recall_stats,
            get_ragas_history,
            log_ui_event,
            hub::list_extensions,
            hub::approve_extension,
            hub::call_extension,
            hub::bridge::ext_call,
            self_test::self_test_mode,
            self_test::self_test_log,
            self_test::self_test_finish,
            self_test::self_test_clipboard_text,
            diagnose_report,
            diagnose_copy,
            self_test::self_test_notify,
            self_test::self_test_repair,
            pet::pet_enter,
            pet::pet_leave,
            pet::pet_press,
            pet::pet_open_menu,
            pet::pet_pick_start,
            pet::pet_pick_choose,
            pet::pet_pick_cancel,
            pet::pet_sprites,
            pet::pet_bubble_done,
            pet::pet_open_source,
            pet::pet_panel_report,
            pet::pet_close_panel,
            pet::pet_status,
            pet::pet_selftest_panel,
            pet::pet_selftest_close_panel,
            pet::pet_behavior_get,
            pet::pet_behavior_set,
            pet::pet_seen_apps,
            sidebar_layout::get_sidebar_layout,
            sidebar_layout::set_sidebar_layout,
            sidebar_layout::list_icon_overrides
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // アプリ終了時、起動したままの子プロセス(llama-server/whisper-server)を
            // 明示的にkillする。std::process::ChildはDropしても自動killされないため、
            // ここで止めないと閉じた後もプロセスが孤立して残り続けてしまう。
            // 本体のウィンドウを閉じたら、詩織全体を終える。ペットの形の窓(段階4)は隠して残してあるので、
            // これが無いと、本体のウィンドウを閉じても詩織が終わらずに残る
            if let tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } = &event {
                if label == "main" {
                    app_handle.exit(0);
                }
            }
            // Macの「終了」(⌘Q・Dock・アプリへの終了の合図)は、ExitRequestedを通らずにExitへ進む
            // (段階6で見つけた)。どちらでも終わりの処理を走らせる(shutdownの中で1回だけにする)
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                shutdown(app_handle);
            }
        });
}

/// 終わりの処理。起動した子プロセスを止め、自分が止めたサービスのロックを消し、拡張機能を止める。
/// 何度呼んでも1回だけ走る。
fn shutdown(app_handle: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    shiori_log::info("APP", "終了する", &[]);
    if let Some(backend) = app_handle.try_state::<BackendState>() {
        if let Ok(mut procs) = backend.0.lock() {
            if let Some(mut c) = procs.llm.take() {
                let _ = c.kill();
            }
            let root = project_root();
            if let Some(mut c) = procs.embedding.take() {
                let _ = c.kill();
                shared_daemon::clear_lock_of(&root, ".shiori-embed.lock", c.id());
            }
            if let Some(mut c) = procs.rag.take() {
                let _ = c.kill();
                shared_daemon::clear_lock_of(&root, ".shiori-rag.lock", c.id());
            }
        }
    }
    if let Some(stt) = app_handle.try_state::<Arc<SttManager>>() {
        stt.stop_now("アプリ終了のため");
    }
    if let Some(programs) = app_handle.try_state::<hub::process::ProgramsState>() {
        programs.0.stop_all();
    }
}

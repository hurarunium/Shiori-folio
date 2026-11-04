//! ペットの形(詩織Ver6.0の段階4)。
//!
//! ウィンドウの形とペットの形を切り替える。ペットの形にすると、本体のウィンドウを隠し(閉じない。
//! 会話・録音・予約は止まらない)、透明で常に最前面の小さなウィンドウ(段階0の方式A)にキャラクターを出す。
//! 本のメニューと小道具の枠(`pet-panel`)、知らせの吹き出し(`pet-bubble`)は、ペットとは別の透明な
//! ウィンドウにする(ペットのウィンドウを大きくすると、クリックを通す範囲が広がるため)。
//!
//! 位置・状態・絵のコマは本体の側(`brain.rs`)で決め、画面へは `pet://frame` で送る。
//! 絵の透明な点のクリックは、下のアプリに通す(カーソルの位置を見て `set_ignore_cursor_events` を切り替える)。
//! Macでは、ペットの形のあいだだけ、詩織をDockとCmd+Tabから外す(本体のウィンドウを隠している間だけなので、
//! ペットのウィンドウだけを外す仕組みは要らない)。

pub mod behavior;
pub mod brain;
pub mod os;
pub mod sprites;
pub mod surfaces;
pub mod windows;

use crate::error_codes;
use crate::notify::{Kind, Notice};
use crate::shiori_log;
use behavior::{Behavior, Like, Preset, SeenApp};
use brain::{Brain, Event, Input, World};
use serde::Serialize;
use serde_json::Value;
use sprites::{Sprites, FRAME_H, FRAME_W};
use surfaces::{Platform, ScreenArea, WinInfo};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

const PART: &str = "PET";
pub const PET: &str = "pet";
pub const PANEL: &str = "pet-panel";
pub const BUBBLE: &str = "pet-bubble";
/// 足場の確かめの表示(画面ごとに1つ。`SHIORI_PET_DEBUG=1` で起動したときだけ出す)
const DEBUG_PREFIX: &str = "pet-debug-";
/// キャラクターの背丈が、画面の短いほうの辺に占める割合(縦置きの画面でも同じ大きさにするため、
/// 高さではなく短いほうの辺で決める。ペットの行動の設計書の3.5節。ふらる、11/3)
const HEIGHT_RATIO: f64 = 0.18;
/// 計算の間隔(1秒に60回)
const TICK: Duration = Duration::from_millis(16);
/// 絵のコマの速さ
const FPS: f64 = 8.0;
/// 本と小道具の枠(100%のとき)、吹き出しの大きさ(論理の画素)。本と枠は設定の割合を掛ける
const PANEL_SIZE: (f64, f64) = (440.0, 470.0);
const BUBBLE_SIZE: (f64, f64) = (280.0, 132.0);
/// 本がペットの押しで閉じたすぐあとの押しでは、開き直さない(もう一度押すと閉じる、のため)
const REOPEN_GUARD: Duration = Duration::from_millis(400);
/// ほかのアプリのウィンドウの一覧を読み直す間隔(ペットの行動の設計書の1節)
const WINDOWS_EVERY: Duration = Duration::from_millis(250);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct FrameDto {
    state: brain::State,
    activity: brain::Activity,
    anim: String,
    file: String,
    frames: u32,
    frame: u32,
    facing: f64,
    /// 1ドットが何画素か(物理の画素)
    scale: u32,
}

struct Inner {
    active: bool,
    brain: Option<Brain>,
    /// 1ドットが何物理画素か
    scale: u32,
    /// 1ドットが座標の単位でいくつか(Windowsはscaleと同じ。Macは論理の点)
    dot: f64,
    sprites: Sprites,
    ignoring: bool,
    last_pos: (i32, i32),
    last_frame: Option<FrameDto>,
    panel_visible: bool,
    panel_hidden_at: Option<Instant>,
    bubble_visible: bool,
    panel_items: Vec<String>,
    panel_tool: Option<Value>,
    /// いまの本と小道具の枠の大きさ(論理の画素)
    panel_size: (f64, f64),
    sleep_after: f64,
    /// 前面のアプリが全画面か(0.5秒ごとに見直す)
    fullscreen: bool,
    fullscreen_checked: Option<Instant>,
    world: Option<World>,
    /// 全部の画面、ほかのアプリのウィンドウ(前面から順)、足場(ペットの行動の設計書の1・2節)
    screens: Vec<ScreenArea>,
    windows: Vec<WinInfo>,
    platforms: Vec<Platform>,
    windows_checked: Option<Instant>,
    /// ウィンドウの一覧を読むのにかかったミリ秒(自己確認で見る)
    windows_ms: f64,
    /// 行動の設定(`data/pet/behavior.json`。ペットの形にするたびに読み、設定画面で保存するとすぐ入れ替える)
    behavior: Behavior,
    /// この起動で見かけたアプリの名前(新しい名前を見たときだけ、見かけたアプリの一覧に書く)
    seen: std::collections::HashSet<String>,
}

fn ctl() -> &'static Mutex<Inner> {
    static C: OnceLock<Mutex<Inner>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(Inner {
            active: false,
            brain: None,
            scale: 1,
            dot: 1.0,
            sprites: Sprites::load(&crate::project_root().join("data").join("ui").join("pet")),
            ignoring: false,
            last_pos: (i32::MIN, i32::MIN),
            last_frame: None,
            panel_visible: false,
            panel_hidden_at: None,
            bubble_visible: false,
            panel_items: Vec::new(),
            panel_tool: None,
            panel_size: PANEL_SIZE,
            sleep_after: 300.0,
            fullscreen: false,
            fullscreen_checked: None,
            world: None,
            screens: Vec::new(),
            windows: Vec::new(),
            platforms: Vec::new(),
            windows_checked: None,
            windows_ms: 0.0,
            behavior: Behavior::default(),
            seen: std::collections::HashSet::new(),
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Inner> {
    ctl().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn is_active() -> bool {
    lock().active
}

/// 絵のファイル(専用のURLの仕組みの `/_shiori/pet/` から返す)
pub fn sprite_file(file: &str) -> Option<Vec<u8>> {
    lock().sprites.file_bytes(file)
}

// ---- ホットキー ----

static SHORTCUT: OnceLock<tauri_plugin_global_shortcut::Shortcut> = OnceLock::new();

/// 形の切り替えのホットキーを登録する。登録できなくても起動は止めない(ボタンからは切り替えられる)。
pub fn register_hotkey(app: &AppHandle, hotkey: &str) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let result = hotkey
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map_err(|e| e.to_string())
        .and_then(|s| app.global_shortcut().register(s).map(|_| s).map_err(|e| e.to_string()));
    match result {
        Ok(s) => {
            let _ = SHORTCUT.set(s);
            shiori_log::info(PART, "形の切り替えのホットキーを登録した", &[("キー", &hotkey)]);
        }
        Err(e) => shiori_log::warn(error_codes::PET_003.code, "形の切り替えのホットキーを登録できない", &[("キー", &hotkey), ("理由", &e)]),
    }
}

pub fn is_pet_shortcut(s: &tauri_plugin_global_shortcut::Shortcut) -> bool {
    SHORTCUT.get().is_some_and(|p| p == s)
}

/// ホットキーから切り替える(ウィンドウを作ることがあるので、非同期の中で行う)
pub fn toggle_from_hotkey(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let result = if is_active() { leave(&app) } else { enter(&app) };
        if let Err(e) = result {
            shiori_log::warn(error_codes::PET_001.code, "形を切り替えられない", &[("理由", &e)]);
        }
    });
}

// ---- 窓 ----

fn build(app: &AppHandle, label: &str, view: &str, size: (f64, f64)) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(label) {
        return Ok(w);
    }
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("index.html?view={view}").into()))
        .title("詩織")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .inner_size(size.0, size.1)
        .visible(false)
        .focused(false)
        .build()
        .map_err(|e| format!("{label}: {e}"))
}

fn ensure_windows(app: &AppHandle) -> Result<(), String> {
    build(app, PET, "pet", (FRAME_W as f64, FRAME_H as f64))?;
    let panel = build(app, PANEL, "pet-panel", PANEL_SIZE)?;
    build(app, BUBBLE, "pet-bubble", BUBBLE_SIZE)?;
    // 本と小道具の枠は、外を押したら(詩織から離れたら)閉じる
    static HOOKED: OnceLock<()> = OnceLock::new();
    if HOOKED.set(()).is_ok() {
        panel.on_window_event(|e| {
            if let tauri::WindowEvent::Focused(false) = e {
                if let Some(app) = crate::notify::app() {
                    hide_panel(app, "外を押した");
                }
            }
        });
    }
    Ok(())
}

fn window(app: &AppHandle, label: &str) -> Option<WebviewWindow> {
    app.get_webview_window(label)
}

// ---- 画面の座標 ----
//
// ペットの位置・画面の範囲・カーソルは、同じ「座標の単位」で扱う。Windowsは物理の画素。
// Macは論理の点にする。Macのtauri/taoの「物理の座標」は、画面の位置と大きさを、その画面の倍率で
// 換算するので、倍率の違う画面(Retinaと外付け)をまたぐと座標が飛ぶ。カーソルは主画面の倍率で
// 換算されるので、外付けの画面ではペットをつまめなかった(10/26)。論理の点なら、画面をまたいでも続く。

/// 画面(座標の単位で表した範囲と、ドット絵の倍率)
#[derive(Clone)]
struct Screen {
    monitor: tauri::Monitor,
    /// 画面全体と作業領域(x, y, 幅, 高さ)
    full: (f64, f64, f64, f64),
    work: (f64, f64, f64, f64),
    /// 1ドットが何物理画素か(絵をにじませないため整数)
    dot_scale: u32,
    /// 1ドットが座標の単位でいくつか
    dot: f64,
    /// 論理の画素1つが座標の単位でいくつか(本と吹き出しの大きさに使う)
    logical: f64,
}

/// 物理の画素1つが、座標の単位でいくつか
fn unit_per_px(monitor: &tauri::Monitor) -> f64 {
    if cfg!(target_os = "macos") {
        1.0 / monitor.scale_factor()
    } else {
        1.0
    }
}

fn screen_of(monitor: tauri::Monitor) -> Screen {
    let k = unit_per_px(&monitor);
    let (p, z, wa) = (monitor.position(), monitor.size(), monitor.work_area());
    let full = (p.x as f64 * k, p.y as f64 * k, z.width as f64 * k, z.height as f64 * k);
    let work =
        (wa.position.x as f64 * k, wa.position.y as f64 * k, wa.size.width as f64 * k, wa.size.height as f64 * k);
    let short = z.width.min(z.height) as f64;
    let dot_scale = ((short * HEIGHT_RATIO / FRAME_H as f64).round() as u32).max(1);
    let logical = monitor.scale_factor() * k;
    Screen { monitor, full, work, dot_scale, dot: dot_scale as f64 * k, logical }
}

/// いまいる画面(点を含む画面。無ければ主な画面)。点は座標の単位
fn screen_at(app: &AppHandle, x: f64, y: f64) -> Option<Screen> {
    let screens: Vec<Screen> = app.available_monitors().unwrap_or_default().into_iter().map(screen_of).collect();
    let inside = |s: &&Screen| {
        let (sx, sy, w, h) = s.full;
        x >= sx && x < sx + w && y >= sy && y < sy + h
    };
    screens.iter().find(inside).cloned().or_else(|| app.primary_monitor().ok().flatten().map(screen_of))
}

/// カーソルの位置(座標の単位)。Macのtaoは主画面の倍率で物理に換算して返すので、論理の点に戻す
fn cursor_position(app: &AppHandle) -> Option<(f64, f64)> {
    let c = app.cursor_position().ok()?;
    let k = if cfg!(target_os = "macos") {
        1.0 / app.primary_monitor().ok().flatten().map_or(1.0, |m| m.scale_factor())
    } else {
        1.0
    };
    Some((c.x * k, c.y * k))
}

/// ウィンドウの位置と大きさ(座標の単位)
fn window_rect(w: &WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    let k = if cfg!(target_os = "macos") { 1.0 / w.scale_factor().ok()? } else { 1.0 };
    let (p, z) = (w.outer_position().ok()?, w.outer_size().ok()?);
    Some((p.x as f64 * k, p.y as f64 * k, z.width as f64 * k, z.height as f64 * k))
}

fn set_window_position(w: &WebviewWindow, x: f64, y: f64) -> tauri::Result<()> {
    if cfg!(target_os = "macos") {
        w.set_position(LogicalPosition::new(x, y))
    } else {
        w.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
    }
}

/// ペットのウィンドウを、絵と同じ大きさにする
fn set_pet_size(w: &WebviewWindow, screen: &Screen) -> tauri::Result<()> {
    if cfg!(target_os = "macos") {
        w.set_size(LogicalSize::new(FRAME_W as f64 * screen.dot, FRAME_H as f64 * screen.dot))
    } else {
        w.set_size(PhysicalSize::new(FRAME_W * screen.dot_scale, FRAME_H * screen.dot_scale))
    }
}

fn area_of(s: &Screen) -> ScreenArea {
    let r = |(x, y, w, h): (f64, f64, f64, f64)| surfaces::Rect::new(x, y, w, h);
    ScreenArea { full: r(s.full), work: r(s.work) }
}

fn all_screens(app: &AppHandle) -> Vec<Screen> {
    app.available_monitors().unwrap_or_default().into_iter().map(screen_of).collect()
}

/// 上辺のボタンが付いている所の幅(座らない所。左端から、右端から)。Macは左の3つの丸、Windowsは右の3つのボタン
fn caption_of(screen: &Screen) -> (f64, f64) {
    if cfg!(target_os = "macos") {
        (76.0 * screen.logical, 0.0)
    } else {
        (0.0, 140.0 * screen.logical)
    }
}

/// 行動の設定のファイル
fn behavior_path() -> std::path::PathBuf {
    crate::project_root().join("data").join("pet").join("behavior.json")
}

/// 見かけたアプリの一覧のファイル
fn seen_path() -> std::path::PathBuf {
    crate::project_root().join("data").join("pet").join("seen-apps.json")
}

fn os_tag() -> &'static str {
    if cfg!(windows) {
        "win"
    } else {
        "mac"
    }
}

/// 新しく見かけたアプリを、見かけたアプリの一覧に足す。SSDへの書き込みは1秒60回の計算を止めないよう、別のスレッドで行う
fn note_seen(names: Vec<String>) {
    std::thread::spawn(move || {
        let path = seen_path();
        let mut list = behavior::load_seen(&path);
        let now = chrono::Local::now().to_rfc3339();
        if !behavior::merge_seen(&mut list, &names, os_tag(), &now) {
            return;
        }
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, serde_json::to_string_pretty(&list).unwrap_or_default()));
        match result {
            Ok(()) => shiori_log::info(PART, "見かけたアプリを一覧に足した", &[("アプリ", &format!("{names:?}"))]),
            Err(e) => shiori_log::warn(error_codes::PET_007.code, "見かけたアプリの一覧を書けない", &[("理由", &e)]),
        }
    });
}

/// 行動の設定を読む。読めなければ「普通」のひな形で動き、PET-006をログに書く
fn load_behavior() -> Behavior {
    let path = behavior_path();
    let (b, err) = Behavior::load(&path);
    if let Some(e) = err {
        shiori_log::warn(error_codes::PET_006.code, "ペットの行動の設定を読めない", &[("場所", &path.display()), ("理由", &e)]);
    }
    b
}

/// ペットのいる世界。`current` はペットの真ん中がある画面
fn world_of(
    c: &Inner,
    screen: &Screen,
    current: usize,
    ride: Option<(u64, surfaces::Presence)>,
    idle: Option<f64>,
) -> World {
    let mut platforms = c.platforms.clone();
    // 全画面のアプリが前面にあるときは、タスクバーの分も床にする(作業領域が変わらず浮くため)
    if c.fullscreen {
        if let Some(p) = platforms.iter_mut().find(|p| p.owner == surfaces::Owner::Floor { screen: current }) {
            p.y = screen.full.1 + screen.full.3;
        }
    }
    World {
        scale: c.dot,
        size: (FRAME_W as f64 * c.dot, FRAME_H as f64 * c.dot),
        screens: c.screens.clone(),
        current,
        platforms,
        ride,
        caption: caption_of(screen),
        behavior: brain::Params::from(&c.behavior),
        prefs: c
            .windows
            .iter()
            .filter_map(|w| {
                let p = c.behavior.pref_of(&w.app)?;
                Some((w.id, brain::Pref { go: p.like.go(), stay: p.like.stay(), favorite: p.favorite }))
            })
            .collect(),
        // 入力中のウィンドウ: 前面で、最後の入力から10秒以内(キーボードとマウスを分けて取る方法がWindowsに無いため。ふらる、11/3)
        busy: (c.behavior.quiet_on_focused && idle.is_some_and(|i| i < brain::BUSY_SECONDS))
            .then(|| c.windows.iter().find(|w| w.focused).map(|w| w.id))
            .flatten(),
    }
}

/// 画面の一覧の中で、その画面が何番目か
fn index_of(c: &Inner, screen: &Screen) -> usize {
    let a = area_of(screen);
    c.screens.iter().position(|s| *s == a).unwrap_or(0)
}

fn set_dock(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if visible { tauri::ActivationPolicy::Regular } else { tauri::ActivationPolicy::Accessory };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}

/// ペットの形にする
pub fn enter(app: &AppHandle) -> Result<(), String> {
    if is_active() {
        return Ok(());
    }
    ensure_windows(app).inspect_err(|e| {
        shiori_log::warn(error_codes::PET_001.code, "ペットのウィンドウを作れない", &[("理由", e)]);
    })?;
    let main = window(app, "main").ok_or("本体のウィンドウが無い")?;
    // 本体のウィンドウがある画面の、下の右端に現れる
    let center = window_rect(&main).map(|(x, y, w, h)| (x + w / 2.0, y + h / 2.0)).unwrap_or((0.0, 0.0));
    let screen = screen_at(app, center.0, center.1).ok_or("画面が見つからない")?;
    let scale = screen.dot_scale;
    {
        let mut c = lock();
        c.scale = scale;
        c.dot = screen.dot;
        c.fullscreen = false;
        c.behavior = load_behavior();
    }
    // 現れる床を決めるため、先に画面とウィンドウを読む
    refresh_surfaces(app);
    let world = {
        let c = lock();
        world_of(&c, &screen, index_of(&c, &screen), None, os::idle_seconds())
    };
    // 自己確認の起動では眠らせない。最後の入力から眠るまでの時間を過ぎてから走らせると、
    // ペットが出た直後に眠り、「現れてから普段」の確かめが走らせる環境で変わってしまうため(段階6)
    let sleep_after =
        if crate::self_test::is_enabled() { f64::INFINITY } else { crate::pet_sleep_minutes() as f64 * 60.0 };
    let brain = Brain::new(&world, crate::notify::now_ms());
    let pet = window(app, PET).ok_or("ペットのウィンドウが無い")?;
    set_pet_size(&pet, &screen).map_err(|e| e.to_string())?;
    set_window_position(&pet, brain.x, brain.y).map_err(|e| e.to_string())?;
    {
        let mut c = lock();
        c.brain = Some(brain);
        c.world = Some(world);
        c.sleep_after = sleep_after;
        c.last_pos = (i32::MIN, i32::MIN);
        c.last_frame = None;
        c.ignoring = false;
        c.active = true;
    }
    if debug_enabled() {
        open_debug(app);
    }
    let _ = main.hide();
    let _ = pet.show();
    set_dock(app, false);
    start_loop(app.clone());
    shiori_log::info(PART, "ペットの形にした", &[("倍率", &scale), ("眠るまで秒", &sleep_after)]);
    let _ = app.emit("pet://mode", "pet");
    Ok(())
}

/// ウィンドウの形に戻す。本体のウィンドウは、隠す前の大きさ・位置で出る
pub fn leave(app: &AppHandle) -> Result<(), String> {
    if !is_active() {
        return Ok(());
    }
    {
        let mut c = lock();
        c.active = false;
        c.brain = None;
        c.panel_visible = false;
        c.bubble_visible = false;
    }
    for label in [PANEL, BUBBLE, PET] {
        if let Some(w) = window(app, label) {
            let _ = w.hide();
        }
    }
    for (label, w) in app.webview_windows() {
        if label.starts_with(DEBUG_PREFIX) {
            let _ = w.hide();
        }
    }
    set_dock(app, true);
    if let Some(main) = window(app, "main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
    shiori_log::info(PART, "ウィンドウの形に戻した", &[]);
    let _ = app.emit("pet://mode", "window");
    Ok(())
}

// ---- 本と吹き出し ----

/// ペットの横に置く。右に余地があれば右、無ければ左。下はペットの足もとにそろえる。
/// 右に置いたらtrue(本は、ペットの側のページに項目を並べるため、どちらに開いたかを使う)
fn place_beside(app: &AppHandle, label: &str, size: (f64, f64), above: bool) -> bool {
    let Some(w) = window(app, label) else { return true };
    let c = lock();
    let Some(b) = c.brain.as_ref() else { return true };
    let (pw, ph) = (FRAME_W as f64 * c.dot, FRAME_H as f64 * c.dot);
    let Some(screen) = screen_at(app, b.x + pw / 2.0, b.y + ph / 2.0) else { return true };
    let (w_px, h_px) = (size.0 * screen.logical, size.1 * screen.logical);
    let (wx, wy, ww, wh) = screen.work;
    let (min_x, max_x) = (wx, wx + ww - w_px);
    let (min_y, max_y) = (wy, wy + wh - h_px);
    let right = b.x + pw <= max_x;
    let (x, y) = if above {
        // 吹き出しは頭の上
        (b.x + pw / 2.0 - w_px / 2.0, b.y - h_px + 28.0 * c.dot)
    } else if right {
        (b.x + pw, b.y + ph - h_px)
    } else {
        (b.x - w_px, b.y + ph - h_px)
    };
    drop(c);
    let x = x.clamp(min_x, max_x.max(min_x));
    let y = y.clamp(min_y, max_y.max(min_y));
    let _ = set_window_position(&w, x, y);
    right
}

#[derive(Serialize, Clone)]
struct PanelOpen {
    side: &'static str,
    /// 本と小道具の枠の大きさ(1.0が100%)。中身も同じ割合で縮める
    scale: f64,
}

fn show_panel(app: &AppHandle) {
    let scale = crate::pet_panel_percent() as f64 / 100.0;
    let size = (PANEL_SIZE.0 * scale, PANEL_SIZE.1 * scale);
    lock().panel_size = size;
    if let Some(w) = window(app, PANEL) {
        let _ = w.set_size(tauri::LogicalSize::new(size.0, size.1));
    }
    let right = place_beside(app, PANEL, size, false);
    let _ = app.emit_to(PANEL, "pet://panel-open", PanelOpen { side: if right { "right" } else { "left" }, scale });
    if let Some(w) = window(app, PANEL) {
        let _ = w.show();
        let _ = w.set_focus();
    }
    lock().panel_visible = true;
    shiori_log::info(PART, "本を開いた", &[]);
}

fn hide_panel(app: &AppHandle, why: &str) {
    {
        let mut c = lock();
        if !c.panel_visible {
            return;
        }
        c.panel_visible = false;
        c.panel_hidden_at = Some(Instant::now());
    }
    if let Some(w) = window(app, PANEL) {
        let _ = w.hide();
    }
    let _ = app.emit_to(PANEL, "pet://panel-closed", ());
    shiori_log::info(PART, "本を閉じた", &[("理由", &why)]);
}

/// ペットを押した: 本が開いていれば閉じ、閉じていれば開く
fn on_pressed(app: &AppHandle) {
    let (visible, just_hidden) = {
        let c = lock();
        (c.panel_visible, c.panel_hidden_at.is_some_and(|t| t.elapsed() < REOPEN_GUARD))
    };
    if visible {
        hide_panel(app, "ペットを押した");
    } else if !just_hidden {
        show_panel(app);
    }
}

/// 知らせの入口(`notify::show`)から。ペットの形なら吹き出しで知らせ、trueを返す
pub fn on_notice(notice: &Notice) -> bool {
    let Some(app) = crate::notify::app() else { return false };
    {
        let mut c = lock();
        if !c.active {
            return false;
        }
        if let Some(b) = c.brain.as_mut() {
            b.notice(notice.kind == Kind::Claude);
        }
        c.bubble_visible = true;
    }
    let _ = place_beside(app, BUBBLE, BUBBLE_SIZE, true);
    if let Some(w) = window(app, BUBBLE) {
        let _ = w.show();
    }
    true
}

// ---- 1秒60回の計算 ----

fn start_loop(app: AppHandle) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let mut last = Instant::now();
        loop {
            std::thread::sleep(TICK);
            let now = Instant::now();
            let dt = (now - last).as_secs_f64().min(0.05);
            last = now;
            if is_active() {
                tick(&app, dt);
            }
        }
    });
}

fn tick(app: &AppHandle, dt: f64) {
    let Some(pet) = window(app, PET) else { return };
    let cursor = cursor_position(app);
    let idle = os::idle_seconds();
    if idle.is_none() {
        static WARNED: OnceLock<()> = OnceLock::new();
        if WARNED.set(()).is_ok() {
            shiori_log::warn(error_codes::PET_004.code, "無操作の秒数を取れないので、眠る動きを止めた", &[]);
        }
    }
    let button_down = os::left_button_down();

    let c = lock();
    let Some(b) = c.brain.as_ref() else { return };
    let (pw, ph) = (FRAME_W as f64 * c.dot, FRAME_H as f64 * c.dot);
    let center = (b.x + pw / 2.0, b.y + ph / 2.0);
    let (riding, airborne) = (b.riding(), b.airborne());
    drop(c);
    let Some(screen) = screen_at(app, center.0, center.1) else { return };
    if lock().windows_checked.is_none_or(|t| t.elapsed() >= WINDOWS_EVERY) {
        refresh_surfaces(app);
    }
    // 乗っているウィンドウは毎回読む(運ばれるときに遅れないため)
    let ride = riding.map(|id| (id, windows::presence(id)));

    let mut c = lock();
    // 前面のアプリが全画面かは、0.5秒ごとに見る
    if c.fullscreen_checked.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
        let (p, s) = (screen.monitor.position(), screen.monitor.size());
        c.fullscreen = os::foreground_covers(p.x, p.y, s.width, s.height);
        c.fullscreen_checked = Some(Instant::now());
    }
    // 画面をまたいだら、その画面の倍率に直す。跳んでいる・飛んでいる間は元の大きさのまま、着いてから直す
    // (重力と速さをドットで決めているので、空中で大きさが変わると線が折れるため。設計書の3節)
    let (scale, dot) = if airborne { (c.scale, c.dot) } else { (screen.dot_scale, screen.dot) };
    let resized = scale != c.scale || dot != c.dot;
    c.scale = scale;
    c.dot = dot;
    let current = index_of(&c, &screen);
    let world = world_of(&c, &screen, current, ride, idle);
    let sleep_after = c.sleep_after;
    let input = Input { cursor, button_down, idle_seconds: idle, sleep_after };
    let Some(b) = c.brain.as_mut() else { return };
    let events = b.step(dt, &world, &input);
    let edge = b.edge;
    let pos = (b.x.round() as i32, b.y.round() as i32);
    let facing_left = b.facing < 0.0;
    let anim_name = b.anim();
    let state = b.state;
    let activity = b.activity;
    let state_t = b.state_t;
    let grabbing = b.grabbing();

    // クリックを受けるのは、絵の不透明な点の上だけ(つまんでいる間は受け続ける)
    let over_sprite = cursor.is_some_and(|(cx, cy)| {
        let (dx, dy) = (((cx - pos.0 as f64) / dot).floor() as i32, ((cy - pos.1 as f64) / dot).floor() as i32);
        let frame = frame_of(&c.sprites, anim_name, state_t);
        c.sprites.opaque_at(anim_name, frame, facing_left, dx, dy)
    });
    // ウィンドウの上にいるとき、乗っている縁より下の点はクリックを通す(足を垂らしても、上辺の
    // クリックやドラッグを妨げない。設定で変えられない決まり。設計書の5節)
    // ぶら下がっている体はウィンドウの横の外にあるので、通すのはそのウィンドウの幅の中だけ
    let below_edge = cursor.zip(edge).is_some_and(|((cx, cy), e)| cy >= e.y && cx >= e.x0 && cx < e.x1);
    let over_sprite = over_sprite && !below_edge;
    let ignore = !(over_sprite || grabbing);
    let ignore_changed = ignore != c.ignoring;
    c.ignoring = ignore;

    let (real_anim, a) = c.sprites.anim(anim_name);
    let dto = FrameDto {
        state,
        activity,
        anim: real_anim.to_string(),
        file: a.info.file.clone(),
        frames: a.info.frames,
        frame: frame_of(&c.sprites, anim_name, state_t),
        facing: if facing_left { -1.0 } else { 1.0 },
        scale,
    };
    let frame_changed = c.last_frame.as_ref() != Some(&dto);
    if frame_changed {
        c.last_frame = Some(dto.clone());
    }
    let moved = pos != c.last_pos;
    c.last_pos = pos;
    let (panel_visible, bubble_visible, panel_size) = (c.panel_visible, c.bubble_visible, c.panel_size);
    c.world = Some(world);
    drop(c);

    // Windowsは、倍率の違う画面へ窓が移ると、ドットの倍率が同じでもOSが窓の大きさを変える。
    // 変えられたままだと絵が縮み、足元が足場から浮くので、本来の大きさに戻す(11/4)
    let target = (FRAME_W * scale, FRAME_H * scale);
    let drifted = cfg!(windows) && pet.outer_size().is_ok_and(|z| (z.width, z.height) != target);
    if drifted {
        let _ = pet.set_size(PhysicalSize::new(target.0, target.1));
    } else if resized {
        let _ = set_pet_size(&pet, &screen);
    }
    if moved {
        let _ = set_window_position(&pet, pos.0 as f64, pos.1 as f64);
        if panel_visible {
            let _ = place_beside(app, PANEL, panel_size, false);
        }
        if bubble_visible {
            let _ = place_beside(app, BUBBLE, BUBBLE_SIZE, true);
        }
    }
    if ignore_changed {
        let _ = pet.set_ignore_cursor_events(ignore);
    }
    if frame_changed {
        let _ = app.emit_to(PET, "pet://frame", &dto);
    }
    for e in events {
        match e {
            Event::Pressed => on_pressed(app),
            Event::StateChanged(s) => shiori_log::debug(PART, "状態が変わった", &[("状態", &format!("{s:?}"))]),
        }
    }
}

// ---- ほかのアプリのウィンドウと足場(ペットの行動の設計書の1・2節) ----

/// 全部の画面とウィンドウの一覧を読み直し、足場を計算し直す。行動にはまだ使わない(Phase 2から)
fn refresh_surfaces(app: &AppHandle) {
    let t = Instant::now();
    let listed = windows::list();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    if listed.is_none() {
        static WARNED: OnceLock<()> = OnceLock::new();
        if WARNED.set(()).is_ok() {
            shiori_log::warn(error_codes::PET_005.code, "ほかのアプリのウィンドウを読めない", &[]);
        }
    }
    let screens = all_screens(app);
    let areas: Vec<ScreenArea> = screens.iter().map(area_of).collect();
    let mut c = lock();
    let screens_changed = areas != c.screens;
    if screens_changed {
        let names: Vec<String> = screens.iter().map(|s| s.monitor.name().cloned().unwrap_or_default()).collect();
        shiori_log::info(PART, "画面の並びが変わった", &[("画面", &format!("{names:?}")), ("範囲", &format!("{areas:?}"))]);
    }
    let pet = (FRAME_W as f64 * c.dot, FRAME_H as f64 * c.dot);
    let windows = listed.unwrap_or_default();
    // 自己確認の起動と、ウィンドウに乗る行動をオフにしたときは、ウィンドウに乗らない
    let ride_windows = !crate::self_test::is_enabled() && c.behavior.enabled;
    let mut platforms = surfaces::platforms(&areas, &windows, pet, ride_windows);
    // 「避ける」アプリのウィンドウと、全画面のアプリが前面にある画面のウィンドウには乗らない(設計書の5節)
    let avoid: Vec<u64> =
        windows.iter().filter(|w| c.behavior.pref_of(&w.app).is_some_and(|p| p.like == Like::Avoid)).map(|w| w.id).collect();
    let full = if c.behavior.no_ride_fullscreen { surfaces::fullscreen_screen(&areas, &windows) } else { None };
    platforms.retain(|p| match p.owner {
        surfaces::Owner::Floor { .. } => true,
        surfaces::Owner::Window { id } => {
            !avoid.contains(&id) && (full.is_none() || surfaces::screen_index_at(&areas, (p.x0 + p.x1) / 2.0, p.y) != full)
        }
    });
    let new_apps: Vec<String> = windows.iter().map(|w| w.app.clone()).filter(|a| !a.is_empty() && !c.seen.contains(a)).collect();
    c.seen.extend(new_apps.iter().cloned());
    c.screens = areas;
    c.windows = windows;
    c.platforms = platforms;
    c.windows_checked = Some(Instant::now());
    c.windows_ms = ms;
    drop(c);
    if !new_apps.is_empty() {
        note_seen(new_apps);
    }
    // 確かめの表示は、読み込み終わる前に送った分を受け取れないので、変わらなくても毎回送る
    if debug_enabled() {
        if screens_changed {
            open_debug(app);
        }
        draw_debug(app, &screens);
    }
}

fn debug_enabled() -> bool {
    std::env::var("SHIORI_PET_DEBUG").is_ok_and(|v| v == "1")
}

/// 足場の確かめの表示を、画面ごとに開く(クリックは全部通す)。画面の並びが変わったときも呼ぶ
fn open_debug(app: &AppHandle) {
    for (i, s) in all_screens(app).iter().enumerate() {
        let (_, _, w, h) = s.full;
        let Ok(win) = build(app, &format!("{DEBUG_PREFIX}{i}"), "pet-debug", (w / s.logical, h / s.logical)) else {
            continue;
        };
        // 前の並びで開いた窓を使い回すときのため、大きさと位置を毎回合わせる
        let _ = win.set_size(LogicalSize::new(w / s.logical, h / s.logical));
        let _ = win.set_ignore_cursor_events(true);
        let _ = set_window_position(&win, s.full.0, s.full.1);
        let _ = win.show();
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DebugLine {
    /// 画面の左上からの、画面の画素(CSSの画素)
    x: f64,
    y: f64,
    w: f64,
    label: String,
    floor: bool,
}

/// 足場を、その足場がある画面の表示へ送る(画面にまたがる足場は、それぞれの画面で切れて見える)
fn draw_debug(app: &AppHandle, screens: &[Screen]) {
    let c = lock();
    let name = |p: &Platform| match p.owner {
        surfaces::Owner::Floor { screen } => format!("床{screen}"),
        surfaces::Owner::Window { id } => c.windows.iter().find(|w| w.id == id).map_or(String::new(), |w| w.app.clone()),
    };
    let per_screen: Vec<Vec<DebugLine>> = screens
        .iter()
        .map(|s| {
            let (sx, sy, _, _) = s.full;
            c.platforms
                .iter()
                .map(|p| DebugLine {
                    x: (p.x0 - sx) / s.logical,
                    y: (p.y - sy) / s.logical,
                    w: (p.x1 - p.x0) / s.logical,
                    label: name(p),
                    floor: matches!(p.owner, surfaces::Owner::Floor { .. }),
                })
                .collect()
        })
        .collect();
    drop(c);
    for (i, lines) in per_screen.into_iter().enumerate() {
        let _ = app.emit_to(format!("{DEBUG_PREFIX}{i}").as_str(), "pet://debug", lines);
    }
}

fn frame_of(sprites: &Sprites, anim: &str, state_t: f64) -> u32 {
    let (_, a) = sprites.anim(anim);
    let n = (state_t * FPS).floor() as u32;
    if a.info.r#loop {
        n % a.info.frames
    } else {
        n.min(a.info.frames - 1)
    }
}

// ---- 窓口 ----

// ---- 行動の設定(ペットの行動の設計書の6節) ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorDto {
    behavior: Behavior,
    /// ひな形(設定画面の「静か・普通・遊ぶ」)
    presets: std::collections::BTreeMap<&'static str, Behavior>,
}

/// 行動の設定と、ひな形を返す
#[tauri::command]
pub async fn pet_behavior_get() -> Result<BehaviorDto, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let presets =
            [("quiet", Preset::Quiet), ("normal", Preset::Normal), ("play", Preset::Play)].map(|(k, p)| (k, p.behavior()));
        BehaviorDto { behavior: load_behavior(), presets: presets.into_iter().collect() }
    })
    .await
    .map_err(|e| e.to_string())
}

/// 行動の設定を保存する。ペットの形のままでもすぐ効く(範囲の外の値は収めて、収めた値を返す)
#[tauri::command]
pub async fn pet_behavior_set(behavior: Behavior) -> Result<Behavior, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let b = behavior.normalized();
        let path = behavior_path();
        let text = serde_json::to_string_pretty(&b).map_err(|e| e.to_string())?;
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&path, text))
            .map_err(|e| format!("ペットの行動の設定を書けない: {e}"))?;
        let mut c = lock();
        c.behavior = b.clone();
        // 乗れるウィンドウ(避けるアプリ・オンオフ)を、次の計算で読み直す
        c.windows_checked = None;
        drop(c);
        shiori_log::info(PART, "ペットの行動の設定を保存した", &[("活発さ", &b.activity), ("アプリ", &b.apps.len())]);
        Ok(b)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 見かけたアプリの一覧(両方のOSの分)
#[tauri::command]
pub async fn pet_seen_apps() -> Result<Vec<SeenApp>, String> {
    tauri::async_runtime::spawn_blocking(|| behavior::load_seen(&seen_path())).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pet_enter(app: AppHandle) -> Result<(), String> {
    enter(&app)
}

#[tauri::command]
pub async fn pet_leave(app: AppHandle) -> Result<(), String> {
    leave(&app)
}

/// ペットの絵を押した(画面のmousedown)
#[tauri::command]
pub fn pet_press(app: AppHandle) {
    let cursor = cursor_position(&app);
    if let (Some(cur), Some(b)) = (cursor, lock().brain.as_mut()) {
        b.press(cur);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpritesDto {
    animations: std::collections::BTreeMap<String, sprites::AnimInfo>,
    overridden: bool,
}

#[tauri::command]
pub fn pet_sprites() -> SpritesDto {
    let c = lock();
    SpritesDto { animations: c.sprites.infos(), overridden: c.sprites.overridden }
}

/// 吹き出しの知らせが全部消えた
#[tauri::command]
pub fn pet_bubble_done(app: AppHandle) {
    lock().bubble_visible = false;
    if let Some(w) = window(&app, BUBBLE) {
        let _ = w.hide();
    }
}

/// 吹き出しを押した: タイマーと拡張機能は本の小道具の枠。Claudeは、ペットの形のまま、作業が終わった
/// セッションのアプリを前に出す(出せなければ下へ)。要確認などはウィンドウに戻ってその画面へ
#[tauri::command]
pub async fn pet_open_source(app: AppHandle, notice: Notice) -> Result<(), String> {
    let focused = match (notice.kind, notice.source.clone()) {
        (Kind::Claude, Some(session_id)) => {
            tauri::async_runtime::spawn_blocking(move || crate::claude_status::focus_session(&session_id))
                .await
                .unwrap_or(false)
        }
        _ => false,
    };
    match (notice.kind, notice.source.as_deref()) {
        (Kind::Timer | Kind::Extension, Some(name)) => {
            show_panel(&app);
            let _ = app.emit_to(PANEL, "pet://open-tool", name);
        }
        (Kind::Claude, _) if focused => {}
        _ => {
            leave(&app)?;
            let _ = app.emit_to("main", "shiori://open-source", &notice);
        }
    }
    Ok(())
}

/// 本の画面が、並べた項目と、小道具の自己確認の結果を知らせる
#[tauri::command]
pub fn pet_panel_report(items: Option<Vec<String>>, tool: Option<Value>) {
    let mut c = lock();
    if let Some(items) = items {
        c.panel_items = items;
    }
    if tool.is_some() {
        c.panel_tool = tool;
    }
}

/// 本を閉じる(本の画面の閉じるボタンから)
#[tauri::command]
pub fn pet_close_panel(app: AppHandle) {
    hide_panel(&app, "閉じるを押した");
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetStatus {
    mode: &'static str,
    state: Option<brain::State>,
    anim: Option<&'static str>,
    scale: u32,
    position: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
    floor: Option<f64>,
    on_floor: bool,
    /// いまの居場所(床かウィンドウか)
    stand: Option<brain::Stand>,
    main_visible: bool,
    pet_visible: bool,
    panel_visible: bool,
    bubble_visible: bool,
    panel_items: Vec<String>,
    panel_tool: Option<Value>,
    idle_seconds: Option<f64>,
    sprites_overridden: bool,
    /// 自己確認: 絵の外の点(0,0)と、絵の真ん中の不透明な点で、クリックを受けるか
    hit_outside: bool,
    hit_center: bool,
    /// 全部の画面・ほかのアプリのウィンドウ(前面から順)・足場と、一覧を読むのにかかったミリ秒
    screens: Vec<ScreenArea>,
    windows: Vec<WinInfo>,
    platforms: Vec<Platform>,
    windows_ms: f64,
}

#[tauri::command]
pub fn pet_status(app: AppHandle) -> PetStatus {
    let visible = |label: &str| window(&app, label).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    let size = window(&app, PET).and_then(|w| w.outer_size().ok()).map(|s| (s.width, s.height));
    let position =
        window(&app, PET).and_then(|w| window_rect(&w)).map(|(x, y, _, _)| (x.round() as i32, y.round() as i32));
    let c = lock();
    // いる画面の床に立ったときの、絵の左上のy
    let floor = c.world.as_ref().and_then(|w| {
        w.platforms.iter().find(|p| p.owner == surfaces::Owner::Floor { screen: w.current }).map(|p| p.y - w.size.1)
    });
    let on_floor = matches!((position, floor), (Some((_, y)), Some(f)) if (y as f64 - f).abs() <= 1.5);
    let center = c.sprites.center_opaque();
    PetStatus {
        mode: if c.active { "pet" } else { "window" },
        state: c.brain.as_ref().map(|b| b.state),
        anim: c.brain.as_ref().map(|b| b.anim()),
        scale: c.scale,
        position,
        size,
        floor,
        on_floor,
        stand: c.brain.as_ref().and_then(|b| b.stand),
        main_visible: visible("main"),
        pet_visible: visible(PET),
        panel_visible: visible(PANEL),
        bubble_visible: visible(BUBBLE),
        panel_items: c.panel_items.clone(),
        panel_tool: c.panel_tool.clone(),
        idle_seconds: os::idle_seconds(),
        sprites_overridden: c.sprites.overridden,
        hit_outside: c.sprites.opaque_at("idle", 0, false, 0, 0),
        hit_center: center.is_some_and(|(x, y)| c.sprites.opaque_at("idle", 0, false, x, y)),
        screens: c.screens.clone(),
        windows: c.windows.clone(),
        platforms: c.platforms.clone(),
        windows_ms: c.windows_ms,
    }
}

/// 自己確認: 本を開き、その中で小道具を開いて、小道具の確かめを走らせる
#[tauri::command]
pub fn pet_selftest_panel(app: AppHandle, tool: Option<String>) {
    if !crate::self_test::is_enabled() {
        return;
    }
    if !lock().panel_visible {
        show_panel(&app);
    }
    if let Some(tool) = tool {
        lock().panel_tool = None;
        let _ = app.emit_to(PANEL, "pet://selftest-tool", tool);
    }
}

/// 自己確認: 本を閉じる
#[tauri::command]
pub fn pet_selftest_close_panel(app: AppHandle) {
    if crate::self_test::is_enabled() {
        hide_panel(&app, "自己確認");
    }
}

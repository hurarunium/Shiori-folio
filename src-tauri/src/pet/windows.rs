//! ほかのアプリのウィンドウを読む(ペットの行動の設計書 `design-shiori-pet-behavior-20261103` の1節)。
//!
//! 前面から奥への順に、位置・大きさ・アプリ名を返す。タイトルや中身は読まない(企画の対象外)。
//! Macでは、タイトルを読まなければ画面収録の許可は要らない。座標はペットと同じ単位
//! (Windowsは物理の画素、Macは主画面の左上が原点の論理の点)。OSの違いはこのファイルの中に閉じ込める。

pub use super::surfaces::Presence;
use super::surfaces::{Rect, WinInfo};

// ---- Mac ----

#[cfg(target_os = "macos")]
mod imp {
    use super::{Presence, Rect, WinInfo};
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow, kCGWindowListOptionOnScreenOnly,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    };

    type Dict = CFDictionary<CFString, CFType>;

    // CoreGraphicsが持つ定数の文字列を、辞書を引く鍵にする
    macro_rules! key {
        ($k:ident) => {
            // SAFETY: CoreGraphicsが持つ、変わらない定数の文字列。持ち主は変えない
            unsafe { CFString::wrap_under_get_rule($k) }
        };
    }

    fn number(d: &Dict, k: CFString) -> Option<f64> {
        d.find(k)?.downcast::<CFNumber>()?.to_f64()
    }

    fn string(d: &Dict, k: CFString) -> Option<String> {
        Some(d.find(k)?.downcast::<CFString>()?.to_string())
    }

    fn bounds(d: &Dict) -> Option<Rect> {
        let b = d.find(key!(kCGWindowBounds))?;
        // SAFETY: kCGWindowBoundsの値は、X・Y・Width・Heightを持つ辞書
        let b: Dict = unsafe { CFDictionary::wrap_under_get_rule(b.as_CFTypeRef() as CFDictionaryRef) };
        let get = |name: &'static str| b.find(CFString::from_static_string(name))?.downcast::<CFNumber>()?.to_f64();
        Some(Rect::new(get("X")?, get("Y")?, get("Width")?, get("Height")?))
    }

    /// ウィンドウの情報の辞書の一覧
    fn infos(option: u32, relative_to: u32) -> Option<Vec<Dict>> {
        let array: CFArray = copy_window_info(option, relative_to)?;
        Some(
            array
                .iter()
                // SAFETY: 一覧の中身はウィンドウごとの辞書
                .map(|p| unsafe { CFDictionary::wrap_under_get_rule(*p as CFDictionaryRef) })
                .collect(),
        )
    }

    pub fn list() -> Option<Vec<WinInfo>> {
        let own = std::process::id() as f64;
        let mut out = Vec::new();
        for d in infos(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)? {
            // layerが0でないもの(メニューバー・Dock・常に前面の窓)と、透明なもの・詩織自身は外す
            if number(&d, key!(kCGWindowLayer)) != Some(0.0)
                || number(&d, key!(kCGWindowAlpha)).is_some_and(|a| a <= 0.0)
                || number(&d, key!(kCGWindowOwnerPID)) == Some(own)
            {
                continue;
            }
            let (Some(id), Some(rect)) = (number(&d, key!(kCGWindowNumber)), bounds(&d)) else { continue };
            if rect.w <= 1.0 || rect.h <= 1.0 {
                continue;
            }
            let app = string(&d, key!(kCGWindowOwnerName)).unwrap_or_default();
            // 一覧は前面から順なので、最初のものが前面のウィンドウ
            let focused = out.is_empty();
            out.push(WinInfo { id: id as u64, app, rect, focused });
        }
        Some(out)
    }

    pub fn presence(id: u64) -> Presence {
        let Some(found) = infos(kCGWindowListOptionIncludingWindow, id as u32) else { return Presence::Gone };
        let Some(d) = found.into_iter().find(|d| number(d, key!(kCGWindowNumber)) == Some(id as f64)) else {
            return Presence::Gone;
        };
        let onscreen =
            d.find(key!(kCGWindowIsOnscreen)).and_then(|v| v.downcast::<CFBoolean>()).is_some_and(|b| bool::from(b));
        match (onscreen, bounds(&d)) {
            (true, Some(r)) => Presence::Visible(r),
            _ => Presence::Hidden,
        }
    }
}

// ---- Windows ----

#[cfg(windows)]
mod imp {
    use super::{Presence, Rect, WinInfo};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic,
        IsWindow, IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };

    fn class_of(hwnd: HWND) -> String {
        let mut buf = [0u16; 64];
        // SAFETY: 長さを渡した配列に書かせる
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn cloaked(hwnd: HWND) -> bool {
        let mut v = 0u32;
        // SAFETY: 4バイトの書き込み先を渡す
        let hr = unsafe {
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as _, &mut v as *mut u32 as *mut _, std::mem::size_of::<u32>() as u32)
        };
        hr == 0 && v != 0
    }

    /// 見えている枠(`GetWindowRect` は見えない縁を含むため)
    fn frame(hwnd: HWND) -> Option<Rect> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: RECTの書き込み先を渡す
        let hr = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as _,
                &mut r as *mut RECT as *mut _,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        (hr == 0).then(|| Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64))
    }

    fn pid_of(hwnd: HWND) -> u32 {
        let mut pid = 0u32;
        // SAFETY: 書き込み先を渡す
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        pid
    }

    /// アプリ名(実行ファイル名から拡張子を除いたもの)。プロセスごとに覚えておく
    fn app_of(pid: u32) -> String {
        static NAMES: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
        let names = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(n) = names.lock().unwrap_or_else(|e| e.into_inner()).get(&pid) {
            return n.clone();
        }
        let mut name = String::new();
        // SAFETY: 情報を聞くだけの権限で開き、必ず閉じる
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if !h.is_null() {
                let mut buf = [0u16; 1024];
                let mut len = buf.len() as u32;
                if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0 {
                    let path = String::from_utf16_lossy(&buf[..len as usize]);
                    name = std::path::Path::new(&path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }
                CloseHandle(h);
            }
        }
        names.lock().unwrap_or_else(|e| e.into_inner()).insert(pid, name.clone());
        name
    }

    struct Search {
        own: u32,
        foreground: HWND,
        out: Vec<WinInfo>,
    }

    // EnumWindowsは前面から奥への順に呼ぶ
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let s = &mut *(lparam as *mut Search);
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || cloaked(hwnd) {
            return 1;
        }
        if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        let class = class_of(hwnd);
        if class == "Progman" || class == "WorkerW" || class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd" {
            return 1;
        }
        let pid = pid_of(hwnd);
        if pid == s.own {
            return 1;
        }
        let Some(rect) = frame(hwnd) else { return 1 };
        if rect.w <= 1.0 || rect.h <= 1.0 {
            return 1;
        }
        s.out.push(WinInfo { id: hwnd as usize as u64, app: app_of(pid), rect, focused: hwnd == s.foreground });
        1
    }

    pub fn list() -> Option<Vec<WinInfo>> {
        // SAFETY: 前面のウィンドウを聞くだけ
        let foreground = unsafe { GetForegroundWindow() };
        let mut s = Search { own: std::process::id(), foreground, out: Vec::new() };
        // SAFETY: 呼び出しの間だけ生きている構造体を渡す
        let ok = unsafe { EnumWindows(Some(visit), &mut s as *mut Search as LPARAM) };
        (ok != 0).then_some(s.out)
    }

    pub fn presence(id: u64) -> Presence {
        let hwnd = id as usize as HWND;
        // SAFETY: 閉じたウィンドウの値でも、IsWindowは偽を返すだけ
        unsafe {
            if IsWindow(hwnd) == 0 {
                Presence::Gone
            } else if IsIconic(hwnd) != 0 {
                Presence::Minimized
            } else if cloaked(hwnd) || IsWindowVisible(hwnd) == 0 {
                Presence::Hidden
            } else {
                frame(hwnd).map_or(Presence::Gone, Presence::Visible)
            }
        }
    }
}

// ---- それ以外 ----

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::{Presence, WinInfo};

    pub fn list() -> Option<Vec<WinInfo>> {
        None
    }

    pub fn presence(_id: u64) -> Presence {
        Presence::Gone
    }
}

/// 開いているウィンドウの一覧(前面から奥への順)。読めなければNone
pub fn list() -> Option<Vec<WinInfo>> {
    imp::list()
}

/// 前に一覧にあったウィンドウの、いまのようす(乗っているウィンドウを毎回読むのに使う)
pub fn presence(id: u64) -> Presence {
    imp::presence(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実機のウィンドウを読んで表示する(`cargo test --lib pet::windows -- --ignored --nocapture`)
    #[test]
    #[ignore]
    fn 実機のウィンドウを読む() {
        let t = std::time::Instant::now();
        let ws = list().expect("ウィンドウの一覧を読めない");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        for w in &ws {
            println!("{:>8} {:<24} {:?} focused={} {:?}", w.id, w.app, w.rect, w.focused, presence(w.id));
        }
        println!("{}件、{ms:.2}ミリ秒", ws.len());
        // 2回目からの時間(0.25秒ごとに読むので、こちらが効く)
        let t = std::time::Instant::now();
        for _ in 0..20 {
            list();
        }
        println!("2回目から: 1回{:.2}ミリ秒", t.elapsed().as_secs_f64() * 1000.0 / 20.0);
        assert!(!ws.is_empty());
    }
}

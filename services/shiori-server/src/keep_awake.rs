//! サーバーが動いている間、PCを眠らせない(Windowsのモダン スタンバイ対策)。
//!
//! 余りのPCは、画面が消えると低電力の状態に入り、Tailscaleが落ちてつながらなくなる。
//! 動いている間だけ、OSに「システムを使用中」「実行を続ける必要がある」という要求を出し続ける。
//! 要求はプロセスに付くので、サーバーを止めれば(プロセスが終われば)消えて、元の動きに戻る。
//! 設定は何も変えない。Windows以外では何もしない。
//! (試験用の `scripts/proto/spare/keep-awake.ps1` と同じ呼び出し)

/// 要求を出す。戻り値は、結果の説明(ログに出す)。失敗してもサーバーは続ける。
pub fn start(reason: &str) -> String {
    imp::start(reason)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    // POWER_REQUEST_CONTEXT_SIMPLE_STRING
    const SIMPLE_STRING: u32 = 0x1;
    const POWER_REQUEST_SYSTEM_REQUIRED: i32 = 1;
    const POWER_REQUEST_EXECUTION_REQUIRED: i32 = 3;

    #[repr(C)]
    struct ReasonContext {
        version: u32,
        flags: u32,
        simple_reason_string: *const u16,
        // 共用体の残り(詳細形式の分。使わない)
        pad1: *const c_void,
        pad2: *const c_void,
    }

    extern "system" {
        fn PowerCreateRequest(context: *const ReasonContext) -> *mut c_void;
        fn PowerSetRequest(request: *mut c_void, request_type: i32) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn start(reason: &str) -> String {
        // 理由の文字列は、プロセスが終わるまで生かす(要求が参照し続けるため、わざと解放しない)
        let wide: &'static [u16] = Box::leak(
            reason.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>().into_boxed_slice(),
        );
        let ctx = ReasonContext {
            version: 0,
            flags: SIMPLE_STRING,
            simple_reason_string: wide.as_ptr(),
            pad1: std::ptr::null(),
            pad2: std::ptr::null(),
        };
        // SAFETY: ctxは呼び出しの間だけ有効で、文字列は解放しない。ハンドルは閉じず、プロセスの終了で消える。
        unsafe {
            let handle = PowerCreateRequest(&ctx);
            if handle.is_null() || handle as isize == -1 {
                return format!("眠らせない要求: 作れなかった(エラー {})", GetLastError());
            }
            let sys = PowerSetRequest(handle, POWER_REQUEST_SYSTEM_REQUIRED) != 0;
            let exe = PowerSetRequest(handle, POWER_REQUEST_EXECUTION_REQUIRED) != 0;
            format!(
                "眠らせない要求: システム={} 実行={}",
                if sys { "成功" } else { "失敗" },
                if exe { "成功" } else { "失敗" }
            )
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start(_reason: &str) -> String {
        "眠らせない要求: Windows以外なので、しない".to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn 要求を出しても落ちない() {
        let s = super::start("shiori-server test");
        assert!(s.starts_with("眠らせない要求"), "{s}");
        #[cfg(windows)]
        assert!(s.contains("システム=成功") && s.contains("実行=成功"), "{s}");
    }
}

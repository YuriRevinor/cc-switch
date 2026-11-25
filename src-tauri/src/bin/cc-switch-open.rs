//! `cc-switch-open.exe`：给快捷键用的小启动器，效果同 `cc-switch.exe --open-claude-desktop`，但快得多。
//!
//! `cc-switch.exe` 有五十多 MB，带参数再启动一次时要先把整个程序（Tauri、WebView2 加载器）初始化到
//! 单实例插件那一步，才把参数转给开着的那个。这里直接照单实例插件
//! （tauri-plugin-single-instance 2.x 的 Windows 实现）的做法：找到它的消息窗口
//! `<identifier>-sic` / `<identifier>-siw`，用 `WM_COPYDATA` 发 `"<cwd>|<程序>|--open-claude-desktop"`，
//! 发完就退出。CC Switch 没开着时，启动同目录下的 `cc-switch.exe --open-claude-desktop`。
//!
//! 放在 `cc-switch.exe` 旁边用。只有 Windows 有这个需要；别的平台编出来什么也不做。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(message) = win::run() {
        win::error_box(&message);
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowW, MessageBoxW, SendMessageTimeoutW, ASFW_ANY,
        MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, SMTO_ABORTIFHUNG, WM_COPYDATA,
    };

    /// 和 `tauri.conf.json` 的 `identifier` 一致
    const IDENTIFIER: &str = "com.ccswitch.desktop";
    /// 和 `services::claude_desktop_account::OPEN_FLAG` 一致
    const OPEN_FLAG: &str = "--open-claude-desktop";
    /// 单实例插件认的 `dwData`
    const SINGLE_INSTANCE_DATA: usize = 1542;
    const MAIN_EXE: &str = "cc-switch.exe";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    fn main_exe() -> Result<PathBuf, String> {
        let me = std::env::current_exe().map_err(|e| format!("找不到自己的位置: {e}"))?;
        Ok(me.with_file_name(MAIN_EXE))
    }

    pub(super) fn run() -> Result<(), String> {
        let exe = main_exe()?;
        let class = wide(&format!("{IDENTIFIER}-sic"));
        let title = wide(&format!("{IDENTIFIER}-siw"));
        // SAFETY: 两个字符串都以 0 结尾、在调用期间有效
        let hwnd = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
        if hwnd.is_null() {
            // CC Switch 没开着：冷启动它，它会带着这个参数在后台打开 Desktop
            if !exe.is_file() {
                return Err(format!(
                    "没找到 {}（cc-switch-open.exe 要和它放在同一个文件夹）",
                    exe.display()
                ));
            }
            return std::process::Command::new(&exe)
                .arg(OPEN_FLAG)
                .spawn()
                .map(drop)
                .map_err(|e| format!("启动 {} 失败: {e}", exe.display()));
        }

        // 这个进程是快捷键刚启动的，允许 CC Switch 把 Desktop 的窗口拿到前台
        // SAFETY: 没有指针参数
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };

        let cwd = std::env::current_dir().unwrap_or_default();
        let data = format!(
            "{}|{}|{OPEN_FLAG}\0",
            cwd.to_string_lossy(),
            exe.to_string_lossy()
        );
        let bytes = data.as_bytes();
        let copy = COPYDATASTRUCT {
            dwData: SINGLE_INSTANCE_DATA,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as _,
        };
        let mut result = 0usize;
        // SAFETY: `copy` 和它指向的 `bytes` 在调用期间有效；WM_COPYDATA 由系统复制到对方进程
        let sent = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_COPYDATA,
                0,
                &copy as *const COPYDATASTRUCT as isize,
                SMTO_ABORTIFHUNG,
                10_000,
                &mut result,
            )
        };
        if sent == 0 {
            return Err("CC Switch 没有响应，请稍后再试".into());
        }
        Ok(())
    }

    pub(super) fn error_box(message: &str) {
        let text = wide(message);
        let title = wide("CC Switch · Claude Desktop");
        // SAFETY: 两个字符串都以 0 结尾、在调用期间有效；没有父窗口
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST,
            );
        }
    }
}

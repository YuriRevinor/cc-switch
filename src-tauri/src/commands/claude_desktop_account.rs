//! Claude Desktop 用授权中心的 Claude 账号（见 `services::claude_desktop_account`）。

use crate::services::claude_desktop_account::{self, DesktopProfilesStatus};

/// 各账号的 Desktop 数据目录登的是谁、在不在运行。
#[tauri::command]
pub async fn get_claude_desktop_profiles() -> Result<DesktopProfilesStatus, String> {
    tokio::task::spawn_blocking(claude_desktop_account::status)
        .await
        .map_err(|error| error.to_string())
}

/// 打开卡片对应的 Desktop，返回是不是第一次用这个账号目录。`account_key` 是账号卡绑的 Claude 账号，
/// 不传是默认数据目录；`quit_running`：用户确认过要先退出正在运行的 Desktop。
#[tauri::command(rename_all = "camelCase")]
pub async fn open_claude_desktop(
    account_key: Option<String>,
    quit_running: bool,
) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        claude_desktop_account::open(account_key.as_deref(), quit_running)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

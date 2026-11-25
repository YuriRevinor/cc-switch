//! Claude Desktop 用授权中心的 Claude 账号（见 `services::claude_desktop_account`）。

use crate::services::claude_desktop_account::{self, DesktopProfilesStatus};

/// 各账号的 Desktop 数据目录登的是谁、在不在运行。
#[tauri::command]
pub async fn get_claude_desktop_profiles() -> Result<DesktopProfilesStatus, String> {
    tokio::task::spawn_blocking(claude_desktop_account::status)
        .await
        .map_err(|error| error.to_string())
}

/// 用户点「打开 Claude Desktop」：打开账号卡对应的 Desktop，返回是不是第一次用这个账号目录。
/// `account_key` 是账号卡绑的 Claude 账号。别的 Desktop 开着时报错，不替用户关闭它。
#[tauri::command(rename_all = "camelCase")]
pub async fn open_claude_desktop(account_key: String) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || claude_desktop_account::open(&account_key))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// 会话同步开关和回收站里的会话。
#[tauri::command]
pub async fn get_claude_desktop_session_sync(
) -> Result<crate::services::claude_desktop_sessions::SessionSyncStatus, String> {
    tokio::task::spawn_blocking(crate::services::claude_desktop_sessions::sync_status)
        .await
        .map_err(|error| error.to_string())
}

/// 打开/关闭多账号同步删除和归档。
#[tauri::command]
pub async fn set_claude_desktop_session_sync(enabled: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::services::claude_desktop_sessions::set_sync_enabled(enabled)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

/// 从回收站恢复一个会话。
#[tauri::command(rename_all = "camelCase")]
pub async fn restore_claude_desktop_session(
    session_id: String,
) -> Result<crate::services::claude_desktop_trash::RestoreReport, String> {
    tokio::task::spawn_blocking(move || claude_desktop_account::restore_session(&session_id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// 清空会话回收站（永久删除）。
#[tauri::command]
pub async fn empty_claude_desktop_session_trash() -> Result<(), String> {
    tokio::task::spawn_blocking(crate::services::claude_desktop_sessions::empty_trash)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

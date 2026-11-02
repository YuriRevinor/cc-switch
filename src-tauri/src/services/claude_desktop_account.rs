//! Claude Desktop 用授权中心里的 Claude 账号：每个账号一个独立的 Desktop 数据目录（macOS）。
//!
//! Desktop 是 Electron 应用，启动时认 `--user-data-dir`：换一个数据目录，登录、设置和会话列表就都是
//! 另一份。`--user-data-dir` 不是 Anthropic 公开支持的用法，以后的版本可能不再认它。
//!
//! - 账号卡：绑了授权中心 Claude 账号（`claude_official`）的 Desktop 官方卡（随机 id）。原来那张固定
//!   id 的官方卡（`claude-desktop-official`）不能绑号，代表默认数据目录里 Desktop 自己的登录；
//! - 切换：账号卡和别的卡一样「切换」。切换照常写默认数据目录的配置（账号卡是官方卡，写回 1P），界面
//!   随后用 [`open`] 把 Desktop 换成这张卡的：账号卡用 `<根>/<accountUuid>/`，别的卡用默认数据目录。
//!   同一时间只开一个 Desktop：别的 Desktop 开着时，界面先确认，用户同意后才由这里结束它们。第一次
//!   打开时在 Desktop 自己的登录页登录这个号，登录由 Desktop 加密保存在目录里，CC Switch 不读；
//! - 只看 `config.json` 的 `lastKnownAccountUuid` 判断目录里登的是谁。
//!
//! 目录放在 `~/Library/Application Support/com.ccswitch.desktop/claude-desktop-accounts/`（不放进可能
//! 被网盘同步的 `~/.cc-switch`）。从授权中心删掉账号不删目录，再加回来还能接着用。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::database::CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID;
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::provider::claude_official;

/// 一个账号的 Desktop 数据目录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProfile {
    /// 目录是给哪个账号建的（目录名）
    pub account_uuid: String,
    /// 目录里实际登的账号；还没登录时为 `None`
    pub signed_in_as: Option<String>,
    pub running: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProfilesStatus {
    /// macOS 上装了 Claude Desktop
    pub supported: bool,
    /// 默认数据目录登的号在授权中心里的 key（原来那张官方卡按它显示账号和额度）
    pub default_account_key: Option<String>,
    pub default_running: bool,
    pub profiles: Vec<DesktopProfile>,
}

/// macOS 上装了 Claude Desktop。
pub fn supported() -> bool {
    cfg!(target_os = "macos") && crate::claude_desktop_config::macos_app_bundle().is_some()
}

/// 账号卡：绑了 Claude 账号的 Desktop 官方卡（不含原来那张固定 id 的）。
pub fn is_account_card(provider: &Provider) -> bool {
    bound_account_key(provider).is_some()
}

/// 卡片代表的授权中心账号：账号卡是绑的号，原来那张官方卡是默认数据目录现在登的号。托盘用它显示
/// 账号和额度。
pub fn card_account_key(provider: &Provider) -> Option<String> {
    if provider.id == CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID {
        return supported().then(default_account_key).flatten();
    }
    bound_account_key(provider)
}

/// 托盘点这张卡时要不要交给界面：切到账号卡、或者账号目录的 Desktop 开着（切过去要换 Desktop），
/// 都要用户在界面上确认退出 / 看着登录。
pub fn switch_needs_ui(provider: &Provider) -> bool {
    supported() && (is_account_card(provider) || running_instances().any_profile())
}

fn bound_account_key(provider: &Provider) -> Option<String> {
    if provider.category.as_deref() != Some("official")
        || provider.id == CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID
    {
        return None;
    }
    provider
        .meta
        .as_ref()?
        .managed_account_id_for(claude_official::AUTH_PROVIDER)
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

/// 默认数据目录登的号在授权中心里的 key（`accountUuid:organizationUuid`）。Desktop 只记账号，同一个
/// 号有几个组织时取排在最前的；号不在授权中心时为 `None`。
fn default_account_key() -> Option<String> {
    let prefix = format!("{}:", signed_in_account(&default_dir()?)?);
    crate::services::claude_account::labels_on_disk(&crate::config::get_app_config_dir())
        .into_keys()
        .filter(|key| key.starts_with(&prefix))
        .min()
}

/// 各账号目录登的是谁、在不在运行。
pub fn status() -> DesktopProfilesStatus {
    if !supported() {
        return DesktopProfilesStatus::default();
    }
    let instances = running_instances();
    DesktopProfilesStatus {
        supported: true,
        default_account_key: default_account_key(),
        default_running: instances.pid_of(None).is_some(),
        profiles: profile_dirs()
            .into_iter()
            .map(|(account_uuid, dir)| DesktopProfile {
                signed_in_as: signed_in_account(&dir),
                running: instances.pid_of(Some(&dir)).is_some(),
                account_uuid,
            })
            .collect(),
    }
}

/// 打开卡片对应的 Desktop，返回是不是第一次用这个账号目录（要在 Desktop 里登录）。`account_key` 是
/// 账号卡绑的号，用这个号的数据目录（默认数据目录登的正好是这个号、又没建过账号目录时用默认的）；
/// `None` 是别的卡，用默认数据目录。
///
/// 要打开的已经开着就只叫到前面。别的 Desktop 开着时：`quit_running`（用户在界面上确认过）就先结束
/// 它们，否则报错、什么都不动。打开前把没在运行的目录的会话列表补齐（见 `claude_desktop_sessions`）。
pub fn open(account_key: Option<&str>, quit_running: bool) -> Result<bool, AppError> {
    if !supported() {
        return Err(AppError::localized(
            "claude_desktop.account.unsupported",
            "目前只支持 macOS 上的 Claude Desktop（装在「应用程序」文件夹里）",
            "Only Claude Desktop on macOS is supported for now (installed in Applications)",
        ));
    }
    let profile_dir = match account_key {
        Some(key) => {
            let uuid = account_uuid_of(key)?;
            let dir = accounts_root().join(uuid);
            let use_default = !dir.is_dir()
                && default_dir()
                    .and_then(|dir| signed_in_account(&dir))
                    .as_deref()
                    == Some(uuid);
            (!use_default).then_some(dir)
        }
        None => None,
    };

    let mut instances = running_instances();
    let target_pid = instances.pid_of(profile_dir.as_deref());
    if instances.any() && target_pid.is_none() {
        if !quit_running {
            return Err(AppError::localized(
                "claude_desktop.account.others_running",
                "还有 Claude Desktop 开着，确认退出它后再打开",
                "Another Claude Desktop is running; confirm quitting it, then open again",
            ));
        }
        quit(&instances)?;
        instances = DesktopInstances::default();
    }
    let first_login = profile_dir
        .as_deref()
        .is_some_and(|dir| signed_in_account(dir).is_none());
    if let Some(dir) = &profile_dir {
        fs::create_dir_all(dir).map_err(|e| AppError::io(dir, e))?;
    }

    // 补不了会话也照样打开
    let dirs: Vec<(PathBuf, bool)> = default_dir()
        .map(|dir| (None, dir))
        .into_iter()
        .chain(
            profile_dirs()
                .into_iter()
                .map(|(_, dir)| (Some(dir.clone()), dir)),
        )
        .map(|(launch, dir)| {
            let idle = instances.pid_of(launch.as_deref()).is_none();
            (dir, idle)
        })
        .collect();
    if let Err(error) = crate::services::claude_desktop_sessions::fill(&dirs) {
        log::info!("[DesktopAccount] 打开前没有补齐会话: {error}");
    }

    // 已经开着就只叫到前面：Desktop 不拦同一个数据目录再开一个
    match target_pid {
        Some(pid) => bring_to_front(pid),
        None => launch(profile_dir.as_deref())?,
    }
    log::info!(
        "[DesktopAccount] 已打开 Claude Desktop（{}）",
        if profile_dir.is_some() {
            "账号目录"
        } else {
            "默认数据目录"
        }
    );
    Ok(first_login)
}

/// 所有账号目录的根。
fn accounts_root() -> PathBuf {
    crate::config::get_home_dir()
        .join("Library")
        .join("Application Support")
        .join("com.ccswitch.desktop")
        .join("claude-desktop-accounts")
}

/// Desktop 原来的数据目录。
fn default_dir() -> Option<PathBuf> {
    crate::claude_desktop_config::desktop_data_dir()
}

/// 名字是 UUID 的账号目录（按名字排序）。
fn profile_dirs() -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(accounts_root()) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            is_uuid(&name).then(|| (name, entry.path()))
        })
        .collect();
    dirs.sort();
    dirs
}

/// 数据目录登的是哪个账号：只读 `config.json` 的 `lastKnownAccountUuid`，其余内容（包括 Desktop 加密
/// 的登录缓存）读完即丢。
pub(crate) fn signed_in_account(desktop_dir: &Path) -> Option<String> {
    let doc: Value =
        serde_json::from_slice(&fs::read(desktop_dir.join("config.json")).ok()?).ok()?;
    doc.get("lastKnownAccountUuid")
        .and_then(Value::as_str)
        .filter(|id| is_uuid(id))
        .map(str::to_string)
}

pub(crate) fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// 账号 key 的前半段；它会变成目录名，必须是 UUID。
fn account_uuid_of(account_key: &str) -> Result<&str, AppError> {
    let uuid = account_key.split(':').next().unwrap_or_default().trim();
    if is_uuid(uuid) {
        Ok(uuid)
    } else {
        Err(AppError::InvalidInput(format!(
            "不是 Claude 账号: {account_key}"
        )))
    }
}

/// 正在运行的 Desktop 主进程：进程号和数据目录（`None` 是默认数据目录）。
#[derive(Debug, Default, PartialEq, Eq)]
struct DesktopInstances(Vec<(u32, Option<PathBuf>)>);

impl DesktopInstances {
    fn any(&self) -> bool {
        !self.0.is_empty()
    }

    fn any_profile(&self) -> bool {
        self.0.iter().any(|(_, dir)| dir.is_some())
    }

    fn pid_of(&self, dir: Option<&Path>) -> Option<u32> {
        self.0
            .iter()
            .find(|(_, running)| match (running, dir) {
                (None, None) => true,
                (Some(running), Some(dir)) => running.components().eq(dir.components()),
                _ => false,
            })
            .map(|(pid, _)| *pid)
    }
}

/// 读进程表里 `Claude.app` 的主进程和它的 `--user-data-dir`。读不出来时当作没在运行。
fn running_instances() -> DesktopInstances {
    if !cfg!(target_os = "macos") {
        return DesktopInstances::default();
    }
    std::process::Command::new("ps")
        .args(["-axww", "-o", "pid=,args="])
        .stdin(std::process::Stdio::null())
        .output()
        .map(|output| instances_from_ps(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

/// `ps -o pid=,args=` 的输出。认 `…/Claude.app/Contents/MacOS/Claude`（复制出来的 `Claude 2.app` 也
/// 算），不认 `Claude Helper (…)` 和命令行的 `claude`。路径里可能有空格，目录取到下一个 ` --` 为止。
fn instances_from_ps(text: &str) -> DesktopInstances {
    const EXE: &str = ".app/Contents/MacOS/Claude";
    const FLAG: &str = "--user-data-dir=";
    DesktopInstances(
        text.lines()
            .filter_map(|line| {
                let (pid, args) = line.trim().split_once(' ')?;
                let args = &args[args.find(EXE)? + EXE.len()..];
                if !(args.is_empty() || args.starts_with(" -")) {
                    return None;
                }
                let dir = args.find(FLAG).and_then(|start| {
                    let value = &args[start + FLAG.len()..];
                    let dir = value[..value.find(" --").unwrap_or(value.len())].trim();
                    (!dir.is_empty()).then(|| PathBuf::from(dir))
                });
                Some((pid.parse().ok()?, dir))
            })
            .collect(),
    )
}

/// 结束正在运行的 Desktop 主进程（SIGTERM：和 ⌘Q 一样走正常退出），等它们都退出。
fn quit(instances: &DesktopInstances) -> Result<(), AppError> {
    #[cfg(unix)]
    for (pid, _) in &instances.0 {
        if let Ok(pid) = i32::try_from(*pid) {
            // SAFETY: 只是给这个进程发信号；进程号来自刚读的进程表
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = instances;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if !running_instances().any() {
            log::info!("[DesktopAccount] 已按用户确认退出正在运行的 Claude Desktop");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(AppError::localized(
        "claude_desktop.account.quit_timeout",
        "Claude Desktop 没能退出，请手动退出（⌘Q）后再打开",
        "Claude Desktop didn't quit. Quit it yourself (⌘Q), then open again",
    ))
}

/// 用 `open` 启动 Desktop。账号目录要 `-n`：另起一个进程才会带上 `--user-data-dir`（所以这个目录已经
/// 在跑时不能走这里，见 [`bring_to_front`]）。
fn launch(profile_dir: Option<&Path>) -> Result<(), AppError> {
    let bundle = crate::claude_desktop_config::macos_app_bundle().ok_or_else(|| {
        AppError::localized(
            "claude_desktop.account.not_installed",
            "没有找到 Claude Desktop",
            "Claude Desktop was not found",
        )
    })?;
    let mut args: Vec<OsString> = Vec::new();
    if profile_dir.is_some() {
        args.push("-n".into());
    }
    args.extend(["-a".into(), bundle.into_os_string()]);
    if let Some(dir) = profile_dir {
        let mut flag = OsString::from("--user-data-dir=");
        flag.push(dir.as_os_str());
        args.extend(["--args".into(), flag]);
    }
    let output = std::process::Command::new("open")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| AppError::Message(format!("打开 Claude Desktop 失败: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Message(format!(
            "打开 Claude Desktop 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// 把开着的那个 Desktop 叫到前面。同一个 `Claude.app` 可能开着好几个（各自的数据目录），`open -a`
/// 挑不了是哪个，所以按进程号找。叫不到前面也不要紧，Desktop 本来就开着。
fn bring_to_front(pid: u32) {
    #[cfg(target_os = "macos")]
    {
        use objc2_06::msg_send;
        use objc2_06::rc::Retained;
        use objc2_06::runtime::{AnyClass, AnyObject};

        /// `NSApplicationActivateAllWindows`
        const ACTIVATE_ALL_WINDOWS: usize = 1;
        let Ok(pid) = i32::try_from(pid) else {
            return;
        };
        objc2_06::rc::autoreleasepool(|_| {
            let Some(class) = AnyClass::get(c"NSRunningApplication") else {
                return;
            };
            // SAFETY: `+runningApplicationWithProcessIdentifier:` 收 pid_t、返回 NSRunningApplication
            // 或 nil；`-activateWithOptions:` 收 NSUInteger 选项、返回 BOOL。NSRunningApplication 线程安全。
            unsafe {
                let app: Option<Retained<AnyObject>> =
                    msg_send![class, runningApplicationWithProcessIdentifier: pid];
                if let Some(app) = app {
                    let _: bool = msg_send![&*app, activateWithOptions: ACTIVATE_ALL_WINDOWS];
                }
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_official_cards_bound_to_a_claude_account_are_account_cards() {
        let key = "aaaaaaaa-0000-4000-8000-000000000001:org";
        let mut card: Provider = serde_json::from_value(json!({
            "id": "desktop",
            "name": "小号",
            "settingsConfig": {},
            "category": "official",
            "meta": { "authBinding": {
                "source": "managed_account",
                "authProvider": "claude_official",
                "accountId": key,
            }},
        }))
        .unwrap();
        assert_eq!(card_account_key(&card).as_deref(), Some(key));
        // 原来那张固定 id 的官方卡代表 Desktop 自己的登录，绑了也不算
        card.id = CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID.to_string();
        assert!(!is_account_card(&card));
        // 账号 key 会变成目录名
        assert!(account_uuid_of("../../Library:org").is_err());
    }

    #[test]
    fn reads_which_data_folders_desktop_is_running_with() {
        let ps = "\
  501 /Applications/Claude.app/Contents/MacOS/Claude -psn_0_12345
  502 /Applications/Claude.app/Contents/Frameworks/Claude Helper (Renderer).app/Contents/MacOS/Claude Helper (Renderer) --user-data-dir=/x
  503 /Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=/Users/me/acc one --enable-features=x
  504 /usr/local/bin/claude --resume
";
        assert_eq!(
            instances_from_ps(ps),
            DesktopInstances(vec![
                (501, None),
                (503, Some(PathBuf::from("/Users/me/acc one"))),
            ])
        );
    }
}

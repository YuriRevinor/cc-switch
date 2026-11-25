//! Claude Desktop 用授权中心里的 Claude 账号：每个账号一个独立的 Desktop 数据目录（macOS、Windows）。
//!
//! Desktop 是 Electron 应用，启动时认 `--user-data-dir`：换一个数据目录，登录、设置和会话列表就都是
//! 另一份。`--user-data-dir` 不是 Anthropic 公开支持的用法，以后的版本可能不再认它。
//!
//! - 账号卡：绑了授权中心 Claude 账号（`claude_official`）的 Desktop 官方卡（随机 id）。原来那张固定
//!   id 的官方卡（`claude-desktop-official`）不能绑号，代表默认数据目录里 Desktop 自己的登录；
//! - 切换：账号卡和别的卡一样「切换」，只写默认数据目录的配置（账号卡是官方卡，写回 1P），不退出、
//!   也不启动 Desktop。用户在使用中的账号卡上点「打开 Claude Desktop」时才由 [`open`] 用
//!   `<根>/<accountUuid>/` 启动它。CC Switch 从不结束 Desktop：别的 Desktop 开着时不打开，请用户自己
//!   先退出。第一次打开时在 Desktop 自己的登录页登录这个号，登录由 Desktop 加密保存在目录里，CC Switch
//!   不读；
//! - 只看 `config.json` 的 `lastKnownAccountUuid` 判断目录里登的是谁。
//!
//! 目录放在 `~/Library/Application Support/com.ccswitch.desktop/claude-desktop-accounts/`（不放进可能
//! 被网盘同步的 `~/.cc-switch`）。从授权中心删掉账号不删目录，再加回来还能接着用。
//!
//! Windows（只认 MSIX 安装版，见 [`win`]）和 macOS 有几处不同：
//! - 目录在 `%USERPROFILE%\.cc-switch-claude-desktop-accounts\`：Desktop 的 MSIX 包开着文件系统写虚拟化，
//!   AppData 下除了它自己的几个目录都会被重定向进包的私有目录，CC Switch 读不到；
//! - 3P 配置（`%LOCALAPPDATA%\Claude-3p`）是全局一份，不跟着数据目录走。新目录还没登录时 Desktop 会
//!   丢掉 1P 选择、改用 3P 重启，所以打开账号目录时带 `--claude-first-party-sign-in` 保住 1P；
//! - 关窗口只是缩到托盘，退出只能结束进程树；已经开着的目录再启动一次，Desktop 的单实例锁会把它
//!   叫到前面。

#[cfg(not(windows))]
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
    /// 装了能按账号目录打开的 Claude Desktop（macOS 的 Claude.app、Windows 的 MSIX 版）
    pub supported: bool,
    /// 默认数据目录登的号在授权中心里的 key（原来那张官方卡按它显示账号和额度）
    pub default_account_key: Option<String>,
    pub default_running: bool,
    pub profiles: Vec<DesktopProfile>,
}

/// 装了能按账号目录打开的 Claude Desktop：macOS 的 Claude.app，或 Windows 的 MSIX 版。
pub fn supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::claude_desktop_config::macos_app_bundle().is_some()
    }
    #[cfg(windows)]
    {
        win::launcher().is_some()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
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

/// 命令行参数：按 Claude Desktop 页当前使用中的卡打开 Desktop（见 [`open_current`]），给快捷键用。
pub const OPEN_FLAG: &str = "--open-claude-desktop";

/// 用户点「打开 Claude Desktop」：打开账号卡对应的 Desktop，返回是不是第一次用这个账号目录（要在
/// Desktop 里登录）。`account_key` 是账号卡绑的号，用这个号的数据目录（默认数据目录登的正好是这个号、
/// 又没建过账号目录时用默认的）。
///
/// 要打开的已经开着就只叫到前面。别的 Desktop 开着时报错、什么都不动：CC Switch 从不结束 Desktop。
/// 打开前把没在运行的目录的会话列表补齐（见 `claude_desktop_sessions`）。
pub fn open(account_key: &str) -> Result<bool, AppError> {
    ensure_supported()?;
    let uuid = account_uuid_of(account_key)?;
    let dir = accounts_root().join(uuid);
    let use_default = !dir.is_dir()
        && default_dir()
            .and_then(|dir| signed_in_account(&dir))
            .as_deref()
            == Some(uuid);
    open_dir((!use_default).then_some(dir), true)
}

/// 命令行 [`OPEN_FLAG`]：和在 Claude Desktop 页当前使用中的卡上点「打开 Claude Desktop」一样。
/// 账号卡用这个号的数据目录；原来那张官方卡用默认数据目录，都带 1P 登录参数；第三方卡（网关）用默认
/// 数据目录、不带。同样不结束别的 Desktop，打开前补齐会话。返回是不是第一次用这个账号目录。
pub fn open_current(state: &crate::store::AppState) -> Result<bool, AppError> {
    use crate::app_config::AppType;
    ensure_supported()?;
    let id = crate::services::provider::ProviderService::current(state, AppType::ClaudeDesktop)?;
    let provider = if id.is_empty() {
        None
    } else {
        state.db.get_provider_by_id(&id, &AppType::ClaudeDesktop)?
    };
    match open_target(provider.as_ref()) {
        OpenTarget::Account(key) => open(&key),
        OpenTarget::Default { first_party } => open_dir(None, first_party),
    }
}

/// 当前卡要用哪个数据目录打开。
#[derive(Debug, PartialEq, Eq)]
enum OpenTarget {
    /// 账号卡绑的号
    Account(String),
    /// 默认数据目录；`first_party`：官方卡（或还没选过卡），带 1P 登录参数
    Default { first_party: bool },
}

fn open_target(card: Option<&Provider>) -> OpenTarget {
    match card {
        Some(card) => match bound_account_key(card) {
            Some(key) => OpenTarget::Account(key),
            None => OpenTarget::Default {
                first_party: crate::claude_desktop_config::is_official_provider(card),
            },
        },
        // 还没选过卡：Desktop 自己的默认登录
        None => OpenTarget::Default { first_party: true },
    }
}

fn ensure_supported() -> Result<(), AppError> {
    if supported() {
        return Ok(());
    }
    Err(AppError::localized(
        "claude_desktop.account.unsupported",
        "目前只支持 macOS 上的 Claude Desktop（装在「应用程序」文件夹里）和 Windows 上 Microsoft Store / MSIX 安装的 Claude Desktop",
        "Only Claude Desktop on macOS (installed in Applications) or the Microsoft Store / MSIX install on Windows is supported for now",
    ))
}

/// 用 `profile_dir`（`None` 是默认数据目录）打开 Desktop。`first_party`：官方卡，启动时带
/// `--claude-first-party-sign-in`（只在 Windows 上有意义，见 `win`）。
fn open_dir(profile_dir: Option<PathBuf>, first_party: bool) -> Result<bool, AppError> {
    let instances = running_instances();
    let target_pid = instances.pid_of(profile_dir.as_deref());
    if instances.any() && target_pid.is_none() {
        return Err(if cfg!(windows) {
            AppError::localized(
                "claude_desktop.account.others_running",
                "还有别的 Claude Desktop 开着，CC Switch 不会替你关闭它。请先在托盘图标上退出 Claude，再重新打开",
                "Another Claude Desktop is running, and CC Switch won't close it for you. Quit Claude from its tray icon, then open it again",
            )
        } else {
            AppError::localized(
                "claude_desktop.account.others_running",
                "还有别的 Claude Desktop 开着，CC Switch 不会替你关闭它。请先退出它（⌘Q），再重新打开",
                "Another Claude Desktop is running, and CC Switch won't close it for you. Quit it (⌘Q), then open it again",
            )
        });
    }
    let first_login = profile_dir
        .as_deref()
        .is_some_and(|dir| signed_in_account(dir).is_none());
    if let Some(dir) = &profile_dir {
        fs::create_dir_all(dir).map_err(|e| AppError::io(dir, e))?;
    }

    // 已经开着：先叫到前面（不用等补齐，它的目录本来就不补），再给别的目录补齐
    if let Some(pid) = target_pid {
        bring_to_front(pid, profile_dir.as_deref());
    }

    // 补不了会话也照样打开。平常后台已经同步过（见 `start_background_sync`），这里只是核对一遍
    sync_sessions(&instances, "打开前");

    // 没开着才启动：Desktop 不拦同一个数据目录再开一个
    if target_pid.is_none() {
        launch(profile_dir.as_deref(), first_party)?;
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

/// 多久看一次 Desktop 开没开着
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

/// 在后台趁 Desktop 没开着时把会话同步好（补齐、同步删除和归档，见 `claude_desktop_sessions`），
/// 打开 Desktop 时只剩核对一遍（文件都没变，几十毫秒）：
///
/// - CC Switch 启动时做一次；
/// - 之后每隔几秒看一眼进程表，有 Desktop 退出了就再做一次。
///
/// 只看进程表，不结束也不启动 Desktop；只写没开着的那些数据目录，和打开前补齐一样。没建过账号目录
/// （没用多账号）时什么都不做。
pub fn start_background_sync() {
    if !supported() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("claude-desktop-session-sync".into())
        .spawn(|| {
            let mut last: Option<DesktopInstances> = None;
            loop {
                if !profile_dirs().is_empty() {
                    let instances = running_instances();
                    let someone_quit = last.as_ref().is_none_or(|before| {
                        before
                            .0
                            .iter()
                            .any(|(_, dir)| instances.pid_of(dir.as_deref()).is_none())
                    });
                    if someone_quit {
                        sync_sessions(&instances, "后台");
                    }
                    last = Some(instances);
                }
                std::thread::sleep(WATCH_INTERVAL);
            }
        });
    if let Err(error) = spawned {
        log::warn!("[DesktopAccount] 没能启动后台会话同步: {error}");
    }
}

/// 补齐、同步一遍没开着的数据目录，结果写日志。补不了也不影响打开。
fn sync_sessions(instances: &DesktopInstances, when: &str) {
    let started = std::time::Instant::now();
    match crate::services::claude_desktop_sessions::fill(&session_dirs(instances)) {
        Ok(report) => log::info!(
            "[DesktopAccount] {when}同步会话（{} ms）：补上 {}，同步删除 {}（留着 {}），同步归档 {}",
            started.elapsed().as_millis(),
            report.added,
            report.sync.trashed,
            report.sync.kept,
            report.sync.archived
        ),
        Err(error) => log::info!("[DesktopAccount] {when}没有同步会话: {error}"),
    }
}

/// 所有 Desktop 数据目录（默认的和各账号目录），以及各自能不能写（那个 Desktop 没开着）。
fn session_dirs(instances: &DesktopInstances) -> Vec<(PathBuf, bool)> {
    default_dir()
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
        .collect()
}

/// 从回收站恢复一个同步删除的会话（见 `claude_desktop_sessions::restore`）。
pub fn restore_session(
    session_id: &str,
) -> Result<crate::services::claude_desktop_trash::RestoreReport, AppError> {
    crate::services::claude_desktop_sessions::restore(
        &session_dirs(&running_instances()),
        session_id,
    )
}

/// 所有账号目录的根。
fn accounts_root() -> PathBuf {
    let home = crate::config::get_home_dir();
    if cfg!(windows) {
        // 不能放在 AppData 下：MSIX 版 Desktop 往那里写会被重定向进包的私有目录（见模块说明）
        return home.join(".cc-switch-claude-desktop-accounts");
    }
    home.join("Library")
        .join("Application Support")
        .join("com.ccswitch.desktop")
        .join("claude-desktop-accounts")
}

/// Desktop 原来的数据目录（Electron 的 userData，`config.json` 和会话列表在这里）。Windows 上它和
/// `claude_desktop_config.json` 所在的 `%LOCALAPPDATA%\Claude` 不是同一个目录。
fn default_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        win::default_data_dir()
    }
    #[cfg(not(windows))]
    {
        crate::claude_desktop_config::desktop_data_dir()
    }
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
                (Some(running), Some(dir)) => same_dir(running, dir),
                _ => false,
            })
            .map(|(pid, _)| *pid)
    }
}

/// 两个目录是不是同一个。Windows 的路径不分大小写，命令行里也可能多带一个结尾的 `\`。
fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |path: &Path| {
            path.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        return norm(a) == norm(b);
    }
    a.components().eq(b.components())
}

/// 读进程表里 Desktop 的主进程和它的 `--user-data-dir`。读不出来时当作没在运行。
fn running_instances() -> DesktopInstances {
    #[cfg(windows)]
    {
        win::running_instances()
    }
    #[cfg(not(windows))]
    {
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
}

/// Windows 上一个 `claude.exe` 进程是不是 Desktop 的主进程，是的话它用的数据目录（`None` 是默认数据
/// 目录）。只认 MSIX 包里的 `…\WindowsApps\Claude_…\app\Claude.exe`：同名的还有 Claude Code CLI
/// （在 Desktop 数据目录的 `claude-code` 下）。带 `--type=` 的是 Electron 的子进程。`args` 不含程序名。
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_desktop_main(exe: &str, args: &[String]) -> Option<Option<PathBuf>> {
    const FLAG: &str = "--user-data-dir=";
    let exe = exe.replace('/', "\\").to_lowercase();
    let package = exe.strip_suffix("\\app\\claude.exe")?;
    let package = package.rsplit('\\').next()?;
    if !package.starts_with("claude_") || !exe.contains("\\windowsapps\\") {
        return None;
    }
    if args.iter().any(|arg| arg.starts_with("--type=")) {
        return None;
    }
    let dir = args
        .iter()
        .find_map(|arg| arg.strip_prefix(FLAG))
        .map(|dir| dir.trim().trim_matches('"'))
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    Some(dir)
}

/// `ps -o pid=,args=` 的输出。认 `…/Claude.app/Contents/MacOS/Claude`（复制出来的 `Claude 2.app` 也
/// 算），不认 `Claude Helper (…)` 和命令行的 `claude`。路径里可能有空格，目录取到下一个 ` --` 为止。
#[cfg_attr(windows, allow(dead_code))]
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

fn not_installed() -> AppError {
    AppError::localized(
        "claude_desktop.account.not_installed",
        "没有找到 Claude Desktop",
        "Claude Desktop was not found",
    )
}

/// 启动 Desktop，`profile_dir` 是账号目录（`None` 用默认数据目录），`first_party` 见 [`open_dir`]。
/// 这个目录已经在跑时不能走这里，见 [`bring_to_front`]。
fn launch(profile_dir: Option<&Path>, first_party: bool) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        win::launch(profile_dir, first_party)
    }
    #[cfg(not(windows))]
    {
        let _ = first_party;
        launch_macos(profile_dir)
    }
}

/// 处理命令行 [`OPEN_FLAG`]（冷启动或转给正在运行的实例）：在后台打开，结果只在出错时弹系统对话框
/// 告诉用户（打开成功时用户看到的就是 Desktop 窗口），不把 CC Switch 的主窗口拿到前台。
pub fn open_from_command_line(app: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::Manager;
    /// 正在打开：快捷键连按时后面几下不再打开一遍（Desktop 刚启动、窗口还没出来时会这样）
    static OPENING: AtomicBool = AtomicBool::new(false);
    if OPENING.swap(true, Ordering::AcqRel) {
        log::info!("[DesktopAccount] 命令行打开 Claude Desktop：上一次还没打开完，这次不重复打开");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                OPENING.store(false, Ordering::Release);
            }
        }
        let _done = Done;
        let Some(state) = app.try_state::<crate::store::AppState>() else {
            log::warn!("[DesktopAccount] 命令行打开 Claude Desktop：应用还没准备好");
            return;
        };
        match open_current(&state) {
            Ok(first_login) => log::info!(
                "[DesktopAccount] 命令行打开 Claude Desktop 完成{}",
                if first_login {
                    "（要在里面登录）"
                } else {
                    ""
                }
            ),
            Err(error) => {
                log::warn!("[DesktopAccount] 命令行打开 Claude Desktop 失败: {error}");
                show_error_dialog(&error.to_string());
            }
        }
    });
}

#[cfg(windows)]
fn show_error_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
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

#[cfg(not(windows))]
fn show_error_dialog(_message: &str) {}

/// 用 `open` 启动 Desktop。账号目录要 `-n`：另起一个进程才会带上 `--user-data-dir`。
#[cfg(not(windows))]
fn launch_macos(profile_dir: Option<&Path>) -> Result<(), AppError> {
    let bundle = crate::claude_desktop_config::macos_app_bundle().ok_or_else(not_installed)?;
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

/// 把开着的那个 Desktop 叫到前面。叫不到前面也不要紧，Desktop 本来就开着。
///
/// - macOS：同一个 `Claude.app` 可能开着好几个（各自的数据目录），`open -a` 挑不了是哪个，所以按
///   进程号找；
/// - Windows：窗口开着（含最小化）就直接把它拿到前台，最快；缩在托盘里或拿不到前台（系统不让后台
///   程序抢焦点）时，用同一个数据目录再启动一次。Desktop 按数据目录拿单实例锁，新进程把启动交给开着的
///   那个（它把窗口叫出来）就退出，要多等一个 Electron 进程起来。
fn bring_to_front(pid: u32, profile_dir: Option<&Path>) {
    #[cfg(target_os = "macos")]
    {
        let _ = profile_dir;
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
    #[cfg(windows)]
    {
        if win::activate_window(pid) {
            return;
        }
        if let Err(error) = win::launch(profile_dir, false) {
            log::info!("[DesktopAccount] 没能把开着的 Claude Desktop 叫到前面: {error}");
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (pid, profile_dir);
}

/// Windows 上的 Claude Desktop：只认 MSIX 安装版（Microsoft Store 或官网的 MSIX 安装包）。
///
/// - 启动走包注册的执行别名 `%LOCALAPPDATA%\Microsoft\WindowsApps\claude-desktop.exe`，Desktop 带着包
///   身份运行、收得到命令行参数。直接运行 `WindowsApps` 里的 exe 不带包身份；
/// - 默认数据目录是 `%LOCALAPPDATA%\Claude-Data\claude_<包发布者哈希>`（Desktop 自己搬过去的，不在
///   MSIX 的写虚拟化范围里）；
/// - 进程表用 Toolhelp 快照读，命令行用 `NtQueryInformationProcess` 读（同一用户的进程只要
///   `PROCESS_QUERY_LIMITED_INFORMATION`），再按 [`windows_desktop_main`] 认。
#[cfg(windows)]
mod win {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{not_installed, windows_desktop_main, DesktopInstances};
    use crate::error::AppError;

    /// MSIX 包注册的执行别名
    const LAUNCHER_ALIAS: &str = "claude-desktop.exe";
    /// 数据目录还没登录（新建的账号目录、或在默认数据目录里退出了登录）时，Desktop 启动会丢掉全局的 1P
    /// 选择（把 `Claude-3p` 里的 `deploymentMode` 清掉），登录页变成可以选网关（3P）的那种。带上这个
    /// 参数它保留 1P，停在 Claude 账号的登录页。只在启动时认，重启时 Desktop 自己去掉。
    const FIRST_PARTY_SIGN_IN: &str = "--claude-first-party-sign-in";

    fn local_app_data() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
    }

    /// 执行别名在就算装了。它是一个 0 字节的重解析点，跟随它取元数据会失败，所以不跟随。
    pub(super) fn launcher() -> Option<PathBuf> {
        let alias = local_app_data()?
            .join("Microsoft")
            .join("WindowsApps")
            .join(LAUNCHER_ALIAS);
        fs::symlink_metadata(&alias).is_ok().then_some(alias)
    }

    /// 默认数据目录：`Claude-Data` 下有 `config.json` 的 `claude_*`，有几个时取最近写过的。
    pub(super) fn default_data_dir() -> Option<PathBuf> {
        let root = local_app_data()?.join("Claude-Data");
        fs::read_dir(&root)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("claude_")
            })
            .filter_map(|entry| {
                let modified = fs::metadata(entry.path().join("config.json"))
                    .ok()?
                    .modified()
                    .ok()?;
                Some((modified, entry.path()))
            })
            .max()
            .map(|(_, dir)| dir)
    }

    /// 启动 Desktop。`first_party` 带上 [`FIRST_PARTY_SIGN_IN`]（账号卡都是官方的）；叫到前面那次不带。
    pub(super) fn launch(profile_dir: Option<&Path>, first_party: bool) -> Result<(), AppError> {
        let launcher = launcher().ok_or_else(not_installed)?;
        let mut command = std::process::Command::new(&launcher);
        if let Some(dir) = profile_dir {
            let mut flag = OsString::from("--user-data-dir=");
            flag.push(dir.as_os_str());
            command.arg(flag);
        }
        if first_party {
            command.arg(FIRST_PARTY_SIGN_IN);
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|e| AppError::Message(format!("打开 Claude Desktop 失败: {e}")))
    }

    /// 把进程 `pid` 的主窗口（看得见、没有所有者、有标题的顶层窗口）拿到前台，最小化的先还原。
    /// 没有这样的窗口（缩在托盘里）或系统不让拿到前台时为 `false`。
    pub(super) fn activate_window(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetForegroundWindow, GetWindow, GetWindowTextLengthW,
            GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow,
            GW_OWNER, SW_RESTORE,
        };

        struct Search {
            pid: u32,
            found: HWND,
        }

        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: lparam 是下面传进来的 `&mut Search`，EnumWindows 返回前一直有效
            let search = unsafe { &mut *(lparam as *mut Search) };
            let mut owner_pid = 0u32;
            // SAFETY: hwnd 由 EnumWindows 给出；owner_pid 是有效的输出指针
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut owner_pid);
                if owner_pid == search.pid
                    && IsWindowVisible(hwnd) != 0
                    && GetWindow(hwnd, GW_OWNER).is_null()
                    && GetWindowTextLengthW(hwnd) > 0
                {
                    search.found = hwnd;
                    return 0;
                }
            }
            1
        }

        let mut search = Search {
            pid,
            found: std::ptr::null_mut(),
        };
        // SAFETY: 回调只在 EnumWindows 执行期间用 `search`
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
        let hwnd = search.found;
        if hwnd.is_null() {
            return false;
        }
        // SAFETY: hwnd 是刚找到的顶层窗口；窗口没了时这些调用只是失败
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd) != 0 && GetForegroundWindow() == hwnd
        }
    }

    pub(super) fn running_instances() -> DesktopInstances {
        DesktopInstances(
            claude_processes()
                .into_iter()
                .filter_map(|(pid, exe, args)| {
                    windows_desktop_main(&exe, &args).map(|dir| (pid, dir))
                })
                .collect(),
        )
    }

    /// 名字是 `claude.exe` 的进程：进程号、exe 路径、参数（不含程序名）。读不了的跳过。
    fn claude_processes() -> Vec<(u32, String, Vec<String>)> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };

        let mut found = Vec::new();
        // SAFETY: 快照句柄用完就关；PROCESSENTRY32W 按要求先填好 dwSize。
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return found;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                if wide_until_nul(&entry.szExeFile).eq_ignore_ascii_case("claude.exe") {
                    if let Some((exe, args)) = process_details(entry.th32ProcessID) {
                        found.push((entry.th32ProcessID, exe, args));
                    }
                }
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        found
    }

    fn wide_until_nul(text: &[u16]) -> String {
        let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        String::from_utf16_lossy(&text[..end])
    }

    /// 一个进程的 exe 完整路径和命令行参数。
    fn process_details(pid: u32) -> Option<(String, Vec<String>)> {
        use windows_sys::Wdk::System::Threading::{
            NtQueryInformationProcess, ProcessCommandLineInformation,
        };
        use windows_sys::Win32::Foundation::{CloseHandle, UNICODE_STRING};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: 句柄只在这个函数里用，最后关掉；缓冲区长度都按 API 要求传。命令行信息是一个
        // UNICODE_STRING 后面跟着它指向的字符，整块放在按 8 字节对齐的缓冲区里。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let details = (|| {
                let mut path = vec![0u16; 32_768];
                let mut len = path.len() as u32;
                if QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_WIN32,
                    path.as_mut_ptr(),
                    &mut len,
                ) == 0
                {
                    return None;
                }
                let exe = String::from_utf16_lossy(&path[..len as usize]);

                let mut needed = 0u32;
                NtQueryInformationProcess(
                    handle,
                    ProcessCommandLineInformation,
                    std::ptr::null_mut(),
                    0,
                    &mut needed,
                );
                if needed == 0 {
                    return None;
                }
                let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
                let status = NtQueryInformationProcess(
                    handle,
                    ProcessCommandLineInformation,
                    buffer.as_mut_ptr().cast(),
                    needed,
                    &mut needed,
                );
                if status < 0 {
                    return None;
                }
                let line = &*(buffer.as_ptr() as *const UNICODE_STRING);
                if line.Buffer.is_null() || line.Length == 0 {
                    return Some((exe, Vec::new()));
                }
                let chars = std::slice::from_raw_parts(line.Buffer, line.Length as usize / 2);
                Some((exe, split_command_line(chars)))
            })();
            CloseHandle(handle);
            details
        }
    }

    /// 按 Windows 的规则拆命令行（引号、反斜杠），去掉程序名。
    fn split_command_line(line: &[u16]) -> Vec<String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

        let mut text: Vec<u16> = line.to_vec();
        text.push(0);
        let mut count = 0i32;
        // SAFETY: `text` 以 0 结尾；返回的数组用 LocalFree 释放，读的时候不越过 `count`。
        unsafe {
            let argv = CommandLineToArgvW(text.as_ptr(), &mut count);
            if argv.is_null() {
                return Vec::new();
            }
            let args = (1..count.max(0) as usize)
                .map(|index| {
                    let arg = *argv.add(index);
                    let len = (0..).take_while(|&i| *arg.add(i) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(arg, len))
                })
                .collect();
            LocalFree(argv.cast());
            args
        }
    }
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
    fn the_command_line_opens_the_desktop_of_the_current_card() {
        let key = "aaaaaaaa-0000-4000-8000-000000000001:org";
        let card = |id: &str, category: &str, bound: bool| -> Provider {
            let meta = if bound {
                json!({ "authBinding": {
                    "source": "managed_account",
                    "authProvider": "claude_official",
                    "accountId": key,
                }})
            } else {
                json!({})
            };
            serde_json::from_value(json!({
                "id": id, "name": id, "settingsConfig": {},
                "category": category, "meta": meta,
            }))
            .unwrap()
        };
        // 账号卡：这个号的目录
        assert_eq!(
            open_target(Some(&card("alt", "official", true))),
            OpenTarget::Account(key.to_string())
        );
        // 原来那张官方卡：默认目录，带 1P
        assert_eq!(
            open_target(Some(&card(
                CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
                "official",
                false
            ))),
            OpenTarget::Default { first_party: true }
        );
        // 第三方网关卡：默认目录，不带 1P
        assert_eq!(
            open_target(Some(&card("kimi", "third_party", false))),
            OpenTarget::Default { first_party: false }
        );
        assert_eq!(open_target(None), OpenTarget::Default { first_party: true });
    }

    #[test]
    fn picks_the_windows_desktop_main_process_out_of_the_claude_exe_processes() {
        let app =
            r"C:\Program Files\WindowsApps\Claude_6.28899.6.0_x64__pzs8sxrjxfjjc\app\Claude.exe";
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        // 默认数据目录：主进程不带参数
        assert_eq!(windows_desktop_main(app, &[]), Some(None));
        // 账号目录，路径里有空格（命令行拆开后引号已经去掉）
        assert_eq!(
            windows_desktop_main(
                app,
                &args(&[
                    r"--user-data-dir=C:\Users\me\.cc-switch-claude-desktop-accounts\acc one",
                    "--claude-first-party-sign-in",
                ]),
            ),
            Some(Some(PathBuf::from(
                r"C:\Users\me\.cc-switch-claude-desktop-accounts\acc one"
            )))
        );
        // Electron 子进程
        assert_eq!(
            windows_desktop_main(app, &args(&["--type=renderer", r"--user-data-dir=C:\x"]),),
            None
        );
        // 同名的 Claude Code CLI
        assert_eq!(
            windows_desktop_main(
                r"C:\Users\me\AppData\Local\Claude-Data\claude_pzs8sxrjxfjjc\claude-code\2.1.330\abc\claude.exe",
                &args(&["--output-format", "stream-json"]),
            ),
            None
        );
        assert_eq!(
            windows_desktop_main(r"C:\Users\me\.local\bin\claude.exe", &[]),
            None
        );
    }

    #[test]
    fn compares_data_folders_the_way_the_platform_does() {
        assert!(same_dir(
            Path::new("/Users/me/acc"),
            Path::new("/Users/me/acc")
        ));
        assert!(!same_dir(
            Path::new("/Users/me/acc"),
            Path::new("/Users/me/other")
        ));
        if cfg!(windows) {
            assert!(same_dir(
                Path::new(r"C:\Users\Me\Acc"),
                Path::new(r"c:\users\me\acc\")
            ));
        }
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

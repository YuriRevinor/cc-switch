//! Claude Desktop「Code」标签页的会话列表：换号后也能看到全部会话。
//!
//! 对话内容在 `~/.claude/projects/`（和账号无关），侧栏列表却按账号分开存在各数据目录的
//! `claude-code-sessions/<accountUuid>/<orgUuid>/local_<id>.json` 里，换了号就只列这个号的。
//! 切到一个号、打开 Desktop 之前（见 `claude_desktop_account::open`），把每个账号文件夹缺的列表
//! 文件从别处复制过去：只补缺，不覆盖、不删除，复制了哪些写进日志；正在运行的 Desktop 的目录不写
//! （它会拿内存里的列表覆盖回去）。Desktop 的登录不读不写。
//!
//! 两种情况不补：
//! - 对话记录（`~/.claude/projects/*/<cliSessionId>.jsonl`）已经不在了：补过去也打不开。
//! - 这个文件夹里以前有、现在没了：是在这个号里删掉的（或被 Desktop 自己清理掉的），不再补回。
//!   各文件夹出现过哪些会话记在 CC Switch 自己的 `~/.cc-switch/claude-desktop-sessions-seen.json`
//!   里，不往 Desktop 的目录写别的文件。文件夹整个空了（新建或被清空）时不按这份记录挡。
//!
//! 这两条都只会少复制；任何目录里已有的文件都不删。
//!
//! 「此电脑」（无文件夹）会话换了数据目录要换工作区路径才显示成「此电脑」，见
//! `claude_desktop_scratch`：复制时顺带处理；以前原样复制过来的，先备份再就地改这一处路径。
//!
//! 在一个号里删除、归档的会话同步到其他号，见 `claude_desktop_sync`（删掉的挪进 CC Switch 的回收站，
//! 见 `claude_desktop_trash`）；补齐时先做这一步。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::AppError;
use crate::services::claude_desktop_account::{is_uuid, signed_in_account};
use crate::services::claude_desktop_scratch::{foreign_workspace, relocate};
use crate::services::claude_desktop_sync::{
    apply_archive, apply_deletes, is_archived, record_archived, SyncLedger, SyncReport,
    MARKER_PREFIX,
};
use crate::services::claude_desktop_trash::{self as trash, RestoreReport, Trash, TrashEntry};

const SESSIONS_DIRNAME: &str = "claude-code-sessions";
const SEEN_FILENAME: &str = "claude-desktop-sessions-seen.json";
const BACKUP_DIRNAME: &str = "claude-desktop-sessions-backup";
const TRASH_DIRNAME: &str = "claude-desktop-sessions-trash";

/// 补齐、恢复、改开关一次只做一件（记录文件和回收站是共用的）
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `<数据目录>/claude-code-sessions/<accountUuid>/<orgUuid>/`
pub(crate) struct Folder {
    /// 所在数据目录（`dirs` 里的下标）
    pub(crate) root: usize,
    pub(crate) account: String,
    pub(crate) org: String,
    pub(crate) dir: PathBuf,
    /// 文件名 → 会话文件
    pub(crate) sessions: BTreeMap<String, Session>,
    /// Desktop 写的删除标记：会话 id → 删除时间（毫秒）
    pub(crate) markers: BTreeMap<String, i64>,
}

/// 一个会话文件里用得到的几项。
#[derive(Clone)]
pub(crate) struct Session {
    /// 会话 id（文件名 `local_<id>.json` 里的 `<id>`）
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    /// `lastActivityAt`（毫秒），没有时为 0
    pub(crate) last_activity: i64,
    /// 文件修改时间（毫秒）
    pub(crate) modified: i64,
    /// 本机对话记录的文件名（不带 `.jsonl`）。远程会话（SSH / WSL）、没有 `cliSessionId` 的为 `None`，
    /// 不按对话记录挡。
    transcript: Option<String>,
    /// 会话文件的摘要：只有 [`SUMMARY_FIELDS`] 这几项。要改写文件时用 [`Session::full_doc`] 读整份。
    pub(crate) doc: Value,
}

/// 补齐和同步要看的字段。会话文件一份三四十 KB，大头是 `promptAppendSnapshot`、
/// `remoteMcpServersConfig` 这些用不到的；只留这几项，读得快、缓存也小。
pub(crate) const SUMMARY_FIELDS: [&str; 13] = [
    "cliSessionId",
    "unarchivedCliSessionId",
    "priorCliSessionIds",
    "overflowCliSessionIds",
    "title",
    "originCwd",
    "cwd",
    "lastActivityAt",
    "sshConfig",
    "wslConfig",
    // claude_desktop_sync::ARCHIVE_FIELDS
    "isArchived",
    "archiveSetAtMs",
    "autoArchiveExempt",
];

impl Session {
    fn transcript_missing(&self, transcripts: Option<&HashSet<String>>) -> bool {
        match (&self.transcript, transcripts) {
            (Some(id), Some(ids)) => !ids.contains(id),
            _ => false,
        }
    }

    /// 从磁盘读整份会话文件（改写、搬「此电脑」时用）。读不了、不是 JSON 对象时为 `None`。
    pub(crate) fn full_doc(&self) -> Option<Value> {
        let doc: Value = serde_json::from_slice(&fs::read(&self.path).ok()?).ok()?;
        doc.is_object().then_some(doc)
    }
}

/// 只取 [`SUMMARY_FIELDS`] 的 JSON 对象；别的字段照样检查语法，但不建出来。
struct Summary(serde_json::Map<String, Value>);

impl<'de> serde::Deserialize<'de> for Summary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = Summary;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Summary, A::Error> {
                let mut fields = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if SUMMARY_FIELDS.contains(&key.as_str()) {
                        let value = map.next_value::<Value>()?;
                        fields.insert(key, value);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(Summary(fields))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

/// 整份会话文件 → 摘要（改写之后更新 [`Session::doc`] 用）。
pub(crate) fn summary_of(doc: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    if let Some(map) = doc.as_object() {
        for key in SUMMARY_FIELDS {
            if let Some(value) = map.get(key) {
                fields.insert(key.to_string(), value.clone());
            }
        }
    }
    Value::Object(fields)
}

/// 运行中的 CC Switch 记着读过的会话文件摘要，按路径认，大小和修改时间都没变就不再读。
/// 每次补齐都是新的一轮，这一轮没见到的文件（删了、挪走了）从缓存里去掉。
#[derive(Default)]
struct SessionCache {
    round: u64,
    files: std::collections::HashMap<PathBuf, Cached>,
}

struct Cached {
    len: u64,
    modified: std::time::SystemTime,
    round: u64,
    session: Session,
}

static CACHE: std::sync::LazyLock<std::sync::Mutex<SessionCache>> =
    std::sync::LazyLock::new(Default::default);

fn cache() -> std::sync::MutexGuard<'static, SessionCache> {
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 一次补齐做了什么。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FillReport {
    /// 补上的会话份数
    pub added: usize,
    pub sync: SyncReport,
}

/// 先同步删除和归档，再把各数据目录里每个账号缺的会话补上。`dirs` 是 (数据目录, 能不能写)。
pub(crate) fn fill(dirs: &[(PathBuf, bool)]) -> Result<FillReport, AppError> {
    let _guard = lock();
    fill_with(dirs, &default_places(true))
}

fn default_places(with_transcripts: bool) -> Places {
    let mut project_dirs = vec![crate::config::get_home_dir()
        .join(".claude")
        .join("projects")];
    let configured = crate::config::get_claude_config_dir().join("projects");
    if !project_dirs.contains(&configured) {
        project_dirs.push(configured);
    }
    let app_dir = crate::config::get_app_config_dir();
    Places {
        seen: app_dir.join(SEEN_FILENAME),
        backup: app_dir.join(BACKUP_DIRNAME),
        trash: app_dir.join(TRASH_DIRNAME),
        transcripts: if with_transcripts {
            transcript_ids(&project_dirs)
        } else {
            None
        },
        projects: project_dirs,
        cache: true,
    }
}

/// 同步开关和回收站里的会话。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSyncStatus {
    pub enabled: bool,
    pub trash: Vec<TrashEntry>,
}

pub(crate) fn sync_status() -> SessionSyncStatus {
    let _guard = lock();
    let places = default_places(false);
    SessionSyncStatus {
        enabled: Seen::load(&places.seen).sync.enabled,
        trash: trash::list(&places.trash),
    }
}

pub(crate) fn set_sync_enabled(enabled: bool) -> Result<(), AppError> {
    let _guard = lock();
    let places = default_places(false);
    let mut seen = Seen::load(&places.seen);
    seen.sync.enabled = enabled;
    seen.save(&places.seen)
}

/// 清空回收站（永久删除里面的一切）。
pub(crate) fn empty_trash() -> Result<(), AppError> {
    let _guard = lock();
    let places = default_places(false);
    trash::empty(&places.trash).map_err(|e| AppError::io(&places.trash, e))
}

/// 从回收站恢复一个会话：挪回去的列表文件、对话记录、工作区；以后不再按恢复之前的删除标记删它，
/// 下次补齐时还会补到别的号里。开着的 Desktop 的目录里的那几份先留在回收站。
pub(crate) fn restore(
    dirs: &[(PathBuf, bool)],
    session_id: &str,
) -> Result<RestoreReport, AppError> {
    let _guard = lock();
    restore_with(dirs, session_id, &default_places(false))
}

fn restore_with(
    dirs: &[(PathBuf, bool)],
    session_id: &str,
    places: &Places,
) -> Result<RestoreReport, AppError> {
    if !is_uuid(session_id) {
        return Err(AppError::Message(format!("不认识的会话 id: {session_id}")));
    }
    let writable = |path: &Path| {
        dirs.iter()
            .all(|(dir, idle)| *idle || !path.starts_with(dir))
    };
    let report = trash::restore(&places.trash, session_id, &writable)
        .map_err(|e| AppError::io(&places.trash, e))?;
    let mut seen = Seen::load(&places.seen);
    seen.sync
        .restored
        .insert(session_id.to_string(), trash::now_ms());
    if let Some(tomb) = seen.sync.deleted.get_mut(session_id) {
        tomb.done = false;
        tomb.cli.clear();
        tomb.workspaces.clear();
    }
    // Desktop 自己的删除标记也去掉（只在没开着的目录），各文件夹「删过」的记录也去掉：下次补齐补回去
    let name = format!("local_{session_id}.json");
    for (root, (dir, idle)) in dirs.iter().enumerate() {
        if !idle {
            continue;
        }
        let round = places.cache.then(|| cache().round);
        for folder in scan(root, &dir.join(SESSIONS_DIRNAME), round)? {
            let _ = fs::remove_file(folder.dir.join(format!("{MARKER_PREFIX}{session_id}")));
        }
    }
    for names in seen.folders.values_mut() {
        names.remove(&name);
    }
    seen.save(&places.seen)?;
    Ok(report)
}

/// `fill` 用到的 CC Switch 自己的文件和对话记录目录。
struct Places {
    seen: PathBuf,
    /// 就地改会话文件前的备份放在这下面的 `<时间>/` 里
    backup: PathBuf,
    /// 同步删除的回收站
    trash: PathBuf,
    /// 还在的对话记录；`None`（分不清）时不按对话记录挡
    transcripts: Option<HashSet<String>>,
    projects: Vec<PathBuf>,
    /// 用 [`CACHE`]（测试里关掉，各测试互不影响）
    cache: bool,
}

/// `fill` 的本体。
fn fill_with(dirs: &[(PathBuf, bool)], places: &Places) -> Result<FillReport, AppError> {
    let seen_path = places.seen.as_path();
    let transcripts = places.transcripts.as_ref();
    let roots: Vec<PathBuf> = dirs.iter().map(|(dir, _)| dir.clone()).collect();
    let round = places.cache.then(|| {
        let mut cache = cache();
        cache.round += 1;
        cache.round
    });
    let mut folders = Vec::new();
    for (root, (dir, _)) in dirs.iter().enumerate() {
        folders.extend(scan(root, &dir.join(SESSIONS_DIRNAME), round)?);
    }
    if let Some(round) = round {
        cache().files.retain(|_, cached| cached.round == round);
    }
    add_signed_in_folders(dirs, &mut folders);
    let mut seen = Seen::load(seen_path);
    let mut backup = Backup::new(&places.backup);

    // 以前原样复制过来的「此电脑」会话：先备份，再就地换成这个数据目录里的工作区
    for folder in folders.iter_mut().filter(|folder| dirs[folder.root].1) {
        for (name, session) in folder.sessions.iter_mut() {
            let Some(place) = foreign_workspace(
                &session.doc,
                &roots,
                folder.root,
                &folder.account,
                &folder.org,
            ) else {
                continue;
            };
            if let Err(error) = backup.save(&session.path, &folder.account, &folder.org, name) {
                log::warn!(
                    "[DesktopSessions] {} 没能备份，不改: {error}",
                    session.path.display()
                );
                continue;
            }
            let Some(full) = session.full_doc() else {
                continue;
            };
            let Some(moved) = relocate(&full, &place, &places.projects) else {
                continue;
            };
            match replace_file(&session.path, &to_bytes(&moved)?) {
                Ok(()) => {
                    log::info!(
                        "[DesktopSessions] {} 换到这个数据目录的「此电脑」",
                        session.path.display()
                    );
                    session.doc = summary_of(&moved);
                }
                Err(error) => log::warn!("[DesktopSessions] {error}"),
            }
        }
    }

    // 别的号里删除、归档的会话同步过来
    let mut report = FillReport::default();
    let mut bin = Trash::new(&places.trash);
    seen.sync.record_markers(&folders);
    if seen.sync.enabled {
        report.sync = apply_deletes(
            &mut folders,
            dirs,
            &mut seen.sync,
            &mut bin,
            &places.projects,
        );
        report.sync.archived = apply_archive(&mut folders, dirs, &mut seen.sync);
    }
    if let Err(error) = bin.finish() {
        log::warn!("[DesktopSessions] 回收站清单没写进去: {error}");
    }

    // 同一个会话在几个文件夹里都有时，取最近活动的那一份
    let mut newest: BTreeMap<&str, &Session> = BTreeMap::new();
    for (name, file) in folders.iter().flat_map(|folder| &folder.sessions) {
        let best = newest.entry(name).or_insert(file);
        if file.last_activity > best.last_activity {
            *best = file;
        }
    }

    let mut added = 0;
    let mut added_archived = Vec::new();
    let mut updates = Vec::new();
    for folder in &folders {
        let key = folder.dir.to_string_lossy().into_owned();
        // 文件夹空着（新建的、被清空的）就不按以前的记录挡，记录也从头来
        let before = if folder.sessions.is_empty() {
            BTreeSet::new()
        } else {
            seen.get(&key)
        };
        let mut now: BTreeSet<String> = folder.sessions.keys().cloned().collect();
        if dirs[folder.root].1 {
            for (name, source) in &newest {
                if folder.sessions.contains_key(*name) {
                    continue;
                }
                let target = folder.dir.join(name);
                if before.contains(*name) {
                    log::info!("[DesktopSessions] 不补 {}：这个号里删过", target.display());
                    continue;
                }
                if seen.sync.blocks_refill(folder, &source.id) {
                    log::info!("[DesktopSessions] 不补 {}：已经删除", target.display());
                    continue;
                }
                if source.transcript_missing(transcripts) {
                    log::info!(
                        "[DesktopSessions] 不补 {}：对话记录已经不在了",
                        target.display()
                    );
                    continue;
                }
                let bytes = match foreign_workspace(
                    &source.doc,
                    &roots,
                    folder.root,
                    &folder.account,
                    &folder.org,
                )
                .and_then(|place| relocate(&source.full_doc()?, &place, &places.projects))
                {
                    Some(moved) => to_bytes(&moved)?,
                    None => fs::read(&source.path).map_err(|e| AppError::io(&source.path, e))?,
                };
                if write_if_absent(&bytes, &folder.dir, name)? {
                    log::info!("[DesktopSessions] 补上 {}", target.display());
                    added += 1;
                    added_archived.push((key.clone(), source.id.clone(), is_archived(&source.doc)));
                }
                now.insert((*name).to_string());
            }
        }
        // 只记加、不记减：删掉的还留在记录里，下次才挡得住
        now.extend(before);
        updates.push((key, now));
    }

    for (key, names) in updates {
        seen.set(key, names);
    }
    record_archived(&folders, &added_archived, &mut seen.sync);
    if let Err(error) = seen.save(seen_path) {
        // 记不下只会让删过的会话下次又被补回来，不影响打开
        log::warn!("[DesktopSessions] 没能记下各文件夹的会话: {error}");
    }
    report.added = added;
    Ok(report)
}

/// 读出一个数据目录里的账号文件夹。名字不是 UUID 的目录、不是 `local_<uuid>.json` 的文件、坏掉的
/// JSON 都跳过，不跟符号链接。`round` 是这一轮补齐的编号：有时用 [`CACHE`]，没变的文件不再读。
fn scan(root: usize, sessions_dir: &Path, round: Option<u64>) -> Result<Vec<Folder>, AppError> {
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for (account, account_dir) in uuid_subdirs(sessions_dir)? {
        for (org, dir) in uuid_subdirs(&account_dir)? {
            let mut markers = BTreeMap::new();
            for entry in fs::read_dir(&dir).map_err(|e| AppError::io(&dir, e))? {
                let entry = entry.map_err(|e| AppError::io(&dir, e))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                if let Some(id) = name.strip_prefix(MARKER_PREFIX).filter(|id| is_uuid(id)) {
                    // 读不出时间的标记按很早以前算：不会把后来用过的副本删掉
                    let at = fs::read_to_string(entry.path())
                        .ok()
                        .and_then(|text| text.trim().parse::<i64>().ok())
                        .unwrap_or(0);
                    markers.insert(id.to_string(), at);
                    continue;
                }
                let Some(id) = name
                    .strip_prefix("local_")
                    .and_then(|rest| rest.strip_suffix(".json"))
                    .filter(|id| is_uuid(id))
                    .map(str::to_string)
                else {
                    continue;
                };
                // Windows 上目录项自带大小和修改时间，不用再打开文件
                let stamp = entry
                    .metadata()
                    .ok()
                    .and_then(|meta| Some((meta.len(), meta.modified().ok()?)));
                files.push(SessionFile {
                    folder: folders.len(),
                    name,
                    id,
                    path: entry.path(),
                    stamp,
                });
            }
            folders.push(Folder {
                root,
                account: account.clone(),
                org,
                dir,
                sessions: BTreeMap::new(),
                markers,
            });
        }
    }
    for (file, session) in read_sessions(&files, round) {
        if let Some(session) = session {
            folders[file.folder]
                .sessions
                .insert(file.name.clone(), session);
        }
    }
    Ok(folders)
}

/// 目录里列出来的一个会话文件。
struct SessionFile {
    /// `scan` 里 `folders` 的下标
    folder: usize,
    name: String,
    id: String,
    path: PathBuf,
    /// 大小和修改时间；读不出时不进缓存
    stamp: Option<(u64, std::time::SystemTime)>,
}

/// 读一批会话文件：缓存里大小和修改时间都对得上的直接用，其余的分几个线程读。
fn read_sessions(
    files: &[SessionFile],
    round: Option<u64>,
) -> Vec<(&SessionFile, Option<Session>)> {
    let mut found: Vec<Option<Session>> = files.iter().map(|_| None).collect();
    let mut misses: Vec<usize> = Vec::new();
    match round {
        Some(round) => {
            let mut cache = cache();
            for (index, file) in files.iter().enumerate() {
                match (cache.files.get_mut(&file.path), file.stamp) {
                    (Some(cached), Some((len, modified)))
                        if cached.len == len && cached.modified == modified =>
                    {
                        cached.round = round;
                        found[index] = Some(cached.session.clone());
                    }
                    _ => misses.push(index),
                }
            }
        }
        None => misses.extend(0..files.len()),
    }

    let read = |index: &usize| {
        let file = &files[*index];
        let modified = file.stamp.map_or(0, |(_, modified)| millis(modified));
        read_session(file.path.clone(), &file.id, modified)
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8);
    let parsed: Vec<Option<Session>> = if threads < 2 || misses.len() < 64 {
        misses.iter().map(read).collect()
    } else {
        let chunk = misses.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let workers: Vec<_> = misses
                .chunks(chunk)
                .map(|part| scope.spawn(move || part.iter().map(read).collect::<Vec<_>>()))
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap_or_default())
                .collect()
        })
    };

    let mut cache = round.map(|round| (round, cache()));
    for (index, session) in misses.into_iter().zip(parsed) {
        if let (Some((round, cache)), Some(session), Some((len, modified))) =
            (cache.as_mut(), &session, files[index].stamp)
        {
            cache.files.insert(
                files[index].path.clone(),
                Cached {
                    len,
                    modified,
                    round: *round,
                    session: session.clone(),
                },
            );
        }
        found[index] = session;
    }
    files.iter().zip(found).collect()
}

fn millis(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// 名字是 UUID 的子目录。目录不在时为空。
fn uuid_subdirs(dir: &Path) -> Result<Vec<(String, PathBuf)>, AppError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AppError::io(dir, error)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| AppError::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && is_uuid(&name) {
            found.push((name, entry.path()));
        }
    }
    Ok(found)
}

/// 读会话文件的摘要；不是 JSON 对象（读不了、坏了）时为 `None`。`modified` 是文件修改时间（毫秒）。
fn read_session(path: PathBuf, id: &str, modified: i64) -> Option<Session> {
    let Summary(doc) = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let remote = ["sshConfig", "wslConfig"]
        .iter()
        .any(|key| doc.get(*key).is_some_and(|value| !value.is_null()));
    let transcript = doc
        .get("cliSessionId")
        .and_then(Value::as_str)
        .filter(|id| !remote && !id.is_empty())
        .map(str::to_string);
    Some(Session {
        id: id.to_string(),
        modified,
        last_activity: doc
            .get("lastActivityAt")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        transcript,
        path,
        doc: Value::Object(doc),
    })
}

/// 各对话记录目录（`projects/<项目>/<id>.jsonl`）里有哪些 id。一个目录都没有、或有项目目录读不了
/// 时为 `None`（分不清就不挡）。
fn transcript_ids(project_dirs: &[PathBuf]) -> Option<HashSet<String>> {
    let mut ids = HashSet::new();
    let mut found = false;
    for projects in project_dirs {
        let entries = match fs::read_dir(projects) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        found = true;
        for project in entries {
            let project = project.ok()?;
            if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            for file in fs::read_dir(project.path()).ok()? {
                let name = file.ok()?.file_name().to_string_lossy().into_owned();
                if let Some(id) = name.strip_suffix(".jsonl") {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    found.then_some(ids)
}

/// 数据目录登的账号还没有会话文件夹（比如刚建的账号目录，Desktop 还没开过 Code 标签页），而别的
/// 目录里有这个账号的：照着那些组织补一个空文件夹，一起填上。这样第一次用账号目录打开 Desktop，
/// 就能看到这个号以前在默认目录里的会话。
fn add_signed_in_folders(dirs: &[(PathBuf, bool)], folders: &mut Vec<Folder>) {
    for (root, (dir, _)) in dirs.iter().enumerate() {
        let Some(account) = signed_in_account(dir) else {
            continue;
        };
        let mut orgs: Vec<String> = folders
            .iter()
            .filter(|folder| folder.account == account && folder.root != root)
            .map(|folder| folder.org.clone())
            .collect();
        orgs.sort();
        orgs.dedup();
        for org in orgs {
            let exists = folders.iter().any(|folder| {
                folder.root == root && folder.account == account && folder.org == org
            });
            if !exists {
                folders.push(Folder {
                    root,
                    dir: dir.join(SESSIONS_DIRNAME).join(&account).join(&org),
                    account: account.clone(),
                    org,
                    sessions: BTreeMap::new(),
                    markers: BTreeMap::new(),
                });
            }
        }
    }
}

fn to_bytes(doc: &Value) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec(doc).map_err(|e| AppError::Message(format!("会话文件序列化失败: {e}")))
}

/// 先写临时文件再改名；目标已经有了就不动（返回 `false`）。
fn write_if_absent(bytes: &[u8], target_dir: &Path, name: &str) -> Result<bool, AppError> {
    let target = target_dir.join(name);
    if target.exists() {
        return Ok(false);
    }
    fs::create_dir_all(target_dir).map_err(|e| AppError::io(target_dir, e))?;
    let temp = target_dir.join(format!(".{name}.ccswitch-tmp"));
    fs::write(&temp, bytes).map_err(|e| AppError::io(&temp, e))?;
    if target.exists() {
        let _ = fs::remove_file(&temp);
        return Ok(false);
    }
    if let Err(error) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(AppError::io(&target, error));
    }
    Ok(true)
}

/// 先写临时文件再改名，换掉已有的文件。
pub(crate) fn replace_file(target: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let temp = target.with_file_name(format!(".{name}.ccswitch-tmp"));
    fs::write(&temp, bytes).map_err(|e| AppError::io(&temp, e))?;
    fs::rename(&temp, target).map_err(|e| {
        let _ = fs::remove_file(&temp);
        AppError::io(target, e)
    })
}

/// 就地改会话文件前的备份：`<备份根>/<时间>/<账号>/<组织>/<文件名>`，一次补齐用同一个时间。
struct Backup<'a> {
    root: &'a Path,
    dir: Option<PathBuf>,
}

impl<'a> Backup<'a> {
    fn new(root: &'a Path) -> Self {
        Self { root, dir: None }
    }

    fn save(
        &mut self,
        source: &Path,
        account: &str,
        org: &str,
        name: &str,
    ) -> Result<(), AppError> {
        let root = self.root;
        let dir = self.dir.get_or_insert_with(|| {
            root.join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string())
        });
        // 两个数据目录里同一个号、同一个组织的同名文件：后来的加序号，不覆盖
        let folder = dir.join(account).join(org);
        fs::create_dir_all(&folder).map_err(|e| AppError::io(&folder, e))?;
        let mut target = folder.join(name);
        let mut n = 1;
        while target.exists() {
            target = folder.join(format!("{name}.{n}"));
            n += 1;
        }
        fs::copy(source, &target).map_err(|e| AppError::io(&target, e))?;
        Ok(())
    }
}

/// CC Switch 自己的记录：`{"version": 2, "folders": {"<文件夹路径>": ["local_<id>.json"]}, "sync": {…}}`。
/// `folders` 是各会话文件夹出现过的会话文件名（version 1 只有这一项）；`sync` 见 `SyncLedger`。
#[derive(Default)]
struct Seen {
    folders: BTreeMap<String, BTreeSet<String>>,
    sync: SyncLedger,
}

impl Seen {
    /// 没有或读不懂就从空的开始（只会让删过的会话再被补回来一次）。
    fn load(path: &Path) -> Self {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => return Self::default(),
        };
        let Ok(mut doc) = serde_json::from_slice::<Value>(&bytes) else {
            log::warn!("[DesktopSessions] {} 读不懂，从头记", path.display());
            return Self::default();
        };
        let sync = doc
            .get_mut("sync")
            .map(Value::take)
            .and_then(|sync| serde_json::from_value(sync).ok())
            .unwrap_or_default();
        let folders = doc
            .get_mut("folders")
            .map(Value::take)
            .and_then(|folders| serde_json::from_value(folders).ok());
        match folders {
            Some(folders) => Self { folders, sync },
            None => {
                log::warn!("[DesktopSessions] {} 读不懂，从头记", path.display());
                Self::default()
            }
        }
    }

    fn get(&self, folder: &str) -> BTreeSet<String> {
        self.folders.get(folder).cloned().unwrap_or_default()
    }

    fn set(&mut self, folder: String, names: BTreeSet<String>) {
        self.folders.insert(folder, names);
    }

    /// 先写临时文件再改名。
    fn save(&self, path: &Path) -> Result<(), AppError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let doc = serde_json::json!({ "version": 2, "folders": self.folders, "sync": self.sync });
        let bytes = serde_json::to_vec_pretty(&doc)
            .map_err(|e| AppError::Message(format!("会话记录序列化失败: {e}")))?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, bytes).map_err(|e| AppError::io(&temp, e))?;
        fs::rename(&temp, path).map_err(|e| {
            let _ = fs::remove_file(&temp);
            AppError::io(path, e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    const ACC_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const ACC_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    const ORG: &str = "11111111-0000-4000-8000-00000000000a";
    const S1: &str = "local_00000000-0000-4000-8000-000000000101.json";
    const S2: &str = "local_00000000-0000-4000-8000-000000000102.json";
    const S3: &str = "local_00000000-0000-4000-8000-000000000103.json";

    fn folder(desktop: &Path, account: &str) -> PathBuf {
        let dir = desktop.join(SESSIONS_DIRNAME).join(account).join(ORG);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session(dir: &Path, name: &str, title: &str, last_activity: i64) {
        let doc = json!({ "title": title, "lastActivityAt": last_activity });
        fs::write(dir.join(name), doc.to_string()).unwrap();
    }

    fn session_with_transcript(dir: &Path, name: &str, transcript: &str) {
        let doc = json!({ "title": name, "lastActivityAt": 1, "cliSessionId": transcript });
        fs::write(dir.join(name), doc.to_string()).unwrap();
    }

    fn title_of(path: &Path) -> String {
        let doc: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        doc["title"].as_str().unwrap().to_string()
    }

    fn places(temp: &TempDir, transcripts: Option<HashSet<String>>) -> Places {
        Places {
            seen: temp.path().join(SEEN_FILENAME),
            backup: temp.path().join(BACKUP_DIRNAME),
            trash: temp.path().join(TRASH_DIRNAME),
            transcripts,
            projects: vec![temp.path().join("projects")],
            // 缓存是全局的，并行的测试会互相清掉对方的；只在专门的测试里开
            cache: false,
        }
    }

    fn fill_in(temp: &TempDir, dirs: &[(PathBuf, bool)]) -> usize {
        fill_with(dirs, &places(temp, None)).unwrap().added
    }

    #[test]
    fn the_summary_keeps_what_sync_reads_and_drops_the_rest() {
        use crate::services::claude_desktop_sync::ARCHIVE_FIELDS;
        for key in ARCHIVE_FIELDS {
            assert!(SUMMARY_FIELDS.contains(&key), "{key}");
        }
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(S1);
        let doc = json!({
            "title": "t", "lastActivityAt": 5, "cliSessionId": "c1", "isArchived": true,
            "promptAppendSnapshot": { "a": [1, 2, 3] }, "remoteMcpServersConfig": "x"
        });
        fs::write(&path, doc.to_string()).unwrap();
        let session = read_session(path.clone(), id_of(S1), 7).unwrap();
        assert_eq!(
            session.doc,
            json!({ "title": "t", "lastActivityAt": 5, "cliSessionId": "c1", "isArchived": true })
        );
        assert_eq!(session.full_doc().unwrap(), doc);
        assert_eq!(summary_of(&doc), session.doc);
        // 坏掉的 JSON（哪怕坏在跳过的字段里）、不是对象的都不认
        fs::write(&path, r#"{"title":"t","promptAppendSnapshot":[1,}"#).unwrap();
        assert!(read_session(path.clone(), id_of(S1), 0).is_none());
        fs::write(&path, "[1]").unwrap();
        assert!(read_session(path, id_of(S1), 0).is_none());
    }

    #[test]
    fn a_changed_session_file_is_read_again() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join("data");
        let a = folder(&data, ACC_A);
        // 够多的文件，走多线程那条路
        let names: Vec<String> = (0..100)
            .map(|i| format!("local_00000000-0000-4000-8000-{i:012}.json"))
            .collect();
        for name in &names {
            session(&a, name, "old", 1);
        }
        let sessions_dir = data.join(SESSIONS_DIRNAME);
        // 和补齐以外的读（恢复）一样，用当前这一轮
        let round = cache().round;
        let title = |round: u64, name: &str| {
            let folders = scan(0, &sessions_dir, Some(round)).unwrap();
            folders[0].sessions[name].doc["title"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            scan(0, &sessions_dir, Some(round)).unwrap()[0]
                .sessions
                .len(),
            100
        );
        assert!(cache().files.contains_key(&a.join(&names[7])));
        assert_eq!(title(round, &names[7]), "old");
        // 内容变了（大小、修改时间跟着变）就重新读
        session(&a, &names[7], "a newer title", 2);
        assert_eq!(title(round, &names[7]), "a newer title");

        // 补齐时这一轮没见到的文件（删掉了）从缓存里去掉
        fs::remove_file(a.join(&names[7])).unwrap();
        let places = Places {
            cache: true,
            ..places(&temp, None)
        };
        fill_with(&[(data.clone(), true)], &places).unwrap();
        assert!(!cache().files.contains_key(&a.join(&names[7])));
        assert!(cache().files.contains_key(&a.join(&names[8])));
    }

    #[test]
    fn fills_gaps_with_the_newest_copy_and_never_overwrites() {
        let temp = TempDir::new().unwrap();
        let a = folder(temp.path(), ACC_A);
        let b = folder(temp.path(), ACC_B);
        session(&a, S1, "one", 100);
        session(&a, S2, "two-old", 200);
        session(&b, S2, "two-new", 300);

        assert_eq!(fill_in(&temp, &[(temp.path().to_path_buf(), true)]), 1);
        assert_eq!(title_of(&b.join(S1)), "one");
        assert_eq!(title_of(&a.join(S2)), "two-old");
    }

    #[test]
    fn a_new_account_folder_is_filled_but_running_folders_are_not() {
        let temp = TempDir::new().unwrap();
        let default_dir = temp.path().join("default");
        let profile_dir = temp.path().join("profile");
        let running_dir = temp.path().join("running");
        session(&folder(&default_dir, ACC_B), S1, "one", 100);
        session(&folder(&running_dir, ACC_A), S2, "two", 200);
        // 给 B 建的账号目录：登好了，还没有会话文件夹
        fs::create_dir_all(&profile_dir).unwrap();
        fs::write(
            profile_dir.join("config.json"),
            json!({ "lastKnownAccountUuid": ACC_B }).to_string(),
        )
        .unwrap();

        fill_in(
            &temp,
            &[
                (default_dir, true),
                (profile_dir.clone(), true),
                (running_dir.clone(), false),
            ],
        );

        let b_new = profile_dir.join(SESSIONS_DIRNAME).join(ACC_B).join(ORG);
        assert_eq!(title_of(&b_new.join(S1)), "one");
        assert_eq!(title_of(&b_new.join(S2)), "two");
        assert!(!running_dir
            .join(SESSIONS_DIRNAME)
            .join(ACC_A)
            .join(ORG)
            .join(S1)
            .exists());
    }

    #[test]
    fn a_session_deleted_in_one_account_is_not_added_back_there() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join("data");
        let a = folder(&data, ACC_A);
        let b = folder(&data, ACC_B);
        session(&a, S1, "one", 100);
        session(&a, S2, "two", 200);
        let dirs = [(data.clone(), true)];

        assert_eq!(fill_in(&temp, &dirs), 2);
        // 在 B 号里删掉 S1；A 号里的 S1 还在，也有一个 B 没见过的新会话 S3
        fs::remove_file(b.join(S1)).unwrap();
        session(&a, S3, "three", 300);

        assert_eq!(fill_in(&temp, &dirs), 1);
        assert!(!b.join(S1).exists());
        assert_eq!(title_of(&b.join(S3)), "three");
        // A 号里的一个不删、不改
        assert_eq!(title_of(&a.join(S1)), "one");
        assert_eq!(title_of(&a.join(S2)), "two");
        // 再来一次也还是不补
        assert_eq!(fill_in(&temp, &dirs), 0);
        assert!(!b.join(S1).exists());
    }

    #[test]
    fn an_emptied_folder_is_filled_again() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join("data");
        let a = folder(&data, ACC_A);
        let b = folder(&data, ACC_B);
        session(&a, S1, "one", 100);
        let dirs = [(data.clone(), true)];
        fill_in(&temp, &dirs);
        fs::remove_file(b.join(S1)).unwrap();

        assert_eq!(fill_in(&temp, &dirs), 1);
        assert!(b.join(S1).exists());
    }

    #[test]
    fn a_session_whose_transcript_is_gone_is_not_copied() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join("data");
        let a = folder(&data, ACC_A);
        let b = folder(&data, ACC_B);
        session_with_transcript(&a, S1, "kept");
        session_with_transcript(&a, S2, "gone");
        session(&a, S3, "never-started", 1);
        let ids: HashSet<String> = ["kept".to_string()].into();

        let added = fill_with(&[(data.clone(), true)], &places(&temp, Some(ids)))
            .unwrap()
            .added;

        assert_eq!(added, 2);
        assert!(b.join(S1).exists());
        assert!(!b.join(S2).exists());
        assert!(b.join(S3).exists());
        // 源文件不动
        assert!(a.join(S2).exists());
    }

    #[test]
    fn transcript_ids_reads_every_project_and_gives_up_when_unsure() {
        let temp = TempDir::new().unwrap();
        let projects = temp.path().join("projects");
        fs::create_dir_all(projects.join("E--work")).unwrap();
        fs::write(projects.join("E--work").join("abc.jsonl"), "").unwrap();
        fs::write(projects.join("E--work").join("notes.txt"), "").unwrap();

        let ids = transcript_ids(&[projects.clone(), temp.path().join("missing")]).unwrap();
        assert_eq!(ids, ["abc".to_string()].into());
        assert!(transcript_ids(&[temp.path().join("missing")]).is_none());
    }

    #[test]
    fn scratch_sessions_land_in_this_pc_of_the_other_data_dir() {
        use crate::services::claude_desktop_scratch::WORKSPACES_DIRNAME;
        let temp = TempDir::new().unwrap();
        let (default_dir, profile_dir) = (temp.path().join("default"), temp.path().join("profile"));
        let a = folder(&default_dir, ACC_A);
        let p = folder(&profile_dir, ACC_A);
        let ws = |leaf: &str| {
            let dir = default_dir
                .join(WORKSPACES_DIRNAME)
                .join(ACC_A)
                .join(ORG)
                .join(leaf);
            fs::create_dir_all(&dir).unwrap();
            dir.to_string_lossy().into_owned()
        };
        let doc = |cwd: String| json!({ "title": "s", "lastActivityAt": 1, "cwd": cwd, "originCwd": cwd, "promptAppendSnapshot": { "text": "big" } });
        fs::write(a.join(S1), doc(ws("scratch-2026-11-12-000001")).to_string()).unwrap();
        // 以前原样复制过去的一份
        let old_copy = doc(ws("scratch-2026-11-12-000002")).to_string();
        fs::write(a.join(S2), &old_copy).unwrap();
        fs::write(p.join(S2), &old_copy).unwrap();
        let dirs = [(default_dir.clone(), true), (profile_dir.clone(), true)];

        assert_eq!(fill_in(&temp, &dirs), 1);

        let mine = profile_dir.join(WORKSPACES_DIRNAME).join(ACC_A).join(ORG);
        for (name, leaf) in [
            (S1, "scratch-2026-11-12-000001"),
            (S2, "scratch-2026-11-12-000002"),
        ] {
            let got: Value = serde_json::from_slice(&fs::read(p.join(name)).unwrap()).unwrap();
            assert_eq!(got["originCwd"], mine.join(leaf).to_string_lossy().as_ref());
            // 摘要里没有的字段照样带过去
            assert_eq!(got["promptAppendSnapshot"]["text"], "big");
            assert!(mine.join(leaf).is_dir());
        }
        // 默认目录里的原件不动；旧副本改之前备份了
        assert_eq!(fs::read_to_string(a.join(S2)).unwrap(), old_copy);
        let backups: Vec<_> = fs::read_dir(temp.path().join(BACKUP_DIRNAME))
            .unwrap()
            .collect();
        assert_eq!(backups.len(), 1);
        let saved = backups[0]
            .as_ref()
            .unwrap()
            .path()
            .join(ACC_A)
            .join(ORG)
            .join(S2);
        assert_eq!(fs::read_to_string(saved).unwrap(), old_copy);
        // 再来一次什么都不变
        assert_eq!(fill_in(&temp, &dirs), 0);
        assert_eq!(
            fs::read_dir(temp.path().join(BACKUP_DIRNAME))
                .unwrap()
                .count(),
            1
        );
    }

    fn id_of(name: &str) -> &str {
        name.strip_prefix("local_")
            .unwrap()
            .strip_suffix(".json")
            .unwrap()
    }

    fn mark_deleted(dir: &Path, name: &str, at: i64) {
        fs::write(
            dir.join(format!("{MARKER_PREFIX}{}", id_of(name))),
            at.to_string(),
        )
        .unwrap();
    }

    fn write_doc(dir: &Path, name: &str, doc: &Value, modified_ms: u64) {
        let path = dir.join(name);
        fs::write(&path, doc.to_string()).unwrap();
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(modified_ms);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    fn doc_of(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn a_deletion_in_one_account_moves_the_other_copies_to_the_trash() {
        let temp = TempDir::new().unwrap();
        let (one, two, three) = (
            temp.path().join("one"),
            temp.path().join("two"),
            temp.path().join("three"),
        );
        let a = folder(&one, ACC_A);
        let b = folder(&two, ACC_A);
        let c = folder(&three, ACC_A);
        // 在 A 里删了 S1（Desktop 留下标记）；B 里的副本删除前就没再用过，C 的 Desktop 开着
        mark_deleted(&a, S1, 1_000);
        let s1 = json!({ "title": "s1", "lastActivityAt": 500, "cliSessionId": "cli-1" });
        fs::write(b.join(S1), s1.to_string()).unwrap();
        fs::write(c.join(S1), s1.to_string()).unwrap();
        // S2 在 A 里删了，但在 B 里删除之后还用过
        mark_deleted(&a, S2, 1_000);
        session(&b, S2, "s2", 2_000);
        // 对话记录
        let projects = temp.path().join("projects").join("E--work");
        fs::create_dir_all(&projects).unwrap();
        fs::write(projects.join("cli-1.jsonl"), "{}").unwrap();

        let dirs = [
            (one.clone(), true),
            (two.clone(), true),
            (three.clone(), false),
        ];
        let report = fill_with(&dirs, &places(&temp, None)).unwrap();

        assert_eq!(report.sync.trashed, 1);
        assert_eq!(report.sync.kept, 1);
        assert!(!b.join(S1).exists());
        assert!(b.join(format!("{MARKER_PREFIX}{}", id_of(S1))).exists());
        // 开着的不动；S1 不补回任何地方，留着的 S2 也不补回 A
        assert!(c.join(S1).exists());
        assert!(!a.join(S1).exists() && !a.join(S2).exists());
        assert!(b.join(S2).exists());
        // 还有一份（C 里）用着，对话记录不动
        assert!(projects.join("cli-1.jsonl").exists());
        let listed = trash::list(&temp.path().join(TRASH_DIRNAME));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "s1");

        // C 的 Desktop 关了：它那份也挪走，再没有会话用到，对话记录一起挪走
        let dirs = [(one, true), (two, true), (three, true)];
        let report = fill_with(&dirs, &places(&temp, None)).unwrap();
        assert_eq!(report.sync.trashed, 1);
        assert!(!c.join(S1).exists());
        assert!(!projects.join("cli-1.jsonl").exists());
        let listed = trash::list(&temp.path().join(TRASH_DIRNAME));
        assert_eq!((listed[0].copies, listed[0].transcripts), (2, 1));
    }

    #[test]
    fn a_transcript_still_used_by_another_session_stays() {
        let temp = TempDir::new().unwrap();
        let (one, two) = (temp.path().join("one"), temp.path().join("two"));
        let a = folder(&one, ACC_A);
        let b = folder(&two, ACC_A);
        mark_deleted(&a, S1, 1_000);
        let s1 = json!({ "title": "s1", "lastActivityAt": 1, "cliSessionId": "shared" });
        fs::write(b.join(S1), s1.to_string()).unwrap();
        // 另一个会话（分支出来的）也记着这份对话记录
        let s3 = json!({ "title": "s3", "lastActivityAt": 1, "priorCliSessionIds": ["shared"] });
        fs::write(b.join(S3), s3.to_string()).unwrap();
        let projects = temp.path().join("projects").join("p");
        fs::create_dir_all(&projects).unwrap();
        fs::write(projects.join("shared.jsonl"), "{}").unwrap();

        fill_with(&[(one, true), (two, true)], &places(&temp, None)).unwrap();
        assert!(!b.join(S1).exists());
        assert!(projects.join("shared.jsonl").exists());
    }

    #[test]
    fn a_restored_session_comes_back_everywhere() {
        let temp = TempDir::new().unwrap();
        let (one, two) = (temp.path().join("one"), temp.path().join("two"));
        let a = folder(&one, ACC_A);
        let b = folder(&two, ACC_A);
        session(&a, S1, "s1", 1);
        let dirs = [(one.clone(), true), (two.clone(), true)];
        fill_with(&dirs, &places(&temp, None)).unwrap();
        assert!(b.join(S1).exists());
        // 在 A 里删掉（Desktop 删文件、写标记）
        fs::remove_file(a.join(S1)).unwrap();
        mark_deleted(&a, S1, 1_000);
        fill_with(&dirs, &places(&temp, None)).unwrap();
        assert!(!b.join(S1).exists());

        let report = restore_with(&dirs, id_of(S1), &places(&temp, None)).unwrap();
        assert_eq!(report.restored, 1);
        assert!(b.join(S1).exists());
        assert!(!a.join(format!("{MARKER_PREFIX}{}", id_of(S1))).exists());
        // 下次补齐补回 A，也不会再被删
        fill_with(&dirs, &places(&temp, None)).unwrap();
        assert!(a.join(S1).exists() && b.join(S1).exists());
        assert!(trash::list(&temp.path().join(TRASH_DIRNAME)).is_empty());
    }

    #[test]
    fn archiving_follows_the_folder_that_changed() {
        let temp = TempDir::new().unwrap();
        let (one, two) = (temp.path().join("one"), temp.path().join("two"));
        let a = folder(&one, ACC_A);
        let b = folder(&two, ACC_A);
        let live = json!({ "title": "s", "lastActivityAt": 1, "isArchived": false });
        write_doc(&a, S1, &live, 10_000);
        write_doc(&b, S1, &live, 10_000);
        fs::write(b.join("archived-sessions.idx"), r#"{"v":1,"archived":[]}"#).unwrap();
        let dirs = [(one.clone(), true), (two.clone(), true)];
        fill_with(&dirs, &places(&temp, None)).unwrap();

        // 在 A 里归档
        let archived = json!({ "title": "s", "lastActivityAt": 1, "isArchived": true, "archiveSetAtMs": 20_000 });
        write_doc(&a, S1, &archived, 20_000);
        let mut b_doc = live.clone();
        b_doc["promptAppendSnapshot"] = json!("kept");
        write_doc(&b, S1, &b_doc, 10_000);
        let report = fill_with(&dirs, &places(&temp, None)).unwrap();
        assert_eq!(report.sync.archived, 1);
        let got = doc_of(&b.join(S1));
        assert_eq!(got["isArchived"], true);
        // 改写的是整份文件，不是摘要
        assert_eq!(got["promptAppendSnapshot"], "kept");
        assert_eq!(got["archiveSetAtMs"], 20_000);
        let idx = doc_of(&b.join("archived-sessions.idx"));
        assert_eq!(idx["archived"], json!([format!("local_{}", id_of(S1))]));

        // 再在 B 里取消归档：A 跟着取消
        write_doc(&b, S1, &live, 30_000);
        let report = fill_with(&dirs, &places(&temp, None)).unwrap();
        assert_eq!(report.sync.archived, 1);
        let got = doc_of(&a.join(S1));
        assert_eq!(got["isArchived"], false);
        assert!(got.get("archiveSetAtMs").is_none());
        // 没有变化时什么都不做
        assert_eq!(
            fill_with(&dirs, &places(&temp, None))
                .unwrap()
                .sync
                .archived,
            0
        );
    }

    #[test]
    fn deleting_a_this_pc_session_elsewhere_trashes_its_workspace_and_restores_it() {
        use crate::services::claude_desktop_scratch::WORKSPACES_DIRNAME;
        let temp = TempDir::new().unwrap();
        let (owner, other) = (temp.path().join("owner"), temp.path().join("other"));
        let a = folder(&owner, ACC_A);
        let b = folder(&other, ACC_A);
        let ws = owner
            .join(WORKSPACES_DIRNAME)
            .join(ACC_A)
            .join(ORG)
            .join("scratch-2026-11-25-000009");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("notes.md"), "keep me").unwrap();
        let cwd = ws.to_string_lossy().into_owned();
        let doc = json!({ "title": "pc", "lastActivityAt": 1, "cwd": cwd, "originCwd": cwd });
        fs::write(a.join(S1), doc.to_string()).unwrap();
        let dirs = [(owner.clone(), true), (other.clone(), true)];
        fill_with(&dirs, &places(&temp, None)).unwrap();
        let link = other
            .join(WORKSPACES_DIRNAME)
            .join(ACC_A)
            .join(ORG)
            .join("scratch-2026-11-25-000009");
        assert!(b.join(S1).exists() && link.join("notes.md").exists());

        // 在 B 里删除：Desktop 删列表文件、删链接（只是链接）、写标记
        fs::remove_file(b.join(S1)).unwrap();
        crate::services::claude_desktop_scratch::remove_link(&link).unwrap();
        mark_deleted(&b, S1, 1_000);
        fill_with(&dirs, &places(&temp, None)).unwrap();
        assert!(!a.join(S1).exists());
        assert!(!ws.exists(), "没人用的工作区挪进回收站");
        let listed = trash::list(&temp.path().join(TRASH_DIRNAME));
        assert_eq!(listed.len(), 1);

        let report = restore_with(&dirs, id_of(S1), &places(&temp, None)).unwrap();
        assert_eq!(report.restored, 1);
        assert_eq!(fs::read_to_string(ws.join("notes.md")).unwrap(), "keep me");
        assert!(a.join(S1).exists());
        // 下次补齐 B 又有了，链接重新建上
        fill_with(&dirs, &places(&temp, None)).unwrap();
        assert!(b.join(S1).exists() && link.join("notes.md").exists());
    }

    #[test]
    fn with_sync_off_only_the_folder_that_deleted_is_left_alone() {
        let temp = TempDir::new().unwrap();
        let (one, two) = (temp.path().join("one"), temp.path().join("two"));
        let a = folder(&one, ACC_A);
        let b = folder(&two, ACC_A);
        let mut seen = Seen::default();
        seen.sync.enabled = false;
        seen.save(&temp.path().join(SEEN_FILENAME)).unwrap();
        mark_deleted(&a, S1, 1_000);
        session(&b, S1, "s1", 1);

        let report = fill_with(&[(one, true), (two, true)], &places(&temp, None)).unwrap();
        assert_eq!(report.sync.trashed, 0);
        assert!(b.join(S1).exists());
        assert!(!a.join(S1).exists());
    }

    #[test]
    fn an_unreadable_seen_file_starts_over() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(SEEN_FILENAME);
        fs::write(&path, "not json").unwrap();
        assert!(Seen::load(&path).folders.is_empty());

        let mut seen = Seen::default();
        seen.set("x".into(), [S1.to_string()].into());
        seen.save(&path).unwrap();
        assert_eq!(Seen::load(&path).get("x"), [S1.to_string()].into());
    }
}

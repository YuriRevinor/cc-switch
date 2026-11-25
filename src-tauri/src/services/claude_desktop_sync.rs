//! 多账号同步删除、同步归档 Claude Desktop 的 Code 会话（`claude_desktop_sessions::fill` 里调用）。
//!
//! **删除**：Desktop 删会话时在那个账号文件夹里写 `deleted_<会话 id>`（内容是删除时间，毫秒；手动删、
//! 批量清理、让 Claude 删都会写）。CC Switch 按这些标记，把别的账号文件夹里同一个会话挪进回收站
//! （`claude_desktop_trash`），并替 Desktop 在那里也写一个标记：
//! - 那份副本在删除之后还用过（`lastActivityAt` 比删除时间晚）就留着；
//! - 正在运行的 Desktop 的目录不动，等它下次关着的时候再处理（标记一直记在 CC Switch 自己的记录里）；
//! - 所有副本都处理完、再没有会话用到时，把这些副本带出来的对话记录、「此电脑」工作区也挪进回收站。
//!   只挪 CC Switch 自己挪走的副本里写着的那些；还有别的会话用到的对话记录不动；
//! - 恢复过（见 `claude_desktop_sessions::restore`）就不按恢复之前的标记删。
//!
//! **归档**：真正的状态是会话文件里的 `isArchived`（连同 `archiveSetAtMs`、`autoArchiveExempt`）。
//! 记下每个文件夹上次看到的状态；某个文件夹里变了，就把变化（以文件修改时间最晚的那份为准）套到
//! 其余副本上，同时更新那个文件夹的 `archived-sessions.idx`（Desktop 加载时用的提示）。那份副本在
//! 变化之后还用过就不改。
//!
//! 开关（默认开）关掉时，删除只挡「这个文件夹里删过的不补回来」，归档不同步。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::services::claude_desktop_scratch::{remove_link, WORKSPACES_DIRNAME};
use crate::services::claude_desktop_sessions::{replace_file, Folder};
use crate::services::claude_desktop_trash::{Kind, Trash};

pub(crate) const MARKER_PREFIX: &str = "deleted_";
const ARCHIVE_INDEX: &str = "archived-sessions.idx";
const ARCHIVE_FIELDS: [&str; 3] = ["isArchived", "archiveSetAtMs", "autoArchiveExempt"];

/// CC Switch 记的同步状态（存在 `claude-desktop-sessions-seen.json` 的 `sync` 里）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncLedger {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// 会话 id → 删除记录
    #[serde(default)]
    pub deleted: BTreeMap<String, Tomb>,
    /// 会话 id → 恢复时间（毫秒）：早于它的删除记录不再算数
    #[serde(default)]
    pub restored: BTreeMap<String, i64>,
    /// 会话 id → 要同步过去的归档状态
    #[serde(default)]
    pub archive: BTreeMap<String, ArchiveIntent>,
    /// 文件夹 → (会话 id → 上次看到的 `isArchived`)
    #[serde(default)]
    pub archived_seen: BTreeMap<String, BTreeMap<String, bool>>,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for SyncLedger {
    fn default() -> Self {
        Self {
            enabled: true,
            deleted: BTreeMap::new(),
            restored: BTreeMap::new(),
            archive: BTreeMap::new(),
            archived_seen: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tomb {
    /// 删除时间（毫秒），几个标记取最晚的
    pub at: i64,
    #[serde(default)]
    pub title: String,
    /// CC Switch 挪走的副本里写着的对话记录 id
    #[serde(default)]
    pub cli: BTreeSet<String>,
    /// CC Switch 挪走的副本用的「此电脑」工作区
    #[serde(default)]
    pub workspaces: BTreeSet<String>,
    /// 对话记录和工作区处理过了
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveIntent {
    pub archived: bool,
    /// 变化发生的时间（那份文件的修改时间，毫秒）
    pub at: i64,
    /// `isArchived` / `archiveSetAtMs` / `autoArchiveExempt` 的值；没有的字段不在里面
    #[serde(default)]
    pub fields: Map<String, Value>,
}

impl SyncLedger {
    /// 这个会话现在算删了：给出删除时间。
    pub(crate) fn deleted_at(&self, id: &str) -> Option<i64> {
        let tomb = self.deleted.get(id)?;
        let restored = self.restored.get(id).copied().unwrap_or(i64::MIN);
        (tomb.at > restored).then_some(tomb.at)
    }

    /// 补齐时要不要跳过这个会话。开着同步：哪里删过都跳过；关着：只看这个文件夹自己的标记。
    pub(crate) fn blocks_refill(&self, folder: &Folder, id: &str) -> bool {
        if self.enabled {
            return self.deleted_at(id).is_some();
        }
        let restored = self.restored.get(id).copied().unwrap_or(i64::MIN);
        folder.markers.get(id).is_some_and(|at| *at > restored)
    }

    /// 把各文件夹里 Desktop 写的删除标记记下来（Desktop 以后清掉标记也还算数）。
    pub(crate) fn record_markers(&mut self, folders: &[Folder]) {
        for folder in folders {
            for (id, at) in &folder.markers {
                let tomb = self.deleted.entry(id.clone()).or_default();
                if *at > tomb.at {
                    tomb.at = *at;
                    tomb.done = false;
                }
            }
        }
    }
}

/// 一次同步做了什么。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyncReport {
    /// 挪进回收站的会话列表文件份数
    pub trashed: usize,
    /// 删除之后还用过、留着的份数
    pub kept: usize,
    /// 改了归档状态的份数
    pub archived: usize,
}

/// 会话文件里所有对话记录 id。
pub(crate) fn cli_ids(doc: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for key in ["cliSessionId", "unarchivedCliSessionId"] {
        if let Some(id) = doc.get(key).and_then(Value::as_str) {
            ids.insert(id.to_string());
        }
    }
    for key in ["priorCliSessionIds", "overflowCliSessionIds"] {
        if let Some(list) = doc.get(key).and_then(Value::as_array) {
            ids.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    ids.retain(|id| {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    ids
}

fn title_of(doc: &Value) -> String {
    doc.get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn origin_of(doc: &Value) -> Option<&str> {
    doc.get("originCwd")
        .and_then(Value::as_str)
        .filter(|origin| !origin.is_empty())
        .or_else(|| doc.get("cwd").and_then(Value::as_str))
}

fn is_scratch(path: &str) -> bool {
    path.to_ascii_lowercase().contains(WORKSPACES_DIRNAME)
}

/// 按删除标记把各文件夹里的副本挪进回收站，再处理没人用的对话记录和工作区。
pub(crate) fn apply_deletes(
    folders: &mut [Folder],
    dirs: &[(PathBuf, bool)],
    ledger: &mut SyncLedger,
    trash: &mut Trash,
    projects: &[PathBuf],
) -> SyncReport {
    let mut report = SyncReport::default();
    for folder in folders.iter_mut() {
        if !dirs[folder.root].1 {
            continue;
        }
        let doomed: Vec<(String, String, i64)> = folder
            .sessions
            .iter()
            .filter_map(|(name, session)| {
                let at = ledger.deleted_at(&session.id)?;
                Some((name.clone(), session.id.clone(), at))
            })
            .collect();
        for (name, id, at) in doomed {
            let session = &folder.sessions[&name];
            if session.last_activity > at {
                log::info!(
                    "[DesktopSessions] 留着 {}：在别处删除之后这里还用过",
                    session.path.display()
                );
                report.kept += 1;
                continue;
            }
            let title = title_of(&session.doc);
            let path = session.path.clone();
            let doc = session.doc.clone();
            // 这个数据目录里 CC Switch 建的「此电脑」链接：删链接（只是链接），记下来恢复时重建
            let link = origin_of(&doc)
                .filter(|origin| is_scratch(origin))
                .map(PathBuf::from)
                .filter(|origin| origin.starts_with(&dirs[folder.root].0))
                .filter(|origin| {
                    fs::symlink_metadata(origin).is_ok_and(|meta| meta.file_type().is_symlink())
                });
            if let Err(error) = trash.move_in(&id, &title, Kind::Session, &path) {
                log::warn!(
                    "[DesktopSessions] {} 没能挪进回收站: {error}",
                    path.display()
                );
                continue;
            }
            if let Some(link) = link {
                let target = fs::canonicalize(&link).ok();
                match remove_link(&link) {
                    Ok(()) => {
                        let _ = trash.note(&id, &title, Kind::Link, &link, target.as_deref());
                    }
                    Err(error) => log::info!(
                        "[DesktopSessions] 目录链接 {} 没删掉: {error}",
                        link.display()
                    ),
                }
            }
            let marker = folder.dir.join(format!("{MARKER_PREFIX}{id}"));
            if fs::symlink_metadata(&marker).is_err() && fs::write(&marker, at.to_string()).is_ok()
            {
                let _ = trash.note(&id, &title, Kind::Marker, &marker, None);
            }
            log::info!("[DesktopSessions] 同步删除 {}", path.display());
            let tomb = ledger.deleted.entry(id.clone()).or_default();
            tomb.cli.extend(cli_ids(&doc));
            if let Some(origin) = origin_of(&doc).filter(|origin| is_scratch(origin)) {
                tomb.workspaces.insert(origin.to_string());
            }
            if tomb.title.is_empty() {
                tomb.title = title;
            }
            tomb.done = false;
            folder.sessions.remove(&name);
            report.trashed += 1;
        }
    }
    release_leftovers(folders, dirs, ledger, trash, projects);
    report
}

/// 删掉的会话再没有副本时，把它们带出来的对话记录、工作区挪进回收站。
fn release_leftovers(
    folders: &[Folder],
    dirs: &[(PathBuf, bool)],
    ledger: &mut SyncLedger,
    trash: &mut Trash,
    projects: &[PathBuf],
) {
    let pending: Vec<String> = ledger
        .deleted
        .iter()
        .filter(|(id, tomb)| !tomb.done && ledger.deleted_at(id).is_some())
        .map(|(id, _)| id.clone())
        .collect();
    if pending.is_empty() {
        return;
    }
    let remaining: HashSet<&str> = folders
        .iter()
        .flat_map(|folder| folder.sessions.values())
        .map(|session| session.id.as_str())
        .collect();
    let remaining_cli: HashSet<String> = folders
        .iter()
        .flat_map(|folder| folder.sessions.values())
        .flat_map(|session| cli_ids(&session.doc))
        .collect();
    let mut remaining_ws: Option<HashSet<PathBuf>> = None;
    let mut transcripts: Option<HashMap<String, Vec<PathBuf>>> = None;

    for id in pending {
        if remaining.contains(id.as_str()) {
            continue;
        }
        let tomb = ledger.deleted.get(&id).cloned().unwrap_or_default();
        let mut ok = true;
        let files = transcripts.get_or_insert_with(|| transcript_files(projects));
        for cli in tomb.cli.iter().filter(|cli| !remaining_cli.contains(*cli)) {
            for path in files.get(cli).into_iter().flatten() {
                if fs::symlink_metadata(path).is_err() {
                    continue;
                }
                if let Err(error) = trash.move_in(&id, &tomb.title, Kind::Transcript, path) {
                    log::warn!(
                        "[DesktopSessions] 对话记录 {} 没挪走: {error}",
                        path.display()
                    );
                    ok = false;
                }
            }
        }
        for workspace in &tomb.workspaces {
            let Ok(real) = fs::canonicalize(workspace) else {
                continue;
            };
            let in_use = remaining_ws
                .get_or_insert_with(|| workspaces_in_use(folders))
                .contains(&real);
            if in_use || !real.is_dir() {
                continue;
            }
            let real = plain(&real);
            let owned_by_idle = dirs.iter().any(|(dir, idle)| {
                *idle
                    && fs::canonicalize(dir.join(WORKSPACES_DIRNAME))
                        .is_ok_and(|root| real.starts_with(plain(&root)))
            });
            if !owned_by_idle {
                continue;
            }
            if let Err(error) = trash.move_in(&id, &tomb.title, Kind::Workspace, &real) {
                log::warn!(
                    "[DesktopSessions] 工作区 {} 没挪走: {error}",
                    real.display()
                );
                ok = false;
            }
        }
        if ok {
            if let Some(tomb) = ledger.deleted.get_mut(&id) {
                tomb.done = true;
            }
        }
    }
}

fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// 剩下的会话用着的「此电脑」工作区（解析过链接的真实路径）。
fn workspaces_in_use(folders: &[Folder]) -> HashSet<PathBuf> {
    folders
        .iter()
        .flat_map(|folder| folder.sessions.values())
        .filter_map(|session| origin_of(&session.doc).filter(|o| is_scratch(o)))
        .filter_map(|origin| fs::canonicalize(origin).ok())
        .collect()
}

/// 对话记录 id → 各 projects 目录里的 `<id>.jsonl` 和 `<id>/`。
fn transcript_files(projects: &[PathBuf]) -> HashMap<String, Vec<PathBuf>> {
    let mut files: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for root in projects {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for project in entries.flatten() {
            if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(items) = fs::read_dir(project.path()) else {
                continue;
            };
            for item in items.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                let is_dir = item.file_type().is_ok_and(|kind| kind.is_dir());
                let id = match name.strip_suffix(".jsonl") {
                    Some(id) => id.to_string(),
                    None if is_dir => name,
                    None => continue,
                };
                files.entry(id).or_default().push(item.path());
            }
        }
    }
    files
}

fn archived(doc: &Value) -> bool {
    doc.get("isArchived").and_then(Value::as_bool) == Some(true)
}

pub(crate) fn is_archived(doc: &Value) -> bool {
    archived(doc)
}

/// 同步归档状态，返回改了几份。
pub(crate) fn apply_archive(
    folders: &mut [Folder],
    dirs: &[(PathBuf, bool)],
    ledger: &mut SyncLedger,
) -> usize {
    let mut copies: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    for (index, folder) in folders.iter().enumerate() {
        for (name, session) in &folder.sessions {
            copies
                .entry(session.id.clone())
                .or_default()
                .push((index, name.clone()));
        }
    }
    let mut changed_count = 0;
    for (id, places) in copies {
        if places.len() < 2 {
            continue;
        }
        // 上次之后哪几份变了：取修改时间最晚的
        let changed = places
            .iter()
            .filter_map(|(index, name)| {
                let folder = &folders[*index];
                let session = &folder.sessions[name];
                let key = folder.dir.to_string_lossy();
                let was = ledger.archived_seen.get(key.as_ref())?.get(&id)?;
                (*was != archived(&session.doc)).then_some(session)
            })
            .max_by_key(|session| session.modified);
        if let Some(winner) = changed {
            let newer = ledger
                .archive
                .get(&id)
                .is_none_or(|intent| intent.at < winner.modified);
            if newer {
                let fields = ARCHIVE_FIELDS
                    .iter()
                    .filter_map(|key| Some((key.to_string(), winner.doc.get(*key)?.clone())))
                    .collect();
                ledger.archive.insert(
                    id.clone(),
                    ArchiveIntent {
                        archived: archived(&winner.doc),
                        at: winner.modified,
                        fields,
                    },
                );
            }
        }
        let Some(intent) = ledger.archive.get(&id).cloned() else {
            continue;
        };
        for (index, name) in &places {
            let folder = &mut folders[*index];
            if !dirs[folder.root].1 {
                continue;
            }
            let dir = folder.dir.clone();
            let session = folder.sessions.get_mut(name).expect("刚列出来的");
            if archived(&session.doc) == intent.archived || session.last_activity > intent.at {
                continue;
            }
            let mut doc = session.doc.clone();
            if let Some(map) = doc.as_object_mut() {
                for key in ARCHIVE_FIELDS {
                    match intent.fields.get(key) {
                        Some(value) => {
                            map.insert(key.to_string(), value.clone());
                        }
                        None => {
                            map.remove(key);
                        }
                    }
                }
                map.insert("isArchived".into(), Value::Bool(intent.archived));
            }
            let Ok(bytes) = serde_json::to_vec(&doc) else {
                continue;
            };
            match replace_file(&session.path, &bytes) {
                Ok(()) => {
                    log::info!(
                        "[DesktopSessions] 同步{} {}",
                        if intent.archived {
                            "归档"
                        } else {
                            "取消归档"
                        },
                        session.path.display()
                    );
                    session.doc = doc;
                    update_archive_index(&dir, name, intent.archived);
                    changed_count += 1;
                }
                Err(error) => log::warn!("[DesktopSessions] {error}"),
            }
        }
    }
    changed_count
}

/// 记下各文件夹现在的归档状态（补齐写进去的新副本也算）。
pub(crate) fn record_archived(
    folders: &[Folder],
    added: &[(String, String, bool)],
    ledger: &mut SyncLedger,
) {
    let mut seen: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    for folder in folders {
        let entry = seen
            .entry(folder.dir.to_string_lossy().into_owned())
            .or_default();
        for session in folder.sessions.values() {
            entry.insert(session.id.clone(), archived(&session.doc));
        }
    }
    for (folder, id, value) in added {
        seen.entry(folder.clone())
            .or_default()
            .insert(id.clone(), *value);
    }
    ledger.archived_seen = seen;
}

/// `archived-sessions.idx`：`{"v":1,"archived":["local_<id>",…]}`。读不懂或没有就不动（Desktop 会不用它）。
fn update_archive_index(dir: &Path, file_name: &str, archived: bool) {
    let path = dir.join(ARCHIVE_INDEX);
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let Ok(mut doc) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    let Some(list) = doc.get_mut("archived").and_then(Value::as_array_mut) else {
        return;
    };
    let id = file_name.strip_suffix(".json").unwrap_or(file_name);
    let mut ids: BTreeSet<String> = list
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let changed = if archived {
        ids.insert(id.to_string())
    } else {
        ids.remove(id)
    };
    if !changed {
        return;
    }
    *list = ids.into_iter().map(Value::String).collect();
    if let Ok(bytes) = serde_json::to_vec(&doc) {
        if let Err(error) = replace_file(&path, &bytes) {
            log::info!("[DesktopSessions] {ARCHIVE_INDEX} 没更新: {error}");
        }
    }
}

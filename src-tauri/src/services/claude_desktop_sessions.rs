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

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::AppError;
use crate::services::claude_desktop_account::{is_uuid, signed_in_account};
use crate::services::claude_desktop_scratch::{foreign_workspace, relocate};

const SESSIONS_DIRNAME: &str = "claude-code-sessions";
const SEEN_FILENAME: &str = "claude-desktop-sessions-seen.json";
const BACKUP_DIRNAME: &str = "claude-desktop-sessions-backup";

/// `<数据目录>/claude-code-sessions/<accountUuid>/<orgUuid>/`
struct Folder {
    /// 所在数据目录（`dirs` 里的下标）
    root: usize,
    account: String,
    org: String,
    dir: PathBuf,
    /// 文件名 → 会话文件
    sessions: BTreeMap<String, Session>,
}

/// 一个会话文件里用得到的几项。
struct Session {
    path: PathBuf,
    /// `lastActivityAt`（毫秒），没有时为 0
    last_activity: i64,
    /// 本机对话记录的文件名（不带 `.jsonl`）。远程会话（SSH / WSL）、没有 `cliSessionId` 的为 `None`，
    /// 不按对话记录挡。
    transcript: Option<String>,
    doc: Value,
}

impl Session {
    fn transcript_missing(&self, transcripts: Option<&HashSet<String>>) -> bool {
        match (&self.transcript, transcripts) {
            (Some(id), Some(ids)) => !ids.contains(id),
            _ => false,
        }
    }
}

/// 把各数据目录里每个账号缺的会话补上，返回补了几个。`dirs` 是 (数据目录, 能不能写)。
pub(crate) fn fill(dirs: &[(PathBuf, bool)]) -> Result<usize, AppError> {
    let mut project_dirs = vec![crate::config::get_home_dir()
        .join(".claude")
        .join("projects")];
    let configured = crate::config::get_claude_config_dir().join("projects");
    if !project_dirs.contains(&configured) {
        project_dirs.push(configured);
    }
    let app_dir = crate::config::get_app_config_dir();
    let places = Places {
        seen: app_dir.join(SEEN_FILENAME),
        backup: app_dir.join(BACKUP_DIRNAME),
        transcripts: transcript_ids(&project_dirs),
        projects: project_dirs,
    };
    fill_with(dirs, &places)
}

/// `fill` 用到的 CC Switch 自己的文件和对话记录目录。
struct Places {
    seen: PathBuf,
    /// 就地改会话文件前的备份放在这下面的 `<时间>/` 里
    backup: PathBuf,
    /// 还在的对话记录；`None`（分不清）时不按对话记录挡
    transcripts: Option<HashSet<String>>,
    projects: Vec<PathBuf>,
}

/// `fill` 的本体。
fn fill_with(dirs: &[(PathBuf, bool)], places: &Places) -> Result<usize, AppError> {
    let seen_path = places.seen.as_path();
    let transcripts = places.transcripts.as_ref();
    let roots: Vec<PathBuf> = dirs.iter().map(|(dir, _)| dir.clone()).collect();
    let mut folders = Vec::new();
    for (root, (dir, _)) in dirs.iter().enumerate() {
        folders.extend(scan(root, &dir.join(SESSIONS_DIRNAME))?);
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
            let Some(moved) = relocate(&session.doc, &place, &places.projects) else {
                continue;
            };
            match replace_file(&session.path, &to_bytes(&moved)?) {
                Ok(()) => {
                    log::info!(
                        "[DesktopSessions] {} 换到这个数据目录的「此电脑」",
                        session.path.display()
                    );
                    session.doc = moved;
                }
                Err(error) => log::warn!("[DesktopSessions] {error}"),
            }
        }
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
                .and_then(|place| relocate(&source.doc, &place, &places.projects))
                {
                    Some(moved) => to_bytes(&moved)?,
                    None => fs::read(&source.path).map_err(|e| AppError::io(&source.path, e))?,
                };
                if write_if_absent(&bytes, &folder.dir, name)? {
                    log::info!("[DesktopSessions] 补上 {}", target.display());
                    added += 1;
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
    if let Err(error) = seen.save(seen_path) {
        // 记不下只会让删过的会话下次又被补回来，不影响打开
        log::warn!("[DesktopSessions] 没能记下各文件夹的会话: {error}");
    }
    Ok(added)
}

/// 读出一个数据目录里的账号文件夹。名字不是 UUID 的目录、不是 `local_<uuid>.json` 的文件、坏掉的
/// JSON 都跳过，不跟符号链接。
fn scan(root: usize, sessions_dir: &Path) -> Result<Vec<Folder>, AppError> {
    let mut folders = Vec::new();
    for (account, account_dir) in uuid_subdirs(sessions_dir)? {
        for (org, dir) in uuid_subdirs(&account_dir)? {
            let mut sessions = BTreeMap::new();
            for entry in fs::read_dir(&dir).map_err(|e| AppError::io(&dir, e))? {
                let entry = entry.map_err(|e| AppError::io(&dir, e))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_session = entry.file_type().is_ok_and(|kind| kind.is_file())
                    && name
                        .strip_prefix("local_")
                        .and_then(|rest| rest.strip_suffix(".json"))
                        .is_some_and(is_uuid);
                if !is_session {
                    continue;
                }
                if let Some(session) = read_session(entry.path()) {
                    sessions.insert(name, session);
                }
            }
            folders.push(Folder {
                root,
                account: account.clone(),
                org,
                dir,
                sessions,
            });
        }
    }
    Ok(folders)
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

/// 读会话文件；不是 JSON 对象（读不了、坏了）时为 `None`。
fn read_session(path: PathBuf) -> Option<Session> {
    let doc: Value = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let doc = doc.as_object()?;
    let remote = ["sshConfig", "wslConfig"]
        .iter()
        .any(|key| doc.get(*key).is_some_and(|value| !value.is_null()));
    let transcript = doc
        .get("cliSessionId")
        .and_then(Value::as_str)
        .filter(|id| !remote && !id.is_empty())
        .map(str::to_string);
    Some(Session {
        last_activity: doc
            .get("lastActivityAt")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        transcript,
        path,
        doc: Value::Object(doc.clone()),
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
fn replace_file(target: &Path, bytes: &[u8]) -> Result<(), AppError> {
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

/// 各会话文件夹出现过的会话文件名：`{"version": 1, "folders": {"<文件夹路径>": ["local_<id>.json"]}}`。
#[derive(Default)]
struct Seen {
    folders: BTreeMap<String, BTreeSet<String>>,
}

impl Seen {
    /// 没有或读不懂就从空的开始（只会让删过的会话再被补回来一次）。
    fn load(path: &Path) -> Self {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => return Self::default(),
        };
        let folders = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|mut doc| doc.get_mut("folders").map(Value::take))
            .and_then(|folders| serde_json::from_value(folders).ok());
        match folders {
            Some(folders) => Self { folders },
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
        let doc = serde_json::json!({ "version": 1, "folders": self.folders });
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
            transcripts,
            projects: vec![temp.path().join("projects")],
        }
    }

    fn fill_in(temp: &TempDir, dirs: &[(PathBuf, bool)]) -> usize {
        fill_with(dirs, &places(temp, None)).unwrap()
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

        let added = fill_with(&[(data.clone(), true)], &places(&temp, Some(ids))).unwrap();

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
        let doc = |cwd: String| json!({ "title": "s", "lastActivityAt": 1, "cwd": cwd, "originCwd": cwd });
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

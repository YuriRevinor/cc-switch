//! Claude Desktop「Code」标签页的会话列表：换号后也能看到全部会话。
//!
//! 对话内容在 `~/.claude/projects/`（和账号无关），侧栏列表却按账号分开存在各数据目录的
//! `claude-code-sessions/<accountUuid>/<orgUuid>/local_<id>.json` 里，换了号就只列这个号的。
//! 切到一个号、打开 Desktop 之前（见 `claude_desktop_account::open`），把每个账号文件夹缺的列表
//! 文件从别处复制过去：只补缺，不覆盖、不删除，复制了哪些写进日志；正在运行的 Desktop 的目录不写
//! （它会拿内存里的列表覆盖回去）。Desktop 的登录不读不写。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::AppError;
use crate::services::claude_desktop_account::{is_uuid, signed_in_account};

const SESSIONS_DIRNAME: &str = "claude-code-sessions";

/// `<数据目录>/claude-code-sessions/<accountUuid>/<orgUuid>/`
struct Folder {
    /// 所在数据目录（`dirs` 里的下标）
    root: usize,
    account: String,
    org: String,
    dir: PathBuf,
    /// 文件名 → (路径, `lastActivityAt`)
    sessions: BTreeMap<String, (PathBuf, i64)>,
}

/// 把各数据目录里每个账号缺的会话补上，返回补了几个。`dirs` 是 (数据目录, 能不能写)。
pub(crate) fn fill(dirs: &[(PathBuf, bool)]) -> Result<usize, AppError> {
    let mut folders = Vec::new();
    for (root, (dir, _)) in dirs.iter().enumerate() {
        folders.extend(scan(root, &dir.join(SESSIONS_DIRNAME))?);
    }
    add_signed_in_folders(dirs, &mut folders);

    // 同一个会话在几个文件夹里都有时，取最近活动的那一份
    let mut newest: BTreeMap<&str, &(PathBuf, i64)> = BTreeMap::new();
    for (name, file) in folders.iter().flat_map(|folder| &folder.sessions) {
        let best = newest.entry(name).or_insert(file);
        if file.1 > best.1 {
            *best = file;
        }
    }

    let mut added = 0;
    for folder in folders.iter().filter(|folder| dirs[folder.root].1) {
        for (name, (source, _)) in &newest {
            if !folder.sessions.contains_key(*name) && copy_if_absent(source, &folder.dir, name)? {
                log::info!("[DesktopSessions] 补上 {}", folder.dir.join(name).display());
                added += 1;
            }
        }
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
                if let Some(last_activity) = last_activity(&entry.path()) {
                    sessions.insert(name, (entry.path(), last_activity));
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

/// 会话文件的 `lastActivityAt`（毫秒），没有时为 0；不是 JSON 对象（读不了、坏了）时为 `None`。
fn last_activity(path: &Path) -> Option<i64> {
    let doc: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    doc.is_object().then(|| {
        doc.get("lastActivityAt")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    })
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

/// 先写临时文件再改名；目标已经有了就不动（返回 `false`）。
fn copy_if_absent(source: &Path, target_dir: &Path, name: &str) -> Result<bool, AppError> {
    let target = target_dir.join(name);
    if target.exists() {
        return Ok(false);
    }
    let bytes = fs::read(source).map_err(|e| AppError::io(source, e))?;
    fs::create_dir_all(target_dir).map_err(|e| AppError::io(target_dir, e))?;
    let temp = target_dir.join(format!(".{name}.ccswitch-tmp"));
    fs::write(&temp, &bytes).map_err(|e| AppError::io(&temp, e))?;
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

    fn folder(desktop: &Path, account: &str) -> PathBuf {
        let dir = desktop.join(SESSIONS_DIRNAME).join(account).join(ORG);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session(dir: &Path, name: &str, title: &str, last_activity: i64) {
        let doc = json!({ "title": title, "lastActivityAt": last_activity });
        fs::write(dir.join(name), doc.to_string()).unwrap();
    }

    fn title_of(path: &Path) -> String {
        let doc: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        doc["title"].as_str().unwrap().to_string()
    }

    #[test]
    fn fills_gaps_with_the_newest_copy_and_never_overwrites() {
        let temp = TempDir::new().unwrap();
        let a = folder(temp.path(), ACC_A);
        let b = folder(temp.path(), ACC_B);
        session(&a, S1, "one", 100);
        session(&a, S2, "two-old", 200);
        session(&b, S2, "two-new", 300);

        assert_eq!(fill(&[(temp.path().to_path_buf(), true)]).unwrap(), 1);
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

        fill(&[
            (default_dir, true),
            (profile_dir.clone(), true),
            (running_dir.clone(), false),
        ])
        .unwrap();

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
}

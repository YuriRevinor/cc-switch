//! 同步删除会话时用的回收站：`~/.cc-switch/claude-desktop-sessions-trash/<批次>/`。
//!
//! 每次同步是一个批次，里面放挪进来的文件和 `manifest.json`（每一项从哪来、放在哪）。从不自动清空；
//! 恢复时按会话把它的各项挪回原处（原处已经有东西的不覆盖，留在回收站里）。
//!
//! 挪的是：会话列表文件、对话记录（`<id>.jsonl` 和 `<id>/`）、没人再用的「此电脑」工作区。另外记两种
//! 不占地方的项：删掉的目录链接（恢复时重建）、替 Desktop 写的删除标记 `deleted_<id>`（恢复时删掉）。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Kind {
    /// `local_<id>.json`
    Session,
    /// `<cliSessionId>.jsonl` 或 `<cliSessionId>/`
    Transcript,
    /// 「此电脑」的真实工作区目录
    Workspace,
    /// 指向工作区的目录链接（只记，不挪）
    Link,
    /// 替 Desktop 写的 `deleted_<id>`（只记）
    Marker,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    /// 会话 id（`local_<id>.json` 里的 `<id>`）
    session: String,
    #[serde(default)]
    title: String,
    kind: Kind,
    /// 原来的位置
    from: String,
    /// 批次目录里的文件名；只记不挪的项为 `None`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stored: Option<String>,
    /// 目录链接指向哪里
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    v: u32,
    /// 批次时间（毫秒）
    at: i64,
    items: Vec<Item>,
}

/// 一次同步里往回收站放东西。第一次放的时候才建批次目录，`finish` 写清单。
pub(crate) struct Trash {
    root: PathBuf,
    batch: Option<(PathBuf, Manifest)>,
}

impl Trash {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            batch: None,
        }
    }

    fn batch(&mut self) -> io::Result<&mut (PathBuf, Manifest)> {
        if self.batch.is_none() {
            let at = now_ms();
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
            let mut dir = self.root.join(&stamp);
            let mut n = 1;
            while dir.exists() {
                dir = self.root.join(format!("{stamp}-{n}"));
                n += 1;
            }
            fs::create_dir_all(&dir)?;
            self.batch = Some((
                dir,
                Manifest {
                    v: 1,
                    at,
                    items: Vec::new(),
                },
            ));
        }
        Ok(self.batch.as_mut().expect("刚建好"))
    }

    /// 把 `path` 挪进回收站。同一个分区直接改名；跨分区时文件复制后删原件，目录不挪（报错）。
    pub(crate) fn move_in(
        &mut self,
        session: &str,
        title: &str,
        kind: Kind,
        path: &Path,
    ) -> io::Result<()> {
        let (dir, manifest) = self.batch()?;
        let base = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".into());
        let stored = format!("{:04}-{base}", manifest.items.len());
        let target = dir.join(&stored);
        move_path(path, &target)?;
        manifest.items.push(Item {
            session: session.into(),
            title: title.into(),
            kind,
            from: path.to_string_lossy().into_owned(),
            stored: Some(stored),
            target: None,
        });
        Ok(())
    }

    /// 记一项不挪的（目录链接、替 Desktop 写的删除标记）。
    pub(crate) fn note(
        &mut self,
        session: &str,
        title: &str,
        kind: Kind,
        from: &Path,
        target: Option<&Path>,
    ) -> io::Result<()> {
        let (_, manifest) = self.batch()?;
        manifest.items.push(Item {
            session: session.into(),
            title: title.into(),
            kind,
            from: from.to_string_lossy().into_owned(),
            stored: None,
            target: target.map(|t| t.to_string_lossy().into_owned()),
        });
        Ok(())
    }

    /// 写清单。这一次什么都没放就什么都不做。
    pub(crate) fn finish(self) -> io::Result<usize> {
        let Some((dir, manifest)) = self.batch else {
            return Ok(0);
        };
        let count = manifest
            .items
            .iter()
            .filter(|item| item.kind == Kind::Session)
            .count();
        write_manifest(&dir, &manifest)?;
        Ok(count)
    }
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    let temp = dir.join(format!("{MANIFEST}.tmp"));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, dir.join(MANIFEST))
}

fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) => {
            let meta = fs::symlink_metadata(from)?;
            if !meta.is_file() {
                return Err(error);
            }
            fs::copy(from, to)?;
            fs::remove_file(from)
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 回收站里的一个会话（各批次合在一起）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    pub session_id: String,
    pub title: String,
    /// 最近一次挪进来的时间（毫秒）
    pub trashed_at: i64,
    /// 挪进来的会话列表文件份数（几个账号文件夹里的）
    pub copies: usize,
    /// 有没有对话记录
    pub transcripts: usize,
}

fn batches(root: &Path) -> Vec<(PathBuf, Manifest)> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Ok(bytes) = fs::read(dir.join(MANIFEST)) else {
            continue;
        };
        if let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) {
            found.push((dir, manifest));
        }
    }
    found.sort_by_key(|(_, manifest)| manifest.at);
    found
}

/// 回收站里的会话，最近的在前。
pub(crate) fn list(root: &Path) -> Vec<TrashEntry> {
    let mut entries: BTreeMap<String, TrashEntry> = BTreeMap::new();
    for (_, manifest) in batches(root) {
        for item in &manifest.items {
            let entry = entries
                .entry(item.session.clone())
                .or_insert_with(|| TrashEntry {
                    session_id: item.session.clone(),
                    title: String::new(),
                    trashed_at: 0,
                    copies: 0,
                    transcripts: 0,
                });
            if !item.title.is_empty() {
                entry.title = item.title.clone();
            }
            entry.trashed_at = entry.trashed_at.max(manifest.at);
            match item.kind {
                Kind::Session => entry.copies += 1,
                Kind::Transcript => entry.transcripts += 1,
                _ => {}
            }
        }
    }
    let mut list: Vec<TrashEntry> = entries
        .into_values()
        .filter(|entry| entry.copies > 0 || entry.transcripts > 0)
        .collect();
    list.sort_by_key(|entry| std::cmp::Reverse(entry.trashed_at));
    list
}

/// 恢复的结果。
#[derive(Debug, Default, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    /// 挪回去的会话列表文件份数
    pub restored: usize,
    /// 因为对应的 Desktop 开着、或原处已经有文件，留在回收站里的份数
    pub waiting: usize,
}

/// 把一个会话的各项挪回原处。`writable(path)`：这个位置现在能不能写（那个 Desktop 没开着）。
pub(crate) fn restore(
    root: &Path,
    session: &str,
    writable: &dyn Fn(&Path) -> bool,
) -> io::Result<RestoreReport> {
    let mut report = RestoreReport::default();
    for (dir, mut manifest) in batches(root) {
        if !manifest.items.iter().any(|item| item.session == session) {
            continue;
        }
        // 先恢复工作区和对话记录，再恢复链接和列表文件
        let mut order: Vec<usize> = (0..manifest.items.len())
            .filter(|&i| manifest.items[i].session == session)
            .collect();
        order.sort_by_key(|&i| match manifest.items[i].kind {
            Kind::Workspace => 0,
            Kind::Transcript => 1,
            Kind::Link => 2,
            Kind::Marker => 3,
            Kind::Session => 4,
        });
        let mut done = vec![false; manifest.items.len()];
        for i in order {
            let item = &manifest.items[i];
            let from = PathBuf::from(&item.from);
            let ok = match item.kind {
                Kind::Marker => {
                    if writable(&from) {
                        let _ = fs::remove_file(&from);
                        true
                    } else {
                        false
                    }
                }
                Kind::Link => {
                    if !writable(&from) {
                        false
                    } else if fs::symlink_metadata(&from).is_ok() {
                        true
                    } else {
                        let target = item.target.as_deref().map(PathBuf::from);
                        match target {
                            Some(target) => {
                                crate::services::claude_desktop_scratch::ensure_link(&from, &target)
                                    .is_ok()
                            }
                            None => true,
                        }
                    }
                }
                _ => {
                    let stored = dir.join(item.stored.as_deref().unwrap_or_default());
                    let movable = item.kind != Kind::Session || writable(&from);
                    if movable && fs::symlink_metadata(&from).is_err() {
                        if let Some(parent) = from.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        move_path(&stored, &from).is_ok()
                    } else {
                        false
                    }
                }
            };
            if item.kind == Kind::Session {
                if ok {
                    report.restored += 1;
                } else {
                    report.waiting += 1;
                }
            }
            done[i] = ok;
        }
        let mut index = 0;
        manifest.items.retain(|_| {
            let keep = !done[index];
            index += 1;
            keep
        });
        if manifest.items.is_empty() {
            let _ = fs::remove_dir_all(&dir);
        } else {
            write_manifest(&dir, &manifest)?;
        }
    }
    Ok(report)
}

/// 清空回收站（永久删除）。
pub(crate) fn empty(root: &Path) -> io::Result<()> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn moved_items_are_listed_and_restored() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("trash");
        let folder = temp.path().join("folder");
        fs::create_dir_all(&folder).unwrap();
        let session = folder.join("local_x.json");
        let marker = folder.join("deleted_x");
        let transcript = temp.path().join("t").join("c.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(&session, "{}").unwrap();
        fs::write(&transcript, "line").unwrap();

        let mut trash = Trash::new(&root);
        trash.move_in("x", "标题", Kind::Session, &session).unwrap();
        trash
            .move_in("x", "", Kind::Transcript, &transcript)
            .unwrap();
        fs::write(&marker, "1").unwrap();
        trash.note("x", "", Kind::Marker, &marker, None).unwrap();
        assert_eq!(trash.finish().unwrap(), 1);
        assert!(!session.exists() && !transcript.exists());

        let listed = list(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "标题");
        assert_eq!((listed[0].copies, listed[0].transcripts), (1, 1));

        // 列表文件那边的 Desktop 开着：只恢复对话记录，列表文件和标记留着
        let busy = restore(&root, "x", &|path| !path.starts_with(&folder)).unwrap();
        assert_eq!(
            busy,
            RestoreReport {
                restored: 0,
                waiting: 1
            }
        );
        assert!(transcript.exists() && !session.exists() && marker.exists());

        let report = restore(&root, "x", &|_| true).unwrap();
        assert_eq!(report.restored, 1);
        assert!(session.exists() && !marker.exists());
        assert!(list(&root).is_empty());
    }

    #[test]
    fn restore_never_overwrites_and_empty_removes_everything() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("trash");
        let file = temp.path().join("local_y.json");
        fs::write(&file, "old").unwrap();
        let mut trash = Trash::new(&root);
        trash.move_in("y", "", Kind::Session, &file).unwrap();
        trash.finish().unwrap();
        fs::write(&file, "new").unwrap();

        let report = restore(&root, "y", &|_| true).unwrap();
        assert_eq!(report.waiting, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(list(&root).len(), 1);

        empty(&root).unwrap();
        assert!(list(&root).is_empty());
        assert!(!root.exists());
    }
}

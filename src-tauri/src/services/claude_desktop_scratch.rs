//! 「此电脑」（无文件夹）会话换号后仍显示成「此电脑」。
//!
//! Desktop 给没选文件夹的会话在自己的数据目录里建一个临时工作区
//! `<数据目录>/scratch-workspaces/<accountUuid>/<orgUuid>/scratch-<日期>-<6 位>`，会话的 `originCwd`
//! 就是它。侧栏只在 `originCwd` 落在**当前**数据目录的 `scratch-workspaces` 下时显示「此电脑」，否则
//! 显示文件夹名。所以把这种会话复制进另一个数据目录 D 时（见 `claude_desktop_sessions`）：
//!
//! 1. 在 D 的 `scratch-workspaces/<D 里这个号>/<组织>/` 下建一个同名的**目录链接**（Windows 上是
//!    junction，别处是符号链接），指向原来的工作区：两边看到、改到的是同一批文件。
//! 2. 对话记录按新路径换了项目目录（`~/.claude/projects/<路径转成的名字>/`），把原来的
//!    `<id>.jsonl`（含 `priorCliSessionIds` 和 `<id>/` 子目录）**硬链接**过去：同一份内容，不多占空间。
//!    建不了硬链接时复制一份。已有的文件不覆盖。
//! 3. 会话文件里以旧工作区开头的路径改成新路径；`*realPath*` 字段不改（目录链接解析出来就是原目录）。
//!
//! 已经按旧办法复制过来的会话（`originCwd` 还指着别的数据目录）也就地这样改，改之前先把原文件备份到
//! `~/.cc-switch/claude-desktop-sessions-backup/<时间>/`。
//!
//! 安全上的依据（Desktop 6.28899 的实现）：删会话时 `fs.rm(工作区, {recursive})` 碰到链接只删链接本身；
//! 定期清理只扫真目录，跳过链接；删对话记录时遇到硬链接数不为 1 的文件不删。原来的工作区和对话记录
//! 这里都不改不删。任何一步做不成就照旧原样复制（显示成文件夹名，但能打开）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::services::claude_desktop_account::is_uuid;

pub(crate) const WORKSPACES_DIRNAME: &str = "scratch-workspaces";

/// 会话文件里的旧工作区换到目标数据目录后的样子。
#[derive(Debug, PartialEq)]
pub(crate) struct Move {
    /// 会话文件里旧工作区的原写法
    old: String,
    /// 目标数据目录里的新工作区
    new: PathBuf,
}

/// 这个会话是不是别的数据目录的「此电脑」会话：是就给出要换成的新工作区。`roots` 是所有已知数据目录，
/// `target` 是要放进去的那个（`roots` 里的下标），`account`/`org` 是目标会话文件夹的。
pub(crate) fn foreign_workspace(
    doc: &Value,
    roots: &[PathBuf],
    target: usize,
    account: &str,
    org: &str,
) -> Option<Move> {
    let origin = doc
        .get("originCwd")
        .and_then(Value::as_str)
        .filter(|origin| !origin.is_empty())
        .or_else(|| doc.get("cwd").and_then(Value::as_str))?;
    for (index, root) in roots.iter().enumerate() {
        let prefix = root.join(WORKSPACES_DIRNAME);
        let Some(rest) = strip_path_prefix(origin, &prefix.to_string_lossy()) else {
            continue;
        };
        let parts: Vec<&str> = rest.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
        let [a, b, leaf, ..] = parts.as_slice() else {
            continue;
        };
        if !is_uuid(a) || !is_uuid(b) || !leaf.starts_with("scratch-") || leaf.contains("..") {
            continue;
        }
        // 就在目标数据目录里（哪个号的都行）：Desktop 本来就显示成「此电脑」
        if index == target {
            return None;
        }
        let start = origin.len() - rest.len();
        let leaf_at = rest.find(leaf)?;
        return Some(Move {
            old: origin[..start + leaf_at + leaf.len()].to_string(),
            new: roots[target]
                .join(WORKSPACES_DIRNAME)
                .join(account)
                .join(org)
                .join(leaf),
        });
    }
    None
}

/// 把会话搬过去：建目录链接、硬链接对话记录、改会话文件里的路径。返回改好的会话文件；做不成时为
/// `None`（调用方照旧原样复制，或不动已有的文件）。
pub(crate) fn relocate(doc: &Value, place: &Move, project_dirs: &[PathBuf]) -> Option<Value> {
    if let Err(error) = ensure_link(&place.new, Path::new(&place.old)) {
        log::info!(
            "[DesktopSessions] 没能在 {} 建目录链接，照旧复制: {error}",
            place.new.display()
        );
        return None;
    }
    let new_root = place.new.to_string_lossy().into_owned();
    let mut moved = doc.clone();
    rewrite(&mut moved, None, &place.old, &new_root);
    if let (Some(old_cwd), Some(new_cwd)) = (
        doc.get("cwd").and_then(Value::as_str),
        moved.get("cwd").and_then(Value::as_str),
    ) {
        link_transcripts(doc, old_cwd, new_cwd, project_dirs);
    }
    Some(moved)
}

/// 新工作区已经是指向原工作区的链接（或者就是原工作区）时什么都不做；不在时建链接。原工作区已经不在
/// 了（被 Desktop 清理掉）就建一个空目录，会话还能打开。
fn ensure_link(link: &Path, original: &Path) -> io::Result<()> {
    match fs::symlink_metadata(link) {
        Ok(_) => {
            let same = matches!(
                (fs::canonicalize(link), fs::canonicalize(original)),
                (Ok(a), Ok(b)) if a == b
            );
            if same {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "那里已经有别的东西",
                ))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = link.parent() {
                fs::create_dir_all(parent)?;
            }
            match fs::canonicalize(original) {
                Ok(target) if target.is_dir() => link_dir(link, &plain(&target)),
                _ => fs::create_dir(link),
            }
        }
        Err(error) => Err(error),
    }
}

/// 去掉 `canonicalize` 在 Windows 上加的 `\\?\`。
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

#[cfg(windows)]
fn link_dir(link: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (link_text, target_text) = (link.to_string_lossy(), target.to_string_lossy());
    // cmd 会展开 %…%；引号不能出现在路径里
    if [&link_text, &target_text]
        .iter()
        .any(|text| text.contains(['"', '%']))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "路径里有 cmd 处理不了的字符",
        ));
    }
    // junction 不需要管理员权限或开发者模式
    let output = std::process::Command::new("cmd")
        .raw_arg(format!(r#"/D /C mklink /J "{link_text}" "{target_text}""#))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .output()?;
    if output.status.success() && fs::symlink_metadata(link).is_ok() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "mklink /J 失败（{}）",
            output.status
        )))
    }
}

#[cfg(not(windows))]
fn link_dir(link: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// 把所有以 `old` 开头的路径字符串换成以 `new` 开头。名字里带 `realpath` 的字段不改。
fn rewrite(value: &mut Value, key: Option<&str>, old: &str, new: &str) {
    if key.is_some_and(|key| key.to_ascii_lowercase().contains("realpath")) {
        return;
    }
    match value {
        Value::String(text) => {
            if let Some(rest) = strip_path_prefix(text, old) {
                let sep = if new.contains('\\') { '\\' } else { '/' };
                let rest = rest.trim_start_matches(['\\', '/']);
                *text = if rest.is_empty() {
                    new.to_string()
                } else {
                    format!("{new}{sep}{}", rest.replace(['\\', '/'], &sep.to_string()))
                };
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite(item, None, old, new);
            }
        }
        Value::Object(map) => {
            for (name, item) in map.iter_mut() {
                rewrite(item, Some(name), old, new);
            }
        }
        _ => {}
    }
}

/// `path` 以目录 `prefix` 开头（不分大小写、`\` 和 `/` 算一样，要在分隔符处断开）时给出剩下的部分。
fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches(is_sep);
    let mut chars = path.chars();
    let mut end = 0;
    for want in prefix.chars() {
        let got = chars.next()?;
        if !(got.eq_ignore_ascii_case(&want) || (is_sep(got) && is_sep(want))) {
            return None;
        }
        end += got.len_utf8();
    }
    let rest = &path[end..];
    (rest.is_empty() || rest.starts_with(is_sep)).then_some(rest)
}

fn is_sep(c: char) -> bool {
    matches!(c, '\\' | '/')
}

/// 把会话的对话记录从旧路径的项目目录硬链接到新路径的项目目录。
fn link_transcripts(doc: &Value, old_cwd: &str, new_cwd: &str, project_dirs: &[PathBuf]) {
    let (old_slug, new_slug) = (project_slug(old_cwd), project_slug(new_cwd));
    if old_slug == new_slug {
        return;
    }
    let mut ids: Vec<&str> = doc
        .get("cliSessionId")
        .and_then(Value::as_str)
        .into_iter()
        .collect();
    if let Some(prior) = doc.get("priorCliSessionIds").and_then(Value::as_array) {
        ids.extend(prior.iter().filter_map(Value::as_str));
    }
    // 只认 Claude Code 自己会起的名字，防止 `..` 之类的跑出项目目录
    ids.retain(|id| {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    for projects in project_dirs {
        let (from, to) = (projects.join(&old_slug), projects.join(&new_slug));
        for id in &ids {
            for name in [format!("{id}.jsonl"), id.to_string()] {
                let source = from.join(&name);
                if fs::symlink_metadata(&source).is_err() {
                    continue;
                }
                if let Err(error) = link_tree(&source, &to.join(&name)) {
                    log::info!(
                        "[DesktopSessions] 对话记录 {} 没能链过去: {error}",
                        source.display()
                    );
                }
            }
        }
    }
}

/// 文件硬链接（不行就复制），目录逐层照做；已有的不覆盖，符号链接跳过。
fn link_tree(source: &Path, target: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            link_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !meta.is_file() || fs::symlink_metadata(target).is_ok() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::hard_link(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => {
            let temp = target.with_extension("ccswitch-tmp");
            fs::copy(source, &temp)?;
            if fs::symlink_metadata(target).is_ok() {
                let _ = fs::remove_file(&temp);
                return Ok(());
            }
            fs::rename(&temp, target).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })
        }
    }
}

/// Claude Code 给工作目录起的项目目录名：非字母数字都换成 `-`；超过 200 个字符时截到 200，再加
/// `-<原路径的 32 位哈希，36 进制>`。
pub(crate) fn project_slug(path: &str) -> String {
    let units: Vec<u16> = path.encode_utf16().collect();
    let slug: String = units
        .iter()
        .map(|&unit| {
            let c = char::from_u32(u32::from(unit)).unwrap_or('-');
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect();
    if slug.len() <= 200 {
        return slug;
    }
    let mut hash: i32 = 0;
    for &unit in &units {
        hash = (hash << 5).wrapping_sub(hash).wrapping_add(i32::from(unit));
    }
    format!("{}-{}", &slug[..200], base36(hash.unsigned_abs()))
}

fn base36(mut n: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    const ACC: &str = "6b5d7bf5-a90b-4e38-adcb-f873d2ad5195";
    const ORG: &str = "57f9ef1e-de96-443e-b7f8-39628389fe4b";

    #[test]
    fn slug_matches_claude_code_including_the_long_path_hash() {
        assert_eq!(project_slug(r"E:\Data\Local"), "E--Data-Local");
        let long = format!(
            r"C:\Users\YuriRevinor\.cc-switch-claude-desktop-accounts\{ACC}\scratch-workspaces\{ACC}\{ORG}\scratch-2026-11-25-e99646"
        );
        assert_eq!(
            project_slug(&long),
            format!(
                "C--Users-YuriRevinor--cc-switch-claude-desktop-accounts-{ACC}-scratch-workspaces-{ACC}-{ORG}-scratch-2026-1-j3hcd8"
            )
        );
    }

    #[test]
    fn path_prefix_respects_separators_and_case() {
        assert_eq!(strip_path_prefix(r"C:\A\b\c", "c:/a/B"), Some(r"\c"));
        assert_eq!(strip_path_prefix(r"C:\A\b", r"C:\A\b\"), Some(""));
        assert_eq!(strip_path_prefix(r"C:\A\bc", r"C:\A\b"), None);
    }

    fn scratch_doc(root: &Path, leaf: &str) -> Value {
        let ws = root
            .join(WORKSPACES_DIRNAME)
            .join(ACC)
            .join(ORG)
            .join(leaf)
            .to_string_lossy()
            .into_owned();
        json!({
            "cwd": ws,
            "originCwd": ws,
            "folderPlace": { "cwd": ws, "realPath": ws },
            "gitAnchorsFolderRealpath": ws,
            "shellFolders": [format!("{ws}{}sub", std::path::MAIN_SEPARATOR)],
            "cliSessionId": "cli-1",
            "priorCliSessionIds": ["cli-0"],
            "title": "t",
        })
    }

    #[test]
    fn a_foreign_scratch_session_gets_a_link_and_its_transcripts() {
        let temp = TempDir::new().unwrap();
        let (a, d) = (temp.path().join("a"), temp.path().join("d"));
        let projects = temp.path().join("projects");
        let doc = scratch_doc(&a, "scratch-2026-11-12-71ff96");
        let old_ws = PathBuf::from(doc["cwd"].as_str().unwrap());
        fs::create_dir_all(&old_ws).unwrap();
        fs::write(old_ws.join("notes.md"), "hi").unwrap();
        let old_proj = projects.join(project_slug(doc["cwd"].as_str().unwrap()));
        fs::create_dir_all(old_proj.join("cli-1").join("subagents")).unwrap();
        fs::write(old_proj.join("cli-1.jsonl"), "{}\n").unwrap();
        fs::write(old_proj.join("cli-0.jsonl"), "{}\n").unwrap();
        fs::write(old_proj.join("cli-1").join("subagents").join("x.jsonl"), "").unwrap();

        let roots = [a.clone(), d.clone()];
        assert_eq!(foreign_workspace(&doc, &roots, 0, ACC, ORG), None);
        let place = foreign_workspace(&doc, &roots, 1, ACC, ORG).unwrap();
        let moved = relocate(&doc, &place, std::slice::from_ref(&projects)).unwrap();

        let new_ws = d
            .join(WORKSPACES_DIRNAME)
            .join(ACC)
            .join(ORG)
            .join("scratch-2026-11-12-71ff96");
        let new_text = new_ws.to_string_lossy().into_owned();
        assert_eq!(moved["cwd"], new_text.as_str());
        assert_eq!(moved["originCwd"], new_text.as_str());
        assert_eq!(moved["folderPlace"]["cwd"], new_text.as_str());
        assert_eq!(moved["folderPlace"]["realPath"], doc["cwd"]);
        assert_eq!(moved["gitAnchorsFolderRealpath"], doc["cwd"]);
        assert!(moved["shellFolders"][0].as_str().unwrap().ends_with("sub"));
        assert!(moved["shellFolders"][0]
            .as_str()
            .unwrap()
            .starts_with(&new_text));
        // 链接后看到的是同一批文件
        assert_eq!(fs::read_to_string(new_ws.join("notes.md")).unwrap(), "hi");
        fs::write(new_ws.join("new.md"), "x").unwrap();
        assert!(old_ws.join("new.md").exists());
        let new_proj = projects.join(project_slug(&new_text));
        assert!(new_proj.join("cli-1.jsonl").exists());
        assert!(new_proj.join("cli-0.jsonl").exists());
        assert!(new_proj
            .join("cli-1")
            .join("subagents")
            .join("x.jsonl")
            .exists());

        // 再来一次：链接已经在，照样成功，不覆盖
        assert!(relocate(&doc, &place, std::slice::from_ref(&projects)).is_some());
        // 删掉链接不影响原目录（Desktop 删会话时就是这样）
        #[cfg(windows)]
        fs::remove_dir(&new_ws).unwrap();
        #[cfg(not(windows))]
        fs::remove_file(&new_ws).unwrap();
        assert!(old_ws.join("notes.md").exists());
    }

    #[test]
    fn something_else_at_the_link_path_means_plain_copy() {
        let temp = TempDir::new().unwrap();
        let (a, d) = (temp.path().join("a"), temp.path().join("d"));
        let doc = scratch_doc(&a, "scratch-2026-11-12-aaaaaa");
        fs::create_dir_all(doc["cwd"].as_str().unwrap()).unwrap();
        let place = foreign_workspace(&doc, &[a, d], 1, ACC, ORG).unwrap();
        fs::create_dir_all(&place.new).unwrap();
        assert!(relocate(&doc, &place, &[]).is_none());
    }

    #[test]
    fn a_gone_workspace_becomes_an_empty_folder() {
        let temp = TempDir::new().unwrap();
        let (a, d) = (temp.path().join("a"), temp.path().join("d"));
        let doc = scratch_doc(&a, "scratch-2026-11-12-bbbbbb");
        let place = foreign_workspace(&doc, &[a, d], 1, ACC, ORG).unwrap();
        assert!(relocate(&doc, &place, &[]).is_some());
        assert!(place.new.is_dir());
    }

    #[test]
    fn ordinary_folders_are_left_alone() {
        let doc = json!({ "cwd": r"E:\work", "originCwd": r"E:\work" });
        assert_eq!(
            foreign_workspace(
                &doc,
                &[PathBuf::from(r"C:\a"), PathBuf::from(r"C:\d")],
                1,
                ACC,
                ORG
            ),
            None
        );
    }
}

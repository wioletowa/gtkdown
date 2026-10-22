//! Obsidian's `.obsidian/bookmarks.json`: read, add, remove, follow renames.
//! Unknown item types and fields are kept untouched.

use crate::json::Json;
use std::io;
use std::path::{Path, PathBuf};

pub enum Kind {
    /// Vault-relative path, optional `#heading` / `#^block` subpath.
    File(String, Option<String>),
    Folder(String),
    Search(String),
    Group,
    Other,
}

pub struct Bookmark {
    pub title: String,
    pub kind: Kind,
    pub depth: usize,
    /// Position in the nested `items` arrays, for removal.
    pub index: Vec<usize>,
}

fn file(root: &Path) -> PathBuf {
    root.join(".obsidian/bookmarks.json")
}

fn load(root: &Path) -> Json {
    crate::json::vault_file(root, "bookmarks.json").unwrap_or_else(|| Json::Obj(vec![("items".into(), Json::Arr(vec![]))]))
}

fn save(root: &Path, j: &Json) -> io::Result<()> {
    std::fs::create_dir_all(root.join(".obsidian"))?;
    std::fs::write(file(root), j.pretty())
}

pub fn list(root: &Path) -> Vec<Bookmark> {
    let mut out = Vec::new();
    walk(load(root).get("items").map(Json::arr).unwrap_or_default(), 0, &mut Vec::new(), &mut out);
    out
}

fn walk(items: &[Json], depth: usize, index: &mut Vec<usize>, out: &mut Vec<Bookmark>) {
    for (i, it) in items.iter().enumerate() {
        index.push(i);
        let s = |k: &str| it.get(k).and_then(Json::str).map(String::from);
        let kind = match s("type").as_deref() {
            Some("file") | Some("heading") | Some("block") => {
                Kind::File(s("path").unwrap_or_default(), s("subpath").map(|p| p.trim_start_matches('#').to_string()))
            }
            Some("folder") => Kind::Folder(s("path").unwrap_or_default()),
            Some("search") => Kind::Search(s("query").unwrap_or_default()),
            Some("group") => Kind::Group,
            _ => Kind::Other,
        };
        let title = s("title").filter(|t| !t.is_empty()).unwrap_or_else(|| match &kind {
            Kind::File(p, sub) => {
                let stem = p.rsplit('/').next().unwrap_or(p).trim_end_matches(".md").to_string();
                sub.as_ref().map_or(stem.clone(), |s| format!("{stem} › {s}"))
            }
            Kind::Folder(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
            Kind::Search(q) => q.clone(),
            _ => String::new(),
        });
        let is_group = matches!(kind, Kind::Group);
        out.push(Bookmark { title, kind, depth, index: index.clone() });
        if is_group {
            walk(it.get("items").map(Json::arr).unwrap_or_default(), depth + 1, index, out);
        }
        index.pop();
    }
}

fn now_ms() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

/// Top-level file bookmark index for `rel`, if any.
pub fn find_file(root: &Path, rel: &str) -> Option<Vec<usize>> {
    list(root)
        .into_iter()
        .find(|b| matches!(&b.kind, Kind::File(p, None) if p == rel))
        .map(|b| b.index)
}

pub fn add_file(root: &Path, rel: &str) -> io::Result<()> {
    let mut j = load(root);
    if j.get("items").is_none()
        && let Json::Obj(kv) = &mut j
    {
        kv.push(("items".into(), Json::Arr(vec![])));
    }
    if let Some(Json::Arr(items)) = j.get_mut("items") {
        items.push(Json::Obj(vec![
            ("type".into(), Json::Str("file".into())),
            ("ctime".into(), Json::Num(now_ms())),
            ("path".into(), Json::Str(rel.into())),
        ]));
    }
    save(root, &j)
}

pub fn remove(root: &Path, index: &[usize]) -> io::Result<()> {
    let mut j = load(root);
    let mut items = j.get_mut("items");
    for (n, &i) in index.iter().enumerate() {
        let Some(Json::Arr(a)) = items else { return Ok(()) };
        if i >= a.len() {
            return Ok(());
        }
        if n + 1 == index.len() {
            a.remove(i);
            break;
        }
        items = a[i].get_mut("items");
    }
    save(root, &j)
}

/// After a move: rewrites bookmark paths under `old` to `new` (vault-relative).
pub fn rename(root: &Path, old: &str, new: &str) -> io::Result<()> {
    if !file(root).exists() {
        return Ok(());
    }
    let mut j = load(root);
    fn fix(v: &mut Json, old: &str, new: &str) -> bool {
        let mut changed = false;
        if let Some(Json::Str(p)) = v.get_mut("path") {
            if p == old {
                *p = new.to_string();
                changed = true;
            } else if let Some(rest) = p.strip_prefix(&format!("{old}/")) {
                *p = format!("{new}/{rest}");
                changed = true;
            }
        }
        if let Some(Json::Arr(items)) = v.get_mut("items") {
            for it in items {
                changed |= fix(it, old, new);
            }
        }
        changed
    }
    if fix(&mut j, old, new) { save(root, &j) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_rename_remove() {
        let root = std::env::temp_dir().join(format!("gtkdown-bm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::write(
            file(&root),
            r##"{"items":[{"type":"group","ctime":1,"title":"G","items":[{"type":"file","ctime":2,"path":"a/X.md","subpath":"#H"}]},{"type":"search","ctime":3,"query":"tag:#x"}]}"##,
        )
        .unwrap();
        add_file(&root, "Top.md").unwrap();
        let titles: Vec<_> = list(&root).iter().map(|b| (b.title.clone(), b.depth)).collect();
        assert_eq!(titles, vec![("G".into(), 0), ("X › H".into(), 1), ("tag:#x".into(), 0), ("Top".into(), 0)]);
        rename(&root, "a", "b").unwrap();
        assert!(matches!(&list(&root)[1].kind, Kind::File(p, _) if p == "b/X.md"));
        remove(&root, &find_file(&root, "Top.md").unwrap()).unwrap();
        remove(&root, &[0, 0]).unwrap();
        assert_eq!(list(&root).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}

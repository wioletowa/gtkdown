//! Moving, renaming and deleting vault files, rewriting links the way Obsidian does.

use crate::markdown::{self, Link};
use gtk::gio::prelude::*;
use crate::vault::{is_external, is_note, list_files, resolve, split_target};
use std::io;
use std::path::{Component, Path, PathBuf};

/// `newLinkFormat` from `.obsidian/app.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkFormat {
    Shortest,
    Relative,
    Absolute,
}

fn app_json(root: &Path, key: &str) -> Option<String> {
    let app = std::fs::read_to_string(root.join(".obsidian/app.json")).ok()?;
    crate::config::json_strings_for_key(&app, key).into_iter().next()
}

pub fn link_format(root: &Path) -> LinkFormat {
    match app_json(root, "newLinkFormat").as_deref() {
        Some("relative") => LinkFormat::Relative,
        Some("absolute") => LinkFormat::Absolute,
        _ => LinkFormat::Shortest,
    }
}

fn slash(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Path of `to` relative to directory `from_dir`, with `..` where needed.
fn relative(from_dir: &Path, to: &Path) -> PathBuf {
    let a: Vec<Component> = from_dir.components().collect();
    let b: Vec<Component> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..a.len() {
        out.push("..");
    }
    for c in &b[common..] {
        out.push(c);
    }
    out
}

fn encode(s: &str) -> String {
    s.replace('%', "%25").replace(' ', "%20")
}

/// How a link to `target` from `from_note` is written, per the vault's link format.
fn link_text(root: &Path, files: &[PathBuf], from_note: &Path, target: &Path, fmt: LinkFormat, wiki: bool) -> String {
    let strip = |s: String| if wiki && is_note(target) { s.trim_end_matches(".md").to_string() } else { s };
    let name = target.file_name().unwrap_or_default().to_string_lossy().to_string();
    let vault_rel = slash(target.strip_prefix(root).unwrap_or(target));
    let text = match fmt {
        LinkFormat::Shortest => {
            let lower = name.to_lowercase();
            let same = files
                .iter()
                .filter(|f| f.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == lower))
                .count();
            strip(if same <= 1 { name } else { vault_rel })
        }
        LinkFormat::Absolute => strip(vault_rel),
        LinkFormat::Relative => strip(slash(&relative(from_note.parent().unwrap_or(root), target))),
    };
    if wiki { text } else { encode(&text) }
}

/// Byte range of the path part of a link's destination inside `src` (before any `#anchor`).
fn dest_path_range(src: &str, l: &Link) -> Option<std::ops::Range<usize>> {
    let el = &src[l.range.clone()];
    let (start, end) = if l.wiki {
        let s = el.find("[[")? + 2;
        let e = s + el[s..].find(['|', ']'])?;
        (s, e)
    } else {
        let s = el.rfind("](")? + 2;
        let mut e = el.len().checked_sub(1)?;
        let mut s2 = s;
        if el[s..].starts_with('<') {
            s2 += 1;
            e = s2 + el[s2..].find('>')?;
        } else if let Some(sp) = el[s..e].find(' ') {
            e = s + sp;
        }
        (s2, e)
    };
    let end = el[start..end].find('#').map_or(end, |h| start + h);
    Some(l.range.start + start..l.range.start + end)
}

/// Moves a file or folder inside the vault and rewrites every link that pointed into it.
/// Returns how many notes had links rewritten.
pub fn move_path(root: &Path, from: &Path, to: &Path) -> io::Result<usize> {
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, to.display().to_string()));
    }
    if to.starts_with(from) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot move a folder into itself"));
    }
    let old_files = list_files(root);
    let moved: Vec<(PathBuf, PathBuf)> = if from.is_dir() {
        old_files
            .iter()
            .filter(|f| f.starts_with(from))
            .map(|f| (f.clone(), to.join(f.strip_prefix(from).unwrap())))
            .collect()
    } else {
        vec![(from.to_path_buf(), to.to_path_buf())]
    };
    let map = |p: &Path| moved.iter().find(|(o, _)| o == p).map(|(_, n)| n.clone());
    let new_files: Vec<PathBuf> = old_files.iter().map(|f| map(f).unwrap_or_else(|| f.clone())).collect();
    let fmt = link_format(root);

    // Plan against the old layout, so links resolve as they did before the move.
    let mut edits: Vec<(PathBuf, String)> = Vec::new();
    for f in old_files.iter().filter(|f| is_note(f)) {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        if !text.contains("[[") && !text.contains("](") {
            continue;
        }
        let note_new = map(f).unwrap_or_else(|| f.clone());
        let mut links = markdown::parse(&text).links;
        links.sort_by_key(|l| l.range.start);
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for l in &links {
            if is_external(&l.target) {
                continue;
            }
            let (path, _) = split_target(&l.target);
            if path.is_empty() {
                continue;
            }
            let Some(target) = resolve(root, &old_files, f, &path) else { continue };
            let target_new = map(&target).unwrap_or_else(|| target.clone());
            // Relative markdown links break when the linking note itself moves.
            let relative_md = !l.wiki && (path.starts_with('.') || path.contains('/'));
            if target_new == target && !(note_new != *f && relative_md) {
                continue;
            }
            let Some(r) = dest_path_range(&text, l) else { continue };
            if r.start < last {
                continue;
            }
            out.push_str(&text[last..r.start]);
            out.push_str(&link_text(root, &new_files, &note_new, &target_new, fmt, l.wiki));
            last = r.end;
        }
        if last > 0 {
            out.push_str(&text[last..]);
            edits.push((f.clone(), out));
        }
    }

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)?;
    let rel = |p: &Path| slash(p.strip_prefix(root).unwrap_or(p));
    let _ = crate::bookmarks::rename(root, &rel(from), &rel(to));
    let n = edits.len();
    for (old, text) in edits {
        std::fs::write(map(&old).unwrap_or(old), text)?;
    }
    Ok(n)
}

/// Deletes per `trashOption` in app.json: "local" moves to `.trash/`, "none" removes,
/// anything else uses the system trash.
pub fn delete(root: &Path, path: &Path) -> io::Result<()> {
    match app_json(root, "trashOption").as_deref() {
        Some("local") => {
            let trash = root.join(".trash");
            std::fs::create_dir_all(&trash)?;
            let name = path.file_name().unwrap_or_default();
            let mut dest = trash.join(name);
            let mut i = 1;
            while dest.exists() {
                dest = trash.join(format!("{} {i}", name.to_string_lossy()));
                i += 1;
            }
            std::fs::rename(path, dest)
        }
        Some("none") if path.is_dir() => std::fs::remove_dir_all(path),
        Some("none") => std::fs::remove_file(path),
        _ => gtk::gio::File::for_path(path)
            .trash(gtk::gio::Cancellable::NONE)
            .map_err(|e| io::Error::other(e.to_string())),
    }
}

/// First free `dir/base.ext`, `dir/base 1.ext`, ... (`ext` empty for folders).
pub fn free_name(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let dot = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    (0..)
        .map(|i| if i == 0 { dir.join(format!("{base}{dot}")) } else { dir.join(format!("{base} {i}{dot}")) })
        .find(|p| !p.exists())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gtkdown-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (p, text) in files {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        root
    }

    fn read(root: &Path, p: &str) -> String {
        std::fs::read_to_string(root.join(p)).unwrap()
    }

    #[test]
    fn rename_note_rewrites_links() {
        let root = vault(
            "note",
            &[
                ("a/Note.md", "body [[Top]]"),
                ("Top.md", "[[Note]] [[a/Note#H|alias]] [x](a/Note.md) ![[img.png]] [[Other]] [y](https://e.x)"),
                ("Other.md", "[[Note#^b]]"),
                ("img.png", ""),
            ],
        );
        let n = move_path(&root, &root.join("a/Note.md"), &root.join("b/Новое имя.md")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            read(&root, "Top.md"),
            "[[Новое имя]] [[Новое имя#H|alias]] [x](Новое%20имя.md) ![[img.png]] [[Other]] [y](https://e.x)"
        );
        assert_eq!(read(&root, "Other.md"), "[[Новое имя#^b]]");
        assert_eq!(read(&root, "b/Новое имя.md"), "body [[Top]]");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ambiguous_name_uses_path_and_folder_moves() {
        let root = vault(
            "dir",
            &[("a/Note.md", "[see](../x/Note.md)"), ("x/Note.md", ""), ("Top.md", "[[a/Note]] ![[a/pic.png]]"), ("a/pic.png", "")],
        );
        move_path(&root, &root.join("a"), &root.join("z/a2")).unwrap();
        assert_eq!(read(&root, "Top.md"), "[[z/a2/Note]] ![[pic.png]]");
        // The moved note's relative link still points at x/Note.md.
        assert_eq!(read(&root, "z/a2/Note.md"), "[see](x/Note.md)");
        assert!(move_path(&root, &root.join("z"), &root.join("z/inner")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_and_names() {
        assert_eq!(relative(Path::new("/v/a/b"), Path::new("/v/c/d.md")), PathBuf::from("../../c/d.md"));
        let root = vault("free", &[("Untitled.md", "")]);
        assert_eq!(free_name(&root, "Untitled", "md"), root.join("Untitled 1.md"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

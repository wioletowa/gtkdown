//! Vault-wide queries: file listing, Obsidian link resolution, search, backlinks.
//! Plain std code so it can run on a worker thread.

use crate::markdown;
use std::path::{Path, PathBuf};

/// Every non-hidden file in the vault, recursively.
pub fn list_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => out.push(e.path()),
                Err(_) => {}
            }
        }
    }
    out.sort();
    out
}

pub fn is_note(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Splits `Note#Heading` / `Note#^block` into the path part (percent-decoded) and the anchor.
pub fn split_target(target: &str) -> (String, Option<String>) {
    let (path, anchor) = match target.split_once('#') {
        Some((p, a)) => (p, Some(a.to_string())),
        None => (target, None),
    };
    (percent_decode(path), anchor.filter(|a| !a.is_empty()))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

pub fn is_external(target: &str) -> bool {
    let scheme = target.split_once(':').map(|(s, _)| s).unwrap_or("");
    !scheme.is_empty() && scheme.len() > 1 && scheme.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-')
}

/// Resolves a link path (no anchor) the way Obsidian does: relative to the linking note, then
/// from the vault root, then by file name anywhere (same folder first, then shortest path).
pub fn resolve(root: &Path, files: &[PathBuf], from: &Path, link: &str) -> Option<PathBuf> {
    if link.is_empty() {
        return Some(from.to_path_buf());
    }
    let with_md = |p: PathBuf| -> Option<PathBuf> {
        if p.is_file() {
            return p.canonicalize().ok();
        }
        let mut s = p.into_os_string();
        s.push(".md");
        PathBuf::from(s).canonicalize().ok().filter(|p| p.is_file())
    };
    let base = from.parent().unwrap_or(root);
    if link.contains('/') || link.starts_with('.') {
        if let Some(p) = with_md(base.join(link)).or_else(|| with_md(root.join(link.trim_start_matches('/')))) {
            return Some(p);
        }
    }

    let want = link.rsplit('/').next().unwrap_or(link).to_lowercase();
    let want_md = format!("{want}.md");
    let suffix = link.to_lowercase();
    let mut best: Option<&PathBuf> = None;
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        if name != want && name != want_md {
            continue;
        }
        // `Folder/Note` must also match its trailing folders.
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().to_lowercase();
        if link.contains('/') && !rel.trim_end_matches(".md").ends_with(suffix.trim_end_matches(".md")) {
            continue;
        }
        let better = match best {
            None => true,
            Some(b) => {
                let same = |p: &Path| p.parent() == Some(base);
                (same(f) && !same(b)) || (same(f) == same(b) && f.components().count() < b.components().count())
            }
        };
        if better {
            best = Some(f);
        }
    }
    best.cloned()
}

/// Folder for new notes per `.obsidian/app.json` (`newFileLocation`, `newFileFolderPath`).
pub fn new_note_dir(root: &Path, current: Option<&Path>) -> PathBuf {
    let app = std::fs::read_to_string(root.join(".obsidian/app.json")).unwrap_or_default();
    let get = |k: &str| crate::config::json_strings_for_key(&app, k).into_iter().next();
    let dir = match get("newFileLocation").as_deref() {
        Some("current") => current.and_then(|c| c.parent()).map(Path::to_path_buf),
        Some("folder") => get("newFileFolderPath").map(|f| root.join(f)),
        _ => None,
    };
    dir.filter(|d| d.starts_with(root) && d.is_dir()).unwrap_or_else(|| root.to_path_buf())
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    /// Char offset of the match in the file.
    pub offset: usize,
    pub snippet: String,
}

const PER_FILE: usize = 5;
const SNIPPET: usize = 120;

fn snippet(line: &str, at_char: usize) -> String {
    let chars: Vec<char> = line.trim_end().chars().collect();
    let start = at_char.saturating_sub(40);
    let end = (start + SNIPPET).min(chars.len());
    let mut s: String = chars[start.min(end)..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s.trim().to_string()
}

/// Case-insensitive search over note names and contents.
pub fn search(files: &[PathBuf], query: &str, limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for f in files.iter().filter(|f| is_note(f)) {
        let name_hit = f.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().contains(&q));
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut n = 0;
        let mut char_pos = 0;
        for line in text.split_inclusive('\n') {
            let lower = line.to_lowercase();
            if let Some(b) = lower.find(&q) {
                let at = lower[..b].chars().count();
                out.push(Hit { path: f.clone(), offset: char_pos + at, snippet: snippet(line, at) });
                n += 1;
                if n >= PER_FILE {
                    break;
                }
            }
            char_pos += line.chars().count();
        }
        if n == 0 && name_hit {
            out.push(Hit { path: f.clone(), offset: 0, snippet: String::new() });
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Notes linking to `target`, with the line of each link.
pub fn backlinks(root: &Path, files: &[PathBuf], target: &Path) -> Vec<Hit> {
    let mut out = Vec::new();
    for f in files.iter().filter(|f| is_note(f) && f.as_path() != target) {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        if !text.contains("[[") && !text.contains("](") {
            continue;
        }
        for l in markdown::parse(&text).links {
            if is_external(&l.target) {
                continue;
            }
            let (path, _) = split_target(&l.target);
            if resolve(root, files, f, &path).as_deref() != Some(target) {
                continue;
            }
            let line_start = text[..l.range.start].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[l.range.start..].find('\n').map_or(text.len(), |i| l.range.start + i);
            let at = text[line_start..l.range.start].chars().count();
            out.push(Hit {
                path: f.clone(),
                offset: text[..l.range.start].chars().count(),
                snippet: snippet(&text[line_start..line_end], at),
            });
        }
    }
    out
}

/// Char offset of the heading whose text matches `anchor` (Obsidian compares loosely).
pub fn heading_offset(text: &str, anchor: &str) -> Option<usize> {
    let norm = |s: &str| s.trim().trim_matches('#').trim().to_lowercase();
    let want = norm(anchor);
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with('#') && norm(t) == want {
            return Some(pos);
        }
        if let Some(block) = anchor.strip_prefix('^')
            && line.trim_end().ends_with(&format!("^{block}"))
        {
            return Some(pos);
        }
        pos += line.chars().count();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_like_obsidian() {
        let root = std::env::temp_dir().join(format!("gtkdown-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for p in ["a/Note.md", "b/Note.md", "b/c/Deep.md", "Top.md", "img.png"] {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        }
        let files = list_files(&root);
        let from = root.join("b/c/Deep.md");
        assert_eq!(resolve(&root, &files, &root.join("a/x.md"), "Note"), Some(root.join("a/Note.md")));
        assert_eq!(resolve(&root, &files, &from, "a/Note"), Some(root.join("a/Note.md")));
        assert_eq!(resolve(&root, &files, &from, "note"), Some(root.join("a/Note.md")));
        assert_eq!(resolve(&root, &files, &from, "img.png"), Some(root.join("img.png")));
        assert_eq!(resolve(&root, &files, &from, "../../Top.md"), Some(root.join("Top.md")));
        assert_eq!(resolve(&root, &files, &from, "Missing"), None);
        assert_eq!(split_target("My%20Note#Head"), ("My Note".into(), Some("Head".into())));

        std::fs::write(root.join("Top.md"), "see [[Note]] and [x](b/Note.md)\n").unwrap();
        let bl = backlinks(&root, &files, &root.join("a/Note.md"));
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0].snippet, "see [[Note]] and [x](b/Note.md)");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn headings() {
        let t = "intro\n## Мой раздел\ntext ^abc\n";
        assert_eq!(heading_offset(t, "мой раздел"), Some(6));
        assert_eq!(heading_offset(t, "^abc"), Some(20));
    }
}

//! Plain `key=value` config in $XDG_CONFIG_HOME/gtkdown/config. Never touches `.obsidian`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Session {
    pub vault: PathBuf,
    pub active: usize,
    /// Vault-relative paths of open tabs.
    pub tabs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub theme: String,
    pub lang: String,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    pub last_vault: Option<PathBuf>,
    /// Most recent first.
    pub vaults: Vec<PathBuf>,
    pub sessions: Vec<Session>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "system".into(),
            lang: "auto".into(),
            width: 1100,
            height: 720,
            maximized: false,
            last_vault: None,
            vaults: Vec::new(),
            sessions: Vec::new(),
        }
    }
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn path() -> PathBuf {
    config_dir().join("gtkdown/config")
}

pub fn user_css_path() -> PathBuf {
    config_dir().join("gtkdown/style.css")
}

impl Config {
    pub fn load() -> Config {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(path()) else { return c };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "theme" => c.theme = v.into(),
                "lang" => c.lang = v.into(),
                "width" => c.width = v.parse().unwrap_or(c.width),
                "height" => c.height = v.parse().unwrap_or(c.height),
                "maximized" => c.maximized = v == "true",
                "last" => c.last_vault = Some(v.into()),
                "vault" => c.vaults.push(v.into()),
                "session" => {
                    let mut it = v.split('\t');
                    if let (Some(vault), Some(active)) = (it.next(), it.next()) {
                        c.sessions.push(Session {
                            vault: vault.into(),
                            active: active.parse().unwrap_or(0),
                            tabs: it.map(String::from).collect(),
                        });
                    }
                }
                _ => {}
            }
        }
        c
    }

    pub fn save(&self) {
        let mut s = format!(
            "theme={}\nlang={}\nwidth={}\nheight={}\nmaximized={}\n",
            self.theme, self.lang, self.width, self.height, self.maximized
        );
        if let Some(v) = &self.last_vault {
            s += &format!("last={}\n", v.display());
        }
        for v in &self.vaults {
            s += &format!("vault={}\n", v.display());
        }
        for ses in &self.sessions {
            s += &format!("session={}\t{}", ses.vault.display(), ses.active);
            for t in &ses.tabs {
                s.push('\t');
                s += t;
            }
            s.push('\n');
        }
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&p, s) {
            eprintln!("gtkdown: cannot write {}: {e}", p.display());
        }
    }

    pub fn remember_vault(&mut self, vault: &Path) {
        self.vaults.retain(|v| v != vault);
        self.vaults.insert(0, vault.to_path_buf());
        self.vaults.truncate(20);
        self.last_vault = Some(vault.to_path_buf());
    }

    pub fn session(&self, vault: &Path) -> Option<&Session> {
        self.sessions.iter().find(|s| s.vault == vault)
    }

    pub fn set_session(&mut self, ses: Session) {
        self.sessions.retain(|s| s.vault != ses.vault);
        self.sessions.push(ses);
    }

    /// Our recent vaults followed by ones Obsidian knows about, existing only.
    pub fn known_vaults(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for v in self.vaults.iter().cloned().chain(obsidian_vaults()) {
            if v.is_dir() && !out.contains(&v) {
                out.push(v);
            }
        }
        out
    }
}

/// Reads vault paths from Obsidian's own obsidian.json (native and flatpak installs).
pub fn obsidian_vaults() -> Vec<PathBuf> {
    let candidates = [
        config_dir().join("obsidian/obsidian.json"),
        home().join(".var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json"),
    ];
    let mut out = Vec::new();
    for c in candidates {
        if let Ok(text) = std::fs::read_to_string(c) {
            out.extend(json_strings_for_key(&text, "path").into_iter().map(PathBuf::from));
        }
    }
    out
}

/// Minimal scan for `"key": "value"` pairs; enough for obsidian.json without a JSON crate.
fn json_strings_for_key(text: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&needle) {
        rest = &rest[i + needle.len()..];
        let after = rest.trim_start();
        let Some(after) = after.strip_prefix(':') else { continue };
        let Some(after) = after.trim_start().strip_prefix('"') else { continue };
        let mut val = String::new();
        let mut chars = after.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => val.push('\n'),
                    Some('t') => val.push('\t'),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            val.push(ch);
                        }
                    }
                    Some(o) => val.push(o),
                    None => break,
                },
                c => val.push(c),
            }
        }
        out.push(val);
        rest = chars.as_str();
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn obsidian_json() {
        let j = r#"{"vaults":{"a1":{"path":"/home/u/OBSI","ts":1,"open":true},"b2":{"path" : "/x/ф \"q\""}}}"#;
        assert_eq!(super::json_strings_for_key(j, "path"), vec!["/home/u/OBSI", "/x/ф \"q\""]);
    }
}

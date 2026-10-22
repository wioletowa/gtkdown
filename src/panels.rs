//! Sidebar result lists (search, backlinks) and the quick switcher dialog.

use crate::i18n::tr;
use crate::vault::{self, Hit};
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Hits grouped by file: a header row per file, then one row per snippet.
pub struct Results {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    placeholder: gtk::Label,
    rows: RefCell<Vec<Hit>>,
}

impl Results {
    pub fn new(on_open: impl Fn(Hit) + 'static) -> Rc<Results> {
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_activate_on_single_click(true);
        let placeholder = gtk::Label::new(None);
        placeholder.add_css_class("dim-label");
        placeholder.set_margin_top(24);
        placeholder.set_wrap(true);
        list.set_placeholder(Some(&placeholder));
        let widget = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let this = Rc::new(Results { widget, list, placeholder, rows: RefCell::new(Vec::new()) });
        let weak = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            let Some(r) = weak.upgrade() else { return };
            let hit = r.rows.borrow().get(row.index() as usize).cloned();
            if let Some(h) = hit {
                on_open(h);
            }
        });
        this
    }

    pub fn set(&self, hits: Vec<Hit>, root: &Path, empty: &str) {
        self.list.remove_all();
        self.placeholder.set_label(empty);
        let mut rows = Vec::new();
        let mut last: Option<PathBuf> = None;
        for h in hits {
            if last.as_ref() != Some(&h.path) {
                let rel = h.path.strip_prefix(root).unwrap_or(&h.path);
                let name = rel.to_string_lossy();
                let title = gtk::Label::builder()
                    .label(name.strip_suffix(".md").unwrap_or(&name))
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .css_classes(["heading"])
                    .build();
                self.list.append(&title);
                rows.push(h.clone());
                last = Some(h.path.clone());
            }
            if !h.snippet.is_empty() {
                let snip = gtk::Label::builder()
                    .label(&h.snippet)
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .lines(3)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .margin_start(10)
                    .css_classes(["caption"])
                    .build();
                self.list.append(&snip);
                rows.push(h);
            }
        }
        *self.rows.borrow_mut() = rows;
    }
}

/// Fuzzy subsequence score; higher is better. `None` if `q` is not a subsequence.
pub fn fuzzy(q: &str, text: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev: Option<usize> = None;
    for qc in q.chars() {
        let pos = t[ti..].iter().position(|&c| c == qc)? + ti;
        score += match prev {
            Some(p) if p + 1 == pos => 8,
            _ => 1,
        };
        if pos == 0 || matches!(t[pos - 1], ' ' | '/' | '-' | '_') {
            score += 4;
        }
        prev = Some(pos);
        ti = pos + 1;
    }
    Some(score * 100 - t.len() as i32)
}

/// Ctrl+O dialog. `on_open(path, new_tab)`; `on_create(name, new_tab)` when nothing matches.
pub fn quick_switcher(
    parent: &impl IsA<gtk::Widget>,
    root: PathBuf,
    on_open: impl Fn(PathBuf, bool) + 'static,
    on_create: impl Fn(String, bool) + 'static,
) {
    let files: Vec<PathBuf> = vault::list_files(&root)
        .into_iter()
        .filter(|p| crate::tree::ext(&p.to_string_lossy()).is_some_and(|e| e == "md" || e == "canvas"))
        .collect();
    let rels: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(&root).unwrap_or(p).to_string_lossy().to_lowercase())
        .collect();

    let entry = gtk::SearchEntry::builder().placeholder_text(tr("Type a note name…")).hexpand(true).build();
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    list.set_activate_on_single_click(true);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(320)
        .max_content_height(420)
        .propagate_natural_height(true)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_bottom(12);
    content.append(&entry);
    content.append(&scroll);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title(tr("Quick switcher"))
        .content_width(560)
        .child(&toolbar)
        .build();

    // Paths shown in the list, in row order; empty path = "create note" row.
    let shown: Rc<RefCell<Vec<Option<PathBuf>>>> = Rc::default();
    let refill = {
        let (list, shown, root) = (list.clone(), shown.clone(), root.clone());
        move |q: &str| {
            list.remove_all();
            let q = q.trim().to_lowercase();
            let mut scored: Vec<(i32, usize)> = rels
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    let name = r.rsplit('/').next().unwrap_or(r);
                    let s = fuzzy(&q, name).map(|s| s + 1000).or_else(|| fuzzy(&q, r))?;
                    Some((s, i))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(rels[a.1].cmp(&rels[b.1])));
            let mut v = Vec::new();
            for (_, i) in scored.into_iter().take(50) {
                let p = &files[i];
                let rel = p.strip_prefix(&root).unwrap_or(p);
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let stem = p.file_stem().unwrap_or_default().to_string_lossy();
                row.append(&gtk::Label::builder().label(&*stem).xalign(0.0).build());
                if let Some(dir) = rel.parent().filter(|d| !d.as_os_str().is_empty()) {
                    let l = gtk::Label::builder()
                        .label(dir.to_string_lossy())
                        .xalign(0.0)
                        .hexpand(true)
                        .ellipsize(gtk::pango::EllipsizeMode::Start)
                        .css_classes(["dim-label", "caption"])
                        .build();
                    row.append(&l);
                }
                list.append(&row);
                v.push(Some(p.clone()));
            }
            if v.is_empty() && !q.is_empty() {
                let l = gtk::Label::builder().label(format!("{} «{}»", tr("Create note"), q)).xalign(0.0).build();
                list.append(&l);
                v.push(None);
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
            *shown.borrow_mut() = v;
        }
    };
    refill("");

    let on_open = Rc::new(on_open);
    let on_create = Rc::new(on_create);
    let choose = {
        let (dialog, entry, shown) = (dialog.clone(), entry.clone(), shown.clone());
        move |index: i32, new_tab: bool| {
            let item = shown.borrow().get(index.max(0) as usize).cloned();
            match item {
                Some(Some(p)) => on_open(p, new_tab),
                Some(None) => on_create(entry.text().trim().to_string(), new_tab),
                None => return,
            }
            dialog.close();
        }
    };

    entry.connect_search_changed(move |e| refill(&e.text()));
    let (c, l) = (choose.clone(), list.clone());
    entry.connect_activate(move |_| {
        let i = l.selected_row().map_or(0, |r| r.index());
        c(i, false);
    });
    let c = choose.clone();
    list.connect_row_activated(move |_, row| c(row.index(), false));

    // Arrow keys move the selection; Ctrl+Enter opens in a new tab.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let l = list.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        use gtk::gdk::Key;
        let cur = l.selected_row().map_or(-1, |r| r.index());
        let target = match key {
            Key::Down => cur + 1,
            Key::Up => cur - 1,
            Key::Return | Key::KP_Enter if state.contains(gtk::gdk::ModifierType::CONTROL_MASK) => {
                choose(cur.max(0), true);
                return glib::Propagation::Stop;
            }
            _ => return glib::Propagation::Proceed,
        };
        if let Some(row) = l.row_at_index(target.max(0)) {
            l.select_row(Some(&row));
        }
        glib::Propagation::Stop
    });
    entry.add_controller(keys);

    dialog.present(Some(parent));
    entry.grab_focus();
}

#[cfg(test)]
mod tests {
    #[test]
    fn fuzzy_order() {
        use super::fuzzy;
        assert!(fuzzy("баз", "база.md") > fuzzy("баз", "большая заметка.md"));
        assert!(fuzzy("xyz", "abc").is_none());
        assert!(fuzzy("nv", "nvidia.md").is_some());
    }
}

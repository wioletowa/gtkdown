//! Vault file tree: lazily loaded, monitored GtkDirectoryList per folder.

use adw::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;

const ATTRS: &str = "standard::name,standard::display-name,standard::type,standard::is-hidden";

/// Extensions Obsidian lists in the file explorer by default.
const SHOWN: &[&str] = &[
    "md", "canvas", "png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "avif", "mp3", "wav", "m4a", "ogg", "flac",
    "webm", "mp4", "mkv", "mov", "ogv", "pdf",
];

pub fn ext(name: &str) -> Option<String> {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
}

pub fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file").and_downcast::<gio::File>()
}

fn dir_model(dir: &gio::File) -> gio::ListModel {
    let list = gtk::DirectoryList::new(Some(ATTRS), Some(dir));
    list.set_monitored(true);

    let filter = gtk::CustomFilter::new(|obj| {
        let info = obj.downcast_ref::<gio::FileInfo>().unwrap();
        let name = info.name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || info.is_hidden() {
            return false;
        }
        is_dir(info) || ext(&name).is_some_and(|e| SHOWN.contains(&e.as_str()))
    });
    let filtered = gtk::FilterListModel::new(Some(list), Some(filter));

    let sorter = gtk::CustomSorter::new(|a, b| {
        let a = a.downcast_ref::<gio::FileInfo>().unwrap();
        let b = b.downcast_ref::<gio::FileInfo>().unwrap();
        is_dir(b)
            .cmp(&is_dir(a))
            .then_with(|| {
                let ka = glib::FilenameCollationKey::from(a.display_name().as_str());
                let kb = glib::FilenameCollationKey::from(b.display_name().as_str());
                ka.cmp(&kb)
            })
            .into()
    });
    gtk::SortListModel::new(Some(filtered), Some(sorter)).upcast()
}

pub struct Tree {
    pub view: gtk::ListView,
    selection: gtk::SingleSelection,
}

impl Tree {
    /// `on_open(path, new_tab)` fires for files; folders toggle open/closed.
    pub fn new(on_open: impl Fn(PathBuf, bool) + 'static) -> Tree {
        let selection = gtk::SingleSelection::new(None::<gio::ListModel>);
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let factory = gtk::SignalListItemFactory::new();
        let on_open = std::rc::Rc::new(on_open);
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            let suffix = gtk::Label::new(None);
            suffix.add_css_class("suffix");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.append(&label);
            row.append(&suffix);
            let expander = gtk::TreeExpander::new();
            expander.set_child(Some(&row));
            expander.set_indent_for_icon(true);

            let click = gtk::GestureClick::new();
            click.set_button(0);
            let weak_item = item.downgrade();
            let on_open = on_open.clone();
            click.connect_released(move |g, n, _, _| {
                let Some(item) = weak_item.upgrade() else { return };
                let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
                let Some(info) = row.item().and_downcast::<gio::FileInfo>() else { return };
                if is_dir(&info) {
                    if n == 1 {
                        row.set_expanded(!row.is_expanded());
                    }
                    return;
                }
                let ctrl = g.current_event_state().contains(gtk::gdk::ModifierType::CONTROL_MASK);
                let new_tab = ctrl || g.current_button() == 2;
                if let Some(path) = file_of(&info).and_then(|f| f.path()) {
                    on_open(path, new_tab);
                }
            });
            expander.add_controller(click);
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = item.item().and_downcast::<gtk::TreeListRow>().unwrap();
            let info = row.item().and_downcast::<gio::FileInfo>().unwrap();
            let expander = item.child().and_downcast::<gtk::TreeExpander>().unwrap();
            expander.set_list_row(Some(&row));
            let b = expander.child().and_downcast::<gtk::Box>().unwrap();
            let label = b.first_child().and_downcast::<gtk::Label>().unwrap();
            let suffix = label.next_sibling().and_downcast::<gtk::Label>().unwrap();

            let name = info.display_name();
            let (stem, e) = match name.rsplit_once('.') {
                Some((s, e)) if !is_dir(&info) => (s.to_string(), e.to_ascii_lowercase()),
                _ => (name.to_string(), String::new()),
            };
            match e.as_str() {
                "md" => {
                    label.set_label(&stem);
                    suffix.set_visible(false);
                }
                "canvas" => {
                    label.set_label(&stem);
                    suffix.set_label("CANVAS");
                    suffix.set_visible(true);
                }
                _ => {
                    label.set_label(&name);
                    suffix.set_visible(false);
                }
            }
        });

        let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        view.add_css_class("navigation-sidebar");
        view.add_css_class("vault-tree");
        Tree { view, selection }
    }

    pub fn set_root(&self, dir: Option<&std::path::Path>) {
        let model = dir.map(|d| {
            let root = dir_model(&gio::File::for_path(d));
            let tree = gtk::TreeListModel::new(root, false, false, |obj| {
                let info = obj.downcast_ref::<gio::FileInfo>()?;
                if !is_dir(info) {
                    return None;
                }
                Some(dir_model(&file_of(info)?))
            });
            tree.upcast::<gio::ListModel>()
        });
        self.selection.set_model(model.as_ref());
    }

    /// Highlights `path` if its row is currently visible.
    pub fn select(&self, path: Option<&std::path::Path>) {
        let n = self.selection.n_items();
        for i in 0..n {
            let row = self.selection.item(i).and_downcast::<gtk::TreeListRow>();
            let p = row
                .and_then(|r| r.item().and_downcast::<gio::FileInfo>())
                .and_then(|i| file_of(&i))
                .and_then(|f| f.path());
            if p.is_some() && p.as_deref() == path {
                self.selection.set_selected(i);
                return;
            }
        }
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
    }
}

//! Main window: vault sidebar, tabs, settings, session.

use crate::config::{Config, Session};
use crate::i18n::tr;
use crate::note::{self, Note};
use crate::theme::{self, Styles};
use crate::bookmarks;
use crate::canvas::Canvas;
use crate::graph::Graph;
use crate::panels::{self, ActionList, Results};
use crate::fileops;
use crate::tree::{self, Tree, TreeEvent};
use crate::vault;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "avif"];

/// Where to put the cursor after opening a note.
pub enum Goto {
    None,
    Offset(usize),
    Heading(String),
}

enum Content {
    Empty,
    Note(Rc<Note>),
    /// Images: read-only view.
    File(PathBuf),
    Canvas(Rc<Canvas>),
    Graph(Rc<Graph>),
}

struct Tab {
    page: adw::TabPage,
    bin: adw::Bin,
    content: RefCell<Content>,
}

impl Tab {
    fn path(&self) -> Option<PathBuf> {
        match &*self.content.borrow() {
            Content::Empty | Content::Graph(_) => None,
            Content::Note(n) => Some(n.path.borrow().clone()),
            Content::File(p) => Some(p.clone()),
            Content::Canvas(c) => Some(c.path.borrow().clone()),
        }
    }

    fn note(&self) -> Option<Rc<Note>> {
        match &*self.content.borrow() {
            Content::Note(n) => Some(n.clone()),
            _ => None,
        }
    }
}

pub struct App {
    window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    tab_view: adw::TabView,
    toasts: adw::ToastOverlay,
    status: gtk::Label,
    backlinks_button: gtk::Button,
    side_stack: gtk::Stack,
    search_entry: gtk::SearchEntry,
    search_results: Rc<Results>,
    backlinks: Rc<Results>,
    bookmarks: Rc<ActionList>,
    tags: Rc<ActionList>,
    outline: Rc<ActionList>,
    vim_status: gtk::Label,
    /// Bumped per request so stale background results are dropped.
    search_gen: Cell<u64>,
    backlinks_gen: Cell<u64>,
    vault_button: gtk::MenuButton,
    tree: Tree,
    styles: Rc<Styles>,
    theme_css: gtk::CssProvider,
    font_css: gtk::CssProvider,
    config: RefCell<Config>,
    vault: Rc<RefCell<Option<PathBuf>>>,
    tabs: RefCell<Vec<Rc<Tab>>>,
    /// Set while a vault switch closes every tab.
    switching: Cell<bool>,
}

/// CSS font-family value from a family name or an Obsidian-style list (`'Inter', sans-serif`).
/// A generic fallback is appended so a missing font (e.g. bundled only with Obsidian's flatpak)
/// degrades to the same kind of face.
fn css_family(f: &str) -> String {
    let lower = f.to_lowercase();
    let generic = if ["mono", "code", "courier"].iter().any(|k| lower.contains(k)) { "monospace" } else { "sans-serif" };
    if f.contains([',', '"', '\'']) { format!("{f}, {generic}") } else { format!("\"{f}\", {generic}") }
}

pub fn find_vault_root(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new("/"));
    parent
        .ancestors()
        .find(|d| d.join(".obsidian").is_dir())
        .unwrap_or(parent)
        .to_path_buf()
}

impl App {
    pub fn new(app: &adw::Application, config: Config) -> Rc<App> {
        let styles = Rc::new(Styles::new());

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("gtkdown")
            .default_width(config.width)
            .default_height(config.height)
            .maximized(config.maximized)
            .build();

        // Sidebar.
        // Our own icons, so the sidebar reads the same in any icon theme.
        gtk::IconTheme::for_display(&gtk::prelude::WidgetExt::display(&window))
            .add_resource_path("/io/github/wioletowa/gtkdown/icons");
        let new_note = gtk::Button::from_icon_name("gtkdown-new-note-symbolic");
        new_note.set_tooltip_text(Some(tr("New note")));
        new_note.set_action_name(Some("win.new-note"));
        let side_header = adw::HeaderBar::new();
        side_header.set_show_end_title_buttons(false);
        side_header.set_title_widget(Some(&gtk::Label::new(None)));


        let side_stack = gtk::Stack::new();
        let mut group: Option<gtk::ToggleButton> = None;
        let side_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        for (name, icon, tip) in [
            ("files", "gtkdown-files-symbolic", tr("Files")),
            ("search", "gtkdown-search-symbolic", tr("Search")),
            ("bookmarks", "gtkdown-bookmarks-symbolic", tr("Bookmarks")),
            ("tags", "gtkdown-tags-symbolic", tr("Tags")),
            ("outline", "gtkdown-outline-symbolic", tr("Outline")),
            ("backlinks", "gtkdown-backlinks-symbolic", tr("Backlinks")),
        ] {
            let b = gtk::ToggleButton::builder().icon_name(icon).tooltip_text(tip).css_classes(["flat"]).build();
            b.set_group(group.as_ref());
            group.get_or_insert_with(|| b.clone());
            let st = side_stack.clone();
            b.connect_toggled(move |b| {
                if b.is_active() {
                    st.set_visible_child_name(name);
                }
            });
            let b2 = b.clone();
            side_stack.connect_visible_child_name_notify(move |st| {
                if st.visible_child_name().as_deref() == Some(name) {
                    b2.set_active(true);
                }
            });
            side_buttons.append(&b);
        }
        side_header.pack_start(&side_buttons);

        let weak: Rc<RefCell<Weak<App>>> = Rc::new(RefCell::new(Weak::new()));
        let w = weak.clone();
        let vault: Rc<RefCell<Option<PathBuf>>> = Rc::default();
        let tree = Tree::new(vault.clone(), move |ev| {
            let Some(a) = w.borrow().upgrade() else { return };
            match ev {
                TreeEvent::Open(path, new_tab) => a.open_file(path, new_tab),
                TreeEvent::Menu(target, widget, x, y) => a.tree_menu(target, &widget, x, y),
                TreeEvent::Move(from, dir) => {
                    if let Some(name) = from.file_name() {
                        let to = dir.join(name);
                        if to != from {
                            a.move_path(&from, &to);
                        }
                    }
                }
            }
        });
        let tree_scroll = gtk::ScrolledWindow::builder()
            .child(&tree.view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        side_stack.add_named(&tree_scroll, Some("files"));

        let open_hit = |w: Rc<RefCell<Weak<App>>>| {
            move |h: vault::Hit| {
                if let Some(a) = w.borrow().upgrade() {
                    a.open_at(h.path, false, Goto::Offset(h.offset));
                }
            }
        };
        let search_results = Results::new(open_hit(weak.clone()));
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search in vault…"))
            .margin_start(8)
            .margin_end(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        search_box.append(&search_entry);
        search_box.append(&search_results.widget);
        side_stack.add_named(&search_box, Some("search"));

        let backlinks = Results::new(open_hit(weak.clone()));
        side_stack.add_named(&backlinks.widget, Some("backlinks"));
        let bookmarks = ActionList::new();
        side_stack.add_named(&bookmarks.widget, Some("bookmarks"));
        let tags = ActionList::new();
        side_stack.add_named(&tags.widget, Some("tags"));
        let outline = ActionList::new();
        side_stack.add_named(&outline.widget, Some("outline"));

        let vault_button = gtk::MenuButton::builder()
            .direction(gtk::ArrowType::Up)
            .always_show_arrow(true)
            .hexpand(true)
            .css_classes(["flat"])
            .build();
        let settings_menu = gio::Menu::new();
        let theme_menu = gio::Menu::new();
        for t in theme::THEMES {
            theme_menu.append(Some(tr(t.name)), Some(&format!("app.theme::{}", t.id)));
        }
        settings_menu.append_section(Some(tr("Theme")), &theme_menu);
        let lang_menu = gio::Menu::new();
        lang_menu.append(Some(tr("Automatic")), Some("app.lang::auto"));
        lang_menu.append(Some("Русский"), Some("app.lang::ru"));
        lang_menu.append(Some("English"), Some("app.lang::en"));
        settings_menu.append_section(Some(tr("Language")), &lang_menu);
        let editor_menu = gio::Menu::new();
        editor_menu.append(Some(tr("Editor font…")), Some("win.choose-font"));
        editor_menu.append(Some(tr("Use vault font")), Some("win.reset-font"));
        editor_menu.append(Some(tr("Vim mode")), Some("win.toggle-vim"));
        let headings_menu = gio::Menu::new();
        headings_menu.append(Some(tr("Automatic")), Some("app.headings::auto"));
        headings_menu.append(Some(tr("Large")), Some("app.headings::large"));
        headings_menu.append(Some(tr("Compact")), Some("app.headings::compact"));
        editor_menu.append_submenu(Some(tr("Headings")), &headings_menu);
        settings_menu.append_section(Some(tr("Editor")), &editor_menu);
        let settings_button = gtk::MenuButton::builder()
            .icon_name("emblem-system-symbolic")
            .direction(gtk::ArrowType::Up)
            .menu_model(&settings_menu)
            .tooltip_text(tr("Settings"))
            .css_classes(["flat"])
            .build();
        let side_bottom = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        side_bottom.set_margin_start(6);
        side_bottom.set_margin_end(6);
        side_bottom.set_margin_top(4);
        side_bottom.set_margin_bottom(6);
        side_bottom.append(&vault_button);
        new_note.add_css_class("flat");
        let graph_button = gtk::Button::builder()
            .icon_name("gtkdown-graph-symbolic")
            .tooltip_text(tr("Graph"))
            .action_name("win.graph")
            .css_classes(["flat"])
            .build();
        side_bottom.append(&graph_button);
        side_bottom.append(&new_note);
        side_bottom.append(&settings_button);

        let sidebar = adw::ToolbarView::new();
        sidebar.add_top_bar(&side_header);
        sidebar.set_content(Some(&side_stack));
        sidebar.add_bottom_bar(&side_bottom);

        // Content: tabs.
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder().view(&tab_view).autohide(false).hexpand(true).build();
        let add_tab = gtk::Button::from_icon_name("list-add-symbolic");
        add_tab.set_tooltip_text(Some(tr("New tab")));
        add_tab.set_action_name(Some("win.new-tab"));
        add_tab.add_css_class("flat");
        tab_bar.set_end_action_widget(Some(&add_tab));

        let split = adw::OverlaySplitView::builder().sidebar(&sidebar).min_sidebar_width(200.0).build();
        let toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text(tr("Toggle sidebar"))
            .build();
        split.bind_property("show-sidebar", &toggle, "active").bidirectional().sync_create().build();

        let header = adw::HeaderBar::new();
        header.pack_start(&toggle);
        header.set_title_widget(Some(&tab_bar));
        split
            .bind_property("show-sidebar", &header, "show-start-title-buttons")
            .invert_boolean()
            .sync_create()
            .build();

        let status = gtk::Label::new(None);
        let backlinks_button = gtk::Button::builder().css_classes(["flat", "statusbar"]).build();
        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        status_bar.set_halign(gtk::Align::End);
        status_bar.add_css_class("statusbar");
        let vim_status = gtk::Label::builder().margin_end(12).build();
        status_bar.append(&vim_status);
        status_bar.append(&backlinks_button);
        status_bar.append(&status);

        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&tab_view));
        content.add_bottom_bar(&status_bar);
        split.set_content(Some(&content));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        // Responsive: sidebar overlays on narrow windows.
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            600.0,
            adw::LengthUnit::Sp,
        ));
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(bp);

        let theme_css = gtk::CssProvider::new();
        let display = gtk::prelude::WidgetExt::display(&window);
        gtk::style_context_add_provider_for_display(
            &display,
            &theme_css,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
        let font_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(&display, &font_css, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
        let base = gtk::CssProvider::new();
        base.load_from_string(theme::BASE_CSS);
        gtk::style_context_add_provider_for_display(&display, &base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        let user_css = crate::config::user_css_path();
        if user_css.is_file() {
            let p = gtk::CssProvider::new();
            p.load_from_path(&user_css);
            gtk::style_context_add_provider_for_display(&display, &p, gtk::STYLE_PROVIDER_PRIORITY_USER + 2);
        }

        let this = Rc::new(App {
            window,
            split,
            tab_view,
            toasts,
            status,
            backlinks_button,
            side_stack,
            search_entry,
            search_results,
            backlinks,
            bookmarks,
            tags,
            outline,
            vim_status,
            search_gen: Cell::new(0),
            backlinks_gen: Cell::new(0),
            vault_button,
            tree,
            styles,
            theme_css,
            font_css,
            config: RefCell::new(config),
            vault,
            tabs: RefCell::new(Vec::new()),
            switching: Cell::new(false),
        });
        *weak.borrow_mut() = Rc::downgrade(&this);
        this.setup_actions(app);
        this.setup_signals();
        this.apply_theme();
        this.apply_editor_style();
        this.rebuild_vault_menu();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn setup_actions(self: &Rc<Self>, app: &adw::Application) {
        let win_action = |name: &str, f: fn(&Rc<App>)| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(s) = weak.upgrade() {
                    f(&s);
                }
            });
            self.window.add_action(&a);
        };
        win_action("new-tab", |s| {
            let t = s.new_tab();
            s.tab_view.set_selected_page(&t.page);
        });
        win_action("close-tab", |s| {
            if let Some(p) = s.tab_view.selected_page() {
                s.tab_view.close_page(&p);
            }
        });
        win_action("new-note", |s| s.new_note());
        win_action("save", |s| s.save_all());
        win_action("toggle-sidebar", |s| s.split.set_show_sidebar(!s.split.shows_sidebar()));
        win_action("choose-vault", |s| s.choose_vault());
        win_action("quick-open", |s| s.quick_open());
        let path_action = |name: &str, f: fn(&Rc<App>, PathBuf)| {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, v| {
                if let (Some(s), Some(p)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) {
                    f(&s, PathBuf::from(p));
                }
            });
            self.window.add_action(&a);
        };
        path_action("note-in", |s, dir| s.new_note_in(&dir));
        path_action("folder-in", |s, dir| s.new_folder_in(&dir));
        path_action("rename", |s, p| s.rename_dialog(&p));
        path_action("delete", |s, p| s.delete_dialog(&p));
        path_action("open-tab", |s, p| s.open_file(p, true));
        path_action("bookmark", |s, p| s.toggle_bookmark(&p));
        let remove = gio::SimpleAction::new("bookmark-remove", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        remove.connect_activate(move |_, v| {
            let (Some(s), Some(v)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else { return };
            let index: Vec<usize> = v.split('/').filter_map(|i| i.parse().ok()).collect();
            if let Some(root) = s.vault.borrow().clone() {
                let _ = bookmarks::remove(&root, &index);
            }
            s.refresh_bookmarks();
        });
        self.window.add_action(&remove);
        win_action("rename-current", |s| {
            if let Some(p) = s.current_tab().and_then(|t| t.path()) {
                s.rename_dialog(&p);
            }
        });
        win_action("choose-font", |s| s.choose_font());
        win_action("reset-font", |s| {
            s.config.borrow_mut().font.clear();
            s.config.borrow().save();
            s.apply_editor_style();
        });
        win_action("search", |s| {
            s.show_side("search");
            s.search_entry.grab_focus();
        });
        win_action("backlinks", |s| s.show_side("backlinks"));
        win_action("bookmarks", |s| s.show_side("bookmarks"));
        win_action("tags", |s| s.show_side("tags"));
        win_action("outline", |s| s.show_side("outline"));
        win_action("files", |s| s.show_side("files"));
        win_action("graph", |s| s.open_graph());
        win_action("command-palette", |s| s.command_palette());
        win_action("bookmark-current", |s| {
            if let Some(p) = s.current_tab().and_then(|t| t.path()) {
                s.toggle_bookmark(&p);
            }
        });
        win_action("delete-current", |s| {
            if let Some(p) = s.current_tab().and_then(|t| t.path()) {
                s.delete_dialog(&p);
            }
        });
        win_action("toggle-vim", |s| {
            let on = !s.vim_enabled();
            s.config.borrow_mut().vim = if on { "on" } else { "off" }.into();
            s.config.borrow().save();
            s.apply_vim();
        });

        let open_vault = gio::SimpleAction::new("open-vault", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        open_vault.connect_activate(move |_, v| {
            if let (Some(s), Some(p)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) {
                s.open_vault(PathBuf::from(p));
            }
        });
        self.window.add_action(&open_vault);

        let theme_id = self.config.borrow().theme.clone();
        let theme = gio::SimpleAction::new_stateful("theme", Some(glib::VariantTy::STRING), &theme_id.to_variant());
        let weak = Rc::downgrade(self);
        theme.connect_activate(move |a, v| {
            let (Some(s), Some(v)) = (weak.upgrade(), v) else { return };
            a.set_state(v);
            s.config.borrow_mut().theme = v.get::<String>().unwrap_or_default();
            s.apply_theme();
            s.config.borrow().save();
        });
        app.add_action(&theme);

        let lang_id = self.config.borrow().lang.clone();
        let lang = gio::SimpleAction::new_stateful("lang", Some(glib::VariantTy::STRING), &lang_id.to_variant());
        let weak = Rc::downgrade(self);
        lang.connect_activate(move |a, v| {
            let (Some(s), Some(v)) = (weak.upgrade(), v) else { return };
            a.set_state(v);
            s.config.borrow_mut().lang = v.get::<String>().unwrap_or_default();
            s.config.borrow().save();
            s.toast(tr("Restart the app to change the language"));
        });
        app.add_action(&lang);

        let headings_id = self.config.borrow().headings.clone();
        let headings =
            gio::SimpleAction::new_stateful("headings", Some(glib::VariantTy::STRING), &headings_id.to_variant());
        let weak = Rc::downgrade(self);
        headings.connect_activate(move |a, v| {
            let (Some(s), Some(v)) = (weak.upgrade(), v) else { return };
            a.set_state(v);
            s.config.borrow_mut().headings = v.get::<String>().unwrap_or_default();
            s.config.borrow().save();
            s.apply_editor_style();
        });
        app.add_action(&headings);

        app.set_accels_for_action("win.new-tab", &["<Ctrl>t"]);
        app.set_accels_for_action("win.close-tab", &["<Ctrl>w"]);
        app.set_accels_for_action("win.new-note", &["<Ctrl>n"]);
        app.set_accels_for_action("win.save", &["<Ctrl>s"]);
        app.set_accels_for_action("win.toggle-sidebar", &["F9"]);
        app.set_accels_for_action("win.quick-open", &["<Ctrl>o"]);
        app.set_accels_for_action("win.rename-current", &["F2"]);
        app.set_accels_for_action("win.command-palette", &["<Ctrl>p"]);
        app.set_accels_for_action("win.graph", &["<Ctrl>g"]);
        app.set_accels_for_action("win.search", &["<Ctrl><Shift>f"]);
    }

    fn setup_signals(self: &Rc<Self>) {
        self.backlinks_button.set_action_name(Some("win.backlinks"));
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |_| {
            if let Some(s) = weak.upgrade() {
                s.run_search();
            }
        });
        let weak = Rc::downgrade(self);
        self.side_stack.connect_visible_child_name_notify(move |st| {
            let Some(s) = weak.upgrade() else { return };
            match st.visible_child_name().as_deref() {
                Some("backlinks") => s.refresh_backlinks(),
                Some("bookmarks") => s.refresh_bookmarks(),
                Some("tags") => s.refresh_tags(),
                Some("outline") => s.refresh_outline(),
                _ => {}
            }
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_close_page(move |tv, page| {
            if let Some(s) = weak.upgrade() {
                let tab = s.tab_for(page);
                if let Some(n) = tab.as_ref().and_then(|t| t.note()) {
                    n.save();
                    n.set_vim(false);
                }
                s.tabs.borrow_mut().retain(|t| &t.page != page);
                tv.close_page_finish(page, true);
                if tv.n_pages() == 0 && !s.switching.get() {
                    let t = s.new_tab();
                    tv.set_selected_page(&t.page);
                }
            }
            glib::Propagation::Stop
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_selected_page_notify(move |_| {
            if let Some(s) = weak.upgrade() {
                s.on_tab_switched();
            }
        });

        let sm = adw::StyleManager::default();
        for prop in ["dark", "accent-color"] {
            let weak = Rc::downgrade(self);
            sm.connect_notify_local(Some(prop), move |_, _| {
                if let Some(s) = weak.upgrade() {
                    s.apply_palette();
                }
            });
        }

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |w| {
            if let Some(s) = weak.upgrade() {
                s.save_all();
                s.store_session();
                for t in s.tabs.borrow().iter() {
                    if let Some(n) = t.note() {
                        n.set_vim(false);
                    }
                }
                let mut c = s.config.borrow_mut();
                c.maximized = w.is_maximized();
                if !c.maximized {
                    let (width, height) = w.default_size();
                    c.width = width;
                    c.height = height;
                }
                c.save();
            }
            glib::Propagation::Proceed
        });
    }

    fn apply_theme(&self) {
        let t = theme::by_id(&self.config.borrow().theme);
        adw::StyleManager::default().set_color_scheme(t.scheme);
        self.theme_css.load_from_string(&t.css());
        self.apply_palette();
    }

    fn apply_palette(&self) {
        let t = theme::by_id(&self.config.borrow().theme);
        let sm = adw::StyleManager::default();
        self.styles.apply(t, sm.is_dark(), Some(sm.accent_color_rgba()));
    }

    /// Editor font, monospace font and heading sizes: our settings first, then the vault's
    /// `.obsidian/appearance.json`, then defaults.
    fn apply_editor_style(self: &Rc<Self>) {
        let cfg = self.config.borrow().clone();
        let look = self.vault.borrow().as_deref().map(vault::appearance).unwrap_or_default();
        let (family, size) = if cfg.font.is_empty() {
            let size = format!("{}px", look.base_size.unwrap_or(16.0));
            (look.text_font.as_deref().map(css_family), size)
        } else {
            let d = gtk::pango::FontDescription::from_string(&cfg.font);
            let size = match d.size() {
                0 => "16px".to_string(),
                s => format!("{}pt", s as f64 / gtk::pango::SCALE as f64),
            };
            (d.family().map(|f| css_family(&f)), size)
        };
        let family = family.map(|f| format!("font-family: {f}; ")).unwrap_or_default();
        // The inline title sits outside the TextView, so it gets the font explicitly.
        let title_size = match size.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
            Some(i) => format!("{}{}", size[..i].parse::<f64>().unwrap_or(16.0) * 1.6, &size[i..]),
            None => size.clone(),
        };
        self.font_css.load_from_string(&format!(
            "textview.note {{ {family}font-size: {size}; }}\nentry.inline-title {{ {family}font-size: {title_size}; }}"
        ));

        let mono = look.mono_font.map(|m| m.replace(['"', '\''], "")).unwrap_or_default();
        self.styles.set_mono_font(&format!("{mono},monospace").trim_start_matches(','));
        let compact = match cfg.headings.as_str() {
            "compact" => true,
            "large" => false,
            _ => look.css_theme.as_deref() == Some("Minimal"),
        };
        self.styles.set_headings(compact);

        // List indents are measured in the new font, after the CSS has applied.
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(s) = weak.upgrade() {
                for t in s.tabs.borrow().iter() {
                    if let Some(n) = t.note() {
                        n.restyle();
                    }
                }
            }
        });
    }

    fn choose_font(self: &Rc<Self>) {
        let dialog = gtk::FontDialog::builder().title(tr("Editor font…")).modal(true).build();
        let current = self.config.borrow().font.clone();
        let initial = (!current.is_empty()).then(|| gtk::pango::FontDescription::from_string(&current));
        let weak = Rc::downgrade(self);
        dialog.choose_font(Some(&self.window), initial.as_ref(), gio::Cancellable::NONE, move |res| {
            if let (Some(s), Ok(d)) = (weak.upgrade(), res) {
                s.config.borrow_mut().font = d.to_str().to_string();
                s.config.borrow().save();
                s.apply_editor_style();
            }
        });
    }

    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    fn tab_for(&self, page: &adw::TabPage) -> Option<Rc<Tab>> {
        self.tabs.borrow().iter().find(|t| &t.page == page).cloned()
    }

    fn current_tab(&self) -> Option<Rc<Tab>> {
        self.tab_view.selected_page().and_then(|p| self.tab_for(&p))
    }

    fn new_tab(self: &Rc<Self>) -> Rc<Tab> {
        let bin = adw::Bin::new();
        let page = self.tab_view.append(&bin);
        let tab = Rc::new(Tab { page, bin, content: RefCell::new(Content::Empty) });
        self.tabs.borrow_mut().push(tab.clone());
        self.set_content(&tab, Content::Empty);
        tab
    }

    fn empty_page(self: &Rc<Self>) -> gtk::Widget {
        let page = adw::StatusPage::builder().icon_name("text-x-generic-symbolic").build();
        if self.vault.borrow().is_some() {
            page.set_title(tr("No file is open"));
            page.set_description(Some(tr("Open a note from the sidebar or create a new one")));
            let b = gtk::Button::with_label(tr("New note"));
            b.set_action_name(Some("win.new-note"));
            b.set_halign(gtk::Align::Center);
            b.add_css_class("pill");
            page.set_child(Some(&b));
        } else {
            page.set_title(tr("No vault"));
            page.set_description(Some(tr("Open a folder with notes — an Obsidian vault works as is")));
            let b = gtk::Button::with_label(tr("Open vault"));
            b.set_action_name(Some("win.choose-vault"));
            b.set_halign(gtk::Align::Center);
            b.add_css_class("pill");
            b.add_css_class("suggested-action");
            page.set_child(Some(&b));
        }
        page.upcast()
    }

    fn set_content(self: &Rc<Self>, tab: &Rc<Tab>, content: Content) {
        if let Some(old) = tab.note() {
            old.save();
            // The Vim input method must let go before the buffer is destroyed.
            old.set_vim(false);
        }
        let widget: gtk::Widget = match &content {
            Content::Empty => self.empty_page(),
            Content::Note(n) => n.widget.clone().upcast(),
            Content::Canvas(c) => c.widget.clone().upcast(),
            Content::Graph(g) => g.widget.clone().upcast(),
            Content::File(p) => {
                let pic = gtk::Picture::for_filename(p);
                pic.set_content_fit(gtk::ContentFit::ScaleDown);
                pic.set_margin_top(24);
                pic.set_margin_bottom(24);
                pic.upcast()
            }
        };
        tab.bin.set_child(Some(&widget));
        *tab.content.borrow_mut() = content;
        self.update_tab_title(tab);

        if let Some(n) = tab.note() {
            let (wa, wt) = (Rc::downgrade(self), Rc::downgrade(tab));
            *n.on_stats.borrow_mut() = Some(Box::new(move |words, chars| {
                if let (Some(a), Some(t)) = (wa.upgrade(), wt.upgrade())
                    && a.tab_view.selected_page().as_ref() == Some(&t.page)
                {
                    a.set_status(words, chars);
                    if a.side_stack.visible_child_name().as_deref() == Some("outline") {
                        a.refresh_outline();
                    }
                }
            }));
            let (wa, wt) = (Rc::downgrade(self), Rc::downgrade(tab));
            *n.on_vim_status.borrow_mut() = Some(Box::new(move |text| {
                if let (Some(a), Some(t)) = (wa.upgrade(), wt.upgrade())
                    && a.tab_view.selected_page().as_ref() == Some(&t.page)
                {
                    a.vim_status.set_label(&text);
                }
            }));
            let (wa, wt) = (Rc::downgrade(self), Rc::downgrade(tab));
            *n.on_quit.borrow_mut() = Some(Box::new(move || {
                if let (Some(a), Some(t)) = (wa.upgrade(), wt.upgrade()) {
                    a.tab_view.close_page(&t.page);
                }
            }));
            n.set_vim(self.vim_enabled());
            let wa = Rc::downgrade(self);
            *n.on_error.borrow_mut() = Some(Box::new(move |msg| {
                if let Some(a) = wa.upgrade() {
                    a.toast(&format!("{}: {msg}", tr("Cannot save file")));
                }
            }));
            let (wa, wt) = (Rc::downgrade(self), Rc::downgrade(tab));
            *n.on_gone.borrow_mut() = Some(Box::new(move || {
                if let (Some(a), Some(t)) = (wa.upgrade(), wt.upgrade()) {
                    a.set_content(&t, Content::Empty);
                }
            }));
            let (wa, wt) = (Rc::downgrade(self), Rc::downgrade(tab));
            *n.on_renamed.borrow_mut() = Some(Box::new(move || {
                if let (Some(a), Some(t)) = (wa.upgrade(), wt.upgrade()) {
                    a.update_tab_title(&t);
                }
            }));
            let (wa, wn) = (Rc::downgrade(self), Rc::downgrade(&n));
            *n.on_title.borrow_mut() = Some(Box::new(move |name| {
                let (Some(a), Some(n)) = (wa.upgrade(), wn.upgrade()) else { return };
                let from = n.path.borrow().clone();
                if name.contains('/') || name.starts_with('.') {
                    a.toast(tr("Invalid name"));
                    n.sync_title();
                    return;
                }
                let ext = from.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                let to = from.with_file_name(format!("{name}.{ext}"));
                a.move_path(&from, &to);
                n.sync_title();
            }));
            let (wa, wn) = (Rc::downgrade(self), Rc::downgrade(&n));
            *n.on_link.borrow_mut() = Some(Box::new(move |target, new_tab| {
                if let (Some(a), Some(n)) = (wa.upgrade(), wn.upgrade()) {
                    let from = n.path.borrow().clone();
                    a.follow_link(&from, &target, new_tab);
                }
            }));
        }
        if self.tab_view.selected_page().as_ref() == Some(&tab.page) {
            self.on_tab_switched();
        }
    }

    fn update_tab_title(&self, tab: &Tab) {
        match tab.path() {
            Some(p) => {
                tab.page.set_title(&p.file_stem().unwrap_or_default().to_string_lossy());
                tab.page.set_tooltip(&glib::markup_escape_text(&p.to_string_lossy()));
            }
            None => {
                let graph = matches!(*tab.content.borrow(), Content::Graph(_));
                tab.page.set_title(tr(if graph { "Graph" } else { "New tab" }));
                tab.page.set_tooltip("");
            }
        }
    }

    fn set_status(&self, words: usize, chars: usize) {
        self.status.set_label(&format!("{}: {words}   {}: {chars}", tr("Words"), tr("Characters")));
    }

    fn on_tab_switched(self: &Rc<Self>) {
        let tab = self.current_tab();
        let path = tab.as_ref().and_then(|t| t.path());
        self.tree.select(path.as_deref());
        match tab.as_ref().and_then(|t| t.note()) {
            Some(n) => {
                let text = n.text();
                self.set_status(note::count_words(&text), text.chars().count());
                n.view.grab_focus();
            }
            None => self.status.set_label(""),
        }
        self.refresh_backlinks();
        if let Some(t) = &tab
            && let Content::Graph(g) = &*t.content.borrow()
        {
            g.reload();
        }
        self.vim_status.set_label("");
        if self.side_stack.visible_child_name().as_deref() == Some("outline") {
            self.refresh_outline();
        }
        let title = path.and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()));
        self.window.set_title(Some(&match title {
            Some(t) => format!("{t} — gtkdown"),
            None => "gtkdown".into(),
        }));
    }

    pub fn open_file(self: &Rc<Self>, path: PathBuf, new_tab: bool) {
        self.open_at(path, new_tab, Goto::None);
    }

    pub fn open_at(self: &Rc<Self>, path: PathBuf, new_tab: bool, goto: Goto) {
        let existing = self.tabs.borrow().iter().find(|t| t.path().as_ref() == Some(&path)).cloned();
        if let Some(t) = existing {
            self.tab_view.set_selected_page(&t.page);
            Self::apply_goto(&t, goto);
            return;
        }
        let ext = tree::ext(&path.to_string_lossy()).unwrap_or_default();
        let content = if ext == "md" {
            let show_title = self
                .vault
                .borrow()
                .as_deref()
                .and_then(|v| crate::json::vault_file(v, "app.json"))
                .and_then(|j| j.get("showInlineTitle").and_then(|b| b.bool()))
                .unwrap_or(true);
            match Note::open(&path, self.styles.clone(), show_title) {
                Ok(n) => Content::Note(n),
                Err(e) => {
                    self.toast(&format!("{}: {e}", tr("Cannot open file")));
                    return;
                }
            }
        } else if ext == "canvas" {
            let root = self.vault.borrow().clone().unwrap_or_else(|| path.parent().unwrap_or(&path).to_path_buf());
            let weak = Rc::downgrade(self);
            match Canvas::open(&path, &root, move |p| {
                if let Some(a) = weak.upgrade() {
                    a.open_file(p, true);
                }
            }) {
                Ok(c) => Content::Canvas(c),
                Err(e) => {
                    self.toast(&format!("{}: {e}", tr("Cannot open file")));
                    return;
                }
            }
        } else if IMAGES.contains(&ext.as_str()) {
            Content::File(path)
        } else {
            gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(
                Some(&self.window),
                gio::Cancellable::NONE,
                |_| {},
            );
            return;
        };
        let tab = match self.current_tab() {
            Some(t) if !new_tab => t,
            _ => self.new_tab(),
        };
        self.set_content(&tab, content);
        self.tab_view.set_selected_page(&tab.page);
        Self::apply_goto(&tab, goto);
    }

    fn apply_goto(tab: &Tab, goto: Goto) {
        let Some(n) = tab.note() else { return };
        let offset = match goto {
            Goto::None => return,
            Goto::Offset(o) => Some(o),
            Goto::Heading(h) => vault::heading_offset(&n.text(), &h),
        };
        if let Some(o) = offset {
            n.goto(o);
        }
    }

    /// Opens a link target like Obsidian: external URLs in the browser, notes by name,
    /// missing notes are created (in the folder `.obsidian/app.json` asks for).
    fn follow_link(self: &Rc<Self>, from: &Path, target: &str, new_tab: bool) {
        if vault::is_external(target) {
            gtk::UriLauncher::new(target).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
            return;
        }
        let Some(root) = self.vault.borrow().clone() else { return };
        let (link, anchor) = vault::split_target(target);
        let goto = anchor.map_or(Goto::None, Goto::Heading);
        let files = vault::list_files(&root);
        if let Some(p) = vault::resolve(&root, &files, from, &link) {
            self.open_at(p, new_tab, goto);
            return;
        }
        if link.is_empty() || link.contains("..") {
            return;
        }
        let name = if link.to_lowercase().ends_with(".md") { link.clone() } else { format!("{link}.md") };
        let path = if link.contains('/') {
            root.join(name.trim_start_matches('/'))
        } else {
            vault::new_note_dir(&root, Some(from)).join(&name)
        };
        if tree::ext(&name).is_some_and(|e| e != "md") {
            return;
        }
        let made = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, ""));
        match made {
            Ok(()) => self.open_at(path, new_tab, Goto::None),
            Err(e) => self.toast(&format!("{}: {e}", tr("Cannot save file"))),
        }
    }

    fn tree_menu(self: &Rc<Self>, target: Option<(PathBuf, bool)>, widget: &gtk::Widget, x: f64, y: f64) {
        let Some(root) = self.vault.borrow().clone() else { return };
        let item = |m: &gio::Menu, label: &str, action: &str, p: &Path| {
            let it = gio::MenuItem::new(Some(label), None);
            it.set_action_and_target_value(Some(action), Some(&p.to_string_lossy().to_variant()));
            m.append_item(&it);
        };
        let dir = match &target {
            Some((p, true)) => p.clone(),
            Some((p, false)) => p.parent().map_or(root.clone(), Path::to_path_buf),
            None => root.clone(),
        };
        let menu = gio::Menu::new();
        let create = gio::Menu::new();
        item(&create, tr("New note"), "win.note-in", &dir);
        item(&create, tr("New folder"), "win.folder-in", &dir);
        menu.append_section(None, &create);
        if let Some((p, is_dir)) = &target {
            let ops = gio::Menu::new();
            if !is_dir {
                item(&ops, tr("Open in new tab"), "win.open-tab", p);
            }
            item(&ops, tr("Rename…"), "win.rename", p);
            item(&ops, tr("Bookmark"), "win.bookmark", p);
            item(&ops, tr("Delete"), "win.delete", p);
            menu.append_section(None, &ops);
        }
        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(widget);
        pop.set_has_arrow(false);
        pop.set_halign(gtk::Align::Start);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    fn new_note_in(self: &Rc<Self>, dir: &Path) {
        let path = fileops::free_name(dir, tr("Untitled"), "md");
        match std::fs::write(&path, "") {
            Ok(()) => self.open_file(path, false),
            Err(e) => self.toast(&format!("{}: {e}", tr("Cannot save file"))),
        }
    }

    fn new_folder_in(self: &Rc<Self>, dir: &Path) {
        let path = fileops::free_name(dir, tr("Untitled"), "");
        match std::fs::create_dir(&path) {
            Ok(()) => self.rename_dialog(&path),
            Err(e) => self.toast(&format!("{}: {e}", tr("Cannot save file"))),
        }
    }

    fn rename_dialog(self: &Rc<Self>, path: &Path) {
        let is_md = !path.is_dir() && tree::ext(&path.to_string_lossy()).is_some_and(|e| e == "md");
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let shown = if is_md { name.trim_end_matches(".md").to_string() } else { name.clone() };
        let entry = gtk::Entry::builder().text(&shown).activates_default(true).build();
        let dialog = adw::AlertDialog::builder().heading(tr("Rename")).extra_child(&entry).build();
        dialog.add_responses(&[("cancel", tr("Cancel")), ("rename", tr("Rename"))]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let path = path.to_path_buf();
        let e = entry.clone();
        dialog.connect_response(None, move |_, resp| {
            let Some(s) = weak.upgrade() else { return };
            let new = e.text().trim().to_string();
            if resp != "rename" || new.is_empty() || new == shown {
                return;
            }
            if new.contains('/') || new.starts_with('.') {
                s.toast(tr("Invalid name"));
                return;
            }
            let new = if is_md { format!("{new}.md") } else { new };
            let to = path.with_file_name(new);
            s.move_path(&path, &to);
        });
        dialog.present(Some(&self.window));
        entry.grab_focus();
        entry.select_region(0, -1);
    }

    fn delete_dialog(self: &Rc<Self>, path: &Path) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dialog = adw::AlertDialog::builder()
            .heading(format!("{} «{name}»?", tr("Delete")))
            .body(if path.is_dir() { tr("The folder and everything in it will be deleted.") } else { "" })
            .build();
        dialog.add_responses(&[("cancel", tr("Cancel")), ("delete", tr("Delete"))]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        let path = path.to_path_buf();
        dialog.connect_response(None, move |_, resp| {
            let Some(s) = weak.upgrade() else { return };
            if resp != "delete" {
                return;
            }
            let Some(root) = s.vault.borrow().clone() else { return };
            s.save_all();
            match fileops::delete(&root, &path) {
                Ok(()) => {
                    let gone: Vec<_> = s
                        .tabs
                        .borrow()
                        .iter()
                        .filter(|t| t.path().is_some_and(|p| p.starts_with(&path)))
                        .cloned()
                        .collect();
                    for t in gone {
                        s.set_content(&t, Content::Empty);
                    }
                }
                Err(e) => s.toast(&format!("{}: {e}", tr("Cannot delete"))),
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Moves/renames inside the vault, rewriting links and following open tabs.
    fn move_path(self: &Rc<Self>, from: &Path, to: &Path) {
        let Some(root) = self.vault.borrow().clone() else { return };
        if !from.starts_with(&root) || !to.starts_with(&root) {
            return;
        }
        self.save_all();
        match fileops::move_path(&root, from, to) {
            Ok(n) => {
                for t in self.tabs.borrow().iter() {
                    let Some(p) = t.path() else { continue };
                    let Ok(rest) = p.strip_prefix(from) else { continue };
                    let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
                    match &mut *t.content.borrow_mut() {
                        Content::Note(n) => n.set_path(new),
                        Content::File(f) => *f = new,
                        Content::Canvas(c) => c.set_path(new),
                        Content::Empty | Content::Graph(_) => {}
                    }
                    self.update_tab_title(t);
                }
                if n > 0 {
                    self.toast(&format!("{}: {n}", tr("Links updated in notes")));
                }
                self.on_tab_switched();
            }
            Err(e) => self.toast(&format!("{}: {e}", tr("Cannot move"))),
        }
    }

    fn quick_open(self: &Rc<Self>) {
        let Some(root) = self.vault.borrow().clone() else { return };
        let (w1, w2) = (Rc::downgrade(self), Rc::downgrade(self));
        let from = self.current_tab().and_then(|t| t.path());
        panels::quick_switcher(
            &self.window,
            root.clone(),
            move |p, new_tab| {
                if let Some(a) = w1.upgrade() {
                    a.open_at(p, new_tab, Goto::None);
                }
            },
            move |name, new_tab| {
                if let Some(a) = w2.upgrade() {
                    let from = from.clone().unwrap_or_else(|| root.join("."));
                    a.follow_link(&from, &name, new_tab);
                }
            },
        );
    }

    fn run_search(self: &Rc<Self>) {
        let Some(root) = self.vault.borrow().clone() else { return };
        let query = self.search_entry.text().to_string();
        let generation = self.search_gen.get() + 1;
        self.search_gen.set(generation);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let q = query.clone();
            let r = root.clone();
            let hits = gio::spawn_blocking(move || vault::search(&vault::list_files(&r), &q, 500))
                .await
                .unwrap_or_default();
            let Some(a) = weak.upgrade() else { return };
            if a.search_gen.get() != generation {
                return;
            }
            let empty = if query.trim().is_empty() { "" } else { tr("Nothing found") };
            a.search_results.set(hits, &root, empty);
        });
    }

    fn refresh_backlinks(self: &Rc<Self>) {
        let generation = self.backlinks_gen.get() + 1;
        self.backlinks_gen.set(generation);
        let root = self.vault.borrow().clone();
        let target = self.current_tab().and_then(|t| t.path());
        let (Some(root), Some(target)) = (root, target) else {
            self.backlinks_button.set_visible(false);
            self.backlinks.set(Vec::new(), Path::new("/"), "");
            return;
        };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (r, t) = (root.clone(), target.clone());
            let hits = gio::spawn_blocking(move || vault::backlinks(&r, &vault::list_files(&r), &t))
                .await
                .unwrap_or_default();
            let Some(a) = weak.upgrade() else { return };
            if a.backlinks_gen.get() != generation {
                return;
            }
            let files = {
                let mut v: Vec<_> = hits.iter().map(|h| &h.path).collect();
                v.dedup();
                v.len()
            };
            a.backlinks_button.set_label(&format!("{}: {files}", tr("Backlinks")));
            a.backlinks_button.set_visible(true);
            a.backlinks.set(hits, &root, tr("No backlinks"));
        });
    }

    fn vim_enabled(&self) -> bool {
        match self.config.borrow().vim.as_str() {
            "on" => true,
            "off" => false,
            _ => self
                .vault
                .borrow()
                .as_deref()
                .and_then(|v| crate::json::vault_file(v, "app.json"))
                .and_then(|j| j.get("vimMode").and_then(|b| b.bool()))
                .unwrap_or(false),
        }
    }

    fn apply_vim(&self) {
        let on = self.vim_enabled();
        for t in self.tabs.borrow().iter() {
            if let Some(n) = t.note() {
                n.set_vim(on);
            }
        }
        self.vim_status.set_label("");
        self.toast(if on { tr("Vim mode on") } else { tr("Vim mode off") });
    }

    fn rel(&self, p: &Path) -> Option<String> {
        let root = self.vault.borrow().clone()?;
        Some(p.strip_prefix(&root).ok()?.to_string_lossy().into_owned())
    }

    fn toggle_bookmark(self: &Rc<Self>, path: &Path) {
        let (Some(root), Some(rel)) = (self.vault.borrow().clone(), self.rel(path)) else { return };
        let res = match bookmarks::find_file(&root, &rel) {
            Some(i) => bookmarks::remove(&root, &i).map(|_| tr("Removed from bookmarks")),
            None => bookmarks::add_file(&root, &rel).map(|_| tr("Added to bookmarks")),
        };
        match res {
            Ok(msg) => self.toast(msg),
            Err(e) => self.toast(&format!("{}: {e}", tr("Cannot save file"))),
        }
        self.refresh_bookmarks();
    }

    fn refresh_bookmarks(self: &Rc<Self>) {
        let Some(root) = self.vault.borrow().clone() else { return };
        let rows = bookmarks::list(&root)
            .into_iter()
            .map(|b| {
                let heading = matches!(b.kind, bookmarks::Kind::Group);
                let weak = Rc::downgrade(self);
                let r = root.clone();
                let activate: Rc<dyn Fn()> = match b.kind {
                    bookmarks::Kind::File(p, sub) => Rc::new(move || {
                        if let Some(a) = weak.upgrade() {
                            let goto = sub.clone().map_or(Goto::None, Goto::Heading);
                            a.open_at(r.join(&p), false, goto);
                        }
                    }),
                    bookmarks::Kind::Search(q) => Rc::new(move || {
                        if let Some(a) = weak.upgrade() {
                            a.show_side("search");
                            a.search_entry.set_text(&q);
                        }
                    }),
                    bookmarks::Kind::Folder(p) => Rc::new(move || {
                        if let Some(a) = weak.upgrade() {
                            a.show_side("files");
                            a.tree.expand_path(&r.join(&p));
                        }
                    }),
                    _ => Rc::new(|| {}),
                };
                let index = b.index.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("/");
                let weak = Rc::downgrade(self);
                let menu: Rc<dyn Fn(&gtk::Widget, f64, f64)> = Rc::new(move |w, x, y| {
                    if let Some(a) = weak.upgrade() {
                        let m = gio::Menu::new();
                        let it = gio::MenuItem::new(Some(tr("Remove from bookmarks")), None);
                        it.set_action_and_target_value(Some("win.bookmark-remove"), Some(&index.to_variant()));
                        m.append_item(&it);
                        a.popup_menu(&m, w, x, y);
                    }
                });
                panels::Row { label: b.title, hint: String::new(), indent: b.depth, heading, activate, menu: Some(menu) }
            })
            .collect();
        self.bookmarks.set(rows, tr("No bookmarks"));
    }

    fn refresh_tags(self: &Rc<Self>) {
        let Some(root) = self.vault.borrow().clone() else { return };
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let r = root.clone();
            let tags = gio::spawn_blocking(move || vault::all_tags(&vault::list_files(&r))).await.unwrap_or_default();
            let Some(a) = weak.upgrade() else { return };
            let rows = tags
                .into_iter()
                .map(|(tag, n)| {
                    let weak = Rc::downgrade(&a);
                    let q = format!("#{tag}");
                    panels::Row {
                        label: q.clone(),
                        hint: n.to_string(),
                        indent: 0,
                        heading: false,
                        activate: Rc::new(move || {
                            if let Some(a) = weak.upgrade() {
                                a.show_side("search");
                                a.search_entry.set_text(&q);
                            }
                        }),
                        menu: None,
                    }
                })
                .collect();
            a.tags.set(rows, tr("No tags"));
        });
    }

    fn refresh_outline(self: &Rc<Self>) {
        let Some(n) = self.current_tab().and_then(|t| t.note()) else {
            self.outline.set(Vec::new(), tr("No headings"));
            return;
        };
        let min = n.headings.borrow().iter().map(|h| h.0).min().unwrap_or(1);
        let rows = n
            .headings
            .borrow()
            .iter()
            .map(|(level, text, offset)| {
                let (wn, off) = (Rc::downgrade(&n), *offset);
                panels::Row {
                    label: text.clone(),
                    hint: String::new(),
                    indent: (level - min) as usize,
                    heading: false,
                    activate: Rc::new(move || {
                        if let Some(n) = wn.upgrade() {
                            n.goto(off);
                        }
                    }),
                    menu: None,
                }
            })
            .collect();
        self.outline.set(rows, tr("No headings"));
    }

    fn popup_menu(&self, menu: &gio::Menu, widget: &gtk::Widget, x: f64, y: f64) {
        let pop = gtk::PopoverMenu::from_model(Some(menu));
        pop.set_parent(widget);
        pop.set_has_arrow(false);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Ctrl+P: every command with its shortcut.
    fn command_palette(self: &Rc<Self>) {
        let mut cmds: Vec<(String, String, Option<String>)> = [
            ("New note", "win.new-note"),
            ("New tab", "win.new-tab"),
            ("Close tab", "win.close-tab"),
            ("Quick switcher", "win.quick-open"),
            ("Search in vault…", "win.search"),
            ("Rename…", "win.rename-current"),
            ("Delete current note", "win.delete-current"),
            ("Bookmark current note", "win.bookmark-current"),
            ("Show files", "win.files"),
            ("Show bookmarks", "win.bookmarks"),
            ("Show tags", "win.tags"),
            ("Show outline", "win.outline"),
            ("Show backlinks", "win.backlinks"),
            ("Toggle sidebar", "win.toggle-sidebar"),
            ("Open graph view", "win.graph"),
            ("Vim mode", "win.toggle-vim"),
            ("Editor font…", "win.choose-font"),
            ("Open folder as vault…", "win.choose-vault"),
            ("Save", "win.save"),
        ]
        .into_iter()
        .map(|(l, a)| (tr(l).to_string(), a.to_string(), None))
        .collect();
        for t in theme::THEMES {
            cmds.push((format!("{}: {}", tr("Theme"), tr(t.name)), "app.theme".into(), Some(t.id.into())));
        }
        let app = self.window.application();
        let items = cmds
            .iter()
            .map(|(label, action, target)| {
                let detailed = match target {
                    Some(t) => format!("{action}::{t}"),
                    None => action.clone(),
                };
                let accel = app
                    .as_ref()
                    .and_then(|a| a.accels_for_action(&detailed).first().cloned())
                    .and_then(|a| gtk::accelerator_parse(&a))
                    .map(|(k, m)| gtk::accelerator_get_label(k, m).to_string())
                    .unwrap_or_default();
                (label.clone(), accel)
            })
            .collect();
        let weak = Rc::downgrade(self);
        panels::picker(&self.window, tr("Command palette"), tr("Type a command…"), items, move |i| {
            let Some(s) = weak.upgrade() else { return };
            let (_, action, target) = &cmds[i];
            let param = target.as_ref().map(|t| t.to_variant());
            let _ = gtk::prelude::WidgetExt::activate_action(&s.window, action, param.as_ref());
        });
    }

    /// Graph tab: reuses an open one, else opens a new tab.
    fn open_graph(self: &Rc<Self>) {
        let Some(root) = self.vault.borrow().clone() else { return };
        let existing = self.tabs.borrow().iter().find(|t| matches!(*t.content.borrow(), Content::Graph(_))).cloned();
        if let Some(t) = existing {
            self.tab_view.set_selected_page(&t.page);
            return;
        }
        let weak = Rc::downgrade(self);
        let g = Graph::new(&root, move |p, new_tab| {
            if let Some(a) = weak.upgrade() {
                // Keep the graph tab: notes open next to it.
                let on_graph = a.current_tab().is_some_and(|t| matches!(*t.content.borrow(), Content::Graph(_)));
                a.open_file(p, new_tab || on_graph);
            }
        });
        let tab = match self.current_tab() {
            Some(t) if matches!(*t.content.borrow(), Content::Empty) => t,
            _ => self.new_tab(),
        };
        self.set_content(&tab, Content::Graph(g.clone()));
        self.tab_view.set_selected_page(&tab.page);
        g.widget.grab_focus();
    }

    fn show_side(&self, name: &str) {
        self.split.set_show_sidebar(true);
        self.side_stack.set_visible_child_name(name);
    }

    fn new_note(self: &Rc<Self>) {
        let Some(vault) = self.vault.borrow().clone() else { return };
        let base = tr("Untitled");
        let dir = vault::new_note_dir(&vault, self.current_tab().and_then(|t| t.path()).as_deref());
        let path = (0..)
            .map(|i| if i == 0 { dir.join(format!("{base}.md")) } else { dir.join(format!("{base} {i}.md")) })
            .find(|p| !p.exists())
            .unwrap();
        if let Err(e) = std::fs::write(&path, "") {
            self.toast(&format!("{}: {e}", tr("Cannot save file")));
            return;
        }
        let blank = self.current_tab().is_some_and(|t| matches!(*t.content.borrow(), Content::Empty));
        self.open_file(path, !blank && self.current_tab().is_some_and(|t| t.note().is_some()));
    }

    fn save_all(&self) {
        for t in self.tabs.borrow().iter() {
            if let Some(n) = t.note() {
                n.save();
            }
        }
    }

    fn store_session(&self) {
        let Some(vault) = self.vault.borrow().clone() else { return };
        let tabs = self.tabs.borrow();
        let paths: Vec<String> = tabs
            .iter()
            .filter_map(|t| t.path())
            .filter_map(|p| p.strip_prefix(&vault).ok().map(|r| r.to_string_lossy().into_owned()))
            .collect();
        let active = self
            .current_tab()
            .and_then(|c| c.path())
            .and_then(|p| tabs.iter().filter_map(|t| t.path()).position(|q| q == p))
            .unwrap_or(0);
        let folders = self
            .tree
            .expanded_paths()
            .iter()
            .filter_map(|p| p.strip_prefix(&vault).ok().map(|r| r.to_string_lossy().into_owned()))
            .collect();
        self.config.borrow_mut().set_session(Session { vault, active, tabs: paths, folders });
    }

    pub fn open_vault(self: &Rc<Self>, dir: PathBuf) {
        let dir = dir.canonicalize().unwrap_or(dir);
        if self.vault.borrow().as_ref() == Some(&dir) {
            return;
        }
        self.save_all();
        self.store_session();
        self.switching.set(true);
        for t in self.tabs.take() {
            self.tab_view.close_page(&t.page);
        }
        self.switching.set(false);

        *self.vault.borrow_mut() = Some(dir.clone());
        self.apply_editor_style();
        self.search_entry.set_text("");
        self.search_results.set(Vec::new(), &dir, "");
        let folders = self
            .config
            .borrow()
            .session(&dir)
            .map(|s| s.folders.iter().map(|f| dir.join(f)).collect())
            .unwrap_or_default();
        self.tree.set_root(Some(&dir), folders);
        self.config.borrow_mut().remember_vault(&dir);

        let session = self.config.borrow().session(&dir).cloned();
        if let Some(s) = &session {
            for rel in &s.tabs {
                let p = dir.join(rel);
                if p.is_file() {
                    self.open_file(p, true);
                }
            }
        }
        if self.tabs.borrow().is_empty() {
            let t = self.new_tab();
            self.tab_view.set_selected_page(&t.page);
        } else if let Some(s) = &session {
            let active = s.tabs.get(s.active).map(|r| dir.join(r));
            if let Some(t) = self.tabs.borrow().iter().find(|t| t.path() == active) {
                self.tab_view.set_selected_page(&t.page);
            }
        }
        self.rebuild_vault_menu();
        self.config.borrow().save();
    }

    fn choose_vault(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title(tr("Open vault")).modal(true).build();
        let weak = Rc::downgrade(self);
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let (Some(s), Ok(f)) = (weak.upgrade(), res)
                && let Some(p) = f.path()
            {
                s.open_vault(p);
            }
        });
    }

    fn rebuild_vault_menu(&self) {
        let name = self
            .vault
            .borrow()
            .as_ref()
            .and_then(|v| v.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| tr("No vault").into());
        self.vault_button.set_label(&name);

        let menu = gio::Menu::new();
        let vaults = gio::Menu::new();
        for v in self.config.borrow().known_vaults() {
            let label = v.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("win.open-vault"), Some(&v.to_string_lossy().to_variant()));
            vaults.append_item(&item);
        }
        menu.append_section(None, &vaults);
        let actions = gio::Menu::new();
        actions.append(Some(tr("Open folder as vault…")), Some("win.choose-vault"));
        menu.append_section(None, &actions);
        self.vault_button.set_menu_model(Some(&menu));
    }

    /// Reopens the last vault on a plain launch.
    pub fn restore(self: &Rc<Self>) {
        let last = self.config.borrow().last_vault.clone();
        match last {
            Some(v) if v.is_dir() => self.open_vault(v),
            _ => self.ensure_tab(),
        }
    }

    pub fn ensure_tab(self: &Rc<Self>) {
        if self.tabs.borrow().is_empty() {
            let t = self.new_tab();
            self.tab_view.set_selected_page(&t.page);
        }
    }
}

/// Debug-build test driver: GTKDOWN_TEST="open:Arch/База.md;cursor:40;type:x;shot:/tmp/a.png;quit".
#[cfg(debug_assertions)]
impl App {
    pub fn run_script(self: &Rc<Self>, script: String) {
        let steps: Vec<String> = script.split(';').map(String::from).collect();
        self.clone().script_step(steps, 0);
    }

    fn script_step(self: Rc<Self>, steps: Vec<String>, i: usize) {
        let Some(step) = steps.get(i) else { return };
        let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
        let mut delay = 300;
        match cmd {
            "wait" => delay = arg.parse().unwrap_or(500),
            "open" => {
                let p = self.vault.borrow().clone().unwrap_or_default().join(arg);
                self.open_file(p, false);
            }
            "tab" => self.open_file(self.vault.borrow().clone().unwrap_or_default().join(arg), true),
            "cursor" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    let it = n.buffer.iter_at_offset(arg.parse().unwrap_or(0));
                    n.buffer.place_cursor(&it);
                    n.view.scroll_to_mark(&n.buffer.get_insert(), 0.0, true, 0.0, 0.3);
                }
            }
            "type" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    n.buffer.insert_at_cursor(&arg.replace("\\n", "\n"));
                }
            }
            "click-task" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    n.debug_toggle_task(arg.parse().unwrap_or(0));
                }
            }
            "theme" => {
                self.config.borrow_mut().theme = arg.into();
                self.apply_theme();
            }
            "size" => {
                let (w, h) = arg.split_once('x').unwrap_or(("1200", "800"));
                self.window.set_default_size(w.parse().unwrap_or(1200), h.parse().unwrap_or(800));
            }
            "shot" => {
                let paintable = gtk::WidgetPaintable::new(Some(&self.window));
                let (w, h) = (self.window.width(), self.window.height());
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, w as f64, h as f64);
                if let (Some(node), Some(r)) = (snap.to_node(), self.window.native().and_then(|n| n.renderer())) {
                    let tex = r.render_texture(&node, None);
                    let _ = tex.save_to_png(arg);
                }
            }
            "vault" => self.open_vault(PathBuf::from(arg)),
            "follow" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    n.debug_follow(arg.parse().unwrap_or(0), false);
                }
            }
            "keys" => {
                if let Some(e) = gtk::prelude::GtkWindowExt::focus(&self.window).and_downcast::<gtk::Text>() {
                    e.set_text(arg);
                }
            }
            "enter" => {
                let f = gtk::prelude::GtkWindowExt::focus(&self.window);
                if let Some(e) = f.and_then(|f| f.ancestor(gtk::SearchEntry::static_type())) {
                    e.emit_by_name::<()>("activate", &[]);
                }
            }
            "side" => self.show_side(arg),
            "action" => {
                let (name, param) = match arg.split_once('=') {
                    Some((n, p)) => (n, Some(p.to_variant())),
                    None => (arg, None),
                };
                let _ = gtk::prelude::WidgetExt::activate_action(&self.window, name, param.as_ref());
            }
            "move" => {
                let root = self.vault.borrow().clone().unwrap_or_default();
                let (a, b) = arg.split_once('>').unwrap_or((arg, arg));
                self.move_path(&root.join(a), &root.join(b));
            }
            "expand" => self.tree.debug_expand(&self.vault.borrow().clone().unwrap_or_default().join(arg)),
            "title" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    n.debug_title(arg);
                }
            }
            "focus-title" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    n.debug_focus_title();
                }
            }
            "print-title" => {
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    n.debug_print_title();
                }
            }
            "print-focus" => {
                let f = gtk::prelude::GtkWindowExt::focus(&self.window);
                eprintln!(
                    "FOCUS {:?} in_title={} active={}",
                    f.as_ref().map(|w| w.type_().name()),
                    f.and_then(|w| w.ancestor(gtk::Entry::static_type())).is_some(),
                    self.window.is_active()
                );
            }
            "present" => self.window.present(),
            "graph-hover" | "graph-open" | "graph-zoom" => {
                let g = self.tabs.borrow().iter().find_map(|t| match &*t.content.borrow() {
                    Content::Graph(g) => Some(g.clone()),
                    _ => None,
                });
                if let Some(g) = g {
                    match cmd {
                        "graph-hover" => g.debug_hover(arg),
                        "graph-open" => g.debug_open(arg),
                        _ => g.debug_zoom(arg.parse().unwrap_or(1.0)),
                    }
                }
            }
            "canvas-state" => {
                if let Some(t) = self.current_tab()
                    && let Content::Canvas(c) = &*t.content.borrow()
                {
                    eprintln!("CANVAS {}", c.debug_state());
                    c.widget.grab_focus();
                }
            }
            "print-search" => eprintln!("SEARCH {:?}", self.search_entry.text()),
            "treemenu" => {
                let root = self.vault.borrow().clone().unwrap_or_default();
                let p = root.join(arg);
                let dir = p.is_dir();
                self.tree_menu(Some((p, dir)), &self.tree.view.clone().upcast(), 60.0, 80.0);
            }
            "append" => {
                use std::io::Write;
                let (rel, text) = arg.split_once(':').unwrap_or((arg, ""));
                let p = self.vault.borrow().clone().unwrap_or_default().join(rel);
                let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
                let _ = f.write_all(text.replace("\\n", "\n").as_bytes());
            }
            "print" => {
                let p = self.vault.borrow().clone().unwrap_or_default().join(arg);
                eprintln!("PRINT {arg}: {:?}", std::fs::read_to_string(p).unwrap_or_default());
                if let Some(n) = self.current_tab().and_then(|t| t.note()) {
                    eprintln!("BUFFER: {:?}", n.text());
                }
            }
            "quit" => {
                self.window.close();
                return;
            }
            _ => eprintln!("gtkdown test: unknown step {step}"),
        }
        glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
            self.script_step(steps, i + 1)
        });
    }
}

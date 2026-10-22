//! One open markdown note: live-preview editor, autosave, reload on external change.

use crate::markdown::{self, Marker};
use crate::theme::Styles;
use adw::prelude::*;
use gtk::{gio, glib};
use sourceview5::prelude::*;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const AUTOSAVE: Duration = Duration::from_millis(1000);
const READABLE_WIDTH: i32 = 760;

pub struct Note {
    pub path: RefCell<PathBuf>,
    pub buffer: sourceview5::Buffer,
    pub view: sourceview5::View,
    pub widget: gtk::ScrolledWindow,
    styles: Rc<Styles>,
    saved: RefCell<String>,
    loading: Cell<bool>,
    save_timer: RefCell<Option<glib::SourceId>>,
    restyle_queued: Cell<bool>,
    /// Markers in char offsets, with whether each is currently hidden.
    markers: RefCell<Vec<(Marker, bool)>>,
    tasks: RefCell<Vec<Range<usize>>>,
    /// Link elements in char offsets with their raw targets.
    links: RefCell<Vec<(Range<usize>, String)>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    pub on_stats: RefCell<Option<Box<dyn Fn(usize, usize)>>>,
    pub on_error: RefCell<Option<Box<dyn Fn(String)>>>,
    pub on_gone: RefCell<Option<Box<dyn Fn()>>>,
    pub on_renamed: RefCell<Option<Box<dyn Fn()>>>,
    /// `(target, new_tab)` when a link is clicked.
    pub on_link: RefCell<Option<Box<dyn Fn(String, bool)>>>,
}

impl Note {
    pub fn open(path: &Path, styles: Rc<Styles>) -> std::io::Result<Rc<Note>> {
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not UTF-8"))?;

        let buffer = sourceview5::Buffer::new(Some(&styles.table));
        buffer.set_highlight_syntax(false);
        buffer.set_highlight_matching_brackets(false);
        // No GtkSourceView scheme: colours (background, caret, selection) come from the GTK theme.
        buffer.set_style_scheme(None::<&sourceview5::StyleScheme>);
        let view = sourceview5::View::with_buffer(&buffer);
        view.add_css_class("note");
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_pixels_below_lines(4);
        view.set_pixels_inside_wrap(2);
        view.set_top_margin(32);
        view.set_bottom_margin(200);
        view.set_left_margin(24);
        view.set_right_margin(24);
        view.set_auto_indent(true);
        view.set_smart_home_end(sourceview5::SmartHomeEndType::Before);

        let widget = gtk::ScrolledWindow::builder()
            .child(&view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .build();
        widget.add_css_class("note-scroll");

        let note = Rc::new(Note {
            path: RefCell::new(path.to_path_buf()),
            buffer,
            view,
            widget,
            styles,
            saved: RefCell::new(String::new()),
            loading: Cell::new(false),
            save_timer: RefCell::new(None),
            restyle_queued: Cell::new(false),
            markers: RefCell::new(Vec::new()),
            tasks: RefCell::new(Vec::new()),
            links: RefCell::new(Vec::new()),
            monitor: RefCell::new(None),
            on_stats: RefCell::new(None),
            on_error: RefCell::new(None),
            on_gone: RefCell::new(None),
            on_renamed: RefCell::new(None),
            on_link: RefCell::new(None),
        });
        note.load_text(&text);
        let start = note.buffer.start_iter();
        note.buffer.place_cursor(&start);
        note.connect();
        note.watch();
        Ok(note)
    }

    fn load_text(&self, text: &str) {
        self.loading.set(true);
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(text);
        self.buffer.end_irreversible_action();
        self.buffer.set_modified(false);
        *self.saved.borrow_mut() = text.to_owned();
        self.loading.set(false);
        self.restyle();
    }

    pub fn text(&self) -> String {
        let (s, e) = self.buffer.bounds();
        self.buffer.text(&s, &e, true).to_string()
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.buffer.connect_changed(move |_| {
            let Some(n) = weak.upgrade() else { return };
            if !n.loading.get() {
                n.schedule_save();
                n.queue_restyle();
            }
        });

        let weak = Rc::downgrade(self);
        self.buffer.connect_notify_local(Some("cursor-position"), move |_, _| {
            if let Some(n) = weak.upgrade() {
                n.update_hidden();
            }
        });
        let weak = Rc::downgrade(self);
        self.buffer.connect_mark_set(move |b, _, mark| {
            if mark == &b.selection_bound()
                && let Some(n) = weak.upgrade()
            {
                n.update_hidden();
            }
        });

        // Readable line length: keep the text column centred like Obsidian.
        let weak = Rc::downgrade(self);
        let adj = self.widget.hadjustment();
        adj.connect_page_size_notify(move |adj| {
            if let Some(n) = weak.upgrade() {
                let w = adj.page_size() as i32;
                let m = ((w - READABLE_WIDTH) / 2).max(24);
                if n.view.left_margin() != m {
                    n.view.set_left_margin(m);
                    n.view.set_right_margin(m);
                }
            }
        });

        // Click toggles a task box or follows a link (Ctrl or middle click: new tab).
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = Rc::downgrade(self);
        click.connect_released(move |g, n_press, x, y| {
            let Some(n) = weak.upgrade() else { return };
            let state = g.current_event_state();
            let button = g.current_button();
            if n_press != 1 || state.contains(gtk::gdk::ModifierType::SHIFT_MASK) || button > 2 {
                return;
            }
            if n.buffer.has_selection() {
                return;
            }
            let Some(off) = n.offset_at(x, y) else { return };
            if button == 1 {
                let hit = n.tasks.borrow().iter().find(|r| r.start <= off && off < r.end).cloned();
                if let Some(r) = hit {
                    n.toggle_task(r);
                    return;
                }
            }
            if let Some(target) = n.link_at(off) {
                let new_tab = button == 2 || state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                if let Some(cb) = &*n.on_link.borrow() {
                    cb(target, new_tab);
                }
            }
        });
        self.view.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            let Some(n) = weak.upgrade() else { return };
            let over = n.offset_at(x, y).is_some_and(|o| {
                n.link_at(o).is_some() || n.tasks.borrow().iter().any(|r| r.start <= o && o < r.end)
            });
            n.view.set_cursor_from_name(Some(if over { "pointer" } else { "text" }));
        });
        self.view.add_controller(motion);
    }

    fn offset_at(&self, x: f64, y: f64) -> Option<usize> {
        let (bx, by) = self.view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let (iter, _) = self.view.iter_at_position(bx, by)?;
        // Ignore clicks past the end of a line.
        let loc = self.view.iter_location(&iter);
        if bx > loc.x() + loc.width() + 2 && iter.ends_line() {
            return None;
        }
        Some(iter.offset() as usize)
    }

    fn link_at(&self, off: usize) -> Option<String> {
        self.links.borrow().iter().find(|(r, _)| r.start <= off && off < r.end).map(|(_, t)| t.clone())
    }

    /// Puts the cursor at `offset` and scrolls it near the top once layout is ready.
    pub fn goto(self: &Rc<Self>, offset: usize) {
        let it = self.buffer.iter_at_offset(offset as i32);
        self.buffer.place_cursor(&it);
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(n) = weak.upgrade() {
                n.view.scroll_to_mark(&n.buffer.get_insert(), 0.0, true, 0.0, 0.15);
                n.view.grab_focus();
            }
        });
    }

    fn toggle_task(&self, r: Range<usize>) {
        let b = &self.buffer;
        let mut s = b.iter_at_offset(r.start as i32 + 1);
        let mut e = b.iter_at_offset(r.start as i32 + 2);
        let cur = b.text(&s, &e, true);
        let next = if cur == " " { "x" } else { " " };
        b.begin_user_action();
        b.delete(&mut s, &mut e);
        b.insert(&mut s, next);
        b.end_user_action();
    }

    fn queue_restyle(self: &Rc<Self>) {
        if self.restyle_queued.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
            if let Some(n) = weak.upgrade() {
                n.restyle_queued.set(false);
                n.restyle();
            }
            glib::ControlFlow::Break
        });
    }

    /// Re-parses the whole note and reapplies tags. Fast enough for typical notes.
    pub fn restyle(&self) {
        let text = self.text();
        let parsed = markdown::parse(&text);
        let map = CharMap::new(&text);
        let b = &self.buffer;
        let (start, end) = b.bounds();
        b.remove_all_tags(&start, &end);

        let iter = |o: usize| b.iter_at_offset(map.get(o) as i32);
        for span in &parsed.spans {
            b.apply_tag(self.styles.tag(span.style), &iter(span.range.start), &iter(span.range.end));
        }
        let conv = |r: &Range<usize>| map.get(r.start)..map.get(r.end);
        *self.tasks.borrow_mut() = parsed.tasks.iter().map(conv).collect();
        *self.links.borrow_mut() = parsed.links.iter().map(|l| (conv(&l.range), l.target.clone())).collect();
        *self.markers.borrow_mut() = parsed
            .markers
            .iter()
            .map(|m| (Marker { range: conv(&m.range), reveal: conv(&m.reveal) }, false))
            .collect();
        self.update_hidden();

        if let Some(cb) = &*self.on_stats.borrow() {
            cb(count_words(&text), text.chars().count());
        }
    }

    /// Hides syntax markers except those around the cursor or selection.
    fn update_hidden(&self) {
        let b = &self.buffer;
        let a = b.iter_at_mark(&b.get_insert()).offset() as usize;
        let c = b.iter_at_mark(&b.selection_bound()).offset() as usize;
        let (lo, hi) = (a.min(c), a.max(c));
        let mut markers = self.markers.borrow_mut();
        for (m, hidden) in markers.iter_mut() {
            let reveal = m.reveal.start <= hi && lo <= m.reveal.end;
            if *hidden == !reveal {
                continue;
            }
            *hidden = !reveal;
            let s = b.iter_at_offset(m.range.start as i32);
            let e = b.iter_at_offset(m.range.end as i32);
            if *hidden {
                b.apply_tag(&self.styles.hidden, &s, &e);
            } else {
                b.remove_tag(&self.styles.hidden, &s, &e);
            }
        }
    }

    fn schedule_save(self: &Rc<Self>) {
        if let Some(id) = self.save_timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(AUTOSAVE, move || {
            if let Some(n) = weak.upgrade() {
                n.save_timer.replace(None);
                n.save();
            }
        });
        self.save_timer.replace(Some(id));
    }

    /// Writes to disk if the buffer differs from what was last saved/loaded.
    pub fn save(&self) {
        if let Some(id) = self.save_timer.take() {
            id.remove();
        }
        let text = self.text();
        if *self.saved.borrow() == text {
            return;
        }
        let path = self.path.borrow().clone();
        match std::fs::write(&path, &text) {
            Ok(()) => *self.saved.borrow_mut() = text,
            Err(e) => {
                if let Some(cb) = &*self.on_error.borrow() {
                    cb(format!("{}: {e}", path.display()));
                }
            }
        }
    }

    fn watch(self: &Rc<Self>) {
        let file = gio::File::for_path(&*self.path.borrow());
        let Ok(mon) = file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
            return;
        };
        let weak = Rc::downgrade(self);
        mon.connect_changed(move |_, _, other, ev| {
            let Some(n) = weak.upgrade() else { return };
            match ev {
                gio::FileMonitorEvent::Renamed => {
                    if let Some(p) = other.and_then(|o| o.path()) {
                        *n.path.borrow_mut() = p;
                        n.watch();
                        if let Some(cb) = &*n.on_renamed.borrow() {
                            cb();
                        }
                    }
                }
                gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::MovedIn
                | gio::FileMonitorEvent::Deleted
                | gio::FileMonitorEvent::MovedOut => n.check_disk(),
                _ => {}
            }
        });
        self.monitor.replace(Some(mon));
    }

    /// Debounced: atomic saves by other apps look like delete + create.
    fn check_disk(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            let Some(n) = weak.upgrade() else { return };
            let path = n.path.borrow().clone();
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let Ok(disk) = String::from_utf8(bytes) else { return };
                    let dirty = n.text() != *n.saved.borrow();
                    if disk != *n.saved.borrow() && !dirty {
                        let off = n.buffer.cursor_position();
                        n.load_text(&disk);
                        let it = n.buffer.iter_at_offset(off);
                        n.buffer.place_cursor(&it);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if let Some(cb) = &*n.on_gone.borrow() {
                        cb();
                    }
                }
                Err(_) => {}
            }
        });
    }
}

impl Drop for Note {
    fn drop(&mut self) {
        if let Some(id) = self.save_timer.take() {
            id.remove();
        }
    }
}

/// Byte offset -> char offset.
struct CharMap(Option<Vec<u32>>);

impl CharMap {
    fn new(text: &str) -> CharMap {
        if text.is_ascii() {
            return CharMap(None);
        }
        let mut v = vec![0u32; text.len() + 1];
        let mut n = 0u32;
        for (i, ch) in text.char_indices() {
            for b in 0..ch.len_utf8() {
                v[i + b] = n;
            }
            n += 1;
        }
        v[text.len()] = n;
        CharMap(Some(v))
    }

    fn get(&self, byte: usize) -> usize {
        match &self.0 {
            None => byte,
            Some(v) => v[byte] as usize,
        }
    }
}

pub fn count_words(text: &str) -> usize {
    text.split(|c: char| c.is_whitespace())
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

#[cfg(debug_assertions)]
impl Note {
    pub fn debug_follow(&self, offset: usize, new_tab: bool) {
        if let (Some(t), Some(cb)) = (self.link_at(offset), &*self.on_link.borrow()) {
            cb(t, new_tab);
        }
    }

    pub fn debug_toggle_task(&self, i: usize) {
        let r = self.tasks.borrow().get(i).cloned();
        if let Some(r) = r {
            self.toggle_task(r);
        }
    }
}

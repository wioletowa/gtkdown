//! Graph view: notes as nodes, resolved links as edges, laid out by a small force simulation.

use crate::markdown;
use crate::vault::{is_external, is_note, list_files, split_target, Names};
use adw::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Notes and the undirected links between them (indices into `paths`).
#[derive(Default, Debug)]
pub struct Data {
    pub paths: Vec<PathBuf>,
    pub edges: Vec<(usize, usize)>,
}

/// Scans the vault; runs on a worker thread.
pub fn build(root: &Path) -> Data {
    let files = list_files(root);
    let paths: Vec<PathBuf> = files.iter().filter(|f| is_note(f)).cloned().collect();
    let index: HashMap<&Path, usize> = paths.iter().enumerate().map(|(i, p)| (p.as_path(), i)).collect();
    let names = Names::new(&files);
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        if !text.contains("[[") && !text.contains("](") {
            continue;
        }
        for l in markdown::parse(&text).links {
            if is_external(&l.target) {
                continue;
            }
            let (link, _) = split_target(&l.target);
            if link.is_empty() {
                continue;
            }
            let Some(j) = names.resolve(root, p, &link).and_then(|t| index.get(t.as_path()).copied()) else {
                continue;
            };
            let key = (i.min(j), i.max(j));
            if i != j && seen.insert(key) {
                edges.push(key);
            }
        }
    }
    Data { paths, edges }
}

/// One step of the layout. Returns the total movement, to know when to stop.
pub fn step(pos: &mut [(f64, f64)], vel: &mut [(f64, f64)], edges: &[(usize, usize)], pinned: Option<usize>, alpha: f64) -> f64 {
    let n = pos.len();
    let mut force = vec![(0.0, 0.0); n];
    for i in 0..n {
        for j in i + 1..n {
            let (dx, dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
            let d2 = (dx * dx + dy * dy).max(25.0);
            if d2 > 250_000.0 {
                continue;
            }
            let f = 4000.0 / d2;
            let d = d2.sqrt();
            let (fx, fy) = (dx / d * f, dy / d * f);
            force[i].0 += fx;
            force[i].1 += fy;
            force[j].0 -= fx;
            force[j].1 -= fy;
        }
    }
    for &(a, b) in edges {
        let (dx, dy) = (pos[b].0 - pos[a].0, pos[b].1 - pos[a].1);
        let d = (dx * dx + dy * dy).sqrt().max(0.01);
        let f = (d - 45.0) * 0.06;
        let (fx, fy) = (dx / d * f, dy / d * f);
        force[a].0 += fx;
        force[a].1 += fy;
        force[b].0 -= fx;
        force[b].1 -= fy;
    }
    let mut moved = 0.0;
    for i in 0..n {
        if Some(i) == pinned {
            vel[i] = (0.0, 0.0);
            continue;
        }
        // Gentle pull to the centre keeps unlinked notes around.
        force[i].0 -= pos[i].0 * 0.004;
        force[i].1 -= pos[i].1 * 0.004;
        vel[i].0 = ((vel[i].0 + force[i].0 * alpha) * 0.6).clamp(-30.0, 30.0);
        vel[i].1 = ((vel[i].1 + force[i].1 * alpha) * 0.6).clamp(-30.0, 30.0);
        pos[i].0 += vel[i].0;
        pos[i].1 += vel[i].1;
        moved += vel[i].0.abs() + vel[i].1.abs();
    }
    moved
}

/// Deterministic starting spot on a spiral, so layouts are stable between openings.
fn seed(i: usize) -> (f64, f64) {
    let a = i as f64 * 2.399963;
    let r = 12.0 * (i as f64 + 1.0).sqrt();
    (r * a.cos(), r * a.sin())
}

pub struct Graph {
    pub widget: gtk::DrawingArea,
    data: RefCell<Data>,
    degree: RefCell<Vec<usize>>,
    pos: RefCell<Vec<(f64, f64)>>,
    vel: RefCell<Vec<(f64, f64)>>,
    alpha: Cell<f64>,
    running: Cell<bool>,
    zoom: Cell<f64>,
    pan: Cell<(f64, f64)>,
    hover: Cell<Option<usize>>,
    dragging: Cell<Option<usize>>,
    /// The user moved the view, so don't auto-fit any more.
    touched: Cell<bool>,
    root: PathBuf,
    on_open: Box<dyn Fn(PathBuf, bool)>,
}

impl Graph {
    pub fn new(root: &Path, on_open: impl Fn(PathBuf, bool) + 'static) -> Rc<Graph> {
        let widget = gtk::DrawingArea::builder().hexpand(true).vexpand(true).focusable(true).build();
        widget.add_css_class("graph");
        let g = Rc::new(Graph {
            widget,
            data: RefCell::default(),
            degree: RefCell::default(),
            pos: RefCell::default(),
            vel: RefCell::default(),
            alpha: Cell::new(1.0),
            running: Cell::new(false),
            zoom: Cell::new(1.0),
            pan: Cell::new((0.0, 0.0)),
            hover: Cell::new(None),
            dragging: Cell::new(None),
            touched: Cell::new(false),
            root: root.to_path_buf(),
            on_open: Box::new(on_open),
        });
        g.connect();
        g.reload();
        g
    }

    /// Rescans the vault in the background, keeping positions of notes already shown.
    pub fn reload(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let root = self.root.clone();
        glib::spawn_future_local(async move {
            let data = gtk::gio::spawn_blocking(move || build(&root)).await.unwrap_or_default();
            let Some(g) = weak.upgrade() else { return };
            let old: HashMap<PathBuf, (f64, f64)> = {
                let d = g.data.borrow();
                d.paths.iter().cloned().zip(g.pos.borrow().iter().copied()).collect()
            };
            let pos: Vec<_> = data.paths.iter().enumerate().map(|(i, p)| old.get(p).copied().unwrap_or_else(|| seed(i))).collect();
            let mut degree = vec![0; data.paths.len()];
            for &(a, b) in &data.edges {
                degree[a] += 1;
                degree[b] += 1;
            }
            let changed = data.paths.len() != old.len() || data.edges.len() != g.data.borrow().edges.len();
            *g.vel.borrow_mut() = vec![(0.0, 0.0); pos.len()];
            *g.pos.borrow_mut() = pos;
            *g.degree.borrow_mut() = degree;
            *g.data.borrow_mut() = data;
            if changed {
                g.heat(1.0);
            }
            g.widget.queue_draw();
        });
    }

    fn heat(self: &Rc<Self>, alpha: f64) {
        self.alpha.set(self.alpha.get().max(alpha));
        if self.running.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        let ticks = Cell::new(0u32);
        self.widget.add_tick_callback(move |w, _| {
            let Some(g) = weak.upgrade() else { return glib::ControlFlow::Break };
            let moved = {
                let edges = &g.data.borrow().edges;
                step(&mut g.pos.borrow_mut(), &mut g.vel.borrow_mut(), edges, g.dragging.get(), g.alpha.get())
            };
            g.alpha.set(g.alpha.get() * 0.985);
            ticks.set(ticks.get() + 1);
            if !g.touched.get() && (ticks.get() == 30 || ticks.get() % 60 == 0) {
                g.fit();
            }
            w.queue_draw();
            let n = g.pos.borrow().len().max(1) as f64;
            if g.dragging.get().is_none() && (g.alpha.get() < 0.02 || moved / n < 0.02) {
                g.running.set(false);
                if !g.touched.get() {
                    g.fit();
                }
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn fit(&self) {
        let pos = self.pos.borrow();
        let (w, h) = (self.widget.width() as f64, self.widget.height() as f64);
        if pos.is_empty() || w < 10.0 {
            self.pan.set((w / 2.0, h / 2.0));
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in pos.iter() {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let z = ((w - 120.0) / (x1 - x0).max(1.0)).min((h - 120.0) / (y1 - y0).max(1.0)).clamp(0.05, 2.0);
        self.zoom.set(z);
        self.pan.set((w / 2.0 - (x0 + x1) / 2.0 * z, h / 2.0 - (y0 + y1) / 2.0 * z));
        self.widget.queue_draw();
    }

    fn to_world(&self, x: f64, y: f64) -> (f64, f64) {
        let (px, py) = self.pan.get();
        let z = self.zoom.get();
        ((x - px) / z, (y - py) / z)
    }

    fn radius(&self, i: usize) -> f64 {
        4.0 + (self.degree.borrow().get(i).copied().unwrap_or(0) as f64).sqrt() * 2.0
    }

    fn node_at(&self, x: f64, y: f64) -> Option<usize> {
        let (wx, wy) = self.to_world(x, y);
        let slack = 4.0 / self.zoom.get();
        self.pos
            .borrow()
            .iter()
            .enumerate()
            .map(|(i, &(nx, ny))| (i, (nx - wx).hypot(ny - wy) - self.radius(i)))
            .filter(|&(_, d)| d < slack)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    fn zoom_at(&self, factor: f64, sx: f64, sy: f64) {
        let z = self.zoom.get();
        let nz = (z * factor).clamp(0.05, 5.0);
        let (px, py) = self.pan.get();
        self.pan.set((sx - (sx - px) * nz / z, sy - (sy - py) * nz / z));
        self.zoom.set(nz);
        self.touched.set(true);
        self.widget.queue_draw();
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.widget.set_draw_func(move |area, cr, _, _| {
            if let Some(g) = weak.upgrade() {
                g.draw(area, cr);
            }
        });

        let motion = gtk::EventControllerMotion::new();
        let pointer = Rc::new(Cell::new((0.0, 0.0)));
        let (weak, p) = (Rc::downgrade(self), pointer.clone());
        motion.connect_motion(move |_, x, y| {
            p.set((x, y));
            let Some(g) = weak.upgrade() else { return };
            let hit = g.node_at(x, y);
            if hit != g.hover.get() {
                g.hover.set(hit);
                g.widget.set_cursor_from_name(Some(if hit.is_some() { "pointer" } else { "default" }));
                g.widget.queue_draw();
            }
        });
        self.widget.add_controller(motion);

        // Drag a node to move it, drag empty space to pan; a click without movement opens.
        let drag = gtk::GestureDrag::new();
        drag.set_button(0);
        let origin = Rc::new(Cell::new((0.0, 0.0)));
        let (weak, o) = (Rc::downgrade(self), origin.clone());
        drag.connect_drag_begin(move |_, x, y| {
            let Some(g) = weak.upgrade() else { return };
            g.widget.grab_focus();
            match g.node_at(x, y) {
                Some(i) => {
                    g.dragging.set(Some(i));
                    o.set(g.pos.borrow()[i]);
                }
                None => o.set(g.pan.get()),
            }
        });
        let (weak, o) = (Rc::downgrade(self), origin.clone());
        drag.connect_drag_update(move |_, dx, dy| {
            let Some(g) = weak.upgrade() else { return };
            if dx.abs() + dy.abs() < 3.0 {
                return;
            }
            let (ox, oy) = o.get();
            match g.dragging.get() {
                Some(i) => {
                    let z = g.zoom.get();
                    g.pos.borrow_mut()[i] = (ox + dx / z, oy + dy / z);
                    g.heat(0.3);
                }
                None => {
                    g.pan.set((ox + dx, oy + dy));
                    g.touched.set(true);
                }
            }
            g.widget.queue_draw();
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |gesture, dx, dy| {
            let Some(g) = weak.upgrade() else { return };
            let node = g.dragging.take();
            if dx.abs() + dy.abs() < 3.0
                && let Some(i) = node
            {
                let new_tab = gesture.current_button() == 2
                    || gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
                let path = g.data.borrow().paths.get(i).cloned();
                if let Some(p) = path {
                    (g.on_open)(p, new_tab);
                }
            }
        });
        self.widget.add_controller(drag);

        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let weak = Rc::downgrade(self);
        wheel.connect_scroll(move |_, _, dy| {
            if let Some(g) = weak.upgrade() {
                let (x, y) = pointer.get();
                g.zoom_at(if dy < 0.0 { 1.15 } else { 1.0 / 1.15 }, x, y);
            }
            glib::Propagation::Stop
        });
        self.widget.add_controller(wheel);

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(g) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (cx, cy) = (g.widget.width() as f64 / 2.0, g.widget.height() as f64 / 2.0);
            match key {
                gdk::Key::_0 | gdk::Key::Home => {
                    g.touched.set(false);
                    g.fit();
                }
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => g.zoom_at(1.2, cx, cy),
                gdk::Key::minus | gdk::Key::KP_Subtract => g.zoom_at(1.0 / 1.2, cx, cy),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.widget.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.widget.connect_resize(move |_, _, _| {
            if let Some(g) = weak.upgrade()
                && !g.touched.get()
            {
                g.fit();
            }
        });
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context) {
        let fg = area.color();
        let accent = adw::StyleManager::default().accent_color_rgba();
        let rgba = |c: &gdk::RGBA, a: f64| cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, a);
        let data = self.data.borrow();
        let pos = self.pos.borrow();
        let z = self.zoom.get();
        let (px, py) = self.pan.get();
        let hover = self.hover.get().or(self.dragging.get());
        let near: HashSet<usize> = hover
            .map(|h| {
                data.edges.iter().filter_map(|&(a, b)| (a == h).then_some(b).or((b == h).then_some(a))).chain([h]).collect()
            })
            .unwrap_or_default();
        let dim = |i: usize| hover.is_some() && !near.contains(&i);

        let screen = |i: usize| (pos[i].0 * z + px, pos[i].1 * z + py);
        cr.set_line_width(1.0);
        for &(a, b) in &data.edges {
            let lit = hover.is_some_and(|h| a == h || b == h);
            if lit {
                rgba(&accent, 0.8);
            } else {
                rgba(&fg, if hover.is_some() { 0.05 } else { 0.14 });
            }
            let ((ax, ay), (bx, by)) = (screen(a), screen(b));
            cr.move_to(ax, ay);
            cr.line_to(bx, by);
            let _ = cr.stroke();
        }
        for i in 0..pos.len() {
            let (x, y) = screen(i);
            let r = (self.radius(i) * z).max(2.0);
            if Some(i) == hover {
                rgba(&accent, 1.0);
            } else {
                rgba(&fg, if dim(i) { 0.12 } else { 0.55 });
            }
            cr.arc(x, y, r, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        // Labels fade in as you zoom; hovered neighbourhood always labelled.
        let base = ((z - 1.2) / 0.6).clamp(0.0, 1.0);
        let mut font = area.pango_context().font_description().unwrap_or_default();
        font.set_size((font.size() as f64 * 0.85) as i32);
        for i in 0..pos.len() {
            let a = if near.contains(&i) { 1.0 } else if dim(i) { base * 0.15 } else { base * 0.85 };
            if a <= 0.01 {
                continue;
            }
            let name = data.paths[i].file_stem().unwrap_or_default().to_string_lossy();
            let layout = area.create_pango_layout(Some(&name));
            layout.set_font_description(Some(&font));
            let (w, _) = layout.pixel_size();
            let (x, y) = screen(i);
            rgba(&fg, a);
            cr.move_to(x - w as f64 / 2.0, y + (self.radius(i) * z).max(2.0) + 3.0);
            pangocairo::functions::show_layout(cr, &layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_settles() {
        let root = std::env::temp_dir().join(format!("gtkdown-graph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("A.md"), "[[B]] [[B]] [[a/C]] [x](https://e.x) [[Missing]]").unwrap();
        std::fs::write(root.join("B.md"), "[[A]]").unwrap();
        std::fs::write(root.join("a/C.md"), "").unwrap();
        std::fs::write(root.join("D.md"), "alone").unwrap();
        let d = build(&root);
        assert_eq!(d.paths.len(), 4);
        assert_eq!(d.edges.len(), 2);

        let mut pos: Vec<_> = (0..4).map(seed).collect();
        let mut vel = vec![(0.0, 0.0); 4];
        let mut alpha = 1.0;
        for _ in 0..400 {
            step(&mut pos, &mut vel, &d.edges, None, alpha);
            alpha *= 0.985;
        }
        // Linked notes end up closer than the unlinked one.
        let dist = |a: usize, b: usize| (pos[a].0 - pos[b].0).hypot(pos[a].1 - pos[b].1);
        let idx = |n: &str| d.paths.iter().position(|p| p.ends_with(n)).unwrap();
        let (a, b, dd) = (idx("A.md"), idx("B.md"), idx("D.md"));
        assert!(dist(a, b) < dist(a, dd), "{pos:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn step_speed() {
        let root = std::path::PathBuf::from(std::env::var("GRAPH_VAULT").unwrap());
        let t = std::time::Instant::now();
        let d = super::build(&root);
        eprintln!("build: {} notes, {} edges, {:?}", d.paths.len(), d.edges.len(), t.elapsed());
        let mut pos: Vec<_> = (0..d.paths.len()).map(super::seed).collect();
        let mut vel = vec![(0.0, 0.0); pos.len()];
        let t = std::time::Instant::now();
        for _ in 0..100 {
            super::step(&mut pos, &mut vel, &d.edges, None, 0.5);
        }
        eprintln!("step: {:?} each", t.elapsed() / 100);
    }
}

#[cfg(debug_assertions)]
impl Graph {
    fn debug_index(&self, name: &str) -> Option<usize> {
        self.data.borrow().paths.iter().position(|p| p.file_stem().is_some_and(|s| s == name))
    }

    pub fn debug_hover(&self, name: &str) {
        self.hover.set(self.debug_index(name));
        self.widget.queue_draw();
    }

    pub fn debug_open(&self, name: &str) {
        let p = self.debug_index(name).map(|i| self.data.borrow().paths[i].clone());
        if let Some(p) = p {
            (self.on_open)(p, false);
        }
    }

    pub fn debug_zoom(&self, f: f64) {
        let (w, h) = (self.widget.width() as f64, self.widget.height() as f64);
        self.zoom_at(f, w / 2.0, h / 2.0);
    }
}

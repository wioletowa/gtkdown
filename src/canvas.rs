//! Read-only JSON Canvas (`.canvas`) viewer: cards are GTK widgets on a `Fixed` scaled with
//! child transforms, edges and arrows are drawn with cairo underneath.

use crate::json::Json;
use adw::prelude::*;
use gtk::{gdk, gio, glib, graphene, gsk};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Text(String),
    /// Vault-relative path.
    File(String),
    Link(String),
    Group(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub color: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: String,
    pub from_side: Option<Side>,
    pub to: String,
    pub to_side: Option<Side>,
    pub from_arrow: bool,
    pub to_arrow: bool,
    pub color: Option<String>,
    pub label: Option<String>,
}

#[derive(Default, Debug)]
pub struct Doc {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

fn side(s: Option<&str>) -> Option<Side> {
    match s? {
        "top" => Some(Side::Top),
        "right" => Some(Side::Right),
        "bottom" => Some(Side::Bottom),
        "left" => Some(Side::Left),
        _ => None,
    }
}

pub fn parse(text: &str) -> Option<Doc> {
    let j = Json::parse(text)?;
    let s = |v: &Json, k: &str| v.get(k).and_then(Json::str).map(String::from);
    let n = |v: &Json, k: &str| v.get(k).and_then(Json::num).unwrap_or(0.0);
    let nodes = j
        .get("nodes")
        .map(Json::arr)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            let kind = match s(v, "type")?.as_str() {
                "text" => NodeKind::Text(s(v, "text").unwrap_or_default()),
                "file" => NodeKind::File(s(v, "file").unwrap_or_default()),
                "link" => NodeKind::Link(s(v, "url").unwrap_or_default()),
                "group" => NodeKind::Group(s(v, "label").unwrap_or_default()),
                _ => return None,
            };
            Some(Node {
                id: s(v, "id")?,
                kind,
                x: n(v, "x"),
                y: n(v, "y"),
                w: n(v, "width").max(1.0),
                h: n(v, "height").max(1.0),
                color: s(v, "color"),
            })
        })
        .collect();
    let edges = j
        .get("edges")
        .map(Json::arr)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            Some(Edge {
                from: s(v, "fromNode")?,
                from_side: side(v.get("fromSide").and_then(Json::str)),
                to: s(v, "toNode")?,
                to_side: side(v.get("toSide").and_then(Json::str)),
                from_arrow: s(v, "fromEnd").as_deref() == Some("arrow"),
                to_arrow: s(v, "toEnd").as_deref() != Some("none"),
                color: s(v, "color"),
                label: s(v, "label"),
            })
        })
        .collect();
    Some(Doc { nodes, edges })
}

/// Obsidian's preset colours "1".."6", or a `#rrggbb` value.
pub fn color(c: &str) -> Option<gdk::RGBA> {
    let hex = match c {
        "1" => "#fb464c",
        "2" => "#e9973f",
        "3" => "#e0de71",
        "4" => "#44cf6e",
        "5" => "#53dfdd",
        "6" => "#a882ff",
        other => other,
    };
    gdk::RGBA::parse(hex).ok()
}

fn anchor(n: &Node, s: Side) -> (f64, f64) {
    match s {
        Side::Top => (n.x + n.w / 2.0, n.y),
        Side::Bottom => (n.x + n.w / 2.0, n.y + n.h),
        Side::Left => (n.x, n.y + n.h / 2.0),
        Side::Right => (n.x + n.w, n.y + n.h / 2.0),
    }
}

fn normal(s: Side) -> (f64, f64) {
    match s {
        Side::Top => (0.0, -1.0),
        Side::Bottom => (0.0, 1.0),
        Side::Left => (-1.0, 0.0),
        Side::Right => (1.0, 0.0),
    }
}

/// Side facing the other node when the file doesn't say.
fn facing(a: &Node, b: &Node) -> Side {
    let dx = (b.x + b.w / 2.0) - (a.x + a.w / 2.0);
    let dy = (b.y + b.h / 2.0) - (a.y + a.h / 2.0);
    if dx.abs() > dy.abs() {
        if dx > 0.0 { Side::Right } else { Side::Left }
    } else if dy > 0.0 {
        Side::Bottom
    } else {
        Side::Top
    }
}

/// Bezier from one node side to another: start, two control points, end.
pub fn edge_curve(a: &Node, b: &Node, e: &Edge) -> [(f64, f64); 4] {
    let sa = e.from_side.unwrap_or_else(|| facing(a, b));
    let sb = e.to_side.unwrap_or_else(|| facing(b, a));
    let p0 = anchor(a, sa);
    let p3 = anchor(b, sb);
    let dist = ((p3.0 - p0.0).powi(2) + (p3.1 - p0.1).powi(2)).sqrt();
    let k = (dist / 2.0).clamp(30.0, 150.0);
    let (na, nb) = (normal(sa), normal(sb));
    [p0, (p0.0 + na.0 * k, p0.1 + na.1 * k), (p3.0 + nb.0 * k, p3.1 + nb.1 * k), p3]
}

/// Markdown subset to Pango markup for card previews.
pub fn markup(md: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut out = String::new();
    for ev in Parser::new(md) {
        match ev {
            Event::Start(Tag::Heading { .. }) => out.push_str("<span size=\"large\" weight=\"bold\">"),
            Event::End(TagEnd::Heading(_)) => out.push_str("</span>\n"),
            Event::Start(Tag::Strong) => out.push_str("<b>"),
            Event::End(TagEnd::Strong) => out.push_str("</b>"),
            Event::Start(Tag::Emphasis) => out.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => out.push_str("</i>"),
            Event::Start(Tag::Strikethrough) => out.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => out.push_str("</s>"),
            Event::Start(Tag::Item) => out.push_str("• "),
            Event::End(TagEnd::Item) | Event::End(TagEnd::Paragraph) => out.push('\n'),
            Event::End(TagEnd::CodeBlock) => out.push('\n'),
            Event::Code(c) => out.push_str(&format!("<tt>{}</tt>", glib::markup_escape_text(&c))),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => out.push_str(&glib::markup_escape_text(&t)),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            _ => {}
        }
    }
    out.trim_end().to_string()
}

struct View {
    zoom: Cell<f64>,
    pan: Cell<(f64, f64)>,
    fitted: Cell<bool>,
}

pub struct Canvas {
    pub path: RefCell<PathBuf>,
    pub widget: gtk::Overlay,
    area: gtk::DrawingArea,
    fixed: gtk::Fixed,
    doc: RefCell<Doc>,
    cards: RefCell<Vec<(gtk::Widget, (f64, f64))>>,
    view: View,
    root: PathBuf,
    on_open: Box<dyn Fn(PathBuf)>,
    monitor: RefCell<Option<gio::FileMonitor>>,
}

impl Canvas {
    pub fn open(path: &Path, root: &Path, on_open: impl Fn(PathBuf) + 'static) -> std::io::Result<Rc<Canvas>> {
        let text = std::fs::read_to_string(path)?;
        let doc = parse(&text).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad canvas"))?;
        let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).build();
        let fixed = gtk::Fixed::new();
        let widget = gtk::Overlay::builder().child(&area).overflow(gtk::Overflow::Hidden).build();
        widget.add_css_class("canvas");
        widget.add_overlay(&fixed);
        let c = Rc::new(Canvas {
            path: RefCell::new(path.to_path_buf()),
            widget,
            area,
            fixed,
            doc: RefCell::new(doc),
            cards: RefCell::new(Vec::new()),
            view: View { zoom: Cell::new(1.0), pan: Cell::new((0.0, 0.0)), fitted: Cell::new(false) },
            root: root.to_path_buf(),
            on_open: Box::new(on_open),
            monitor: RefCell::new(None),
        });
        c.build_cards();
        c.connect();
        c.watch();
        Ok(c)
    }

    fn build_cards(self: &Rc<Self>) {
        while let Some(ch) = self.fixed.first_child() {
            self.fixed.remove(&ch);
        }
        let mut cards = Vec::new();
        let doc = self.doc.borrow();
        // Groups first so cards stack above them.
        let order = doc.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Group(_))).chain(
            doc.nodes.iter().filter(|n| !matches!(n.kind, NodeKind::Group(_))),
        );
        for n in order {
            let w = self.card(n);
            w.set_size_request(n.w as i32, n.h as i32);
            self.fixed.put(&w, 0.0, 0.0);
            cards.push((w, (n.x, n.y)));
        }
        // Edge labels: widgets centred on the curve's midpoint.
        const LABEL_W: f64 = 160.0;
        for e in &doc.edges {
            let (Some(l), Some(a), Some(b)) = (
                e.label.as_ref(),
                doc.nodes.iter().find(|n| n.id == e.from),
                doc.nodes.iter().find(|n| n.id == e.to),
            ) else {
                continue;
            };
            let [p0, c1, c2, p3] = edge_curve(a, b, e);
            let mx = (p0.0 + 3.0 * c1.0 + 3.0 * c2.0 + p3.0) / 8.0;
            let my = (p0.1 + 3.0 * c1.1 + 3.0 * c2.1 + p3.1) / 8.0;
            let w = gtk::Label::builder()
                .label(l)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .css_classes(["canvas-edge-label"])
                .build();
            w.set_size_request(LABEL_W as i32, -1);
            self.fixed.put(&w, 0.0, 0.0);
            cards.push((w.upcast(), (mx - LABEL_W / 2.0, my - 10.0)));
        }
        drop(doc);
        *self.cards.borrow_mut() = cards;
        self.relayout();
    }

    fn card(self: &Rc<Self>, n: &Node) -> gtk::Widget {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
        // Cards keep their canvas size; longer content scrolls inside, as in Obsidian.
        let frame: gtk::Widget = if let NodeKind::Group(_) = n.kind {
            b.set_overflow(gtk::Overflow::Hidden);
            b.clone().upcast()
        } else {
            gtk::ScrolledWindow::builder()
                .child(&b)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::External)
                .build()
                .upcast()
        };
        frame.add_css_class(if matches!(n.kind, NodeKind::Group(_)) { "canvas-group" } else { "canvas-card" });
        if let Some(c) = n.color.as_deref().and_then(color) {
            let css = gtk::CssProvider::new();
            css.load_from_string(&format!(
                "* {{ border-color: {c}; background-color: alpha({c}, 0.12); }}",
                c = c.to_str()
            ));
            #[allow(deprecated)]
            frame.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let label = |markup: &str| {
            gtk::Label::builder()
                .use_markup(true)
                .label(markup)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .xalign(0.0)
                .yalign(0.0)
                .build()
        };
        match &n.kind {
            NodeKind::Text(t) => b.append(&label(&markup(t))),
            NodeKind::Group(name) => {
                let l = label(&format!("<b>{}</b>", glib::markup_escape_text(name)));
                l.add_css_class("canvas-group-label");
                b.append(&l);
            }
            NodeKind::Link(url) => {
                let l = label(&format!("<a href=\"{0}\">{0}</a>", glib::markup_escape_text(url)));
                b.append(&l);
            }
            NodeKind::File(rel) => {
                let p = self.root.join(rel);
                let ext = crate::tree::ext(rel).unwrap_or_default();
                let name = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                if ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"].contains(&ext.as_str()) {
                    let pic = gtk::Picture::for_filename(&p);
                    pic.set_content_fit(gtk::ContentFit::Cover);
                    pic.set_vexpand(true);
                    b.append(&pic);
                } else {
                    b.append(&label(&format!("<b>{}</b>", glib::markup_escape_text(&name))));
                    if ext == "md" {
                        let text = std::fs::read_to_string(&p).unwrap_or_default();
                        let body: String = strip_frontmatter(&text).chars().take(3000).collect();
                        b.append(&label(&markup(&body)));
                    }
                }
                let click = gtk::GestureClick::new();
                let weak = Rc::downgrade(self);
                click.connect_pressed(move |_, n_press, _, _| {
                    if n_press == 2
                        && let Some(c) = weak.upgrade()
                    {
                        (c.on_open)(p.clone());
                    }
                });
                frame.add_controller(click);
            }
        }
        frame
    }

    /// Positions every card for the current pan/zoom and redraws edges.
    fn relayout(&self) {
        let z = self.view.zoom.get();
        let (px, py) = self.view.pan.get();
        for (w, (x, y)) in self.cards.borrow().iter() {
            let t = gsk::Transform::new()
                .translate(&graphene::Point::new((x * z + px) as f32, (y * z + py) as f32))
                .scale(z as f32, z as f32);
            self.fixed.set_child_transform(w, Some(&t));
        }
        self.area.queue_draw();
    }

    fn fit(&self) {
        let (w, h) = (self.area.width() as f64, self.area.height() as f64);
        let doc = self.doc.borrow();
        if doc.nodes.is_empty() || w < 10.0 {
            return;
        }
        let x0 = doc.nodes.iter().map(|n| n.x).fold(f64::MAX, f64::min);
        let y0 = doc.nodes.iter().map(|n| n.y).fold(f64::MAX, f64::min);
        let x1 = doc.nodes.iter().map(|n| n.x + n.w).fold(f64::MIN, f64::max);
        let y1 = doc.nodes.iter().map(|n| n.y + n.h).fold(f64::MIN, f64::max);
        let z = ((w - 80.0) / (x1 - x0)).min((h - 80.0) / (y1 - y0)).clamp(0.1, 1.0);
        self.view.zoom.set(z);
        self.view.pan.set((w / 2.0 - (x0 + x1) / 2.0 * z, h / 2.0 - (y0 + y1) / 2.0 * z));
        drop(doc);
        self.relayout();
    }

    /// Zooms by `factor` keeping the canvas point under (sx, sy) fixed.
    fn zoom_at(&self, factor: f64, sx: f64, sy: f64) {
        let z = self.view.zoom.get();
        let nz = (z * factor).clamp(0.1, 4.0);
        let (px, py) = self.view.pan.get();
        self.view.pan.set((sx - (sx - px) * nz / z, sy - (sy - py) * nz / z));
        self.view.zoom.set(nz);
        self.relayout();
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.area.set_draw_func(move |area, cr, _, _| {
            if let Some(c) = weak.upgrade() {
                c.draw(area, cr);
            }
        });
        let weak = Rc::downgrade(self);
        self.area.connect_resize(move |_, _, _| {
            if let Some(c) = weak.upgrade()
                && !c.view.fitted.replace(true)
            {
                c.fit();
            }
        });

        // Drag anywhere pans.
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        let (weak, s) = (Rc::downgrade(self), start.clone());
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(c) = weak.upgrade() {
                s.set(c.view.pan.get());
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(c) = weak.upgrade() {
                let (x, y) = start.get();
                c.view.pan.set((x + dx, y + dy));
                c.relayout();
            }
        });
        self.widget.add_controller(drag);

        // Wheel pans (Shift: sideways); Ctrl+wheel zooms at the pointer.
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let pointer = Rc::new(Cell::new((0.0, 0.0)));
        let (weak, p) = (Rc::downgrade(self), pointer.clone());
        wheel.connect_scroll(move |ctl, dx, dy| {
            let Some(c) = weak.upgrade() else { return glib::Propagation::Proceed };
            let state = ctl.current_event_state();
            if state.contains(gdk::ModifierType::CONTROL_MASK) {
                let (sx, sy) = p.get();
                c.zoom_at(if dy < 0.0 { 1.1 } else { 1.0 / 1.1 }, sx, sy);
            } else {
                let (dx, dy) = if state.contains(gdk::ModifierType::SHIFT_MASK) { (dy, dx) } else { (dx, dy) };
                let (x, y) = c.view.pan.get();
                c.view.pan.set((x - dx * 40.0, y - dy * 40.0));
                c.relayout();
            }
            glib::Propagation::Stop
        });
        self.widget.add_controller(wheel);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, x, y| pointer.set((x, y)));
        self.widget.add_controller(motion);

        let pinch = gtk::GestureZoom::new();
        let base = Rc::new(Cell::new(1.0));
        let (weak, b) = (Rc::downgrade(self), base.clone());
        pinch.connect_begin(move |_, _| {
            if let Some(c) = weak.upgrade() {
                b.set(c.view.zoom.get());
            }
        });
        let weak = Rc::downgrade(self);
        pinch.connect_scale_changed(move |g, scale| {
            if let Some(c) = weak.upgrade() {
                let (sx, sy) = g.bounding_box_center().unwrap_or((0.0, 0.0));
                let target = base.get() * scale;
                c.zoom_at(target / c.view.zoom.get(), sx, sy);
            }
        });
        self.widget.add_controller(pinch);

        // Keyboard: 0 / Home fits, +/- zoom at the centre.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(c) = weak.upgrade() else { return glib::Propagation::Proceed };
            let (cx, cy) = (c.area.width() as f64 / 2.0, c.area.height() as f64 / 2.0);
            match key {
                gdk::Key::_0 | gdk::Key::Home => c.fit(),
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => c.zoom_at(1.2, cx, cy),
                gdk::Key::minus | gdk::Key::KP_Subtract => c.zoom_at(1.0 / 1.2, cx, cy),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.widget.add_controller(keys);
        self.widget.set_focusable(true);
        let w = self.widget.downgrade();
        let click = gtk::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(w) = w.upgrade() {
                w.grab_focus();
            }
        });
        self.widget.add_controller(click);
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context) {
        let fg = area.color();
        let z = self.view.zoom.get();
        let (px, py) = self.view.pan.get();
        cr.translate(px, py);
        cr.scale(z, z);
        let doc = self.doc.borrow();
        let node = |id: &str| doc.nodes.iter().find(|n| n.id == id);
        cr.set_line_width(2.0);
        for e in &doc.edges {
            let (Some(a), Some(b)) = (node(&e.from), node(&e.to)) else { continue };
            let c = e.color.as_deref().and_then(color).unwrap_or(gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.55));
            cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
            let [p0, c1, c2, p3] = edge_curve(a, b, e);
            cr.move_to(p0.0, p0.1);
            cr.curve_to(c1.0, c1.1, c2.0, c2.1, p3.0, p3.1);
            let _ = cr.stroke();
            if e.to_arrow {
                arrow(cr, c2, p3);
            }
            if e.from_arrow {
                arrow(cr, c1, p0);
            }
        }
    }

    fn watch(self: &Rc<Self>) {
        let Ok(mon) = gio::File::for_path(&*self.path.borrow())
            .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        mon.connect_changed(move |_, _, _, ev| {
            if !matches!(ev, gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Created) {
                return;
            }
            let Some(c) = weak.upgrade() else { return };
            let text = std::fs::read_to_string(&*c.path.borrow()).unwrap_or_default();
            if let Some(doc) = parse(&text) {
                *c.doc.borrow_mut() = doc;
                c.build_cards();
            }
        });
        self.monitor.replace(Some(mon));
    }

    pub fn set_path(self: &Rc<Self>, p: PathBuf) {
        *self.path.borrow_mut() = p;
        self.watch();
    }
}

fn strip_frontmatter(text: &str) -> &str {
    text.strip_prefix("---\n")
        .and_then(|r| r.find("\n---").map(|i| r[i + 4..].trim_start_matches(['-', '\n'])))
        .unwrap_or(text)
}

fn arrow(cr: &gtk::cairo::Context, from: (f64, f64), tip: (f64, f64)) {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (s, w) = (12.0, 6.0);
    cr.move_to(tip.0, tip.1);
    cr.line_to(tip.0 - ux * s - uy * w, tip.1 - uy * s + ux * w);
    cr.line_to(tip.0 - ux * s + uy * w, tip.1 - uy * s - ux * w);
    cr.close_path();
    let _ = cr.fill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spec() {
        let d = parse(
            r##"{"nodes":[{"id":"a","x":0,"y":0,"width":100,"height":50,"type":"text","text":"**hi**","color":"1"},
            {"id":"b","x":300,"y":0,"width":100,"height":50,"type":"file","file":"Notes/X.md"},
            {"id":"g","x":-10,"y":-10,"width":500,"height":100,"type":"group","label":"G"},
            {"id":"z","type":"unknown"}],
            "edges":[{"id":"e","fromNode":"a","toNode":"b","toEnd":"none","label":"go"}]}"##,
        )
        .unwrap();
        assert_eq!(d.nodes.len(), 3);
        assert_eq!(d.nodes[1].kind, NodeKind::File("Notes/X.md".into()));
        let e = &d.edges[0];
        assert!(!e.to_arrow && !e.from_arrow);
        let [p0, _, _, p3] = edge_curve(&d.nodes[0], &d.nodes[1], e);
        assert_eq!((p0, p3), ((100.0, 25.0), (300.0, 25.0)));
        assert_eq!(markup("# T\n**b** <x>"), "<span size=\"large\" weight=\"bold\">T</span>\n<b>b</b> &lt;x&gt;");
    }
}

#[cfg(debug_assertions)]
impl Canvas {
    pub fn debug_state(&self) -> String {
        let (x, y) = self.view.pan.get();
        format!("zoom={:.3} pan=({x:.0},{y:.0})", self.view.zoom.get())
    }
}

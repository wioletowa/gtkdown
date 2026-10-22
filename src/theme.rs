//! Themes (CSS over libadwaita variables) and the shared text-tag table for live preview.

use crate::markdown::Style;
use adw::prelude::*;
use gtk::{gdk, pango};

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub scheme: adw::ColorScheme,
    pub ui: Option<Ui>,
    /// None: derive the palette from the system (libadwaita dark flag + accent).
    pub palette: Option<Palette>,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub muted: &'static str,
    pub faint: &'static str,
    pub accent: &'static str,
    pub link: &'static str,
    pub code_bg: &'static str,
    pub highlight_bg: &'static str,
    pub tag_fg: &'static str,
    pub tag_bg: &'static str,
    pub quote_bg: &'static str,
    pub callout_bg: &'static str,
}

const OBSIDIAN_DARK: Palette = Palette {
    muted: "#b3b3b3",
    faint: "#666666",
    accent: "#8a5cf5",
    link: "#a88bfa",
    code_bg: "#262626",
    highlight_bg: "rgba(255, 208, 0, 0.4)",
    tag_fg: "#a88bfa",
    tag_bg: "rgba(138, 92, 245, 0.15)",
    quote_bg: "rgba(255, 255, 255, 0.04)",
    callout_bg: "rgba(138, 92, 245, 0.1)",
};

const OBSIDIAN_LIGHT: Palette = Palette {
    muted: "#5c5c5c",
    faint: "#ababab",
    accent: "#705dcf",
    link: "#705dcf",
    code_bg: "#f6f6f6",
    highlight_bg: "rgba(255, 208, 0, 0.4)",
    tag_fg: "#705dcf",
    tag_bg: "rgba(112, 93, 207, 0.1)",
    quote_bg: "rgba(0, 0, 0, 0.04)",
    callout_bg: "rgba(112, 93, 207, 0.08)",
};

const GRUVBOX_DARK: Palette = Palette {
    muted: "#a89984",
    faint: "#665c54",
    accent: "#fe8019",
    link: "#83a598",
    code_bg: "#32302f",
    highlight_bg: "rgba(250, 189, 47, 0.35)",
    tag_fg: "#8ec07c",
    tag_bg: "rgba(142, 192, 124, 0.15)",
    quote_bg: "rgba(235, 219, 178, 0.04)",
    callout_bg: "rgba(131, 165, 152, 0.12)",
};

const GRUVBOX_LIGHT: Palette = Palette {
    muted: "#7c6f64",
    faint: "#bdae93",
    accent: "#af3a03",
    link: "#076678",
    code_bg: "#f2e5bc",
    highlight_bg: "rgba(215, 153, 33, 0.35)",
    tag_fg: "#427b58",
    tag_bg: "rgba(66, 123, 88, 0.12)",
    quote_bg: "rgba(60, 56, 54, 0.04)",
    callout_bg: "rgba(7, 102, 120, 0.08)",
};

pub const THEMES: &[Theme] = &[
    Theme { id: "system", name: "System", scheme: adw::ColorScheme::Default, ui: None, palette: None },
    Theme {
        id: "light",
        name: "Light",
        scheme: adw::ColorScheme::ForceLight,
        palette: Some(OBSIDIAN_LIGHT),
        ui: Some(Ui {
            accent: "#705dcf",
            accent_bg: "#705dcf",
            bg: "#ffffff",
            fg: "#222222",
            side_bg: "#f6f6f6",
            popover_bg: "#ffffff",
        }),
    },
    Theme {
        id: "dark",
        name: "Dark",
        scheme: adw::ColorScheme::ForceDark,
        palette: Some(OBSIDIAN_DARK),
        ui: Some(Ui {
            accent: "#a88bfa",
            accent_bg: "#7f6df2",
            bg: "#1e1e1e",
            fg: "#dadada",
            side_bg: "#262626",
            popover_bg: "#2a2a2a",
        }),
    },
    Theme {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        scheme: adw::ColorScheme::ForceDark,
        palette: Some(GRUVBOX_DARK),
        ui: Some(Ui {
            accent: "#fe8019",
            accent_bg: "#d65d0e",
            bg: "#282828",
            fg: "#ebdbb2",
            side_bg: "#1d2021",
            popover_bg: "#32302f",
        }),
    },
    Theme {
        id: "gruvbox-light",
        name: "Gruvbox Light",
        scheme: adw::ColorScheme::ForceLight,
        palette: Some(GRUVBOX_LIGHT),
        ui: Some(Ui {
            accent: "#af3a03",
            accent_bg: "#af3a03",
            bg: "#fbf1c7",
            fg: "#3c3836",
            side_bg: "#f2e5bc",
            popover_bg: "#fbf1c7",
        }),
    },
];

/// Window chrome colours of a fixed theme.
pub struct Ui {
    pub accent: &'static str,
    pub accent_bg: &'static str,
    pub bg: &'static str,
    pub fg: &'static str,
    pub side_bg: &'static str,
    pub popover_bg: &'static str,
}

impl Theme {
    /// CSS for a fixed theme. Loaded above user priority and sets both libadwaita variables and
    /// legacy named colours, so a user's own gtk.css theme cannot bleed into it.
    pub fn css(&self) -> String {
        let Some(u) = &self.ui else { return String::new() };
        let vars = [
            ("accent-color", u.accent),
            ("accent-bg-color", u.accent_bg),
            ("window-bg-color", u.bg),
            ("window-fg-color", u.fg),
            ("view-bg-color", u.bg),
            ("view-fg-color", u.fg),
            ("headerbar-bg-color", u.side_bg),
            ("headerbar-fg-color", u.fg),
            ("headerbar-backdrop-color", u.side_bg),
            ("sidebar-bg-color", u.side_bg),
            ("sidebar-fg-color", u.fg),
            ("sidebar-backdrop-color", u.side_bg),
            ("popover-bg-color", u.popover_bg),
            ("popover-fg-color", u.fg),
            ("dialog-bg-color", u.popover_bg),
            ("dialog-fg-color", u.fg),
            ("card-bg-color", "rgba(127, 127, 127, 0.08)"),
        ];
        let mut css = String::from(":root {\n");
        for (k, v) in vars {
            css += &format!("  --{k}: {v};\n");
        }
        css += "}\n";
        for (k, v) in vars {
            css += &format!("@define-color {} {v};\n", k.replace('-', "_"));
        }
        css += "window, .note-scroll, textview, textview > text, tabview, statuspage {
  background-color: var(--view-bg-color); color: var(--view-fg-color); }
headerbar, tabbar .box { background-color: var(--headerbar-bg-color); color: var(--headerbar-fg-color); }
.sidebar-pane, .sidebar-pane headerbar, .sidebar-pane listview {
  background-color: var(--sidebar-bg-color); color: var(--sidebar-fg-color); }
popover > contents, popover > arrow { background-color: var(--popover-bg-color); color: var(--popover-fg-color); }
";
        css
    }
}

pub fn by_id(id: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

pub const BASE_CSS: &str = "
textview.note { font-size: 16px; }
textview.note text { background: none; }
.note-scroll { background: var(--view-bg-color); }
.vault-tree row { padding: 0 4px; min-height: 26px; }
.vault-tree .suffix { font-size: 9px; font-weight: bold; opacity: 0.5; margin-left: 6px; }
.statusbar { font-size: 12px; opacity: 0.6; padding: 2px 10px; }
";

fn rgba(s: &str) -> gdk::RGBA {
    gdk::RGBA::parse(s).unwrap_or(gdk::RGBA::BLACK)
}

fn system_palette(dark: bool) -> Palette {
    // Accent follows the system through libadwaita; only neutral tints are fixed here.
    let mut p = if dark { OBSIDIAN_DARK } else { OBSIDIAN_LIGHT };
    p.code_bg = if dark { "rgba(255, 255, 255, 0.07)" } else { "rgba(0, 0, 0, 0.06)" };
    p
}

/// One tag per markdown style plus `hidden`; the table is shared by every open note.
pub struct Styles {
    pub table: gtk::TextTagTable,
    tags: Vec<gtk::TextTag>,
    pub hidden: gtk::TextTag,
}

impl Styles {
    pub fn new() -> Styles {
        let table = gtk::TextTagTable::new();
        let tags: Vec<gtk::TextTag> = Style::ALL
            .iter()
            .map(|s| {
                let t = gtk::TextTag::new(None);
                configure(&t, *s);
                table.add(&t);
                t
            })
            .collect();
        // Collapsed to 1 Pango unit rather than `invisible`: GTK's invisible-text paths crash on
        // cursor moves, and larger tiny sizes still leave glyph hairlines.
        let hidden = gtk::TextTag::builder()
            .size(1)
            .foreground_rgba(&gdk::RGBA::TRANSPARENT)
            .build();
        table.add(&hidden);
        Styles { table, tags, hidden }
    }

    pub fn tag(&self, s: Style) -> &gtk::TextTag {
        &self.tags[s as usize]
    }

    pub fn apply(&self, theme: &Theme, dark: bool, accent: Option<gdk::RGBA>) {
        let p = theme.palette.unwrap_or_else(|| system_palette(dark));
        let accent = match (&theme.palette, accent) {
            (None, Some(a)) => a,
            _ => rgba(p.accent),
        };
        let link = if theme.palette.is_none() { accent } else { rgba(p.link) };
        let tint = |a: f32| gdk::RGBA::new(accent.red(), accent.green(), accent.blue(), a);
        let set_fg = |s: Style, c: &gdk::RGBA| self.tag(s).set_foreground_rgba(Some(c));
        let set_bg = |s: Style, c: &gdk::RGBA| self.tag(s).set_background_rgba(Some(c));
        let set_pbg = |s: Style, c: &gdk::RGBA| self.tag(s).set_paragraph_background_rgba(Some(c));

        for s in [Style::H6, Style::Frontmatter, Style::TaskDone, Style::Hr, Style::Html, Style::ListMarker] {
            set_fg(s, &rgba(p.muted));
        }
        for s in [Style::Syntax, Style::Comment] {
            set_fg(s, &rgba(p.faint));
        }
        for s in [Style::Link, Style::WikiLink, Style::Embed, Style::FootnoteRef] {
            set_fg(s, &link);
        }
        for s in [Style::CalloutTitle, Style::Checkbox, Style::Math] {
            set_fg(s, &accent);
        }
        set_pbg(Style::CodeBlock, &rgba(p.code_bg));
        set_bg(Style::Code, &rgba(p.code_bg));
        set_bg(Style::Highlight, &rgba(p.highlight_bg));
        set_pbg(Style::Quote, &rgba(p.quote_bg));
        if theme.palette.is_none() {
            set_fg(Style::Tag, &accent);
            set_bg(Style::Tag, &tint(0.15));
            set_pbg(Style::Callout, &tint(0.1));
        } else {
            set_fg(Style::Tag, &rgba(p.tag_fg));
            set_bg(Style::Tag, &rgba(p.tag_bg));
            set_pbg(Style::Callout, &rgba(p.callout_bg));
        }
    }
}

fn configure(t: &gtk::TextTag, s: Style) {
    let heading = |scale: f64| {
        t.set_weight(700);
        t.set_scale(scale);
        t.set_pixels_above_lines(6);
        t.set_pixels_below_lines(4);
    };
    match s {
        Style::H1 => heading(1.8),
        Style::H2 => heading(1.55),
        Style::H3 => heading(1.35),
        Style::H4 => heading(1.2),
        Style::H5 => heading(1.1),
        Style::H6 => heading(1.0),
        Style::Bold | Style::TableHead | Style::CalloutTitle => t.set_weight(700),
        Style::Italic | Style::Embed => t.set_style(pango::Style::Italic),
        Style::Strike => t.set_strikethrough(true),
        Style::TaskDone => t.set_strikethrough(true),
        Style::Link => t.set_underline(pango::Underline::Single),
        Style::Code | Style::Math | Style::Table | Style::CodeBlock => t.set_family(Some("monospace")),
        Style::Frontmatter => {
            t.set_family(Some("monospace"));
            t.set_scale(0.9);
        }
        Style::Checkbox => {
            t.set_family(Some("monospace"));
            t.set_weight(700);
        }
        Style::ListMarker => t.set_weight(700),
        Style::FootnoteRef => {
            t.set_scale(0.8);
            t.set_rise(4 * pango::SCALE);
        }
        _ => {}
    }
}

//! Markdown -> style spans for live preview. Pure, no GTK; offsets are byte offsets.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Quote,
    Callout,
    CalloutTitle,
    CodeBlock,
    Table,
    TableHead,
    Frontmatter,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Bold,
    Italic,
    Strike,
    Highlight,
    Code,
    Math,
    Link,
    WikiLink,
    Embed,
    Tag,
    FootnoteRef,
    ListMarker,
    Checkbox,
    TaskDone,
    Hr,
    Html,
    Syntax,
    Comment,
}

impl Style {
    pub const ALL: [Style; 31] = [
        Style::Quote,
        Style::Callout,
        Style::CalloutTitle,
        Style::CodeBlock,
        Style::Table,
        Style::TableHead,
        Style::Frontmatter,
        Style::H1,
        Style::H2,
        Style::H3,
        Style::H4,
        Style::H5,
        Style::H6,
        Style::Bold,
        Style::Italic,
        Style::Strike,
        Style::Highlight,
        Style::Code,
        Style::Math,
        Style::Link,
        Style::WikiLink,
        Style::Embed,
        Style::Tag,
        Style::FootnoteRef,
        Style::ListMarker,
        Style::Checkbox,
        Style::TaskDone,
        Style::Hr,
        Style::Html,
        Style::Syntax,
        Style::Comment,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

/// Syntax characters hidden unless the cursor is inside `reveal`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub range: Range<usize>,
    pub reveal: Range<usize>,
}

#[derive(Default, Debug)]
pub struct Parsed {
    pub spans: Vec<Span>,
    pub markers: Vec<Marker>,
    /// Ranges of `[ ]` / `[x]` task boxes.
    pub tasks: Vec<Range<usize>>,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Whole element, markup included.
    pub range: Range<usize>,
    /// Destination as written: `Note#Heading` for wikilinks, a URL or path otherwise.
    pub target: String,
    pub wiki: bool,
    pub embed: bool,
}

struct Frame<'a> {
    tag: Tag<'a>,
    range: Range<usize>,
    children: Option<Range<usize>>,
}

fn extend(children: &mut Option<Range<usize>>, r: &Range<usize>) {
    match children {
        Some(c) => {
            c.start = c.start.min(r.start);
            c.end = c.end.max(r.end);
        }
        None => *children = Some(r.clone()),
    }
}

fn line_end(src: &str, from: usize) -> usize {
    src[from..].find('\n').map_or(src.len(), |i| from + i)
}

fn line_start(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map_or(0, |i| i + 1)
}

pub fn parse(src: &str) -> Parsed {
    let mut out = Parsed::default();
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS;

    let comments = scan_comments(src);
    let mut stack: Vec<Frame> = Vec::new();

    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        match ev {
            Event::Start(tag) => {
                stack.push(Frame { tag, range, children: None });
                continue;
            }
            Event::End(end) => {
                let Some(frame) = stack.pop() else { continue };
                debug_assert!(tag_matches(&frame.tag, end));
                out.finish(src, &frame);
                if let Some(parent) = stack.last_mut() {
                    extend(&mut parent.children, &frame.range);
                }
                continue;
            }
            Event::Text(_) => {
                let in_code = stack
                    .iter()
                    .any(|f| matches!(f.tag, Tag::CodeBlock(_) | Tag::MetadataBlock(_)));
                if !in_code {
                    scan_inline(src, range.clone(), &comments, &mut out);
                }
            }
            Event::Code(_) => {
                let s = &src[range.clone()];
                let n = s.bytes().take_while(|&b| b == b'`').count();
                let m = s.bytes().rev().take_while(|&b| b == b'`').count();
                if n + m <= s.len() {
                    let inner = range.start + n..range.end - m;
                    out.span(inner, Style::Code);
                    out.marker(range.start..range.start + n, range.clone());
                    out.marker(range.end - m..range.end, range.clone());
                }
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => out.span(range.clone(), Style::Math),
            Event::Html(_) | Event::InlineHtml(_) => out.span(range.clone(), Style::Html),
            Event::FootnoteReference(_) => out.span(range.clone(), Style::FootnoteRef),
            Event::Rule => out.span(range.clone(), Style::Hr),
            Event::TaskListMarker(checked) => {
                out.span(range.clone(), Style::Checkbox);
                out.tasks.push(range.clone());
                if checked {
                    let end = line_end(src, range.end);
                    out.span(range.end..end, Style::TaskDone);
                }
            }
            Event::SoftBreak | Event::HardBreak => {}
        }
        if let Some(parent) = stack.last_mut() {
            extend(&mut parent.children, &range);
        }
    }

    for c in comments {
        out.spans.retain(|s| !(s.style == Style::Tag && s.range.start >= c.start && s.range.end <= c.end));
        out.span(c, Style::Comment);
    }
    out.markers.sort_by_key(|m| m.range.start);
    out.markers.dedup_by_key(|m| m.range.start);
    out
}

fn tag_matches(tag: &Tag, end: TagEnd) -> bool {
    TagEnd::from(tag.clone()) == end
}

impl Parsed {
    fn span(&mut self, range: Range<usize>, style: Style) {
        if range.start < range.end {
            self.spans.push(Span { range, style });
        }
    }

    /// Adds a hidden-syntax marker, split around line breaks so lines never merge.
    fn marker(&mut self, range: Range<usize>, reveal: Range<usize>) {
        self.marker_in(None, range, reveal);
    }

    fn marker_in(&mut self, src: Option<&str>, range: Range<usize>, reveal: Range<usize>) {
        if range.start >= range.end {
            return;
        }
        let Some(src) = src else {
            self.spans.push(Span { range: range.clone(), style: Style::Syntax });
            self.markers.push(Marker { range, reveal });
            return;
        };
        let mut start = range.start;
        for (i, b) in src[range.clone()].bytes().enumerate() {
            let at = range.start + i;
            if b == b'\n' || b == b'\r' {
                if start < at {
                    self.marker_in(None, start..at, reveal.clone());
                }
                start = at + 1;
            }
        }
        if start < range.end {
            self.marker_in(None, start..range.end, reveal);
        }
    }

    fn finish(&mut self, src: &str, f: &Frame) {
        let r = f.range.clone();
        match &f.tag {
            Tag::Heading { level, .. } => {
                let style = match level {
                    HeadingLevel::H1 => Style::H1,
                    HeadingLevel::H2 => Style::H2,
                    HeadingLevel::H3 => Style::H3,
                    HeadingLevel::H4 => Style::H4,
                    HeadingLevel::H5 => Style::H5,
                    HeadingLevel::H6 => Style::H6,
                };
                self.span(r.clone(), style);
                match &f.children {
                    Some(c) => {
                        self.marker_in(Some(src), r.start..c.start, r.clone());
                        self.marker_in(Some(src), c.end..r.end, r.clone());
                    }
                    None => self.marker_in(Some(src), r.clone(), r.clone()),
                }
            }
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                let style = match f.tag {
                    Tag::Emphasis => Style::Italic,
                    Tag::Strong => Style::Bold,
                    _ => Style::Strike,
                };
                self.span(r.clone(), style);
                if let Some(c) = &f.children {
                    self.marker_in(Some(src), r.start..c.start, r.clone());
                    self.marker_in(Some(src), c.end..r.end, r.clone());
                }
            }
            Tag::Link { link_type, dest_url, .. } => {
                let target = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.to_string(),
                };
                self.links.push(Link {
                    range: r.clone(),
                    target,
                    wiki: matches!(link_type, LinkType::WikiLink { .. }),
                    embed: false,
                });
                let style = if matches!(link_type, LinkType::WikiLink { .. }) {
                    Style::WikiLink
                } else {
                    Style::Link
                };
                match &f.children {
                    Some(c) if !matches!(link_type, LinkType::Autolink | LinkType::Email) => {
                        self.span(c.clone(), style);
                        self.marker_in(Some(src), r.start..c.start, r.clone());
                        self.marker_in(Some(src), c.end..r.end, r.clone());
                    }
                    _ => self.span(r, style),
                }
            }
            Tag::Image { link_type, dest_url, .. } => {
                self.links.push(Link {
                    range: r.clone(),
                    target: dest_url.to_string(),
                    wiki: matches!(link_type, LinkType::WikiLink { .. }),
                    embed: true,
                });
                self.span(r, Style::Embed)
            }
            Tag::CodeBlock(kind) => {
                self.span(r.clone(), Style::CodeBlock);
                if matches!(kind, CodeBlockKind::Fenced(_)) {
                    let first = r.start..line_end(src, r.start).min(r.end);
                    self.marker_in(Some(src), first.clone(), r.clone());
                    let body = src[r.clone()].trim_end_matches(['\n', '\r']);
                    let last_start = line_start(src, r.start + body.len());
                    let last = &src[last_start..r.start + body.len()];
                    let t = last.trim_start();
                    if last_start > first.start && (t.starts_with("```") || t.starts_with("~~~")) {
                        self.marker_in(Some(src), last_start..r.start + body.len(), r.clone());
                    }
                }
            }
            Tag::BlockQuote(_) => self.quote(src, r),
            Tag::Item => {
                let s = &src[r.clone()];
                let digits = s.bytes().take_while(u8::is_ascii_digit).count();
                let len = match s.as_bytes() {
                    [b'-' | b'*' | b'+', b' ' | b'\t', ..] => 1,
                    _ if digits > 0 && matches!(s.as_bytes().get(digits), Some(b'.' | b')')) => digits + 1,
                    _ => 0,
                };
                self.span(r.start..r.start + len, Style::ListMarker);
            }
            Tag::Table(_) => {
                self.span(r.clone(), Style::Table);
                let s = &src[r.clone()];
                let bytes = s.as_bytes();
                for (i, &b) in bytes.iter().enumerate() {
                    if b == b'|' && (i == 0 || bytes[i - 1] != b'\\') {
                        self.span(r.start + i..r.start + i + 1, Style::Syntax);
                    }
                }
                if let Some(nl) = s.find('\n') {
                    let delim = r.start + nl + 1;
                    self.span(delim..line_end(src, delim).min(r.end), Style::Syntax);
                }
            }
            Tag::TableHead => self.span(r, Style::TableHead),
            Tag::MetadataBlock(_) => self.span(r, Style::Frontmatter),
            Tag::FootnoteDefinition(_) => {
                let s = &src[r.clone()];
                if let Some(i) = s.find("]:") {
                    self.span(r.start..r.start + i + 2, Style::Syntax);
                }
            }
            Tag::HtmlBlock => self.span(r, Style::Html),
            _ => {}
        }
    }

    fn quote(&mut self, src: &str, r: Range<usize>) {
        let mut pos = r.start;
        let mut first = true;
        let mut callout = false;
        while pos < r.end {
            let end = line_end(src, pos).min(r.end);
            let line = &src[pos..end];
            let ws = line.len() - line.trim_start().len();
            let mut i = ws;
            let b = line.as_bytes();
            while i < b.len() && b[i] == b'>' {
                i += 1;
                while i < b.len() && b[i] == b' ' {
                    i += 1;
                }
            }
            if i > ws {
                self.marker_in(Some(src), pos + ws..pos + i, pos..end);
            }
            if first {
                callout = line[i..].starts_with("[!");
                if callout {
                    self.span(pos + i..end, Style::CalloutTitle);
                }
                first = false;
            }
            pos = end + 1;
        }
        self.spans.push(Span { range: r, style: if callout { Style::Callout } else { Style::Quote } });
    }
}

/// `==highlight==` and `#tags` inside a text run.
fn scan_inline(src: &str, r: Range<usize>, comments: &[Range<usize>], out: &mut Parsed) {
    let s = &src[r.clone()];

    let mut i = 0;
    while let Some(open) = s[i..].find("==") {
        let a = i + open;
        let Some(close) = s[a + 2..].find("==") else { break };
        let c = a + 2 + close;
        if c > a + 2 && !s[a + 2..c].contains('\n') {
            let full = r.start + a..r.start + c + 2;
            out.span(full.clone(), Style::Highlight);
            out.marker(r.start + a..r.start + a + 2, full.clone());
            out.marker(r.start + c..r.start + c + 2, full);
        }
        i = c + 2;
    }

    for (i, ch) in s.char_indices() {
        if ch != '#' {
            continue;
        }
        let abs = r.start + i;
        let prev = if i > 0 { s[..i].chars().next_back() } else { src[..abs].chars().next_back() };
        if prev.is_some_and(|p| !p.is_whitespace()) {
            continue;
        }
        let len: usize = s[i + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
            .map(char::len_utf8)
            .sum();
        let name = &s[i + 1..i + 1 + len];
        if len > 0 && !name.bytes().all(|c| c.is_ascii_digit()) && !in_any(comments, abs) {
            out.span(abs..abs + 1 + len, Style::Tag);
        }
    }
}

fn in_any(ranges: &[Range<usize>], at: usize) -> bool {
    ranges.iter().any(|r| r.contains(&at))
}

/// Obsidian `%%comments%%`, possibly multi-line, outside fenced code and inline code.
fn scan_comments(src: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    let mut fence: Option<&str> = None;
    let mut pos = 0;
    while pos < src.len() {
        let end = line_end(src, pos);
        let line = &src[pos..end];
        let t = line.trim_start();
        if open.is_none() {
            if let Some(f) = fence {
                if t.starts_with(f) {
                    fence = None;
                }
                pos = end + 1;
                continue;
            }
            if t.starts_with("```") || t.starts_with("~~~") {
                fence = Some(&t[..3]);
                pos = end + 1;
                continue;
            }
        }
        let b = line.as_bytes();
        let mut i = 0;
        let mut in_code = false;
        while i < b.len() {
            if b[i] == b'`' && open.is_none() {
                in_code = !in_code;
            } else if !in_code && b[i] == b'%' && b.get(i + 1) == Some(&b'%') {
                match open.take() {
                    Some(s) => out.push(s..pos + i + 2),
                    None => open = Some(pos + i),
                }
                i += 2;
                continue;
            }
            i += 1;
        }
        pos = end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(src: &str, style: Style) -> Vec<&str> {
        parse(src).spans.iter().filter(|s| s.style == style).map(|s| &src[s.range.clone()]).collect()
    }

    fn hidden(src: &str) -> Vec<&str> {
        parse(src).markers.iter().map(|m| &src[m.range.clone()]).collect()
    }

    #[test]
    fn heading_markers() {
        let src = "## Title\ntext";
        assert_eq!(styles(src, Style::H2), vec!["## Title\n"]);
        assert_eq!(hidden(src), vec!["## "]);
    }

    #[test]
    fn emphasis_and_code() {
        let src = "a **b** _c_ `d` ~~e~~";
        assert_eq!(hidden(src), vec!["**", "**", "_", "_", "`", "`", "~~", "~~"]);
        assert_eq!(styles(src, Style::Code), vec!["d"]);
    }

    #[test]
    fn links() {
        let src = "[[Note|alias]] [[Other]] [t](http://x) ![[img.png]]";
        assert_eq!(styles(src, Style::WikiLink), vec!["alias", "Other"]);
        assert_eq!(styles(src, Style::Link), vec!["t"]);
        assert_eq!(styles(src, Style::Embed), vec!["![[img.png]]"]);
        assert_eq!(hidden(src), vec!["[[Note|", "]]", "[[", "]]", "[", "](http://x)"]);
    }

    #[test]
    fn link_targets() {
        let src = "[[Note#Head|a]] [b](Other%20note.md) ![[img.png]] <x@y.z>";
        let t: Vec<_> = parse(src).links.into_iter().map(|l| (l.target, l.wiki, l.embed)).collect();
        assert_eq!(
            t,
            vec![
                ("Note#Head".into(), true, false),
                ("Other%20note.md".into(), false, false),
                ("img.png".into(), true, true),
                ("mailto:x@y.z".into(), false, false),
            ]
        );
    }

    #[test]
    fn fenced_code() {
        let src = "```rust\nlet a = 1; // #notag\n```\n";
        assert_eq!(hidden(src), vec!["```rust", "```"]);
        assert!(styles(src, Style::Tag).is_empty());
    }

    #[test]
    fn obsidian_extras() {
        let src = "x ==hi== #tag #123 a#b %%secret #t%%\n";
        assert_eq!(styles(src, Style::Highlight), vec!["==hi=="]);
        assert_eq!(styles(src, Style::Tag), vec!["#tag"]);
        assert_eq!(styles(src, Style::Comment), vec!["%%secret #t%%"]);
    }

    #[test]
    fn multiline_comment_and_frontmatter() {
        let src = "---\ntags: a\n---\n%%a\nb%%\n";
        assert_eq!(styles(src, Style::Frontmatter), vec!["---\ntags: a\n---"]);
        assert_eq!(styles(src, Style::Comment), vec!["%%a\nb%%"]);
    }

    #[test]
    fn tasks_and_quotes() {
        let src = "- [x] done\n- [ ] todo\n\n> [!note] Title\n> body\n";
        let p = parse(src);
        assert_eq!(p.tasks.len(), 2);
        assert_eq!(styles(src, Style::TaskDone), vec![" done"]);
        assert_eq!(styles(src, Style::CalloutTitle), vec!["[!note] Title"]);
        assert!(hidden(src).contains(&"> "));
    }

    #[test]
    fn cyrillic_offsets() {
        let src = "**жирный** и #тег";
        assert_eq!(styles(src, Style::Bold), vec!["**жирный**"]);
        assert_eq!(styles(src, Style::Tag), vec!["#тег"]);
    }
}

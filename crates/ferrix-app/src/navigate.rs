//! Finding your way around a PDF: the outline, links on pages, and Back / Forward.

use std::sync::Arc;

use ferrix_core::{PageKey, SourceId};
use ferrix_engine::{LinkTarget, OutlineItem, PageLink, PdfEngine};
use ferrix_types::{DocId, PageIndex};
use iced::Task;

use crate::{
    app::{App, Message},
    worker::flatten,
};

/// Jumps remembered for Back.
const MAX_BACK: usize = 50;
/// Outline levels indented; deeper entries line up with the last one.
pub(crate) const MAX_INDENT: usize = 6;

/// One outline entry, flattened in document order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutlineNode {
    pub(crate) title: String,
    /// Target page in the document's own pages.
    pub(crate) page: Option<PageIndex>,
    /// Where on the page, in points from the top.
    pub(crate) top: Option<f32>,
    pub(crate) depth: usize,
    pub(crate) parent: Option<usize>,
    /// Index just past this entry's descendants.
    pub(crate) end: usize,
    pub(crate) open: bool,
}

impl OutlineNode {
    pub(crate) fn has_children(&self, index: usize) -> bool {
        self.end > index + 1
    }
}

/// Flattens the outline tree. With a single top-level entry (a title wrapping the chapters),
/// that entry starts open; everything else starts closed.
pub(crate) fn flatten_outline(items: &[OutlineItem]) -> Vec<OutlineNode> {
    fn walk(
        items: &[OutlineItem],
        depth: usize,
        parent: Option<usize>,
        out: &mut Vec<OutlineNode>,
    ) {
        for item in items {
            let index = out.len();
            out.push(OutlineNode {
                title: item.title.split_whitespace().collect::<Vec<_>>().join(" "),
                page: item.page,
                top: item.top,
                depth,
                parent,
                end: index + 1,
                open: false,
            });
            walk(&item.children, depth + 1, Some(index), out);
            out[index].end = out.len();
        }
    }
    let mut out = Vec::new();
    walk(items, 0, None, &mut out);
    if items.len() == 1
        && let Some(first) = out.first_mut()
    {
        first.open = true;
    }
    out
}

/// The entries shown: those whose ancestors are all open.
pub(crate) fn visible_rows(nodes: &[OutlineNode]) -> Vec<usize> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        rows.push(i);
        i = if nodes[i].open { i + 1 } else { nodes[i].end };
    }
    rows
}

/// The visible entry for the section holding page `current`: the last entry in document order
/// that starts at or before it, or its nearest visible ancestor if it is folded away.
pub(crate) fn current_section(
    nodes: &[OutlineNode],
    visible: &[usize],
    position: impl Fn(&OutlineNode) -> Option<usize>,
    current: usize,
) -> Option<usize> {
    let mut found = None;
    for (i, node) in nodes.iter().enumerate() {
        if let Some(p) = position(node) {
            if p <= current {
                found = Some(i);
            } else {
                // Outlines are usually in page order; an entry further on ends the search.
                break;
            }
        }
    }
    let mut at = found?;
    while visible.binary_search(&at).is_err() {
        at = nodes[at].parent?;
    }
    Some(at)
}

/// A place in the view to come back to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spot {
    pub(crate) page: PageKey,
    pub(crate) within: f32,
}

/// Back and Forward through jumps (links, outline, typed page numbers).
#[derive(Debug, Clone, Default)]
pub(crate) struct Trail {
    back: Vec<Spot>,
    forward: Vec<Spot>,
}

impl Trail {
    /// Remembers `from` before a jump; a new jump clears Forward.
    pub(crate) fn push(&mut self, from: Spot) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
            if self.back.len() > MAX_BACK {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    pub(crate) fn back(&mut self, here: Spot) -> Option<Spot> {
        let to = self.back.pop()?;
        self.forward.push(here);
        Some(to)
    }

    pub(crate) fn forward(&mut self, here: Spot) -> Option<Spot> {
        let to = self.forward.pop()?;
        self.back.push(here);
        Some(to)
    }

    pub(crate) fn last(&self) -> Option<Spot> {
        self.back.last().copied()
    }
}

/// What the status bar says while the pointer is on a link.
pub(crate) fn link_hint(target: &LinkTarget, page_number: Option<usize>) -> String {
    match target {
        LinkTarget::Page(_) => match page_number {
            Some(n) => format!("Go to page {n}"),
            None => "This link’s page was deleted".into(),
        },
        LinkTarget::Uri(uri) => middle_ellipsis(uri, 80),
    }
}

/// `text` shortened to `max` characters by cutting its middle.
pub(crate) fn middle_ellipsis(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_owned();
    }
    let keep = max.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Whether a URI from a document may be offered to the system at all, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UriKind {
    Web,
    Mail,
    /// Refused; holds the scheme as written, for the message.
    Refused(String),
}

pub(crate) fn uri_kind(uri: &str) -> UriKind {
    let scheme = uri
        .split_once(':')
        .map_or(String::new(), |(s, _)| s.to_ascii_lowercase());
    // Control characters and spaces could split or smuggle arguments; real links escape them.
    let clean = !uri.chars().any(|c| c.is_control() || c.is_whitespace());
    match scheme.as_str() {
        "http" | "https" if clean && uri.len() > scheme.len() + 3 => UriKind::Web,
        "mailto" if clean && uri.len() > 7 => UriKind::Mail,
        _ if scheme.is_empty() => UriKind::Refused(uri.chars().take(20).collect()),
        _ => UriKind::Refused(format!("{scheme}:")),
    }
}

/// The host of a web address, for showing it prominently.
pub(crate) fn host_of(uri: &str) -> Option<&str> {
    let rest = uri.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    Some(host).filter(|h| !h.is_empty())
}

/// Hands a checked URI to the system's browser or mail app.
pub(crate) fn open_uri(uri: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(uri);
        c
    };
    #[cfg(windows)]
    let mut command = {
        // Not `cmd /C start`, which would parse `&` in the address as a command separator.
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(uri);
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(uri);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

impl App {
    /// Reads the outline of a newly opened document in the background.
    pub(crate) fn load_outline(&self, doc: DocId) -> Task<Message> {
        let Ok(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { flatten(engine.call(move |e| e.outline(doc)).await) },
            move |result| Message::OutlineLoaded(doc, result),
        )
    }

    pub(crate) fn outline_loaded(
        &mut self,
        doc_id: DocId,
        result: Result<Vec<OutlineItem>, String>,
    ) {
        let Some(doc) = self.doc.as_mut().filter(|d| d.info.id == doc_id) else {
            return;
        };
        doc.outline = Some(match result {
            Ok(items) => flatten_outline(&items),
            Err(e) => {
                tracing::warn!("outline: {e}");
                Vec::new()
            }
        });
    }

    /// Opens or closes an outline entry; `all` does its descendants too.
    pub(crate) fn outline_toggle(&mut self, index: usize, all: bool) {
        let Some(nodes) = self.doc.as_mut().and_then(|d| d.outline.as_mut()) else {
            return;
        };
        let Some(node) = nodes.get(index) else {
            return;
        };
        let (open, end) = (!node.open, node.end);
        let range = if all { index..end } else { index..index + 1 };
        for n in &mut nodes[range] {
            n.open = open;
        }
    }

    /// The position in the page list of `page` of source `source`, if it is still there.
    pub(crate) fn position_of(&self, source: SourceId, page: PageIndex) -> Option<usize> {
        self.doc
            .as_ref()?
            .state
            .pages
            .entries()
            .iter()
            .position(|e| e.source == source && e.page == page)
    }

    pub(crate) fn outline_jump(&mut self, index: usize) -> Task<Message> {
        let Some(node) = self
            .doc
            .as_ref()
            .and_then(|d| d.outline.as_ref())
            .and_then(|n| n.get(index))
            .cloned()
        else {
            return Task::none();
        };
        let Some(page) = node.page else {
            return Task::none();
        };
        match self.position_of(SourceId::BASE, page) {
            Some(position) => self.jump(position, node.top),
            None => Task::none(),
        }
    }

    /// Follows a link clicked on the page with `key`.
    pub(crate) fn follow_link(&mut self, key: PageKey, target: LinkTarget) -> Task<Message> {
        match target {
            LinkTarget::Page(dest) => {
                let source = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.state.pages.entries().iter().find(|e| e.key == key))
                    .map_or(SourceId::BASE, |e| e.source);
                match self.position_of(source, dest.page) {
                    Some(position) => self.jump(position, dest.top),
                    None => {
                        self.error = Some("This link goes to a page that was deleted.".into());
                        Task::none()
                    }
                }
            }
            LinkTarget::Uri(uri) => {
                match uri_kind(&uri) {
                    UriKind::Web | UriKind::Mail => self.link_prompt = Some(uri),
                    UriKind::Refused(scheme) => {
                        self.error = Some(format!(
                            "Ferrix doesn’t open “{scheme}” links from documents."
                        ));
                    }
                }
                Task::none()
            }
        }
    }

    /// The link confirmation's answer.
    pub(crate) fn link_answer(&mut self, open: bool) -> Task<Message> {
        let Some(uri) = self.link_prompt.take() else {
            return Task::none();
        };
        if open
            && matches!(uri_kind(&uri), UriKind::Web | UriKind::Mail)
            && let Err(e) = open_uri(&uri)
        {
            self.error = Some(format!("Couldn’t open the link: {e}"));
        }
        Task::none()
    }

    pub(crate) fn copy_link(&mut self) -> Task<Message> {
        let Some(uri) = self.link_prompt.take() else {
            return Task::none();
        };
        let uri = uri
            .strip_prefix("mailto:")
            .map_or(uri.clone(), str::to_owned);
        iced::clipboard::write(uri)
    }

    /// Where the view is now, to come back to.
    fn spot(&self) -> Option<Spot> {
        let doc = self.doc.as_ref()?;
        Some(Spot {
            page: doc.key_at(doc.current)?,
            within: doc.scroll_within,
        })
    }

    /// Jumps to page `position`, `top` points down it, remembering where the view was.
    pub(crate) fn jump(&mut self, position: usize, top: Option<f32>) -> Task<Message> {
        if let Some(here) = self.spot()
            && let Some(doc) = self.doc.as_mut()
        {
            doc.trail.push(here);
        }
        self.show_at(position, top)
    }

    /// Shows page `position` with `top` points of it above the view (rotation-aware: a page
    /// turned in Ferrix ignores the top, which is in the file's orientation).
    fn show_at(&mut self, position: usize, top: Option<f32>) -> Task<Message> {
        let within = self.doc.as_ref().and_then(|doc| {
            let entry = doc.state.pages.get(position)?;
            let top = top.filter(|_| entry.rotation == ferrix_types::Rotation::R0)?;
            let height = doc.page_size(entry).height;
            (height > 0.0).then(|| (top / height).clamp(0.0, 1.0))
        });
        let task = self.go_to(position);
        let Some(within) = within else {
            return task;
        };
        let height = self.viewport_height();
        let Some(doc) = self.doc.as_mut() else {
            return task;
        };
        let at = doc.strip.at_fraction(position, within, height);
        doc.scroll_within = doc.strip.fraction(position, at);
        Task::batch([task, self.scroll_pages_to(at)])
    }

    fn show_spot(&mut self, spot: Spot) -> Task<Message> {
        let Some(position) = self
            .doc
            .as_ref()
            .and_then(|d| d.state.pages.position(spot.page))
        else {
            return Task::none();
        };
        let height = self
            .doc
            .as_ref()
            .and_then(|d| d.state.pages.get(position).map(|e| d.page_size(e).height))
            .unwrap_or(0.0);
        self.show_at(position, Some(spot.within * height))
    }

    pub(crate) fn go_back(&mut self) -> Task<Message> {
        let Some(here) = self.spot() else {
            return Task::none();
        };
        match self.doc.as_mut().and_then(|d| d.trail.back(here)) {
            Some(spot) => self.show_spot(spot),
            None => Task::none(),
        }
    }

    pub(crate) fn go_forward(&mut self) -> Task<Message> {
        let Some(here) = self.spot() else {
            return Task::none();
        };
        match self.doc.as_mut().and_then(|d| d.trail.forward(here)) {
            Some(spot) => self.show_spot(spot),
            None => Task::none(),
        }
    }

    /// The page number Back returns to, for its button.
    pub(crate) fn back_page(&self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        doc.state
            .pages
            .position(doc.trail.last()?.page)
            .map(|p| p + 1)
    }

    /// The status-bar hint for a hovered link.
    pub(crate) fn link_hover(&mut self, page: PageKey, target: Option<LinkTarget>) {
        self.link_hint = target.map(|target| {
            let number = match &target {
                LinkTarget::Page(dest) => {
                    let source = self
                        .doc
                        .as_ref()
                        .and_then(|d| d.state.pages.entries().iter().find(|e| e.key == page))
                        .map_or(SourceId::BASE, |e| e.source);
                    self.position_of(source, dest.page).map(|p| p + 1)
                }
                LinkTarget::Uri(_) => None,
            };
            link_hint(&target, number)
        });
    }
}

/// Links of a page, read with its text.
pub(crate) type Links = Arc<Vec<PageLink>>;

/// Reads a page's text and links in one engine call.
pub(crate) fn text_and_links(
    engine: &mut impl PdfEngine,
    doc: DocId,
    page: PageIndex,
) -> Result<(ferrix_engine::TextPage, Vec<PageLink>), ferrix_engine::EngineError> {
    let text = engine.text_page(doc, page)?;
    // A page whose links can't be read still has its text.
    let links = engine.links(doc, page).unwrap_or_default();
    Ok((text, links))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, page: u32, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem {
            title: title.into(),
            page: Some(PageIndex(page)),
            top: None,
            children,
        }
    }

    fn sample() -> Vec<OutlineNode> {
        flatten_outline(&[
            item(
                "One",
                0,
                vec![item("1.1", 1, vec![]), item("1.2", 3, vec![])],
            ),
            item("Two", 5, vec![item("2.1", 6, vec![])]),
            item("Three", 8, vec![]),
        ])
    }

    #[test]
    fn outlines_flatten_in_document_order() {
        let n = sample();
        let titles: Vec<&str> = n.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["One", "1.1", "1.2", "Two", "2.1", "Three"]);
        assert_eq!((n[0].end, n[3].end, n[5].end), (3, 5, 6));
        assert_eq!(n[2].parent, Some(0));
        assert!(n[0].has_children(0) && !n[1].has_children(1));
        assert!(
            n.iter().all(|n| !n.open),
            "several top-level entries start closed"
        );
        let single = flatten_outline(&[item("Book", 0, vec![item("Ch 1", 1, vec![])])]);
        assert!(single[0].open);
    }

    #[test]
    fn only_entries_under_open_parents_show() {
        let mut n = sample();
        assert_eq!(visible_rows(&n), [0, 3, 5]);
        n[0].open = true;
        assert_eq!(visible_rows(&n), [0, 1, 2, 3, 5]);
    }

    #[test]
    fn the_current_section_is_the_last_one_started() {
        let mut n = sample();
        let position = |node: &OutlineNode| node.page.map(|p| p.0 as usize);
        n[0].open = true;
        let visible = visible_rows(&n);
        assert_eq!(current_section(&n, &visible, position, 0), Some(0));
        assert_eq!(current_section(&n, &visible, position, 4), Some(2));
        // 2.1 is folded away inside Two.
        assert_eq!(current_section(&n, &visible, position, 7), Some(3));
        assert_eq!(current_section(&n, &visible, position, 100), Some(5));
    }

    #[test]
    fn back_and_forward_retrace_jumps() {
        let pages = ferrix_core::PageList::identity(10);
        let spot = |n: usize| Spot {
            page: pages.entries()[n].key,
            within: 0.0,
        };
        let mut trail = Trail::default();
        trail.push(spot(1));
        trail.push(spot(5));
        assert_eq!(trail.back(spot(9)), Some(spot(5)));
        assert_eq!(trail.back(spot(5)), Some(spot(1)));
        assert_eq!(trail.back(spot(1)), None);
        assert_eq!(trail.forward(spot(1)), Some(spot(5)));
        trail.push(spot(5));
        assert_eq!(trail.forward(spot(2)), None, "a new jump clears Forward");
    }

    #[test]
    fn only_web_and_mail_links_are_offered() {
        assert_eq!(uri_kind("https://example.com/a?b=1&c"), UriKind::Web);
        assert_eq!(uri_kind("HTTP://example.com"), UriKind::Web);
        assert_eq!(uri_kind("mailto:ana@example.com"), UriKind::Mail);
        assert_eq!(
            uri_kind("file:///etc/passwd"),
            UriKind::Refused("file:".into())
        );
        assert_eq!(
            uri_kind("javascript:alert(1)"),
            UriKind::Refused("javascript:".into())
        );
        assert_eq!(uri_kind("https://"), UriKind::Refused("https:".into()));
        assert_eq!(
            uri_kind("https://example.com/\n--flag"),
            UriKind::Refused("https:".into())
        );
    }

    #[test]
    fn hosts_and_hints() {
        assert_eq!(
            host_of("https://user@pricing.example.com:8080/q3?x"),
            Some("pricing.example.com:8080")
        );
        assert_eq!(host_of("https://example.com"), Some("example.com"));
        assert_eq!(host_of("mailto:a@b.c"), None);
        assert_eq!(middle_ellipsis("abcdefghij", 5), "ab…ij");
        assert_eq!(middle_ellipsis("short", 10), "short");
        let target = LinkTarget::Page(ferrix_engine::Destination {
            page: PageIndex(0),
            top: None,
        });
        assert_eq!(link_hint(&target, Some(12)), "Go to page 12");
    }
}

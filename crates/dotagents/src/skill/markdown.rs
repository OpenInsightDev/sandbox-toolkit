//! Agent Skills load progressively: metadata first, the `SKILL.md` body on
//! activation, referenced files only when needed. A client that can address
//! *part* of a body — "the Rollback section", "everything under Deploy" —
//! carries that one level further, so the body is parsed once into a
//! [`Document`]: a preamble plus a tree of [`Section`]s, each addressable by
//! heading path or by [`Slug`], each knowing its own span of the source.
//!
//! The parse is faithful about the two things that actually bite:
//!
//! * **Fenced code is inert.** A `# comment` inside a ``` fence is a comment,
//!   not a heading — and skill bodies are full of shell snippets.
//! * **Nesting follows heading level**, so a section's span covers its
//!   subsections and ends exactly where a sibling or shallower heading begins.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeadingLevel(u8);

impl HeadingLevel {
    /// `#`, the shallowest heading.
    pub const H1: Self = Self(1);
    pub const H2: Self = Self(2);

    pub const fn new(level: u8) -> Option<Self> {
        if level >= 1 && level <= 6 { Some(Self(level)) } else { None }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

impl fmt::Display for HeadingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "h{}", self.0)
    }
}

/// A URL-safe identifier derived from heading text, unique within a document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slug(String);

impl Slug {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Slug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for Slug {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

/// Where a link points, once classified.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    /// A relative path inside the skill directory — `references/runbook.md`,
    /// `scripts/deploy.sh`. These are the progressive-disclosure resources.
    InPackage(String),
    /// An absolute URL with a scheme, or a protocol-relative one.
    External(String),
    /// A same-document anchor, `#section`.
    Fragment(String),
}

impl LinkTarget {
    /// The target exactly as written, minus any `#` for fragments.
    pub fn as_str(&self) -> &str {
        match self {
            Self::InPackage(t) | Self::External(t) | Self::Fragment(t) => t,
        }
    }
}

/// An inline Markdown link, `[text](target)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Link {
    pub text: String,
    pub target: LinkTarget,
}

/// One heading and everything beneath it.
///
/// A section carries a handle to its document's source, so `body()` and
/// `content()` work wherever the section travels.
#[derive(Clone)]
pub struct Section {
    source: Arc<str>,
    level: HeadingLevel,
    heading: String,
    slug: Slug,
    /// Heading line through the end of all nested content.
    span: Range<usize>,
    /// Prose between this heading and its first subsection.
    body: Range<usize>,
    children: Vec<Section>,
}

impl Section {
    pub fn level(&self) -> HeadingLevel {
        self.level
    }

    /// Without `#` markers.
    pub fn heading(&self) -> &str {
        &self.heading
    }

    pub fn slug(&self) -> &Slug {
        &self.slug
    }

    pub fn children(&self) -> &[Section] {
        &self.children
    }

    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    /// Prose directly under this heading, excluding subsections.
    pub fn body(&self) -> &str {
        &self.source[self.body.clone()]
    }

    /// Heading, prose, and every subsection — the slice to hand a model when
    /// it needs one part of a long skill.
    pub fn content(&self) -> &str {
        &self.source[self.span.clone()]
    }

    /// This section and its descendants, depth-first, with depth relative to
    /// this section (0 for itself).
    pub fn walk(&self) -> impl Iterator<Item = (usize, &Section)> {
        Walk { stack: vec![(0, self)] }
    }

    pub fn links(&self) -> Vec<Link> {
        collect_links(self.content())
    }
}

impl fmt::Debug for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Section")
            .field("level", &self.level)
            .field("heading", &self.heading)
            .field("slug", &self.slug)
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Section {
    fn eq(&self, other: &Self) -> bool {
        self.level == other.level
            && self.heading == other.heading
            && self.slug == other.slug
            && self.content() == other.content()
            && self.children == other.children
    }
}

impl Eq for Section {}

// Manual, to stay consistent with the hand-written `PartialEq`, which compares
// section *content* rather than the byte spans the fields hold.
impl Hash for Section {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.level.hash(state);
        self.heading.hash(state);
        self.slug.hash(state);
        self.content().hash(state);
        self.children.hash(state);
    }
}

/// A parsed Markdown document: a preamble and a tree of sections.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Document {
    source: Arc<str>,
    preamble: Range<usize>,
    sections: Vec<Section>,
}

impl Document {
    /// Always succeeds: text with no headings is simply all preamble.
    pub fn parse(source: impl AsRef<str>) -> Self {
        let source: Arc<str> = Arc::from(source.as_ref());
        let headings = scan_headings(&source);
        let preamble_end = headings.first().map_or(source.len(), |h| h.line.start);
        let sections = build_tree(&source, &headings);
        Self { source, preamble: 0..preamble_end, sections }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn preamble(&self) -> &str {
        &self.source[self.preamble.clone()]
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn is_flat(&self) -> bool {
        self.sections.is_empty()
    }

    /// Depth-first, each section paired with its nesting depth (0 at the top).
    pub fn outline(&self) -> impl Iterator<Item = (usize, &Section)> {
        Walk { stack: self.sections.iter().rev().map(|s| (0, s)).collect() }
    }

    pub fn find(&self, slug: &str) -> Option<&Section> {
        self.outline().map(|(_, s)| s).find(|s| s.slug == *slug)
    }

    /// Follow a path of heading texts from the top, ignoring case and
    /// surrounding whitespace: `["Deploy", "Rollback"]` finds `Rollback`
    /// nested under `Deploy`.
    pub fn section<'a>(&self, path: impl IntoIterator<Item = &'a str>) -> Option<&Section> {
        let mut level = &self.sections;
        let mut found = None;
        for step in path {
            let step = step.trim();
            let next = level.iter().find(|s| s.heading.trim().eq_ignore_ascii_case(step))?;
            level = &next.children;
            found = Some(next);
        }
        found
    }

    /// Targets classified as [`LinkTarget::InPackage`] are the skill's on-demand
    /// resources.
    pub fn links(&self) -> Vec<Link> {
        collect_links(&self.source)
    }
}

impl fmt::Debug for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("preamble", &self.preamble())
            .field("sections", &self.sections)
            .finish()
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

struct Walk<'a> {
    stack: Vec<(usize, &'a Section)>,
}

impl<'a> Iterator for Walk<'a> {
    type Item = (usize, &'a Section);

    fn next(&mut self) -> Option<Self::Item> {
        let (depth, section) = self.stack.pop()?;
        for child in section.children.iter().rev() {
            self.stack.push((depth + 1, child));
        }
        Some((depth, section))
    }
}

struct RawHeading {
    level: HeadingLevel,
    text: String,
    slug: String,
    /// Byte range of the heading line (or, for setext, both its lines).
    line: Range<usize>,
}

/// Find every ATX (`#`) and setext (`===`/`---` underline) heading outside
/// fenced code blocks.
fn scan_headings(source: &str) -> Vec<RawHeading> {
    let mut headings: Vec<RawHeading> = Vec::new();
    let mut slugs: Vec<String> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0;
    // The previous line, as a candidate setext heading text.
    let mut previous: Option<(Range<usize>, String)> = None;

    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let content = trimmed.trim_start();
        let indent = trimmed.len() - content.len();

        // Fences open and close only at four-or-fewer spaces of indentation.
        if indent <= 3 {
            if let Some((marker, open_len)) = fence {
                let run = content.chars().take_while(|c| *c == marker).count();
                if run >= open_len && content.trim_end().chars().all(|c| c == marker) {
                    fence = None;
                }
                offset = end;
                previous = None;
                continue;
            }
            if let Some(marker) = content.chars().next().filter(|c| *c == '`' || *c == '~') {
                let run = content.chars().take_while(|c| *c == marker).count();
                if run >= 3 {
                    fence = Some((marker, run));
                    offset = end;
                    previous = None;
                    continue;
                }
            }
        } else if fence.is_some() {
            offset = end;
            previous = None;
            continue;
        }

        let mut found: Option<(HeadingLevel, String, Range<usize>)> = None;

        if indent <= 3 && content.starts_with('#') {
            let hashes = content.chars().take_while(|c| *c == '#').count();
            let rest = &content[hashes..];
            if let Some(level) = HeadingLevel::new(hashes as u8)
                && (rest.is_empty() || rest.starts_with([' ', '\t']))
            {
                let text = rest.trim().trim_end_matches('#').trim().to_owned();
                found = Some((level, text, offset..end));
            }
        } else if indent <= 3 && !content.is_empty() {
            // Setext: an underline of `=` or `-` beneath a non-blank line.
            if let Some(marker) = content.chars().next().filter(|c| *c == '=' || *c == '-')
                && content.trim_end().chars().all(|c| c == marker)
                && let Some((prev_span, prev_text)) = previous.take()
            {
                let level = if marker == '=' { HeadingLevel::H1 } else { HeadingLevel::H2 };
                found = Some((level, prev_text, prev_span.start..end));
            }
        }

        match found {
            Some((level, text, span)) => {
                let slug = slugify(&text, &slugs);
                slugs.push(slug.clone());
                headings.push(RawHeading { level, text, slug, line: span });
                previous = None;
            }
            None => {
                previous = (!content.is_empty()).then(|| (offset..end, content.trim().to_owned()));
            }
        }
        offset = end;
    }

    headings
}

/// A stack holds the currently open sections. Each new heading closes every
/// open section at its level or deeper — that closing point is exactly where
/// the closed section's span ends.
fn build_tree(source: &Arc<str>, headings: &[RawHeading]) -> Vec<Section> {
    struct Open {
        level: HeadingLevel,
        heading: String,
        slug: String,
        start: usize,
        body_start: usize,
        children: Vec<Section>,
    }

    let mut roots: Vec<Section> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();

    let close = |open: Open, end: usize, source: &Arc<str>| Section {
        source: Arc::clone(source),
        level: open.level,
        heading: open.heading,
        slug: Slug(open.slug),
        span: open.start..end,
        // Prose runs from the heading line to the first subsection, or to
        // the end of the section when there are none.
        body: open.body_start..open.children.first().map_or(end, |c| c.span.start),
        children: open.children,
    };

    for head in headings {
        while stack.last().is_some_and(|open| open.level >= head.level) {
            let open = stack.pop().expect("checked by the condition");
            let section = close(open, head.line.start, source);
            match stack.last_mut() {
                Some(parent) => parent.children.push(section),
                None => roots.push(section),
            }
        }
        stack.push(Open {
            level: head.level,
            heading: head.text.clone(),
            slug: head.slug.clone(),
            start: head.line.start,
            body_start: head.line.end,
            children: Vec::new(),
        });
    }

    while let Some(open) = stack.pop() {
        let section = close(open, source.len(), source);
        match stack.last_mut() {
            Some(parent) => parent.children.push(section),
            None => roots.push(section),
        }
    }

    roots
}

/// GitHub-style anchor: lowercase, non-alphanumeric runs to hyphens,
/// collisions numbered.
fn slugify(text: &str, taken: &[String]) -> String {
    let mut base = String::with_capacity(text.len());
    let mut pending_hyphen = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if pending_hyphen && !base.is_empty() {
                base.push('-');
            }
            pending_hyphen = false;
            base.extend(c.to_lowercase());
        } else {
            pending_hyphen = true;
        }
    }
    if base.is_empty() {
        base.push_str("section");
    }
    if !taken.contains(&base) {
        return base;
    }
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unused suffix always exists")
}

fn collect_links(source: &str) -> Vec<Link> {
    let mut links = Vec::new();
    let mut fence: Option<(char, usize)> = None;

    for line in source.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']).trim_start();
        if let Some((marker, open_len)) = fence {
            let run = content.chars().take_while(|c| *c == marker).count();
            if run >= open_len && content.trim_end().chars().all(|c| c == marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = content.chars().next().filter(|c| *c == '`' || *c == '~')
            && content.chars().take_while(|c| *c == marker).count() >= 3
        {
            fence = Some((marker, content.chars().take_while(|c| *c == marker).count()));
            continue;
        }
        links.extend(links_in_line(content));
    }
    links
}

fn links_in_line(line: &str) -> Vec<Link> {
    let chars: Vec<char> = line.chars().collect();
    let mut links = Vec::new();
    let mut in_code_span = false;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => in_code_span = !in_code_span,
            '[' if !in_code_span && (i == 0 || chars[i - 1] != '\\') => {
                if let Some((text, target, next)) = parse_link(&chars, i) {
                    links.push(Link { text, target: classify(target) });
                    i = next;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    links
}

/// Parse `[text](target)` starting at `open`, tolerating nested brackets in
/// the text. Returns the text, the target, and the index just past the `)`.
fn parse_link(chars: &[char], open: usize) -> Option<(String, String, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (offset, c) in chars.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let target_end = chars[close + 2..].iter().position(|c| *c == ')')? + close + 2;
    let text: String = chars[open + 1..close].iter().collect();
    let raw_target: String = chars[close + 2..target_end].iter().collect();
    // Drop an optional title: [text](target "title")
    let target = raw_target
        .split_once(char::is_whitespace)
        .map_or(raw_target.as_str(), |(t, _)| t)
        .trim_matches(['<', '>'])
        .to_owned();
    if target.is_empty() {
        return None;
    }
    Some((text, target, target_end + 1))
}

fn classify(target: String) -> LinkTarget {
    if let Some(fragment) = target.strip_prefix('#') {
        return LinkTarget::Fragment(fragment.to_owned());
    }
    let has_scheme = target.split_once(':').is_some_and(|(scheme, _)| {
        scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if has_scheme || target.starts_with("//") {
        LinkTarget::External(target)
    } else {
        LinkTarget::InPackage(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_and_spans() {
        let doc = Document::parse(
            "Preamble.\n\n# One\n\nalpha\n\n## One-A\n\nbeta\n\n## One-B\n\ngamma\n\n# Two\n\ndelta\n",
        );
        assert_eq!(doc.preamble().trim(), "Preamble.");
        assert_eq!(doc.sections().len(), 2);

        let one = &doc.sections()[0];
        assert_eq!(one.heading(), "One");
        assert_eq!(one.body().trim(), "alpha");
        assert!(one.content().contains("beta") && one.content().contains("gamma"));
        assert!(!one.content().contains("delta"));
        assert_eq!(one.children().len(), 2);
        assert_eq!(one.children()[0].body().trim(), "beta");

        let two = &doc.sections()[1];
        assert_eq!(two.heading(), "Two");
        assert_eq!(two.body().trim(), "delta");
        assert!(two.is_leaf());
    }

    #[test]
    fn headings_inside_fences_are_inert() {
        let doc = Document::parse(
            "# Real\n\n```sh\n# not a heading\necho hi\n```\n\n~~~\n## also not\n~~~\n\n## Real Two\n",
        );
        let headings: Vec<_> = doc.outline().map(|(_, s)| s.heading()).collect();
        assert_eq!(headings, ["Real", "Real Two"]);
    }

    #[test]
    fn setext_headings() {
        let doc = Document::parse("Title\n=====\n\nbody\n\nSub\n---\n\nmore\n");
        assert_eq!(doc.sections().len(), 1);
        let title = &doc.sections()[0];
        assert_eq!(title.heading(), "Title");
        assert_eq!(title.level(), HeadingLevel::H1);
        assert_eq!(title.children()[0].heading(), "Sub");
        assert_eq!(title.children()[0].level(), HeadingLevel::H2);
    }

    #[test]
    fn navigation_by_path_and_slug() {
        let doc = Document::parse("# Deploy\n## Roll Back!\ntext\n");
        assert_eq!(doc.section(["deploy", "roll back!"]).unwrap().heading(), "Roll Back!");
        assert_eq!(doc.find("roll-back").unwrap().body().trim(), "text");
        assert!(doc.section(["Deploy", "Nope"]).is_none());
    }

    #[test]
    fn duplicate_headings_get_distinct_slugs() {
        let doc = Document::parse("# Notes\n# Notes\n# Notes\n");
        let slugs: Vec<_> = doc.outline().map(|(_, s)| s.slug().as_str().to_owned()).collect();
        assert_eq!(slugs, ["notes", "notes-1", "notes-2"]);
    }

    #[test]
    fn skipped_levels_still_nest() {
        let doc = Document::parse("# A\n### Deep\n## B\n");
        let a = &doc.sections()[0];
        assert_eq!(a.children().len(), 2);
        assert_eq!(a.children()[0].heading(), "Deep");
        assert_eq!(a.children()[1].heading(), "B");
    }

    #[test]
    fn links_are_classified() {
        let doc = Document::parse(
            "# Refs\n\nSee [the runbook](references/runbook.md) and \
             [home](https://example.com) and [top](#refs).\n\n\
             ```\n[not a link](nope.md)\n```\n\nInline `[skip](me.md)` code.\n",
        );
        let links = doc.links();
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, LinkTarget::InPackage("references/runbook.md".into()));
        assert_eq!(links[0].text, "the runbook");
        assert_eq!(links[1].target, LinkTarget::External("https://example.com".into()));
        assert_eq!(links[2].target, LinkTarget::Fragment("refs".into()));
    }

    #[test]
    fn documents_without_headings_are_all_preamble() {
        let doc = Document::parse("just prose\n");
        assert!(doc.is_flat());
        assert_eq!(doc.preamble(), "just prose\n");
        assert_eq!(doc.outline().count(), 0);
    }

    #[test]
    fn heading_level_is_bounded() {
        assert_eq!(HeadingLevel::new(0), None);
        assert_eq!(HeadingLevel::new(7), None);
        assert_eq!(HeadingLevel::new(6).map(HeadingLevel::get), Some(6));
        let doc = Document::parse("####### nope\n");
        assert!(doc.is_flat());
    }
}

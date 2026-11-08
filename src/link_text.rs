use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};

use api::graph::text::{Align, Block, TextPart, drawable_form, is_url};

pub fn parts(text: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, link) = rest.split_at(open);
        plain.push_str(before);
        match parse_link(link) {
            Some((label, target, len)) if !link[..len - 1].ends_with('>') && drawn(target) => {
                if !plain.is_empty() {
                    parts.push(TextPart::Text {
                        text: std::mem::take(&mut plain),
                    });
                }
                let label = label.to_owned();
                parts.push(if target == DOC_LINK {
                    TextPart::Doc { label }
                } else {
                    TextPart::Link {
                        label,
                        href: target.to_owned(),
                    }
                });
                rest = &link[len..];
            }
            _ => {
                plain.push('[');
                rest = &link[1..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        parts.push(TextPart::Text { text: plain });
    }
    parts
}

const PATH_RESERVED: &AsciiSet = &CONTROLS.add(b'%').add(b'#').add(b'?');

pub fn path_link(path: &str, task_root: &[&str]) -> TextPart {
    let slashed = path.replace('\\', "/");
    let marker = format!("/{}/", task_root.join("/"));
    let label = slashed
        .split_once(&marker)
        .and_then(|(_, task)| task.split_once('/'))
        .map_or(path, |(_, inside)| inside);
    TextPart::Link {
        label: label.to_owned(),
        href: if is_url(path) {
            drawable_form(path)
        } else {
            drawable_form(&utf8_percent_encode(path, PATH_RESERVED).to_string())
        },
    }
}

pub fn parse_link(link: &str) -> Option<(&str, &str, usize)> {
    let (label, after) = link[1..].split_once("](")?;
    let (target, end) = match after.strip_prefix('<') {
        Some(wrapped) => {
            let (target, tail) = wrapped.split_once('>')?;
            tail.starts_with(')')
                .then_some((target, target.len() + 3))?
        }
        None => {
            let target = after.split_once(')')?.0;
            if target.contains(char::is_whitespace) {
                return None;
            }
            (target, target.len() + 1)
        }
    };
    let ok = !label.is_empty()
        && !label.contains(['[', ']', '\n'])
        && !target.is_empty()
        && !target.contains('\n');
    ok.then_some((label, target, label.len() + 3 + end))
}

pub const DOC_LINK: &str = "loom:doc";

pub fn bare_link(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, link) = rest.split_at(open);
        if let Some(found) = bare_in(before) {
            return Some(found);
        }
        rest = match parse_link(link) {
            Some((_, target, len)) if link[..len - 1].ends_with('>') => {
                return Some(target.to_owned());
            }
            Some((label, target, _)) if !drawn(target) => {
                let from = label.len() + 3;
                return Some(balanced_target(&link[from..]).to_owned());
            }
            Some((_, _, len)) => &link[len..],
            None => {
                if let Some(target) = spaced_target(link) {
                    return Some(target.to_owned());
                }
                &link[1..]
            }
        };
    }
    bare_in(rest)
}

fn spaced_target(link: &str) -> Option<&str> {
    let (label, after) = link[1..].split_once("](")?;
    let target = after.split_once(')')?.0.trim();
    let ok = !label.is_empty()
        && !label.contains(['[', ']', '\n'])
        && !target.contains(['\n', '<'])
        && target.contains(' ')
        && locates(target);
    ok.then_some(target)
}

fn locates(target: &str) -> bool {
    let drive = matches!(
        target.as_bytes(),
        [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic()
    );
    is_url(target) || target.starts_with(['/', '\\', '~']) || drive
}

fn drawn(target: &str) -> bool {
    target == DOC_LINK
        || (!target.contains(['(', '<', '>'])
            && ["file://", "http://", "https://"]
                .iter()
                .any(|s| target.strip_prefix(s).is_some_and(|r| !r.is_empty())))
}

fn starts_token(before: Option<char>) -> bool {
    before.is_none_or(|c| !c.is_alphanumeric() && !"/\\.-_~".contains(c))
}

fn bare_in(segment: &str) -> Option<String> {
    let chars: Vec<(usize, char)> = segment.char_indices().collect();
    (0..chars.len()).find_map(|i| {
        let (at, c) = chars[i];
        let tail = &segment[at..];
        let before = i.checked_sub(1).map(|j| chars[j].1);
        let url = ["http://", "https://", "file://"]
            .iter()
            .any(|s| tail.starts_with(s));
        let unix = c == '/' && {
            let token = tail.split(char::is_whitespace).next().unwrap_or(tail);
            token[1..].find('/').is_some_and(|n| n > 0)
        };
        let drive = c.is_ascii_alphabetic()
            && tail[1..].starts_with(':')
            && (tail[2..].starts_with('\\') || tail[2..].starts_with('/'));
        let unc = tail.starts_with("\\\\") && chars.get(i + 2).is_some();
        (url || (starts_token(before) && (unix || drive || unc))).then(|| span(tail))
    })
}

fn span(tail: &str) -> String {
    let cut = tail.find(['\n', '`', '"', '\'', '<', '>']);
    let quoted = cut.is_some_and(|p| matches!(tail.as_bytes()[p], b'`' | b'"' | b'\''));
    let found = &tail[..cut.unwrap_or(tail.len())];
    if quoted {
        return found.to_owned();
    }
    trim_tail(found.split(char::is_whitespace).next().unwrap_or(found)).to_owned()
}

fn balanced_target(text: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return &text[..i],
            ')' => depth -= 1,
            c if c.is_whitespace() => return &text[..i],
            _ => {}
        }
    }
    text
}

fn trim_tail(mut found: &str) -> &str {
    while let Some(last) = found.chars().next_back() {
        let unmatched = last == ')' && found.matches(')').count() > found.matches('(').count();
        if !(unmatched || ".,;:!?]}」』。，".contains(last)) {
            break;
        }
        found = &found[..found.len() - last.len_utf8()];
    }
    found
}

pub fn markdown(text: &str) -> Vec<Block> {
    let mut out = Shaped::default();
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        out.event(event, &text[range]);
    }
    out.flush();
    match out.open.pop() {
        Some(Open::Blocks(blocks)) => blocks,
        _ => Vec::new(),
    }
}

enum Open {
    Blocks(Vec<Block>),
    Quote(Vec<Block>),
    List(Option<u32>, Vec<Vec<Block>>),
    Item(Vec<Block>),
    Table {
        align: Vec<Align>,
        head: Vec<Vec<TextPart>>,
        rows: Vec<Vec<Vec<TextPart>>>,
        row: Vec<Vec<TextPart>>,
    },
}

struct Shaped {
    open: Vec<Open>,
    inline: Vec<TextPart>,
    heading: Option<u8>,
    strong: usize,
    emphasis: usize,
    code: Option<String>,
    link: Option<(String, String)>,
    skip: usize,
}

impl Default for Shaped {
    fn default() -> Self {
        Self {
            open: vec![Open::Blocks(Vec::new())],
            inline: Vec::new(),
            heading: None,
            strong: 0,
            emphasis: 0,
            code: None,
            link: None,
            skip: 0,
        }
    }
}

impl Shaped {
    fn event(&mut self, event: Event, source: &str) {
        if self.skip > 0 {
            match event {
                Event::Start(Tag::Link { .. } | Tag::Image { .. }) => self.skip += 1,
                Event::End(TagEnd::Link | TagEnd::Image) => self.skip -= 1,
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(Tag::Paragraph | Tag::HtmlBlock) => self.flush(),
            Event::End(TagEnd::Paragraph | TagEnd::HtmlBlock) => self.flush(),
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush();
                self.heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => self.flush(),
            Event::Start(Tag::Strong) => self.strong += 1,
            Event::End(TagEnd::Strong) => self.strong -= 1,
            Event::Start(Tag::Emphasis) => self.emphasis += 1,
            Event::End(TagEnd::Emphasis) => self.emphasis -= 1,
            Event::Start(Tag::BlockQuote(_)) => self.open(Open::Quote(Vec::new())),
            Event::Start(Tag::List(first)) => self.open(Open::List(
                first.map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
                Vec::new(),
            )),
            Event::Start(Tag::Item) => self.open(Open::Item(Vec::new())),
            Event::Start(Tag::Table(align)) => self.open(Open::Table {
                align: align.into_iter().map(aligned).collect(),
                head: Vec::new(),
                rows: Vec::new(),
                row: Vec::new(),
            }),
            Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::Table) => {
                self.close()
            }
            Event::End(TagEnd::TableCell) => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(Open::Table { row, .. }) = self.open.last_mut() {
                    row.push(cell);
                }
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                if let Some(Open::Table {
                    head, rows, row, ..
                }) = self.open.last_mut()
                {
                    let cells = std::mem::take(row);
                    if head.is_empty() {
                        *head = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                self.flush();
                self.code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                let text = self.code.take().unwrap_or_default();
                self.push(Block::Code {
                    text: text.trim_end_matches('\n').to_owned(),
                });
            }
            Event::Start(Tag::Link { dest_url, .. })
                if self.link.is_none() && !source.contains("](<") && drawn(&dest_url) =>
            {
                self.link = Some((dest_url.into_string(), String::new()));
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                let shown = if source.contains(dest_url.as_ref()) {
                    source.to_owned()
                } else {
                    format!("{source}({dest_url})")
                };
                match self.link.as_mut() {
                    Some((_, label)) => label.push_str(&shown),
                    None => self.put(plain(shown)),
                }
                self.skip = 1;
            }
            Event::End(TagEnd::Link) => {
                if let Some((target, label)) = self.link.take() {
                    self.put(if target == DOC_LINK {
                        TextPart::Doc { label }
                    } else {
                        TextPart::Link {
                            label,
                            href: target,
                        }
                    });
                }
            }
            Event::Rule => self.push(Block::Rule),
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t)
                if self.code.is_some() || self.link.is_some() =>
            {
                if let Some(block) = self.code.as_mut() {
                    block.push_str(&t);
                } else if let Some((_, label)) = self.link.as_mut() {
                    label.push_str(&t);
                }
            }
            Event::Text(t) if self.strong > 0 => self.put(TextPart::Strong { text: t.into() }),
            Event::Text(t) if self.emphasis > 0 => self.put(TextPart::Emphasis { text: t.into() }),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                self.put(plain(t.into_string()))
            }
            Event::Code(t) => self.put(TextPart::Code { text: t.into() }),
            Event::SoftBreak | Event::HardBreak => match self.link.as_mut() {
                Some((_, label)) => label.push(' '),
                None => self.put(plain("\n")),
            },
            _ => {}
        }
    }

    fn open(&mut self, open: Open) {
        self.flush();
        self.open.push(open);
    }

    fn close(&mut self) {
        self.flush();
        let block = match self.open.pop() {
            Some(Open::Quote(blocks)) => Block::Quote { blocks },
            Some(Open::List(start, items)) => Block::List { start, items },
            Some(Open::Table {
                align, head, rows, ..
            }) => Block::Table { align, head, rows },
            Some(Open::Item(item)) => {
                if let Some(Open::List(_, items)) = self.open.last_mut() {
                    items.push(item);
                }
                return;
            }
            Some(Open::Blocks(blocks)) => {
                self.open.push(Open::Blocks(blocks));
                return;
            }
            None => return,
        };
        self.push(block);
    }

    fn flush(&mut self) {
        let parts = std::mem::take(&mut self.inline);
        let block = match self.heading.take() {
            Some(level) => Block::Heading { level, parts },
            None if parts.is_empty() => return,
            None => Block::Paragraph { parts },
        };
        self.push(block);
    }

    fn push(&mut self, block: Block) {
        if !matches!(block, Block::Paragraph { .. } | Block::Heading { .. }) {
            self.flush();
        }
        match self.open.last_mut() {
            Some(Open::Blocks(blocks) | Open::Quote(blocks) | Open::Item(blocks)) => {
                blocks.push(block)
            }
            Some(Open::List(..) | Open::Table { .. }) | None => {}
        }
    }

    fn put(&mut self, part: TextPart) {
        match (self.inline.last_mut(), part) {
            (Some(TextPart::Text { text: last }), TextPart::Text { text })
            | (Some(TextPart::Strong { text: last }), TextPart::Strong { text })
            | (Some(TextPart::Emphasis { text: last }), TextPart::Emphasis { text }) => {
                last.push_str(&text)
            }
            (_, part) => self.inline.push(part),
        }
    }
}

fn aligned(align: Alignment) -> Align {
    match align {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

fn plain(text: impl Into<String>) -> TextPart {
    TextPart::Text { text: text.into() }
}

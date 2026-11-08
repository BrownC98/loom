use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 화면이 그리는 글 조각. 화면은 링크를 찾지 않는다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "part", rename_all = "camelCase")]
#[ts(export)]
pub enum TextPart {
    Text {
        text: String,
    },
    /// `file://`·`http(s)://` 주소.
    Link {
        label: String,
        href: String,
    },
    /// 이 글에 붙은 문서(`DOC_LINK`).
    Doc {
        label: String,
    },
    /// Markdown 굵게(`**…**`).
    Strong {
        text: String,
    },
    /// Markdown 기울임(`*…*`).
    Emphasis {
        text: String,
    },
    /// Markdown 글 안 코드(`` `…` ``).
    Code {
        text: String,
    },
}

impl TextPart {
    pub fn shown(&self) -> &str {
        match self {
            Self::Text { text }
            | Self::Strong { text }
            | Self::Emphasis { text }
            | Self::Code { text } => text,
            Self::Link { label, .. } | Self::Doc { label } => label,
        }
    }
}

/// 화면이 그리는 글 덩어리(GitHub Markdown). 데몬이 한 번 읽고, 화면은 Markdown을 읽지 않는다.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "block", rename_all = "camelCase")]
#[ts(export)]
pub enum Block {
    Paragraph {
        parts: Vec<TextPart>,
    },
    /// `level`은 1~6이다.
    Heading {
        level: u8,
        parts: Vec<TextPart>,
    },
    /// 코드 블록. 화면은 줄을 바꾸지 않고 그린다.
    Code {
        text: String,
    },
    /// `start`가 있으면 번호 목록이고 첫 번호다.
    List {
        start: Option<u32>,
        items: Vec<Vec<Block>>,
    },
    Quote {
        blocks: Vec<Block>,
    },
    /// `align`은 열마다 하나다. `rows`의 줄마다 칸 수가 `head`와 같다.
    Table {
        align: Vec<Align>,
        head: Vec<Vec<TextPart>>,
        rows: Vec<Vec<Vec<TextPart>>>,
    },
    Rule,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

impl Block {
    pub fn paragraph(parts: Vec<TextPart>) -> Vec<Block> {
        if parts.is_empty() {
            Vec::new()
        } else {
            vec![Self::Paragraph { parts }]
        }
    }

    pub fn shown(blocks: &[Block]) -> String {
        let inline = |parts: &[TextPart]| parts.iter().map(TextPart::shown).collect::<String>();
        let row = |cells: &[Vec<TextPart>]| {
            cells
                .iter()
                .map(|cell| inline(cell))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        blocks
            .iter()
            .map(|block| match block {
                Self::Paragraph { parts } | Self::Heading { parts, .. } => inline(parts),
                Self::Code { text } => text.clone(),
                Self::List { items, .. } => items
                    .iter()
                    .map(|item| format!("• {}", Self::shown(item)))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Self::Quote { blocks } => Self::shown(blocks),
                Self::Table { head, rows, .. } => std::iter::once(head)
                    .chain(rows)
                    .map(|cells| row(cells))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Self::Rule => String::new(),
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn drawable_form(found: &str) -> String {
    let url = if is_url(found) {
        found.to_owned()
    } else {
        let path = found.replace('\\', "/");
        match path.as_bytes() {
            [b'/', b'/', ..] => format!("file:{path}"),
            [_, b':', ..] => format!("file:///{path}"),
            _ => format!("file://{path}"),
        }
    };
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

pub fn is_url(text: &str) -> bool {
    ["file://", "http://", "https://"]
        .iter()
        .any(|s| text.starts_with(s))
}

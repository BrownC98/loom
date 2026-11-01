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

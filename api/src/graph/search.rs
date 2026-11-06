use super::waits::{Closed, ClosedWait, HumanWait, WaitKind};

pub struct WaitSearch {
    words: String,
    id: Option<u32>,
}

impl WaitSearch {
    pub fn new(query: &str) -> Option<Self> {
        let words = query.trim().to_lowercase();
        if words.is_empty() {
            return None;
        }
        let digits = words.strip_prefix('q').unwrap_or(&words);
        let id = match digits.bytes().all(|b| b.is_ascii_digit()) {
            true => digits.parse().ok(),
            false => None,
        };
        Some(WaitSearch { words, id })
    }

    pub fn words(&self) -> &str {
        &self.words
    }

    pub fn id(&self) -> Option<u32> {
        self.id
    }

    pub fn finds_open(&self, node: &str, wait: &HumanWait) -> bool {
        self.finds(node, wait, None)
    }

    pub fn finds_closed(&self, closed: &ClosedWait) -> bool {
        let said = match &closed.how {
            Closed::Answered { text } | Closed::Instructed { text } => Some(text.as_str()),
            Closed::Confirmed | Closed::Unanswered { .. } | Closed::NodeRemoved => None,
        };
        self.finds(&closed.node.name, &closed.wait.wait, said)
    }

    fn finds(&self, node: &str, wait: &HumanWait, said: Option<&str>) -> bool {
        (wait.kind == WaitKind::Question && self.id == Some(wait.id.0))
            || [
                Some(node),
                Some(wait.text.as_str()),
                wait.instruction.as_deref(),
                said,
            ]
            .into_iter()
            .flatten()
            .any(|text| text.to_lowercase().contains(&self.words))
    }
}

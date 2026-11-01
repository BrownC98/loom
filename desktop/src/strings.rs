use std::sync::atomic::{AtomicU8, Ordering};

use api::settings::Language;
use gpui::Global;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Ko,
    En,
}

impl Global for Lang {}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

impl Lang {
    pub fn remember(self) {
        CURRENT.store(self as u8, Ordering::Relaxed);
    }

    pub fn current() -> Lang {
        match CURRENT.load(Ordering::Relaxed) {
            0 => Lang::Ko,
            _ => Lang::En,
        }
    }

    pub fn resolve(language: Language, system_locale: &str) -> Lang {
        match language {
            Language::Ko => Lang::Ko,
            Language::En => Lang::En,
            Language::System if system_locale.to_ascii_lowercase().starts_with("ko") => Lang::Ko,
            Language::System => Lang::En,
        }
    }
}

fn fill(template: &str, args: &[(&str, &str)]) -> String {
    args.iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(&format!("{{{{{name}}}}}"), value)
        })
}

include!(concat!(env!("OUT_DIR"), "/strings.rs"));

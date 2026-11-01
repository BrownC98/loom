use api::settings::Language;
use gpui::Global;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Ko,
    En,
}

impl Global for Lang {}

impl Lang {
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

use api::settings::SettingsView;
use gpui::{App, WeakEntity};
use serde_json::{Map, Value};

use super::dialog::Dialog;
use crate::problem::Failure;

fn diff(before: Option<&Value>, after: &Value) -> Option<Value> {
    if let (Some(Value::Object(old)), Value::Object(new)) = (before, after) {
        let mut out = Map::new();
        let added = new.keys().filter(|key| !old.contains_key(*key));
        for key in old.keys().chain(added) {
            let change = match new.get(key) {
                Some(value) => diff(old.get(key), value),
                None => Some(Value::Null),
            };
            if let Some(change) = change {
                out.insert(key.clone(), change);
            }
        }
        return (!out.is_empty()).then_some(Value::Object(out));
    }
    (before != Some(after)).then(|| after.clone())
}

pub fn settings_patch(before: &SettingsView, after: &SettingsView) -> Option<Value> {
    let before = serde_json::to_value(before).ok()?;
    let after = serde_json::to_value(after).ok()?;
    diff(Some(&before), &after)
}

pub fn nulled(path: &[&str]) -> Value {
    path.iter().rev().fold(Value::Null, |inner, key| {
        Value::Object(Map::from_iter([((*key).to_owned(), inner)]))
    })
}

pub fn nulls(keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .map(|key| ((*key).to_owned(), Value::Null))
            .collect(),
    )
}

#[derive(Clone)]
pub struct Writer(WeakEntity<Dialog>);

impl Writer {
    pub fn new(dialog: WeakEntity<Dialog>) -> Self {
        Self(dialog)
    }

    pub fn edit(&self, cx: &mut App, change: impl FnOnce(&mut SettingsView)) {
        let _ = self.0.update(cx, |dialog, cx| dialog.edit(change, cx));
    }

    pub fn edit_then(
        &self,
        cx: &mut App,
        change: impl FnOnce(&mut SettingsView),
        done: impl FnOnce(Result<(), &Failure>, &mut App) + 'static,
    ) {
        let _ = self
            .0
            .update(cx, |dialog, cx| dialog.edit_then(change, done, cx));
    }

    pub fn reset(&self, cx: &mut App, path: &[&str]) {
        let patch = nulled(path);
        let _ = self.0.update(cx, |dialog, cx| dialog.send(patch, cx));
    }
}

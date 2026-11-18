use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use api::graph::errors::Problem;
use api::settings::{FloatPlace, Language, SettingsView, merge};

use crate::db::Database;
use crate::roles::{self, ChoiceCheck, RoleProblem};
use api::settings::roles::Roles;

fn place_problem(place: FloatPlace, field: &str) -> Option<String> {
    let ok = |v: f32| (0.0..=1.0).contains(&v);
    (!(ok(place.x) && ok(place.y))).then(|| format!("{field} must be numbers from 0 to 1"))
}

pub struct NameWords {
    pub lead: &'static str,
    pub shell: &'static str,
}

#[cfg(not(windows))]
fn system_korean() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .is_some_and(|v| v.starts_with("ko"))
}

#[cfg(windows)]
fn system_korean() -> bool {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut name = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    len > 0 && String::from_utf16_lossy(&name[..len as usize]).starts_with("ko")
}

pub struct Settings {
    db: Database,
    check: ChoiceCheck,
}

impl Settings {
    pub fn new(db: Database, check: ChoiceCheck) -> Self {
        Self { db, check }
    }

    pub fn get(&self) -> rusqlite::Result<SettingsView> {
        Ok(read(&self.db.lock())?.0)
    }

    pub fn name_words(&self) -> rusqlite::Result<NameWords> {
        let korean = match self.get()?.language {
            Language::Ko => true,
            Language::En => false,
            Language::System => system_korean(),
        };
        Ok(match korean {
            true => NameWords {
                lead: "총괄",
                shell: "셸",
            },
            false => NameWords {
                lead: "lead",
                shell: "shell",
            },
        })
    }

    pub fn patch(&self, patch: &serde_json::Value) -> Result<SettingsView, PatchProblem> {
        self.write(|_, _, value| {
            merge(value, patch);
            checked(value, self.check).map_err(PatchProblem::Shape)
        })
    }

    pub fn edit_roles(
        &self,
        allowed: impl FnOnce(&Connection) -> Result<(), Problem>,
        edit: impl FnOnce(&Roles) -> Result<Roles, RoleProblem>,
    ) -> Result<Roles, AgentEditProblem> {
        self.write(|db, view, value| {
            allowed(db).map_err(AgentEditProblem::Refused)?;
            let next = edit(&view.roles).map_err(AgentEditProblem::Role)?;
            merge(value, &serde_json::json!({ "roles": next }));
            checked(value, self.check).map_err(|e| AgentEditProblem::Role(RoleProblem::Shape(e)))
        })
        .map(|view| view.roles)
    }

    pub fn set_key(
        &self,
        key: &str,
        new: serde_json::Value,
        allowed: impl FnOnce(&Connection, &serde_json::Value) -> Result<(), Problem>,
    ) -> Result<serde_json::Value, AgentEditProblem> {
        let patch = key_patch(key, &new).map_err(AgentEditProblem::Shape)?;
        let mut kept_value = serde_json::Value::Null;
        self.write(|db, _, value| {
            allowed(db, &patch).map_err(AgentEditProblem::Refused)?;
            let unknown = || AgentEditProblem::Shape(format!("the settings have no key '{key}'"));
            if !nulls_exist(Some(value), &patch) {
                return Err(AgentEditProblem::Shape(format!(
                    "the settings have no key '{key}' or no field that {new} clears"
                )));
            }
            merge(value, &patch);
            let view = checked(value, self.check).map_err(AgentEditProblem::Shape)?;
            let stored = serde_json::to_value(&view).expect("설정은 늘 직렬화된다");
            let got = stored.pointer(key);
            if !kept(&new, got) {
                return Err(match got {
                    Some(got) => AgentEditProblem::Shape(format!(
                        "'{key}' does not keep {new}: the settings store {got}"
                    )),
                    None => unknown(),
                });
            }
            kept_value = got.cloned().unwrap_or_default();
            Ok(view)
        })?;
        Ok(kept_value)
    }

    pub fn update(&self, edit: impl FnOnce(&mut SettingsView)) -> rusqlite::Result<()> {
        let result = self.write::<PatchProblem>(|_, mut view, value| {
            edit(&mut view);
            *value = serde_json::to_value(&view).expect("설정은 늘 직렬화된다");
            Ok(view)
        });
        match result {
            Ok(_) => Ok(()),
            Err(PatchProblem::Db(e)) => Err(e),
            Err(PatchProblem::Shape(_)) => unreachable!("데몬이 만든 설정은 늘 맞는 모양이다"),
        }
    }

    pub fn move_into_wsl(&self, distro: &str) -> rusqlite::Result<()> {
        self.update(|view| {
            if let Some(dir) = view.workspace_dir.take_if(|d| d.starts_with('/')) {
                view.wsl_workspace_dirs
                    .entry(distro.to_owned())
                    .or_insert(dir);
            }
            let pc = std::mem::take(&mut view.accounts);
            let in_use = view.wsl_accounts.entry(distro.to_owned()).or_default();
            for (provider, id) in pc.0 {
                in_use.0.entry(provider).or_insert(id);
            }
        })
    }

    fn write<E: From<rusqlite::Error>>(
        &self,
        change: impl FnOnce(
            &Connection,
            SettingsView,
            &mut serde_json::Value,
        ) -> Result<SettingsView, E>,
    ) -> Result<SettingsView, E> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        let (view, mut value) = read(&tx)?;
        let next = change(&tx, view, &mut value)?;
        tx.execute(
            "INSERT OR REPLACE INTO settings (id, value) VALUES (1, ?1)",
            [serde_json::to_string(&next).expect("설정은 늘 직렬화된다")],
        )?;
        tx.commit()?;
        Ok(next)
    }
}

#[derive(Serialize, Deserialize)]
pub struct SetSetting {
    pub key: String,
    pub value: serde_json::Value,
}

fn nulls_exist(old: Option<&serde_json::Value>, new: &serde_json::Value) -> bool {
    match new {
        serde_json::Value::Null => old.is_some(),
        serde_json::Value::Object(fields) => fields
            .iter()
            .all(|(name, inner)| nulls_exist(old.and_then(|o| o.get(name)), inner)),
        _ => true,
    }
}

fn kept(new: &serde_json::Value, got: Option<&serde_json::Value>) -> bool {
    use serde_json::Value;
    match (new, got) {
        (Value::Null, None | Some(Value::Null)) => true,
        (Value::Object(fields), Some(Value::Object(stored))) => fields
            .iter()
            .all(|(name, inner)| kept(inner, stored.get(name))),
        (Value::Number(a), Some(Value::Number(b))) => {
            a.as_f64() == b.as_f64() || a.as_f64().map(|a| a as f32) == b.as_f64().map(|b| b as f32)
        }
        (new, Some(got)) => new == got,
        (_, None) => false,
    }
}

fn key_segments(key: &str) -> Result<Vec<String>, String> {
    let segments: Option<Vec<String>> = key.strip_prefix('/').map(|rest| {
        rest.split('/')
            .map(|p| p.replace("~1", "/").replace("~0", "~"))
            .collect()
    });
    match segments {
        Some(segments) if segments.iter().all(|s| !s.is_empty()) => Ok(segments),
        _ => Err(format!(
            "the key '{key}' is not a key from loom settings (a JSON pointer such as /theme)"
        )),
    }
}

fn key_patch(key: &str, value: &serde_json::Value) -> Result<serde_json::Value, String> {
    Ok(key_segments(key)?
        .iter()
        .rev()
        .fold(value.clone(), |inner, p| serde_json::json!({ p: inner })))
}

pub fn keys(value: serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    let mut lines = BTreeMap::new();
    let mut open = vec![(String::new(), value)];
    while let Some((key, value)) = open.pop() {
        match value {
            serde_json::Value::Object(fields) if !fields.is_empty() => {
                for (name, inner) in fields {
                    let name = name.replace('~', "~0").replace('/', "~1");
                    open.push((format!("{key}/{name}"), inner));
                }
            }
            leaf => {
                lines.insert(key, leaf);
            }
        }
    }
    lines
}

fn read(db: &Connection) -> rusqlite::Result<(SettingsView, serde_json::Value)> {
    let text: Option<String> = db
        .query_row("SELECT value FROM settings WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()?;
    let view: SettingsView = text
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let value = serde_json::to_value(&view).expect("설정은 늘 직렬화된다");
    Ok((view, value))
}

fn checked(value: &serde_json::Value, check: ChoiceCheck) -> Result<SettingsView, String> {
    let view: SettingsView = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let problem = roles::launch_problem(&view.roles, check).or_else(|| {
        view.composer_place
            .and_then(|p| place_problem(p, "composerPlace"))
    });
    match problem {
        Some(problem) => Err(problem),
        None => Ok(view),
    }
}

#[derive(Debug)]
pub enum PatchProblem {
    Db(rusqlite::Error),
    Shape(String),
}

pub enum AgentEditProblem {
    Db(rusqlite::Error),
    Shape(String),
    Role(RoleProblem),
    Refused(Problem),
}

impl From<rusqlite::Error> for AgentEditProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<rusqlite::Error> for PatchProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

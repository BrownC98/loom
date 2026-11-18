use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use api::settings::{FloatPlace, Language, SettingsView};

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
        self.write(|_, value| {
            merge(value, patch);
            checked(value, self.check).map_err(PatchProblem::Shape)
        })
    }

    pub fn edit_roles(
        &self,
        edit: impl FnOnce(&Roles) -> Result<Roles, RoleProblem>,
    ) -> Result<Roles, RolesProblem> {
        self.write(|view, value| {
            let next = edit(&view.roles).map_err(RolesProblem::Role)?;
            merge(value, &serde_json::json!({ "roles": next }));
            checked(value, self.check).map_err(|e| RolesProblem::Role(RoleProblem::Shape(e)))
        })
        .map(|view| view.roles)
    }

    pub fn set_key(
        &self,
        key: &str,
        new: serde_json::Value,
    ) -> Result<serde_json::Value, PatchProblem> {
        let patch = key_patch(key, &new)?;
        let pointer = format!("/{}", key.replace('.', "/"));
        let unknown = || PatchProblem::Shape(format!("the settings have no key '{key}'"));
        let mut kept = serde_json::Value::Null;
        self.write(|_, value| {
            if new.is_null() && value.pointer(&pointer).is_none() {
                return Err(unknown());
            }
            merge(value, &patch);
            let view = checked(value, self.check).map_err(PatchProblem::Shape)?;
            let stored = serde_json::to_value(&view).expect("설정은 늘 직렬화된다");
            kept = stored.pointer(&pointer).cloned().unwrap_or_default();
            match (new.is_null(), stored.pointer(&pointer)) {
                (true, _) => Ok(view),
                (false, Some(got)) if *got == new => Ok(view),
                (false, Some(got)) => Err(PatchProblem::Shape(format!(
                    "'{key}' does not keep {new}: the settings store {got}"
                ))),
                (false, None) => Err(unknown()),
            }
        })?;
        Ok(kept)
    }

    pub fn update(&self, edit: impl FnOnce(&mut SettingsView)) -> rusqlite::Result<()> {
        let result = self.write::<PatchProblem>(|mut view, value| {
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
        change: impl FnOnce(SettingsView, &mut serde_json::Value) -> Result<SettingsView, E>,
    ) -> Result<SettingsView, E> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        let (view, mut value) = read(&tx)?;
        let next = change(view, &mut value)?;
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

pub fn key_patch(key: &str, value: &serde_json::Value) -> Result<serde_json::Value, PatchProblem> {
    if key.split('.').any(str::is_empty) {
        return Err(PatchProblem::Shape(format!("the key '{key}' is not valid")));
    }
    Ok(key
        .rsplit('.')
        .fold(value.clone(), |inner, p| serde_json::json!({ p: inner })))
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

fn merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
    let serde_json::Value::Object(fields) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = serde_json::Value::Object(serde_json::Map::new());
    }
    let map = target.as_object_mut().expect("바로 위에서 객체로 만들었다");
    for (key, value) in fields {
        if value.is_null() {
            map.remove(key);
        } else {
            merge(
                map.entry(key.clone()).or_insert(serde_json::Value::Null),
                value,
            );
        }
    }
}

#[derive(Debug)]
pub enum PatchProblem {
    Db(rusqlite::Error),
    Shape(String),
}

pub enum RolesProblem {
    Db(rusqlite::Error),
    Role(RoleProblem),
}

impl From<rusqlite::Error> for RolesProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<rusqlite::Error> for PatchProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

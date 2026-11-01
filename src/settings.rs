use rusqlite::{Connection, OptionalExtension};

use api::settings::{FloatPlace, SettingsView};

use crate::db::Database;
use crate::roles::{self, ChoiceCheck, RoleProblem, SetRole};
use api::settings::roles::Roles;

fn place_problem(place: FloatPlace, field: &str) -> Option<String> {
    let ok = |v: f32| (0.0..=1.0).contains(&v);
    (!(ok(place.x) && ok(place.y))).then(|| format!("{field} must be numbers from 0 to 1"))
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

    pub fn patch(&self, patch: &serde_json::Value) -> Result<SettingsView, PatchProblem> {
        self.write(|_, value| {
            merge(value, patch);
            checked(value, self.check).map_err(PatchProblem::Shape)
        })
    }

    pub fn set_role(&self, set: &SetRole, known: ChoiceCheck) -> Result<Roles, SetRoleProblem> {
        self.write(|view, value| {
            let next = roles::with(&view.roles, set, known).map_err(SetRoleProblem::Role)?;
            merge(value, &serde_json::json!({ "roles": next }));
            checked(value, self.check).map_err(SetRoleProblem::Shape)
        })
        .map(|view| view.roles)
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
    let problem = roles::launch_problem(&view.roles, check)
        .or_else(|| view.pill_place.and_then(|p| place_problem(p, "pillPlace")))
        .or_else(|| {
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

pub enum SetRoleProblem {
    Db(rusqlite::Error),
    Shape(String),
    Role(RoleProblem),
}

impl From<rusqlite::Error> for SetRoleProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<rusqlite::Error> for PatchProblem {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

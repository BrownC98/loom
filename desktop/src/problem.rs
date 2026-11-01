use serde_json::Value;

use crate::strings::{Lang, Str};

type Vars = Vec<(String, String)>;

pub fn status(status: u16) -> String {
    Str::ProblemStatus.with(Lang::current(), &[("status", &status.to_string())])
}

pub fn text(body: &str) -> Option<String> {
    let problem: Value = serde_json::from_str(body).ok()?;
    let value = problem.get("value");
    let (keys, vars): (Vec<String>, Vars) = match problem.get("code")?.as_str()? {
        "refused" => (vec![format!("problem.refused.{}", code(value?)?)], vars(value?)),
        "repo" => (vec![format!("problem.repo.{}", code(value?)?)], vars(value?)),
        "launch" => {
            let launch = value?;
            if code(launch)? == "wsl" {
                let inner = launch.get("value")?;
                (vec![format!("problem.wsl.{}", code(inner)?)], fields(inner))
            } else {
                let key = format!("problem.launch.{}", code(launch)?);
                let vars = vars(launch);
                let wsl = vars.iter().any(|(name, _)| name == "distro");
                let keys = if wsl {
                    vec![format!("{key}_wsl"), key]
                } else {
                    vec![key]
                };
                (keys, vars)
            }
        }
        "noLogin" => (vec!["problem.noLogin".to_owned()], Vars::new()),
        "internal" => (
            vec!["problem.internal".to_owned()],
            vec![("value".to_owned(), joined(value?))],
        ),
        _ => return None,
    };
    let template = keys.iter().find_map(|key| Str::find(key))?;
    let args: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    Some(template.with(Lang::current(), &args))
}

fn code(coded: &Value) -> Option<&str> {
    coded.get("code")?.as_str()
}

fn vars(coded: &Value) -> Vars {
    match coded.get("value") {
        Some(value @ Value::Object(_)) => fields(value),
        Some(value) => vec![("value".to_owned(), joined(value))],
        None => Vars::new(),
    }
}

fn fields(value: &Value) -> Vars {
    value
        .as_object()
        .map(|map| map.iter().map(|(k, v)| (k.clone(), joined(v))).collect())
        .unwrap_or_default()
}

fn joined(value: &Value) -> String {
    match value {
        Value::Array(items) => items.iter().map(scalar).collect::<Vec<_>>().join(", "),
        other => scalar(other),
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

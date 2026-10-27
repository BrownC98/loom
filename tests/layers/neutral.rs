use std::path::{Path, PathBuf};

const PROFILE_DIR: &str = "src/profile";

const PROFILE_FILE: &str = "src/profile.rs";

const ADAPTER_IMPL: &str = "Adapter for ";

const SHOWN_AS: [(&str, &str); 2] = [("Omp", "OMP"), ("OpenCode2", "OpenCode")];

const DATA: [(&str, &str); 6] = [
    (
        "src/roles.rs",
        r#"Some(("claude", "claude-opus-5-5", "medium")),"#,
    ),
    (
        "src/roles.rs",
        r#"Some(("claude", "claude-sonnet-5-5", "medium")),"#,
    ),
    (
        "src/roles.rs",
        r#"Some(("claude", "claude-opus-5-5", "high")),"#,
    ),
    ("src/roles.rs", r#"Some(("codex", "gpt-6.1-sol", "high")),"#),
    ("src/protocol/agent.rs", "Claude,"),
    ("src/protocol/agent.rs", "Codex,"),
];

struct Agent {
    id: String,
    names: Vec<String>,
}

fn module_source(entry: &Path) -> Option<PathBuf> {
    match entry.extension() {
        Some(ext) if ext == "rs" => Some(entry.to_path_buf()),
        Some(_) => None,
        None => Some(entry.join("mod.rs")).filter(|file| file.is_file()),
    }
}

fn adapter_type(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let line = line.trim();
        let (_, rest) = line.strip_prefix("impl ")?.split_once(ADAPTER_IMPL)?;
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

fn agents(repo: &Path) -> Vec<Agent> {
    let entries = std::fs::read_dir(repo.join(PROFILE_DIR)).expect("src/profile exists");
    let mut agents: Vec<Agent> = entries
        .filter_map(|entry| {
            let path = entry.expect("src/profile entry").path();
            let id = path.file_stem()?.to_str()?.to_owned();
            let source = std::fs::read_to_string(module_source(&path)?).ok()?;
            let variant = adapter_type(&source)?;
            let mut names = vec![variant.clone()];
            names.extend(
                SHOWN_AS
                    .iter()
                    .filter(|(of, _)| *of == variant)
                    .map(|(_, shown)| (*shown).to_owned()),
            );
            Some(Agent { id, names })
        })
        .collect();
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    agents
}

fn core_files(dir: &Path, repo: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a src folder") {
        let path = entry.expect("src entry").path();
        let relative = path.strip_prefix(repo).expect("under the repository");
        if relative == Path::new(PROFILE_DIR) || relative == Path::new(PROFILE_FILE) {
            continue;
        }
        if path.is_dir() {
            core_files(&path, repo, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(relative.to_path_buf());
        }
    }
}

fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn names_module(line: &str, id: &str) -> bool {
    let path = format!("profile::{id}");
    line.match_indices(&path)
        .any(|(at, _)| !line[at + path.len()..].starts_with(ident) && !line[..at].ends_with(ident))
}

fn hits(line: &str, agents: &[Agent]) -> Vec<String> {
    let words: Vec<&str> = line.split(|c: char| !ident(c)).collect();
    let mut found = Vec::new();
    for agent in agents {
        if agent
            .names
            .iter()
            .any(|name| words.contains(&name.as_str()))
        {
            found.push(format!("the agent name of `{}`", agent.id));
        }
        if line.contains(&format!("\"{}\"", agent.id)) {
            found.push(format!("the agent id \"{}\"", agent.id));
        }
        if names_module(line, &agent.id) {
            found.push(format!("the module `profile::{}`", agent.id));
        }
    }
    found
}

pub fn problems(repo: &Path) -> Vec<String> {
    let agents = agents(repo);
    let mut problems = Vec::new();
    if agents.is_empty() {
        problems.push(format!(
            "{PROFILE_DIR} has no module with `impl {ADAPTER_IMPL}`"
        ));
    }
    for (of, _) in SHOWN_AS {
        if !agents
            .iter()
            .any(|a| a.names.first().is_some_and(|n| n == of))
        {
            problems.push(format!(
                "old display name: no adapter type `{of}` in {PROFILE_DIR}; remove it from SHOWN_AS"
            ));
        }
    }
    let mut files = Vec::new();
    core_files(&repo.join("src"), repo, &mut files);
    files.sort();
    let mut allowed = vec![false; DATA.len()];
    for file in files {
        let name = file.display().to_string();
        let source = std::fs::read_to_string(repo.join(&file)).expect("read a core file");
        for (row, line) in source.lines().enumerate() {
            let text = line.trim();
            if let Some(at) = DATA.iter().position(|d| *d == (name.as_str(), text)) {
                allowed[at] = true;
                continue;
            }
            for hit in hits(text, &agents) {
                problems.push(format!("{name}:{}: `{text}` has {hit}", row + 1));
            }
        }
    }
    for ((file, text), used) in DATA.iter().zip(allowed) {
        if !used {
            problems.push(format!(
                "old allow entry: {file} has no line `{text}`; remove it from DATA"
            ));
        }
    }
    problems
}

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

const LAYERS: [&str; 4] = ["기반", "기능", "조합", "상호작용"];

const FEATURE: &str = "기능";

const DOC: &str = "docs/design/레이어.md";

const ROOT: &str = "main";

const MODULE_TABLE: &str = "| 모듈 | 레이어 | 맡는 일 |";

const VALUE_TABLE: &str = "| 값 타입 | 모듈 |";

const EXCEPTION_TABLE: &str = "| 의존 | 항목 | 고칠 방법 |";

type Link = (String, String, String);

#[derive(Default)]
struct Doc {
    layer_of: BTreeMap<String, usize>,
    owner_of: BTreeMap<String, String>,
    exceptions: BTreeSet<Link>,
}

fn table_rows<'a>(text: &'a str, header: &str) -> Result<Vec<Vec<&'a str>>, String> {
    let mut lines = text.lines().skip_while(|line| line.trim() != header);
    if lines.next().is_none() {
        return Err(format!("{DOC} has no table with the header `{header}`"));
    }
    Ok(lines
        .skip(1)
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            line.trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect()
        })
        .collect())
}

fn code_name(cell: &str) -> Option<&str> {
    let name = cell.trim().strip_prefix('`')?.strip_suffix('`')?;
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .then_some(name)
}

fn each_row(
    text: &str,
    header: &str,
    need: &str,
    problems: &mut Vec<String>,
    mut take: impl FnMut(&[&str]) -> Option<()>,
) {
    match table_rows(text, header) {
        Ok(rows) => {
            for row in rows {
                if take(&row).is_none() {
                    problems.push(format!("{DOC}: the row `{}` needs {need}", row.join(" | ")));
                }
            }
        }
        Err(problem) => problems.push(problem),
    }
}

fn read_doc(text: &str) -> Result<Doc, Vec<String>> {
    let mut problems = Vec::new();
    let mut doc = Doc::default();
    each_row(
        text,
        MODULE_TABLE,
        "a module name in backticks and a layer name",
        &mut problems,
        |row| {
            let module = code_name(row.first()?)?;
            let layer = LAYERS.iter().position(|name| Some(name) == row.get(1))?;
            doc.layer_of.insert(module.to_owned(), layer);
            Some(())
        },
    );
    each_row(
        text,
        VALUE_TABLE,
        "a type name and a module name in backticks",
        &mut problems,
        |row| {
            let value = code_name(row.first()?)?;
            let owner = code_name(row.get(1)?)?;
            doc.owner_of.insert(value.to_owned(), owner.to_owned());
            Some(())
        },
    );
    each_row(
        text,
        EXCEPTION_TABLE,
        "`from` → `to` and a list of item names in backticks",
        &mut problems,
        |row| {
            let (from, to) = row.first()?.split_once('→')?;
            let (from, to) = (code_name(from)?, code_name(to)?);
            for item in row.get(1)?.split(',') {
                let link = (from.to_owned(), to.to_owned(), code_name(item)?.to_owned());
                doc.exceptions.insert(link);
            }
            Some(())
        },
    );
    if problems.is_empty() {
        Ok(doc)
    } else {
        Err(problems)
    }
}

struct Reference {
    module: Vec<String>,
    segments: Vec<String>,
    at: String,
}

struct UseItem {
    module: Vec<String>,
    path: Vec<String>,
    alias: Option<String>,
    at: String,
}

#[derive(Default)]
struct Crate {
    top_level: BTreeMap<String, String>,
    modules: BTreeSet<Vec<String>>,
    references: Vec<Reference>,
    uses: Vec<UseItem>,
    exported: BTreeMap<String, String>,
}

fn test_only(attribute: &str) -> bool {
    let text: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(cfg) = text
        .strip_prefix("#[cfg(")
        .or_else(|| text.strip_prefix("#![cfg("))
        .and_then(|rest| rest.strip_suffix(")]"))
    else {
        return false;
    };
    cfg == "test"
        || cfg
            .strip_prefix("all(")
            .and_then(|rest| rest.strip_suffix(')'))
            .is_some_and(|args| args.split(',').any(|arg| arg == "test"))
}

fn path_attribute(attribute: &str) -> Option<String> {
    let text = attribute.strip_prefix("#[")?.strip_suffix(']')?.trim();
    let value = text.strip_prefix("path")?.trim().strip_prefix('=')?.trim();
    Some(value.trim_matches('"').to_owned())
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn segments(node: Node, source: &str) -> Vec<String> {
    match node.kind() {
        "identifier" | "type_identifier" | "crate" | "self" | "super" => {
            vec![text(node, source).to_owned()]
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            let mut path = match node.child_by_field_name("path") {
                Some(path) => segments(path, source),
                None => Vec::new(),
            };
            if let Some(name) = node.child_by_field_name("name") {
                path.push(text(name, source).to_owned());
            }
            path
        }
        "generic_type" => node
            .child_by_field_name("type")
            .map(|inner| segments(inner, source))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn expand_use(
    node: Node,
    source: &str,
    prefix: &[String],
    out: &mut Vec<(Vec<String>, Option<String>)>,
) {
    let joined = |tail: Vec<String>| [prefix, &tail].concat();
    match node.kind() {
        "self" if !prefix.is_empty() => out.push((prefix.to_vec(), None)),
        "identifier" | "crate" | "self" | "super" | "scoped_identifier" => {
            out.push((joined(segments(node, source)), None));
        }
        "use_as_clause" => {
            let path: Vec<String> = node
                .child_by_field_name("path")
                .map(|path| segments(path, source))
                .unwrap_or_default()
                .into_iter()
                .filter(|segment| !(segment == "self" && !prefix.is_empty()))
                .collect();
            let alias = node
                .child_by_field_name("alias")
                .map(|alias| text(alias, source).to_owned());
            out.push((joined(path), alias));
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                expand_use(child, source, prefix, out);
            }
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .map(|path| segments(path, source))
                .unwrap_or_default();
            if let Some(list) = node.child_by_field_name("list") {
                expand_use(list, source, &joined(path), out);
            }
        }
        "use_wildcard" => {
            let mut path = node
                .named_child(0)
                .map(|path| segments(path, source))
                .unwrap_or_default();
            path.push("*".to_owned());
            out.push((joined(path), None));
        }
        _ => {}
    }
}

fn macro_paths(node: Node, source: &str, found: &mut Vec<(Vec<String>, usize)>) {
    let mut tokens = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            tokens.push(node);
        } else {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
    }
    let is_name = |token: &Node| {
        matches!(
            token.kind(),
            "identifier" | "crate" | "self" | "super" | "type_identifier"
        )
    };
    let mut at = 0;
    while at < tokens.len() {
        if !is_name(&tokens[at]) {
            at += 1;
            continue;
        }
        let mut path = vec![text(tokens[at], source).to_owned()];
        let line = tokens[at].start_position().row + 1;
        while at + 2 < tokens.len() && tokens[at + 1].kind() == "::" && is_name(&tokens[at + 2]) {
            path.push(text(tokens[at + 2], source).to_owned());
            at += 2;
        }
        at += 1;
        if path.len() > 1 {
            found.push((path, line));
        }
    }
}

struct File<'a> {
    source: &'a str,
    name: String,
    dir: PathBuf,
}

impl Crate {
    fn read(repo: &Path) -> Crate {
        let mut krate = Crate::default();
        krate.read_file(
            repo,
            Path::new("src/main.rs"),
            Vec::new(),
            PathBuf::from("src"),
        );
        krate
    }

    fn read_file(&mut self, repo: &Path, file: &Path, module: Vec<String>, dir: PathBuf) {
        let source = std::fs::read_to_string(repo.join(file))
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("Rust grammar loads");
        let tree = parser.parse(&source, None).expect("tree-sitter parses");
        let root = tree.root_node();
        let mut cursor = root.walk();
        if root
            .children(&mut cursor)
            .any(|child| child.kind() == "inner_attribute_item" && test_only(text(child, &source)))
        {
            return;
        }
        self.modules.insert(module.clone());
        let at = File {
            source: &source,
            name: file.display().to_string(),
            dir,
        };
        self.visit(repo, &at, root, &module);
    }

    fn visit(&mut self, repo: &Path, file: &File, node: Node, module: &[String]) {
        let mut attributes: Vec<String> = Vec::new();
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for child in children {
            if child.kind() == "attribute_item" {
                attributes.push(text(child, file.source).to_owned());
                continue;
            }
            if !child.is_named() || child.kind().ends_with("comment") {
                continue;
            }
            let skip = attributes.iter().any(|a| test_only(a));
            let declared = std::mem::take(&mut attributes);
            if child.kind() == "mod_item" {
                self.module_item(repo, file, child, module, &declared, skip);
                continue;
            }
            if skip {
                continue;
            }
            if child.kind() == "macro_definition" && declared.iter().any(|a| a == "#[macro_export]")
            {
                if let Some(name) = child.child_by_field_name("name") {
                    self.exported
                        .insert(text(name, file.source).to_owned(), top(module));
                }
            }
            self.item(repo, file, child, module);
        }
    }

    fn module_item(
        &mut self,
        repo: &Path,
        file: &File,
        node: Node,
        module: &[String],
        attributes: &[String],
        test: bool,
    ) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, file.source).to_owned();
        let path = [module, std::slice::from_ref(&name)].concat();
        if module.is_empty() {
            let source = format!("src/{name}.rs");
            self.top_level.insert(name.clone(), source);
        }
        if test {
            return;
        }
        match node.child_by_field_name("body") {
            Some(body) => {
                self.modules.insert(path.clone());
                let inner = File {
                    source: file.source,
                    name: file.name.clone(),
                    dir: file.dir.join(&name),
                };
                self.visit(repo, &inner, body, &path);
            }
            None => {
                let explicit = attributes.iter().find_map(|a| path_attribute(a));
                let file_dir = repo.join(&file.name);
                let parent = file_dir.parent().expect("file has a folder");
                let (source, dir) = match explicit {
                    Some(explicit) => {
                        let source = parent.join(explicit);
                        let dir = source.with_extension("");
                        (source, dir)
                    }
                    None => {
                        let flat = file.dir.join(format!("{name}.rs"));
                        if repo.join(&flat).is_file() {
                            (flat, file.dir.join(&name))
                        } else {
                            (file.dir.join(&name).join("mod.rs"), file.dir.join(&name))
                        }
                    }
                };
                let source = source.strip_prefix(repo).unwrap_or(&source).to_path_buf();
                let dir = dir.strip_prefix(repo).unwrap_or(&dir).to_path_buf();
                if module.is_empty() {
                    self.top_level
                        .insert(name.clone(), source.display().to_string());
                }
                self.read_file(repo, &source, path, dir);
            }
        }
    }

    fn item(&mut self, repo: &Path, file: &File, node: Node, module: &[String]) {
        let line = |node: Node| format!("{}:{}", file.name, node.start_position().row + 1);
        match node.kind() {
            "use_declaration" => {
                let mut found = Vec::new();
                if let Some(argument) = node.child_by_field_name("argument") {
                    expand_use(argument, file.source, &[], &mut found);
                }
                for (path, alias) in found {
                    self.uses.push(UseItem {
                        module: module.to_vec(),
                        path,
                        alias,
                        at: line(node),
                    });
                }
                return;
            }
            "scoped_identifier" | "scoped_type_identifier" => {
                self.references.push(Reference {
                    module: module.to_vec(),
                    segments: segments(node, file.source),
                    at: line(node),
                });
            }
            "token_tree" => {
                let mut found = Vec::new();
                macro_paths(node, file.source, &mut found);
                for (segments, row) in found {
                    self.references.push(Reference {
                        module: module.to_vec(),
                        segments,
                        at: format!("{}:{row}", file.name),
                    });
                }
                return;
            }
            _ => {}
        }
        self.visit(repo, file, node, module);
    }

    fn absolute(&self, module: &[String], path: &[String]) -> Option<Vec<String>> {
        let first = path.first()?;
        match first.as_str() {
            "crate" => Some(path[1..].to_vec()),
            "self" => Some([module, &path[1..]].concat()),
            "super" => {
                let ups = path.iter().take_while(|s| *s == "super").count();
                let base = module.get(..module.len().checked_sub(ups)?)?;
                Some([base, &path[ups..]].concat())
            }
            _ => {
                let child = [module, std::slice::from_ref(first)].concat();
                self.modules
                    .contains(&child)
                    .then(|| [module, path].concat())
            }
        }
    }

    fn edges(&self) -> Vec<Edge> {
        let mut aliases: BTreeMap<(Vec<String>, String), Vec<String>> = BTreeMap::new();
        let mut targets = Vec::new();
        for item in &self.uses {
            let Some(path) = self.absolute(&item.module, &item.path) else {
                continue;
            };
            let name = item.alias.clone().or_else(|| path.last().cloned());
            if let Some(name) = name {
                aliases.insert((item.module.clone(), name), path.clone());
            }
            targets.push((item.module.clone(), path, item.at.clone()));
        }
        for reference in &self.references {
            let alias = reference
                .segments
                .first()
                .and_then(|first| aliases.get(&(reference.module.clone(), first.clone())));
            let path = match alias {
                Some(alias) => Some([alias, &reference.segments[1..]].concat()),
                None => self.absolute(&reference.module, &reference.segments),
            };
            if let Some(path) = path {
                targets.push((reference.module.clone(), path, reference.at.clone()));
            }
        }
        let mut edges = Vec::new();
        for (module, path, at) in targets {
            let depth = (0..=path.len())
                .rev()
                .find(|n| self.modules.contains(&path[..*n]))
                .unwrap_or(0);
            let Some(item) = path.get(depth) else {
                continue;
            };
            let from = top(&module);
            let to = match self.exported.get(item) {
                Some(owner) if depth == 0 => owner.clone(),
                _ => top(&path[..depth]),
            };
            if from != to {
                edges.push(Edge {
                    from,
                    to,
                    item: item.clone(),
                    at,
                });
            }
        }
        edges
    }
}

fn top(module: &[String]) -> String {
    module.first().cloned().unwrap_or_else(|| ROOT.to_owned())
}

struct Edge {
    from: String,
    to: String,
    item: String,
    at: String,
}

impl Edge {
    fn link(&self) -> Link {
        (self.from.clone(), self.to.clone(), self.item.clone())
    }
}

fn cycles(edges: &[&Edge]) -> Vec<Vec<String>> {
    let mut next: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in edges {
        next.entry(&edge.from).or_default().insert(&edge.to);
    }
    let reach = |start: &str| {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start];
        while let Some(at) = stack.pop() {
            for to in next.get(at).into_iter().flatten() {
                if seen.insert(*to) {
                    stack.push(to);
                }
            }
        }
        seen
    };
    let reaches: BTreeMap<&str, BTreeSet<&str>> =
        next.keys().map(|module| (*module, reach(module))).collect();
    let mut groups: Vec<Vec<String>> = Vec::new();
    for (module, seen) in &reaches {
        if !seen.contains(module) || groups.iter().flatten().any(|m| m == module) {
            continue;
        }
        let group = seen
            .iter()
            .filter(|other| {
                reaches
                    .get(*other)
                    .is_some_and(|back| back.contains(module))
            })
            .map(|m| (*m).to_owned())
            .collect();
        groups.push(group);
    }
    groups
}

fn problems(krate: &Crate, doc: &Doc) -> Vec<String> {
    let mut problems = Vec::new();
    let mut declared: Vec<(&str, &str)> = vec![(ROOT, "src/main.rs")];
    declared.extend(
        krate
            .top_level
            .iter()
            .map(|(name, source)| (name.as_str(), source.as_str())),
    );
    for (module, source) in &declared {
        if !doc.layer_of.contains_key(*module) {
            problems.push(format!(
                "no layer: the module `{module}` ({source}) has no row in the table of {DOC}"
            ));
        }
    }
    for module in doc.layer_of.keys() {
        if !declared.iter().any(|(name, _)| name == module) {
            problems.push(format!(
                "no module: the table of {DOC} has a row `{module}`, but src/main.rs declares no such module"
            ));
        }
    }
    for (value, owner) in &doc.owner_of {
        if !doc.layer_of.contains_key(owner) {
            problems.push(format!(
                "no module: the value type `{value}` names the module `{owner}`, which has no row in the table of {DOC}"
            ));
        }
    }
    let edges = krate.edges();
    let mut broken = BTreeMap::new();
    for edge in &edges {
        let (Some(&from), Some(&to)) = (doc.layer_of.get(&edge.from), doc.layer_of.get(&edge.to))
        else {
            continue;
        };
        let value = doc.owner_of.get(&edge.item) == Some(&edge.to);
        let rule = if to > from {
            format!(
                "upward: `{}` ({}) uses `{}::{}` ({}); dependencies go only down",
                edge.from, LAYERS[from], edge.to, edge.item, LAYERS[to]
            )
        } else if to == from && LAYERS[from] == FEATURE && !value {
            format!(
                "feature to feature: `{}` uses `{}::{}`; feature modules use only the value types in the table of {DOC}",
                edge.from, edge.to, edge.item
            )
        } else {
            continue;
        };
        broken
            .entry(edge.link())
            .or_insert_with(|| format!("{rule} at {}", edge.at));
    }
    for (link, problem) in &broken {
        if !doc.exceptions.contains(link) {
            problems.push(problem.clone());
        }
    }
    for (from, to, item) in &doc.exceptions {
        if !broken.contains_key(&(from.clone(), to.clone(), item.clone())) {
            problems.push(format!(
                "old exception: `{from}` no longer breaks the rule with `{to}::{item}`; remove `{item}` from the exception table of {DOC}"
            ));
        }
    }
    let kept: Vec<&Edge> = edges
        .iter()
        .filter(|edge| !doc.exceptions.contains(&edge.link()))
        .collect();
    for group in cycles(&kept) {
        let inside: BTreeMap<(&str, &str), &Edge> = kept
            .iter()
            .filter(|e| group.contains(&e.from) && group.contains(&e.to))
            .map(|e| ((e.from.as_str(), e.to.as_str()), *e))
            .collect();
        let links: Vec<String> = inside
            .values()
            .map(|e| format!("{} -> {}::{} at {}", e.from, e.to, e.item, e.at))
            .collect();
        problems.push(format!(
            "cycle: the modules {} use each other: {}",
            group.join(", "),
            links.join("; ")
        ));
    }
    problems
}

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn modules_follow_the_layer_table() {
    let text = std::fs::read_to_string(repo().join(DOC)).expect("the layer document exists");
    let doc = read_doc(&text).unwrap_or_else(|problems| panic!("{}", problems.join("\n")));
    let problems = problems(&Crate::read(repo()), &doc);
    assert!(
        problems.is_empty(),
        "The Rust modules break the layer rule of {DOC}. Move the code to the right layer or correct the table:\n{}",
        problems.join("\n")
    );
}

#[test]
fn use_groups_expand_to_each_path() {
    let source = "use crate::{a, b::{c, self as d, e::*}};";
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("Rust grammar loads");
    let tree = parser.parse(source, None).expect("tree-sitter parses");
    let argument = tree
        .root_node()
        .named_child(0)
        .and_then(|node| node.child_by_field_name("argument"))
        .expect("use argument");
    let mut found = Vec::new();
    expand_use(argument, source, &[], &mut found);
    let shown: Vec<String> = found
        .iter()
        .map(|(path, alias)| {
            format!(
                "{}{}",
                path.join("::"),
                alias
                    .as_ref()
                    .map(|a| format!(" as {a}"))
                    .unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(
        shown,
        ["crate::a", "crate::b::c", "crate::b as d", "crate::b::e::*"]
    );
}

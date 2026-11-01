#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tree_sitter::{Language, Node, Parser};

const GEN: &str = "web/src/gen";

const SOURCES: &str = "src";

const RELEASED: &str = "tests/contract/released.txt";

const METHODS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

const NULLABLE: &str = " | null";

const BUMP: &str = "memcap -m 4608M cargo test --test contract -- --ignored bump";

type Surface = BTreeMap<String, String>;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn parse(language: Language, source: &str) -> tree_sitter::Tree {
    let mut parser = Parser::new();
    parser.set_language(&language).expect("grammar loads");
    parser.parse(source, None).expect("tree-sitter parses")
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn named<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
        .collect()
}

fn files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn union_members<'a>(node: Node<'a>, members: &mut Vec<Node<'a>>) {
    for child in named(node) {
        if child.kind() == "union_type" {
            union_members(child, members);
        } else {
            members.push(child);
        }
    }
}

struct Property<'a> {
    name: String,
    optional: bool,
    kind: Node<'a>,
}

fn properties<'a>(object: Node<'a>, source: &str) -> Vec<Property<'a>> {
    named(object)
        .into_iter()
        .filter(|child| child.kind() == "property_signature")
        .filter_map(|property| {
            let name = property.child_by_field_name("name")?;
            let kind = named(property.child_by_field_name("type")?)
                .into_iter()
                .next()?;
            let mut cursor = property.walk();
            let optional = property.children(&mut cursor).any(|c| c.kind() == "?");
            Some(Property {
                name: text(name, source).trim_matches('"').to_owned(),
                optional,
                kind,
            })
        })
        .collect()
}

fn variant_tag(member: Node, source: &str) -> Option<String> {
    match member.kind() {
        "literal_type" => Some(text(member, source).trim_matches('"').to_owned()),
        "object_type" => {
            let properties = properties(member, source);
            let tags: Vec<String> = properties
                .iter()
                .filter(|p| p.kind.kind() == "literal_type")
                .map(|p| format!("{}={}", p.name, text(p.kind, source).trim_matches('"')))
                .collect();
            match &properties[..] {
                [only] if tags.is_empty() => Some(only.name.clone()),
                _ => (!tags.is_empty()).then(|| tags.join(",")),
            }
        }
        _ => None,
    }
}

fn add_fields(path: &str, object: Node, source: &str, surface: &mut Surface) {
    for property in properties(object, source) {
        if property.kind.kind() == "literal_type" {
            continue;
        }
        let marker = if property.optional { "?: " } else { ": " };
        let path = format!("{path}.{}", property.name);
        add_type(&path, marker, property.kind, source, surface);
    }
    for member in named(object) {
        if member.kind() != "property_signature" {
            let key = format!("{path}.[{}]", squeeze(text(member, source)));
            surface.insert(key, "member".to_owned());
        }
    }
}

fn add_type(path: &str, marker: &str, node: Node, source: &str, surface: &mut Surface) {
    if node.kind() == "object_type" {
        surface.insert(path.to_owned(), format!("{marker}{{}}"));
        add_fields(path, node, source, surface);
        return;
    }
    if node.kind() == "union_type" {
        let mut members = Vec::new();
        union_members(node, &mut members);
        let tags: Option<Vec<String>> = members.iter().map(|m| variant_tag(*m, source)).collect();
        if let Some(tags) = tags {
            surface.insert(path.to_owned(), format!("{marker}variants"));
            for (member, tag) in members.into_iter().zip(tags) {
                let path = format!("{path}|{tag}");
                surface.insert(path.clone(), "variant".to_owned());
                if member.kind() == "object_type" {
                    add_fields(&path, member, source, surface);
                }
            }
            return;
        }
    }
    surface.insert(
        path.to_owned(),
        format!("{marker}{}", squeeze(text(node, source))),
    );
}

fn add_types(surface: &mut Surface) {
    let language: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    for path in files(&repo().join(GEN), "ts") {
        let source = read(&path);
        let tree = parse(language.clone(), &source);
        for statement in named(tree.root_node()) {
            let Some(alias) = named(statement)
                .into_iter()
                .find(|c| c.kind() == "type_alias_declaration")
            else {
                continue;
            };
            let (Some(name), Some(value)) = (
                alias.child_by_field_name("name"),
                alias.child_by_field_name("value"),
            ) else {
                continue;
            };
            let key = format!("type {}", text(name, &source));
            add_type(&key, "= ", value, &source, surface);
        }
    }
}

fn string_value(node: Node, source: &str) -> Option<String> {
    (node.kind() == "string_literal").then(|| text(node, source).trim_matches('"').to_owned())
}

fn walk<'a>(node: Node<'a>, each: &mut impl FnMut(Node<'a>)) {
    each(node);
    for child in named(node) {
        walk(child, each);
    }
}

fn route_methods(router: Node, source: &str) -> BTreeSet<String> {
    let mut methods = BTreeSet::new();
    walk(router, &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let name = match function.kind() {
            "identifier" => Some(function),
            "field_expression" => function.child_by_field_name("field"),
            _ => None,
        };
        if let Some(name) = name.map(|n| text(n, source))
            && METHODS.contains(&name)
        {
            methods.insert(name.to_uppercase());
        }
    });
    methods
}

fn add_routes(surface: &mut Surface) {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let sources: Vec<String> = files(&repo().join(SOURCES), "rs")
        .iter()
        .map(|p| read(p))
        .collect();
    let trees: Vec<_> = sources.iter().map(|s| parse(language.clone(), s)).collect();
    let mut constants = BTreeMap::new();
    for (source, tree) in sources.iter().zip(&trees) {
        walk(tree.root_node(), &mut |node| {
            if node.kind() != "const_item" {
                return;
            }
            if let (Some(name), Some(value)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("value"),
            ) && let Some(value) = string_value(value, source)
            {
                constants.insert(text(name, source).to_owned(), value);
            }
        });
    }
    for (source, tree) in sources.iter().zip(&trees) {
        walk(tree.root_node(), &mut |node| {
            if node.kind() != "call_expression" {
                return;
            }
            let is_route = node
                .child_by_field_name("function")
                .filter(|f| f.kind() == "field_expression")
                .and_then(|f| f.child_by_field_name("field"))
                .is_some_and(|f| text(f, source) == "route");
            let arguments = node
                .child_by_field_name("arguments")
                .map(named)
                .unwrap_or_default();
            let [path, router] = arguments[..] else {
                return;
            };
            if !is_route {
                return;
            }
            let path = string_value(path, source)
                .or_else(|| constants.get(text(path, source)).cloned())
                .unwrap_or_else(|| panic!("route path `{}` is not a string", text(path, source)));
            let methods = route_methods(router, source);
            assert!(
                !methods.is_empty(),
                "route `{path}` has no {} call",
                METHODS.join("/")
            );
            for method in methods {
                surface.insert(format!("route {method} {path}"), "route".to_owned());
            }
        });
    }
}

#[derive(Default)]
struct Input {
    all_default: bool,
    defaults: BTreeSet<String>,
}

fn attributes(node: Node, source: &str) -> String {
    let mut found = String::new();
    let mut before = node.prev_named_sibling();
    while let Some(attribute) = before.filter(|b| b.kind() == "attribute_item") {
        found.push_str(text(attribute, source));
        before = attribute.prev_named_sibling();
    }
    found
}

fn words(text: &str) -> BTreeSet<&str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .collect()
}

fn has_serde_default(attributes: &str) -> bool {
    attributes
        .split("#[")
        .any(|a| a.trim_start().starts_with("serde(") && words(a).contains("default"))
}

fn field_key(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

fn input_types() -> BTreeMap<String, Input> {
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let mut inputs = BTreeMap::new();
    for path in files(&repo().join(SOURCES), "rs") {
        let source = read(&path);
        let tree = parse(language.clone(), &source);
        walk(tree.root_node(), &mut |node| {
            if !matches!(node.kind(), "struct_item" | "enum_item") {
                return;
            }
            let container = attributes(node, &source);
            let derives = words(&container);
            if !(derives.contains("derive")
                && derives.contains("TS")
                && derives.contains("Deserialize"))
            {
                return;
            }
            let Some(name) = node.child_by_field_name("name") else {
                return;
            };
            let mut input = Input {
                all_default: has_serde_default(&container),
                defaults: BTreeSet::new(),
            };
            if let Some(body) = node.child_by_field_name("body") {
                for field in named(body) {
                    if field.kind() == "field_declaration"
                        && has_serde_default(&attributes(field, &source))
                        && let Some(field_name) = field.child_by_field_name("name")
                    {
                        input.defaults.insert(field_key(text(field_name, &source)));
                    }
                }
            }
            inputs.insert(text(name, &source).to_owned(), input);
        });
    }
    inputs
}

fn current() -> Surface {
    let mut surface = Surface::new();
    add_routes(&mut surface);
    add_types(&mut surface);
    surface
}

fn released() -> Surface {
    read(&repo().join(RELEASED))
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn type_name(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("type ")?;
    Some(rest.split(['.', '|']).next().unwrap_or(rest))
}

fn breaks(released: &Surface, current: &Surface, inputs: &BTreeMap<String, Input>) -> Vec<String> {
    let mut problems = Vec::new();
    for (key, old) in released {
        match current.get(key) {
            None => problems.push(format!("removed: {key}")),
            Some(new) if new != old => problems.push(format!("changed: {key}: `{old}` → `{new}`")),
            Some(_) => {}
        }
    }
    for (key, value) in current {
        if released.contains_key(key) || !value.starts_with(": ") || value.ends_with(NULLABLE) {
            continue;
        }
        let Some((parent, field)) = key.rsplit_once('.') else {
            continue;
        };
        let Some(input) = type_name(key).and_then(|name| inputs.get(name)) else {
            continue;
        };
        let top_field = type_name(key).is_some_and(|name| parent == format!("type {name}"));
        let omissible =
            top_field && (input.all_default || input.defaults.contains(&field_key(field)));
        if released.contains_key(parent) && !omissible {
            problems.push(format!("new required input field: {key}"));
        }
    }
    problems
}

fn serialize(surface: &Surface) -> String {
    surface
        .iter()
        .map(|(key, value)| format!("{key}\t{value}\n"))
        .collect()
}

#[test]
fn api_only_grows() {
    let released = released();
    let current = current();
    let problems = breaks(&released, &current, &input_types());
    let added = current
        .keys()
        .filter(|k| !released.contains_key(*k))
        .count();
    println!("{added} additions are not in {RELEASED}");
    assert!(
        problems.is_empty(),
        "the daemon API breaks a client of {RELEASED}:\n{}\nto release a break on purpose, run `{BUMP}` and commit the file",
        problems.join("\n")
    );
}

#[test]
#[ignore = "rewrites tests/contract/released.txt"]
fn bump() {
    let path = repo().join(RELEASED);
    std::fs::write(&path, serialize(&current()))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

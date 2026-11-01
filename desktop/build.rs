use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::PathBuf;
use std::{env, fs};

const WEB: &str = "../web/src";
const PRETENDARD: &str = "../web/node_modules/pretendard/dist/web/static/woff2";

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let web = manifest.join(WEB);
    let read = |name: &str| {
        let path = web.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    let write = |name: &str, text: String| {
        fs::write(out.join(name), text).unwrap_or_else(|e| panic!("{name}: {e}"));
    };

    let pretendard = manifest.join(PRETENDARD);
    assert!(
        pretendard.is_dir(),
        "{} is missing: run `bash scripts/worktree/setup.sh` or `npm ci` in web/ first",
        pretendard.display()
    );
    println!("cargo:rerun-if-changed={}", pretendard.display());
    println!("cargo:rustc-env=PRETENDARD_DIR={}", pretendard.display());

    write("tokens.rs", tokens(&read("styles.css")));
    write("terminals.rs", terminals(&read("terminalThemes.ts")));
    write(
        "strings.rs",
        strings(&read("locales/ko.ts"), &read("locales/en.ts")),
    );
}

#[derive(Clone, Copy)]
struct Oklch {
    l: f64,
    c: f64,
    h: f64,
    a: f64,
}

enum Value {
    Color(Oklch),
    Var(String),
    LightDark(Box<Value>, Box<Value>),
    Alpha(String, f64),
}

enum Expr {
    Lit(Oklch),
    Seed(String),
    Alpha(Box<Expr>, f64),
}

fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn number(text: &str) -> f64 {
    match text.strip_suffix('%') {
        Some(percent) => percent.parse::<f64>().expect(text) / 100.0,
        None => text.parse().expect(text),
    }
}

fn parse_value(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix("light-dark(")
        .and_then(|t| t.strip_suffix(')'))
    {
        let parts = split_top(inner, ',');
        assert_eq!(parts.len(), 2, "light-dark needs two values: {text}");
        return Some(Value::LightDark(
            Box::new(parse_value(parts[0])?),
            Box::new(parse_value(parts[1])?),
        ));
    }
    if let Some(name) = text.strip_prefix("var(").and_then(|t| t.strip_suffix(')')) {
        return Some(Value::Var(name.trim().to_owned()));
    }
    let inner = text.strip_prefix("oklch(")?.strip_suffix(')')?;
    let (main, alpha) = match inner.split_once('/') {
        Some((main, alpha)) => (main.trim(), number(alpha.trim())),
        None => (inner.trim(), 1.0),
    };
    if let Some(rest) = main.strip_prefix("from ") {
        let (source, channels) = rest.trim().split_once(' ').expect(text);
        assert_eq!(channels.trim(), "l c h", "only `l c h` is read: {text}");
        let name = source.strip_prefix("var(")?.strip_suffix(')')?;
        return Some(Value::Alpha(name.trim().to_owned(), alpha));
    }
    let channels: Vec<&str> = main.split_whitespace().collect();
    assert_eq!(channels.len(), 3, "oklch needs three channels: {text}");
    Some(Value::Color(Oklch {
        l: number(channels[0]),
        c: number(channels[1]),
        h: number(channels[2]),
        a: alpha,
    }))
}

fn declarations(css: &str) -> BTreeMap<String, Value> {
    let header = ":root, [data-scheme] {";
    let start = css
        .find(header)
        .expect("styles.css has no `:root, [data-scheme]` block")
        + header.len();
    let body = &css[start..start + css[start..].find('}').expect("unclosed :root block")];
    split_top(body, ';')
        .into_iter()
        .filter_map(|decl| {
            let (name, value) = decl.split_once(':')?;
            Some((name.trim().to_owned(), parse_value(value)?))
        })
        .collect()
}

fn pixels(css: &str, name: &str) -> f64 {
    let start = css
        .find(&format!("{name}:"))
        .unwrap_or_else(|| panic!("styles.css has no {name}"));
    let value = css[start + name.len() + 1..]
        .split(['}', ';'])
        .next()
        .expect(name)
        .trim();
    value
        .strip_suffix("px")
        .unwrap_or_else(|| panic!("{name} is not px: {value}"))
        .trim()
        .parse()
        .expect(name)
}

fn seed_base(name: &str) -> Option<&str> {
    name.strip_suffix("-light")
        .or_else(|| name.strip_suffix("-dark"))
}

fn seed_names(decls: &BTreeMap<String, Value>) -> BTreeSet<String> {
    decls
        .keys()
        .filter_map(|name| {
            let base = name.strip_suffix("-light")?;
            let dark = format!("{base}-dark");
            let both_colors = matches!(decls.get(name), Some(Value::Color(_)))
                && matches!(decls.get(&dark), Some(Value::Color(_)));
            both_colors.then(|| base.to_owned())
        })
        .collect()
}

fn reference(
    decls: &BTreeMap<String, Value>,
    seeds: &BTreeSet<String>,
    name: &str,
    dark: bool,
) -> Expr {
    match seed_base(name) {
        Some(base) if seeds.contains(base) => Expr::Seed(base.to_owned()),
        _ => value_expr(
            decls,
            seeds,
            decls
                .get(name)
                .unwrap_or_else(|| panic!("unknown token {name}")),
            dark,
        ),
    }
}

fn value_expr(
    decls: &BTreeMap<String, Value>,
    seeds: &BTreeSet<String>,
    value: &Value,
    dark: bool,
) -> Expr {
    match value {
        Value::Color(color) => Expr::Lit(*color),
        Value::Var(name) => reference(decls, seeds, name, dark),
        Value::LightDark(light, dk) => {
            value_expr(decls, seeds, if dark { dk } else { light }, dark)
        }
        Value::Alpha(name, alpha) => {
            Expr::Alpha(Box::new(reference(decls, seeds, name, dark)), *alpha)
        }
    }
}

fn ident(name: &str) -> String {
    name.trim_start_matches("--").replace('-', "_")
}

fn oklch_code(expr: &Expr) -> String {
    match expr {
        Expr::Lit(c) => format!("Oklch::new({:?}, {:?}, {:?}, {:?})", c.l, c.c, c.h, c.a),
        Expr::Seed(name) => format!("seeds.{}", ident(name)),
        Expr::Alpha(inner, alpha) => format!("{}.with_alpha({alpha:?})", oklch_code(inner)),
    }
}

fn tokens(css: &str) -> String {
    let decls = declarations(css);
    let seeds = seed_names(&decls);
    let is_leaf = |name: &str| seed_base(name).is_some_and(|base| seeds.contains(base));
    let fields: Vec<&String> = decls.keys().filter(|name| !is_leaf(name)).collect();
    let mut code = String::new();

    code.push_str("#[allow(dead_code)]\n#[derive(Clone, Copy)]\npub struct Seeds {\n");
    for seed in &seeds {
        writeln!(code, "    pub {}: Oklch,", ident(seed)).unwrap();
    }
    code.push_str("}\n\nimpl Seeds {\n");
    for (constant, suffix) in [("LIGHT", "light"), ("DARK", "dark")] {
        writeln!(code, "    pub const {constant}: Seeds = Seeds {{").unwrap();
        for seed in &seeds {
            let Some(Value::Color(c)) = decls.get(&format!("{seed}-{suffix}")) else {
                unreachable!()
            };
            writeln!(
                code,
                "        {}: Oklch::new({:?}, {:?}, {:?}, {:?}),",
                ident(seed),
                c.l,
                c.c,
                c.h,
                c.a
            )
            .unwrap();
        }
        code.push_str("    };\n");
    }
    code.push_str("}\n\n#[allow(dead_code)]\n#[derive(Clone, Copy)]\npub struct Colors {\n");
    for name in &fields {
        writeln!(code, "    pub {}: Rgba,", ident(name)).unwrap();
    }
    code.push_str("}\n\nimpl Colors {\n");
    for (function, dark) in [("light", false), ("dark", true)] {
        writeln!(
            code,
            "    fn {function}(seeds: &Seeds) -> Colors {{\n        Colors {{"
        )
        .unwrap();
        for name in &fields {
            let expr = value_expr(&decls, &seeds, &decls[*name], dark);
            writeln!(
                code,
                "            {}: to_rgba({}),",
                ident(name),
                oklch_code(&expr)
            )
            .unwrap();
        }
        code.push_str("        }\n    }\n");
    }
    code.push_str("}\n\n");
    writeln!(
        code,
        "#[allow(dead_code)]\npub const RADIUS: f32 = {:?};",
        pixels(css, "--radius")
    )
    .unwrap();
    writeln!(
        code,
        "#[allow(dead_code)]\npub const STATUS_H: f32 = {:?};",
        pixels(css, "--status-h")
    )
    .unwrap();
    code
}

fn rgba_code(hex: &str) -> String {
    let digits = hex
        .strip_prefix('#')
        .filter(|d| d.len() == 6)
        .unwrap_or_else(|| panic!("not #rrggbb: {hex}"));
    let channel =
        |i: usize| f32::from(u8::from_str_radix(&digits[i..i + 2], 16).expect(hex)) / 255.0;
    format!(
        "Rgba {{ r: {:?}, g: {:?}, b: {:?}, a: 1.0 }}",
        channel(0),
        channel(2),
        channel(4)
    )
}

const ANSI: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];

fn quoted_after<'a>(text: &'a str, marker: &str) -> &'a str {
    let start = text
        .find(marker)
        .unwrap_or_else(|| panic!("terminalThemes.ts has no `{marker}`"))
        + marker.len();
    let rest = &text[start..];
    &rest[..rest.find('\'').expect(marker)]
}

fn terminals(source: &str) -> String {
    let mut themes = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.strip_prefix("  \"") else {
            continue;
        };
        let Some((name, body)) = rest.split_once("\": ") else {
            continue;
        };
        let json: serde_json::Value = serde_json::from_str(body.trim_end().trim_end_matches(','))
            .unwrap_or_else(|e| panic!("terminal theme {name}: {e}"));
        let colors: BTreeMap<String, String> = json
            .as_object()
            .unwrap_or_else(|| panic!("terminal theme {name} is not an object"))
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("{name}.{key} is not a string"))
                        .to_owned(),
                )
            })
            .collect();
        themes.push((name.to_owned(), colors));
    }
    assert!(!themes.is_empty(), "terminalThemes.ts has no themes");

    let default_line = source
        .lines()
        .find(|l| l.contains("DEFAULT_TERMINAL_THEME") && l.contains("dark:"))
        .expect("terminalThemes.ts has no DEFAULT_TERMINAL_THEME");
    let default_dark = quoted_after(default_line, "dark: '");
    let default_light = quoted_after(default_line, "light: '");
    for default in [default_dark, default_light] {
        assert!(
            themes.iter().any(|(name, _)| name == default),
            "default theme {default} is not in the list"
        );
    }

    let mut code = String::new();
    writeln!(code, "const DEFAULT_DARK: &str = {default_dark:?};").unwrap();
    writeln!(code, "const DEFAULT_LIGHT: &str = {default_light:?};\n").unwrap();
    code.push_str("static TERMINAL_THEMES: &[(&str, Terminal)] = &[\n");
    for (name, colors) in &themes {
        let color = |key: &str| {
            colors
                .get(key)
                .unwrap_or_else(|| panic!("terminal theme {name} has no {key}"))
                .clone()
        };
        writeln!(code, "    ({name:?}, Terminal {{").unwrap();
        for (field, key) in [
            ("background", "background"),
            ("foreground", "foreground"),
            ("cursor", "cursor"),
            ("cursor_accent", "cursorAccent"),
            ("selection_background", "selectionBackground"),
            ("selection_foreground", "selectionForeground"),
        ] {
            writeln!(code, "        {field}: {},", rgba_code(&color(key))).unwrap();
        }
        code.push_str("        ansi: [\n");
        for key in ANSI {
            writeln!(code, "            {},", rgba_code(&color(key))).unwrap();
        }
        code.push_str("        ],\n    }),\n");
    }
    code.push_str("];\n");
    code
}

struct Scan<'a> {
    chars: Vec<char>,
    at: usize,
    file: &'a str,
}

impl Scan<'_> {
    fn skip(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> char {
        self.skip();
        *self
            .chars
            .get(self.at)
            .unwrap_or_else(|| panic!("{}: text ends early", self.file))
    }

    fn eat(&mut self, expected: char) {
        let got = self.peek();
        assert_eq!(
            got,
            expected,
            "{}: expected `{expected}` near {}",
            self.file,
            self.context()
        );
        self.at += 1;
    }

    fn context(&self) -> String {
        self.chars[self.at.saturating_sub(20)..(self.at + 20).min(self.chars.len())]
            .iter()
            .collect()
    }

    fn word(&mut self) -> String {
        let quote = self.peek();
        if quote == '\'' || quote == '"' {
            return self.string();
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
        {
            self.at += 1;
        }
        assert!(
            self.at > start,
            "{}: no key near {}",
            self.file,
            self.context()
        );
        self.chars[start..self.at].iter().collect()
    }

    fn string(&mut self) -> String {
        let quote = self.chars[self.at];
        self.at += 1;
        let mut text = String::new();
        loop {
            let ch = *self
                .chars
                .get(self.at)
                .unwrap_or_else(|| panic!("{}: unclosed string", self.file));
            self.at += 1;
            if ch == quote {
                return text;
            }
            if ch != '\\' {
                text.push(ch);
                continue;
            }
            let escaped = self.chars[self.at];
            self.at += 1;
            text.push(match escaped {
                'n' => '\n',
                't' => '\t',
                '\\' | '\'' | '"' | '`' => escaped,
                other => panic!("{}: unknown escape \\{other}", self.file),
            });
        }
    }

    fn object(&mut self, prefix: &str, out: &mut BTreeMap<String, String>) {
        self.eat('{');
        while self.peek() != '}' {
            let key = self.word();
            self.eat(':');
            let path = if prefix.is_empty() {
                key
            } else {
                format!("{prefix}.{key}")
            };
            if self.peek() == '{' {
                self.object(&path, out);
            } else {
                let value = self.string();
                assert!(
                    out.insert(path.clone(), value).is_none(),
                    "{}: duplicate key {path}",
                    self.file
                );
            }
            if self.peek() == ',' {
                self.at += 1;
            }
        }
        self.at += 1;
    }
}

fn locale(file: &str, source: &str) -> BTreeMap<String, String> {
    let start = source
        .find("= {")
        .unwrap_or_else(|| panic!("{file}: no object"))
        + 2;
    let mut scan = Scan {
        chars: source[start..].chars().collect(),
        at: 0,
        file,
    };
    let mut out = BTreeMap::new();
    scan.object("", &mut out);
    out
}

fn variant(key: &str) -> String {
    key.split(|c: char| !c.is_ascii_alphanumeric())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

fn strings(ko_source: &str, en_source: &str) -> String {
    let ko = locale("ko.ts", ko_source);
    let en = locale("en.ts", en_source);
    let only_ko: Vec<_> = ko.keys().filter(|k| !en.contains_key(*k)).collect();
    let only_en: Vec<_> = en.keys().filter(|k| !ko.contains_key(*k)).collect();
    assert!(
        only_ko.is_empty() && only_en.is_empty(),
        "locale keys differ: ko only {only_ko:?}, en only {only_en:?}"
    );

    let mut names = BTreeMap::new();
    for key in en.keys() {
        if let Some(other) = names.insert(variant(key), key) {
            panic!("keys {other} and {key} make the same name {}", variant(key));
        }
    }

    let mut code = String::from(
        "#[allow(dead_code)]\n#[derive(Clone, Copy, PartialEq, Eq, Debug)]\npub enum Str {\n",
    );
    for key in en.keys() {
        writeln!(code, "    {},", variant(key)).unwrap();
    }
    code.push_str("}\n\n");
    writeln!(code, "static TEXTS: [[&str; 2]; {}] = [", en.len()).unwrap();
    for (key, english) in &en {
        writeln!(code, "    [{:?}, {english:?}],", ko[key]).unwrap();
    }
    code.push_str("];\n\n#[allow(dead_code)]\nimpl Str {\n");
    code.push_str("    pub fn text(self, lang: Lang) -> &'static str {\n        TEXTS[self as usize][lang as usize]\n    }\n\n");
    code.push_str("    pub fn with(self, lang: Lang, args: &[(&str, &str)]) -> String {\n        fill(self.text(lang), args)\n    }\n}\n");
    code
}

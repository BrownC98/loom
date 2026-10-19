//! 고정한 Codex app-server 스키마(`src/profile/codex/schema/`)에서 Rust 타입을 만든다.
//! 근거: docs/design.md 「에이전트 실행」. 파일마다 같은 `definitions`가 겹쳐 있으므로 한 묶음으로 합친다.

use std::collections::BTreeMap;
use std::path::Path;

use schemars::schema::{RootSchema, Schema};
use serde_json::Value;
use typify::{TypeSpace, TypeSpaceSettings};

const SCHEMA_DIR: &str = "src/profile/codex/schema";

fn main() {
    println!("cargo::rerun-if-changed={SCHEMA_DIR}");
    let mut definitions: BTreeMap<String, Value> = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(SCHEMA_DIR)
        .expect("스키마 폴더가 있다")
        .map(|e| e.expect("스키마 폴더를 읽는다").path())
        .collect();
    files.sort();
    for path in files {
        let mut root: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("스키마 파일을 읽는다"))
                .expect("스키마 파일은 JSON이다");
        let root = root.as_object_mut().expect("스키마 최상위는 객체다");
        let defs = root.remove("definitions").unwrap_or_default();
        root.remove("$schema");
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("파일 이름");
        let defs = defs.as_object().cloned().unwrap_or_default();
        for (key, def) in defs
            .into_iter()
            .chain([(name.to_owned(), Value::Object(root.clone()))])
        {
            match definitions.get(&key) {
                Some(seen) if *seen != def => panic!("{key}: 스키마 파일마다 정의가 다르다"),
                _ => {
                    definitions.insert(key, def);
                }
            }
        }
    }
    let definitions = definitions
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                serde_json::from_value::<Schema>(v).expect("JSON Schema다"),
            )
        })
        .collect();
    let mut space = TypeSpace::new(&TypeSpaceSettings::default());
    space
        .add_root_schema(RootSchema {
            definitions,
            ..Default::default()
        })
        .expect("typify가 스키마를 받는다");
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo가 OUT_DIR을 준다"))
        .join("codex_protocol.rs");
    std::fs::write(out, space.to_stream().to_string()).expect("생성한 타입을 쓴다");
}

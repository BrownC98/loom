use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git;
use crate::place::Files;

const SEPARATOR: char = '\0';
const TAG_PREFIX: &str = "tag: ";

#[derive(Serialize, TS, PartialEq, Debug)]
#[ts(export)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// 작성 시각, 유닉스 초.
    #[ts(type = "number")]
    pub time: i64,
    pub refs: Vec<GitRef>,
}

#[derive(Serialize, TS, PartialEq, Debug)]
#[serde(tag = "kind", content = "name", rename_all = "camelCase")]
#[ts(export)]
pub enum GitRef {
    /// 로컬·원격 브랜치. 원격은 `origin/main`처럼 원격 이름이 붙는다.
    Branch(String),
    Tag(String),
}

const STASH_REF: &str = "refs/stash";

/// 화면이 읽는 구간: 앞에서 `skip`개를 건너뛴 `limit`개. 화면이 아래로 내릴 때 다음 구간을 읽는다.
#[derive(Deserialize, TS, Clone, Copy)]
#[ts(export)]
pub struct CommitRange {
    pub skip: u32,
    pub limit: u32,
}

pub async fn read(
    files: Files<'_>,
    repo: &Path,
    range: CommitRange,
) -> Result<Vec<Commit>, String> {
    let log = git::run(
        files,
        repo,
        &[
            "log",
            &format!("--exclude={STASH_REF}"),
            "--all",
            "--topo-order",
            &format!("--skip={}", range.skip),
            &format!("--max-count={}", range.limit),
            "--format=%H%x00%P%x00%s%x00%an%x00%at%x00%D",
        ],
    )
    .await?;
    Ok(log.lines().filter_map(commit).collect())
}

pub async fn version(files: Files<'_>, repo: &Path) -> Result<String, String> {
    let refs = git::run(
        files,
        repo,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
    )
    .await?;
    let mut hasher = DefaultHasher::new();
    for line in refs.lines().filter(|l| !l.ends_with(STASH_REF)) {
        line.hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn commit(line: &str) -> Option<Commit> {
    let [hash, parents, subject, author, time, refs] =
        line.splitn(6, SEPARATOR).collect::<Vec<_>>()[..]
    else {
        return None;
    };
    Some(Commit {
        hash: hash.to_owned(),
        parents: parents.split_whitespace().map(str::to_owned).collect(),
        subject: subject.to_owned(),
        author: author.to_owned(),
        time: time.parse().ok()?,
        refs: refs
            .split(", ")
            .filter_map(|r| {
                let r = r.strip_prefix("HEAD -> ").unwrap_or(r);
                match r {
                    "" | "HEAD" => None,
                    _ => Some(r.strip_prefix(TAG_PREFIX).map_or_else(
                        || GitRef::Branch(r.to_owned()),
                        |t| GitRef::Tag(t.to_owned()),
                    )),
                }
            })
            .collect(),
    })
}

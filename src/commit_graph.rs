use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git;

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

pub async fn read(repo: &Path, range: CommitRange) -> Result<Vec<Commit>, String> {
    let log = git::run(
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

pub async fn version(repo: &Path) -> Result<String, String> {
    let refs = git::run(repo, &["for-each-ref", "--format=%(objectname) %(refname)"]).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: CommitRange = CommitRange {
        skip: 0,
        limit: i32::MAX as u32,
    };

    use crate::git::test_run as run;

    #[test]
    fn commit_line_splits_parents_and_refs() {
        let line = "bbb\0aaa ccc\0Merge feat\0t\x001792414055\0HEAD -> main, tag: v1, origin/main";
        assert_eq!(
            commit(line),
            Some(Commit {
                hash: "bbb".into(),
                parents: vec!["aaa".into(), "ccc".into()],
                subject: "Merge feat".into(),
                author: "t".into(),
                time: 1792414055,
                refs: vec![
                    GitRef::Branch("main".into()),
                    GitRef::Tag("v1".into()),
                    GitRef::Branch("origin/main".into()),
                ],
            })
        );
        assert_eq!(
            commit("aaa\0\0root\0t\x001\0").unwrap().parents,
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn reads_all_branches_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
        run(&repo, &["tag", "v1"]);
        run(
            &repo,
            &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
        );
        run(&wt, &["commit", "-q", "--allow-empty", "-m", "b"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "c"]);
        run(
            &repo,
            &["merge", "-q", "--no-ff", "--no-edit", "-m", "m", "feat"],
        );
        run(&wt, &["commit", "-q", "--allow-empty", "-m", "d"]);
        std::fs::write(repo.join("f"), "x").unwrap();
        run(&repo, &["add", "f"]);
        run(&repo, &["stash", "-q"]);

        let commits = read(&repo, ALL).await.unwrap();
        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects.len(), 5);
        let at = |s: &str| subjects.iter().position(|x| *x == s).unwrap();
        assert!(at("m") < at("c") && at("m") < at("b") && at("d") < at("b") && at("b") < at("a"));
        let merge = &commits[at("m")];
        assert_eq!(merge.parents.len(), 2);
        assert_eq!(merge.refs, [GitRef::Branch("main".into())]);
        assert_eq!(commits[at("a")].refs, [GitRef::Tag("v1".into())]);

        let mut paged = read(&repo, CommitRange { skip: 0, limit: 2 })
            .await
            .unwrap();
        paged.extend(
            read(&repo, CommitRange { skip: 2, limit: 2 })
                .await
                .unwrap(),
        );
        paged.extend(
            read(&repo, CommitRange { skip: 4, limit: 2 })
                .await
                .unwrap(),
        );
        assert_eq!(paged, commits);

        let before = version(&repo).await.unwrap();
        run(&repo, &["stash", "drop", "-q"]);
        assert_eq!(version(&repo).await.unwrap(), before);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "e"]);
        assert_ne!(version(&repo).await.unwrap(), before);
    }

    #[tokio::test]
    async fn repository_without_commits_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        assert!(read(dir.path(), ALL).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn outside_a_repository_returns_git_error() {
        let dir = tempfile::tempdir().unwrap();
        let Err(err) = read(dir.path(), ALL).await else {
            panic!()
        };
        assert!(err.contains("not a git repository"), "{err}");
    }
}

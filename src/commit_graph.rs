//! 커밋 그래프 데이터: 저장소의 모든 브랜치 커밋. 읽기만 한다. 워크트리마다 체크아웃한 커밋은 `worktree`가 읽는다.
//! 차선 배치와 그리기는 화면(`web/src/CommitGraph.tsx`)이 한다.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git;

/// 커밋 줄의 칸 구분 문자. 커밋 제목·작성자·참조 이름에는 NUL이 들어가지 않는다.
const SEPARATOR: char = '\0';
/// `%D`의 태그 앞말.
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

/// stash는 모든 워크트리가 함께 쓰는 임시 커밋이라 브랜치 이력이 아니다.
const STASH_REF: &str = "refs/stash";

/// 화면이 읽는 구간: 앞에서 `skip`개를 건너뛴 `limit`개. 화면이 아래로 내릴 때 다음 구간을 읽는다.
#[derive(Deserialize, TS, Clone, Copy)]
#[ts(export)]
pub struct CommitRange {
    pub skip: u32,
    pub limit: u32,
}

/// 자식이 부모보다 먼저 온다(`--topo-order`). 화면의 차선 계산이 이 순서를 쓴다. 구간이 바뀌어도 순서는 같다.
pub async fn read(repo: &Path, range: CommitRange) -> Result<Vec<Commit>, String> {
    let log = git::run(
        repo,
        &[
            "log",
            // `--exclude`는 뒤에 오는 `--all`에만 걸린다.
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

/// 참조(브랜치·태그)와 그 커밋의 지문. 같으면 `read`가 주는 커밋도 같다. 화면은 3초마다 이것만 읽고, 바뀔 때만 커밋을 다시 읽는다.
/// 워크트리마다 체크아웃한 커밋(분리된 HEAD)은 워크트리 목록이 준다.
pub async fn version(repo: &Path) -> Result<String, String> {
    let refs = git::run(repo, &["for-each-ref", "--format=%(objectname) %(refname)"]).await?;
    let mut hasher = DefaultHasher::new();
    for line in refs.lines().filter(|l| !l.ends_with(STASH_REF)) {
        line.hash(&mut hasher);
    }
    // 16진 글자: JS 숫자는 u64를 다 담지 못한다.
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
                // `HEAD -> main`의 HEAD는 원래 체크아웃이다. 워크트리 목록이 이미 그 사실을 준다.
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

    /// Windows git은 `--max-count`를 32비트 부호 있는 정수로 읽는다. `u32::MAX`는 "not an integer"로 실패한다.
    const ALL: CommitRange = CommitRange {
        skip: 0,
        limit: i32::MAX as u32,
    };

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

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

    /// 실제 git으로 브랜치·병합·태그·워크트리·stash가 있는 저장소를 만들고 읽는다.
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
        // stash 커밋(WIP, index)은 없다. 자식이 부모보다 먼저 온다.
        assert_eq!(subjects.len(), 5);
        let at = |s: &str| subjects.iter().position(|x| *x == s).unwrap();
        assert!(at("m") < at("c") && at("m") < at("b") && at("d") < at("b") && at("b") < at("a"));
        let merge = &commits[at("m")];
        assert_eq!(merge.parents.len(), 2);
        assert_eq!(merge.refs, [GitRef::Branch("main".into())]);
        assert_eq!(commits[at("a")].refs, [GitRef::Tag("v1".into())]);

        // 구간을 이어 붙이면 한 번에 읽은 순서와 같다.
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

        // stash는 지문을 바꾸지 않는다. 커밋은 바꾼다.
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

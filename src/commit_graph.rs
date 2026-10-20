//! 커밋 그래프 데이터: 저장소의 모든 브랜치 커밋. 읽기만 한다. 워크트리마다 체크아웃한 커밋은 `worktree`가 읽는다.
//! 차선 배치와 그리기는 화면(`web/src/CommitGraph.tsx`)이 한다.

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use crate::git;

/// 그리는 커밋 수 상한. 화면은 최근 이력을 본다. 1,000줄은 화면이 가상 스크롤 없이 그린다(목업 실측 중앙값 39.8ms).
const MAX_COMMITS: u32 = 1000;
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

/// 자식이 부모보다 먼저 온다(`--topo-order`). 화면의 차선 계산이 이 순서를 쓴다.
pub async fn read(repo: &Path) -> Result<Vec<Commit>, String> {
    let log = git::run(
        repo,
        &[
            "log",
            // stash는 모든 워크트리가 함께 쓰는 임시 커밋이라 브랜치 이력이 아니다.
            // `--exclude`는 뒤에 오는 `--all`에만 걸린다.
            "--exclude=refs/stash",
            "--all",
            "--topo-order",
            &format!("--max-count={MAX_COMMITS}"),
            "--format=%H%x00%P%x00%s%x00%an%x00%at%x00%D",
        ],
    )
    .await?;
    Ok(log.lines().filter_map(commit).collect())
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

        let commits = read(&repo).await.unwrap();
        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        // stash 커밋(WIP, index)은 없다. 자식이 부모보다 먼저 온다.
        assert_eq!(subjects.len(), 5);
        let at = |s: &str| subjects.iter().position(|x| *x == s).unwrap();
        assert!(at("m") < at("c") && at("m") < at("b") && at("d") < at("b") && at("b") < at("a"));
        let merge = &commits[at("m")];
        assert_eq!(merge.parents.len(), 2);
        assert_eq!(merge.refs, [GitRef::Branch("main".into())]);
        assert_eq!(commits[at("a")].refs, [GitRef::Tag("v1".into())]);
    }

    #[tokio::test]
    async fn repository_without_commits_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        assert!(read(dir.path()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn outside_a_repository_returns_git_error() {
        let dir = tempfile::tempdir().unwrap();
        let Err(err) = read(dir.path()).await else {
            panic!()
        };
        assert!(err.contains("not a git repository"), "{err}");
    }
}

//! 커밋 그래프: 저장소의 모든 브랜치를 `git log --graph`로 그린 글자(ANSI 색 포함).
//! 워크트리가 체크아웃한 커밋 줄 끝에 그 워크트리 표시를 붙인다. 읽기만 한다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::process::Command;

/// 그리는 커밋 수 상한. 화면은 최근 이력을 본다. 전체 이력은 큰 저장소에서 git과 화면 버퍼를 무겁게 한다.
const MAX_COMMITS: u32 = 1000;
/// 커밋 줄을 「그래프 선, SHA, 나머지」로 가르는 문자. 커밋 제목에는 NUL이 들어가지 않는다.
/// SHA를 `%C(auto)` 앞에 둔다. 뒤에 두면 git이 SHA에도 색 코드를 붙인다.
const SEPARATOR: char = '\0';
/// 워크트리 표시의 ANSI 모양: 반전. 색을 따로 두지 않고 화면 터미널의 글자색·바탕색을 뒤집는다.
const MARK_START: &str = "\x1b[7m";
const RESET: &str = "\x1b[0m";

/// `repo`는 데몬이 보이는 저장소다. 이 경로의 워크트리는 「원래 체크아웃」, 나머지는 「워크트리」로 표시한다.
pub async fn draw(repo: &Path) -> Result<String, String> {
    let worktrees = git(repo, &["worktree", "list", "--porcelain"]).await?;
    let log = git(
        repo,
        &[
            "log",
            "--graph",
            "--color=always",
            // stash는 모든 워크트리가 함께 쓰는 임시 커밋이라 브랜치 이력이 아니다.
            "--exclude=refs/stash",
            "--all",
            &format!("--max-count={MAX_COMMITS}"),
            "--format=%x00%H%x00%C(auto)%h%d %s",
        ],
    )
    .await?;
    Ok(annotate(&log, &marks(repo, &worktrees)))
}

async fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("git을 실행하지 못했다: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 커밋 SHA → 그 커밋을 체크아웃한 워크트리 표시들. `porcelain`은 `git worktree list --porcelain` 출력이다.
fn marks(repo: &Path, porcelain: &str) -> HashMap<String, Vec<String>> {
    let repo = canonical(repo);
    let mut marks: HashMap<String, Vec<String>> = HashMap::new();
    for block in porcelain.split("\n\n") {
        let field = |key: &str| {
            block
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
        };
        let (Some(path), Some(head)) = (field("worktree"), field("HEAD")) else {
            continue;
        };
        let kind = if canonical(Path::new(path)) == repo {
            "원래 체크아웃"
        } else {
            "워크트리"
        };
        let branch = field("branch").map_or("HEAD", |b| b.strip_prefix("refs/heads/").unwrap_or(b));
        marks
            .entry(head.to_owned())
            .or_default()
            .push(format!("{kind} {branch}"));
    }
    marks
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// 커밋 줄에서 SHA를 지우고, 그 커밋을 체크아웃한 워크트리가 있으면 줄 끝에 표시를 붙인다.
fn annotate(log: &str, marks: &HashMap<String, Vec<String>>) -> String {
    log.lines()
        .map(
            |line| match line.splitn(3, SEPARATOR).collect::<Vec<_>>()[..] {
                [graph, sha, rest] => {
                    let tags: String = marks
                        .get(sha)
                        .into_iter()
                        .flatten()
                        .map(|mark| format!(" {MARK_START} {mark} {RESET}"))
                        .collect();
                    format!("{graph}{rest}{RESET}{tags}")
                }
                _ => line.to_owned(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|&c| c == 'm');
            } else {
                out.push(c);
            }
        }
        out
    }

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
    fn annotate_marks_checked_out_commits() {
        let marks = HashMap::from([("bbb".to_owned(), vec!["워크트리 feat".to_owned()])]);
        let log = "* \0aaa\0a1 (main) first\n|\n* \0bbb\0b2 (feat) second";
        assert_eq!(
            annotate(log, &marks),
            "* a1 (main) first\x1b[0m\n|\n* b2 (feat) second\x1b[0m \x1b[7m 워크트리 feat \x1b[0m"
        );
    }

    #[test]
    fn marks_name_original_checkout_and_detached_head() {
        let porcelain = "worktree /nowhere/repo\nHEAD aaa\nbranch refs/heads/main\n\n\
                         worktree /nowhere/wt\nHEAD bbb\ndetached\n\n\
                         worktree /nowhere/wt2\nHEAD bbb\nbranch refs/heads/feat/x\n";
        let marks = marks(Path::new("/nowhere/repo"), porcelain);
        assert_eq!(marks["aaa"], ["원래 체크아웃 main"]);
        assert_eq!(marks["bbb"], ["워크트리 HEAD", "워크트리 feat/x"]);
    }

    /// 실제 git으로 브랜치·병합·워크트리가 있는 저장소를 만들고 그린다.
    #[tokio::test]
    async fn draws_all_branches_merges_and_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
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

        let text = strip_ansi(&draw(&repo).await.unwrap());
        let short = |rev: &str| run(&repo, &["rev-parse", "--short", rev]);
        let lines: Vec<&str> = text.lines().collect();
        let line = |subject: &str| {
            *lines
                .iter()
                .find(|l| l.ends_with(subject) || l.contains(&format!(" {subject} ")))
                .unwrap()
        };

        assert!(line("d").contains(&format!("{} (feat) d  워크트리 feat ", short("feat"))));
        assert!(line("m").contains(&format!(
            "{} (HEAD -> main) m  원래 체크아웃 main ",
            short("main")
        )));
        // 병합 커밋 아래에 갈래 선이 있다.
        assert!(lines.iter().any(|l| l.contains("|\\")));
        // 커밋 a, b, c, m, d만 있다. stash 커밋(WIP, index)은 그리지 않는다.
        assert_eq!(lines.iter().filter(|l| l.contains('*')).count(), 5);
        assert!(!text.contains("WIP"));
    }

    #[tokio::test]
    async fn outside_a_repository_returns_git_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = draw(dir.path()).await.unwrap_err();
        assert!(err.contains("not a git repository"), "{err}");
    }
}

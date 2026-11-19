use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pretty_assertions::assert_eq;

use super::GitCommitFact;
use super::GitEnvironmentFacts;
use super::GitFactList;
use super::GitRepositoryFacts;
use super::GitStatusEntry;
use super::git_environment_facts;

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Fact Test",
            "-c",
            "user.email=facts@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .current_dir(cwd)
        .output()
        .expect("run Git to prepare fixture");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 fixture output")
}

fn repository(facts: GitEnvironmentFacts) -> Box<GitRepositoryFacts> {
    let GitEnvironmentFacts::Repository(repository) = facts else {
        panic!("expected a repository, got {facts:?}");
    };
    repository
}

fn change(code: &str, path: &str) -> GitStatusEntry {
    GitStatusEntry {
        code: code.to_string(),
        path: path.to_string(),
    }
}

#[test]
fn collects_staged_and_unstaged_status_from_one_index() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    git(cwd, &["init", "-b", "main"]);
    for name in ["staged", "unstaged", "both"] {
        std::fs::write(cwd.join(name), "baseline").unwrap();
    }
    git(cwd, &["add", "--all"]);
    git(cwd, &["commit", "-m", "baseline"]);
    let sha = git(cwd, &["rev-parse", "--short=9", "HEAD"]);

    git(cwd, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        cwd,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    std::fs::write(cwd.join("staged"), "staged change").unwrap();
    std::fs::write(cwd.join("both"), "staged change").unwrap();
    git(cwd, &["add", "staged", "both"]);
    std::fs::write(cwd.join("both"), "different unstaged change").unwrap();
    std::fs::write(cwd.join("unstaged"), "unstaged change").unwrap();
    std::fs::write(cwd.join(".git/info/exclude"), "ignored\n").unwrap();
    std::fs::write(cwd.join("ignored"), "not reported").unwrap();
    std::fs::write(cwd.join("untracked"), "reported").unwrap();
    std::fs::create_dir(cwd.join("directory")).unwrap();
    std::fs::write(cwd.join("directory/file"), "reported as a directory").unwrap();
    let index = std::fs::read(cwd.join(".git/index")).unwrap();

    let facts = repository(git_environment_facts(
        cwd,
        &Arc::new(AtomicBool::new(false)),
    ));
    assert_eq!(
        facts.status,
        Some(GitFactList {
            entries: vec![
                change("MM", "both"),
                change("M ", "staged"),
                change(" M", "unstaged"),
                change("??", "directory/"),
                change("??", "untracked"),
            ],
            truncated: false,
        })
    );
    assert_eq!(facts.branch, "main");
    assert_eq!(facts.head, sha.trim());
    assert_eq!(facts.default_branch.as_deref(), Some("main"));
    let history = facts.history.expect("history for resolved HEAD");
    assert_eq!(
        history.remote_branches_containing_head,
        Some(GitFactList {
            entries: vec!["origin/main".to_string()],
            truncated: false,
        })
    );
    assert_eq!(
        history.recent_commits,
        Some(vec![GitCommitFact {
            id: sha.trim().to_string(),
            subject: "baseline".to_string(),
        }])
    );
    assert_eq!(std::fs::read(cwd.join(".git/index")).unwrap(), index);
}

#[test]
fn reports_dirty_submodules_without_running_their_filters() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let repository = directory.path().join("parent");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&repository).unwrap();
    git(&source, &["init", "-b", "main"]);
    std::fs::write(source.join("tracked"), "source").unwrap();
    git(&source, &["add", "tracked"]);
    git(&source, &["commit", "-m", "source"]);
    git(&repository, &["init", "-b", "main"]);
    git(
        &repository,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--",
            source.to_str().unwrap(),
            "module",
        ],
    );
    git(&repository, &["commit", "-m", "add submodule"]);

    let module = repository.join("module");
    std::fs::write(module.join("tracked"), "changed").unwrap();
    let interrupt = Arc::new(AtomicBool::new(false));
    assert_eq!(
        self::repository(git_environment_facts(&repository, &interrupt)).status,
        Some(GitFactList {
            entries: vec![change(" M", "module")],
            truncated: false
        })
    );

    git(
        &module,
        &[
            "config",
            "filter.probe.clean",
            "printf ran > ../filter-ran; cat",
        ],
    );
    std::fs::write(module.join(".gitattributes"), "tracked filter=probe\n").unwrap();
    assert_eq!(
        self::repository(git_environment_facts(&repository, &interrupt)).status,
        None
    );
    assert!(!repository.join("filter-ran").exists());
}

#[test]
fn reports_unborn_and_non_repository_cwds() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let interrupt = Arc::new(AtomicBool::new(false));
    assert_eq!(
        git_environment_facts(cwd, &interrupt),
        GitEnvironmentFacts::NotRepository
    );
    gix::init(cwd).unwrap();
    std::fs::write(cwd.join(".git/HEAD"), "ref: refs/heads/test-branch\n").unwrap();
    let facts = repository(git_environment_facts(cwd, &interrupt));
    assert_eq!(facts.branch, "test-branch");
    assert_eq!(facts.head, "unborn");
    assert_eq!(facts.history, None);
    assert_eq!(
        facts.status,
        Some(GitFactList {
            entries: Vec::new(),
            truncated: false
        })
    );
}

#[test]
fn respects_global_ignore_configuration() {
    const WORKTREE: &str = "CODEX_TEST_FACTS_WORKTREE";
    if let Some(cwd) = std::env::var_os(WORKTREE) {
        let facts = repository(git_environment_facts(
            Path::new(&cwd),
            &Arc::new(AtomicBool::new(false)),
        ));
        assert_eq!(
            facts.status,
            Some(GitFactList {
                entries: vec![change("??", "visible")],
                truncated: false
            })
        );
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let config = home.join(".config");
    let cwd = tmp.path().join("repo");
    std::fs::create_dir_all(config.join("git")).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    git(&cwd, &["init", "-b", "main"]);
    for name in ["ignored", "visible"] {
        std::fs::write(cwd.join(name), name).unwrap();
    }
    let ignore = "ignored\nrelative-ignore\n";
    std::fs::write(home.join("custom-ignore"), ignore).unwrap();
    std::fs::write(cwd.join("relative-ignore"), ignore).unwrap();
    let global = home.join(".gitconfig");

    // Give each subprocess its own HOME and XDG settings without mutating the running tests.
    for (global_config, default_ignore) in [
        ("[filter \"example\"]\n\tclean = false\n", ignore),
        (
            "[core]\n\texcludesFile = ~/custom-ignore\n[filter \"example\"]\n\tclean = false\n",
            "",
        ),
        ("[core]\n\texcludesFile = relative-ignore\n", ""),
    ] {
        std::fs::write(&global, global_config).unwrap();
        std::fs::write(config.join("git/ignore"), default_ignore).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "environment_facts::tests::respects_global_ignore_configuration",
            ])
            .env_clear()
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(WORKTREE, &cwd)
            .output()
            .expect("run facts probe with isolated configuration");
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

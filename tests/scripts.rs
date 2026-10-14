//! Smoke tests for the embedded release scripts + materialization. Guards two things the
//! embed-and-materialize pattern depends on: the scripts are actually embedded (non-empty, generic),
//! and `materialize_scripts` writes an executable copy.

#[test]
fn scripts_are_embedded_and_generic() {
    for (label, body) in [
        ("release", shell_core::RELEASE_SH),
        ("gen-latest", shell_core::GEN_LATEST_SH),
        ("install", shell_core::INSTALL_APP_SH),
        ("launch", shell_core::LAUNCH_APP_SH),
        ("core-patch", shell_core::CORE_PATCH_SH),
    ] {
        assert!(!body.trim().is_empty(), "{label} script is empty");
        // Every script sources the per-app tooling.env — the generalization seam.
        assert!(
            body.contains(r#"source "$(dirname "$0")/tooling.env""#),
            "{label} script must source tooling.env"
        );
        // No app name may be baked into the generic script.
        assert!(!body.contains("warden"), "{label} script leaks 'warden'");
        assert!(!body.contains("curator"), "{label} script leaks 'curator'");
        assert!(!body.contains("lector"), "{label} script leaks 'lector'");
    }
}

/// The release artifact is a universal binary, and the updater matches on the running install's
/// OWN arch — so a manifest missing a platform key makes auto-update go silently quiet (no error,
/// no bar) for every user on that architecture, invisibly from the machine that cut the release.
/// Nothing else fails when a key is dropped, so assert both are present and that the build that
/// justifies them actually targets universal.
#[test]
fn updater_manifest_covers_both_macos_architectures() {
    for key in ["darwin-aarch64", "darwin-x86_64"] {
        assert!(
            shell_core::GEN_LATEST_SH.contains(key),
            "gen-latest-json.sh dropped the {key} platform entry — auto-update goes silently \
             quiet for every install on that architecture"
        );
    }
    assert!(
        shell_core::RELEASE_SH.contains("universal-apple-darwin"),
        "release.sh must build --target universal-apple-darwin — a host-arch bundle cannot back \
         both platform keys in the manifest"
    );
}

#[test]
fn materialize_writes_executable_scripts() {
    let dir = std::env::temp_dir().join(format!("shell-core-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    shell_core::materialize_scripts(&dir).unwrap();

    for name in [
        "release.sh",
        "gen-latest-json.sh",
        "install-app.sh",
        "launch-app.sh",
        "core-patch.sh",
    ] {
        let path = dir.join(name);
        assert!(path.exists(), "{name} not materialized");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "{name} is not executable");
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

// ── behaviour: the materialized scripts run against a throwaway app repo ──

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway consuming-app repo on `dev` (tag v1.2.2, then one more commit at version 1.2.3),
/// pushed to a bare `origin`, with the scripts materialized git-ignored as a real app has them.
/// Git runs with no global/system config so the machine's hooks, signing and identity stay out.
struct Fixture {
    root: PathBuf,
    app: PathBuf,
    stub_bin: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("shell-core-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = root.join("app");
        let stub_bin = root.join("bin");
        std::fs::create_dir_all(app.join("scripts")).unwrap();
        std::fs::create_dir_all(app.join("src-tauri")).unwrap();
        std::fs::create_dir_all(&stub_bin).unwrap();
        // `gh auth status` is a release.sh precondition; the stub answers it (and nothing else
        // is reached under DRY_RUN or before a refusal).
        write_exec(&stub_bin.join("gh"), "#!/bin/sh\nexit 0\n");
        let f = Fixture {
            root,
            app,
            stub_bin,
        };

        f.git(&["init", "-q", "--bare", "../origin.git"]);
        f.git(&["init", "-q", "-b", "dev"]);
        f.write(".gitignore", "/scripts/*.sh\nlatest.json\n*.zip\n");
        f.write(
            "scripts/tooling.env",
            "APP_NAME=demo\nTAURI_CRATE_DIR=src-tauri\nUPDATER_REPO=owner/demo\n\
             CORES=\"widget:widget-core:src-tauri/Cargo.toml\"\n",
        );
        f.write(
            "Cargo.toml",
            "[workspace]\n#PATCH:widget#[patch.\"https://github.com/Lockyc/widget-core\"]\n\
             #PATCH:widget#widget-core = { path = \"../widget-core\" }\n",
        );
        f.write("src-tauri/Cargo.toml", "[package]\nversion = \"1.2.2\"\n");
        f.git(&["add", "-A"]);
        f.git(&["commit", "-q", "-m", "Initial"]);
        f.git(&["tag", "-a", "v1.2.2", "-m", "v1.2.2"]);
        f.git(&["branch", "main"]);
        f.write("src-tauri/Cargo.toml", "[package]\nversion = \"1.2.3\"\n");
        f.git(&["commit", "-q", "-am", "Add the widget"]);
        f.git(&["remote", "add", "origin", "../origin.git"]);
        f.git(&["push", "-q", "origin", "dev", "main", "v1.2.2"]);
        shell_core::materialize_scripts(&f.app.join("scripts")).unwrap();
        f
    }

    fn write(&self, rel: &str, body: &str) {
        std::fs::write(self.app.join(rel), body).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        let out = self.cmd("git").args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }

    fn cmd(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        let path = format!(
            "{}:{}",
            self.stub_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        c.current_dir(&self.app)
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env("APPLE_SIGNING_IDENTITY", "test")
            .env("TAURI_SIGNING_PRIVATE_KEY", "test")
            .env_remove("DRY_RUN");
        c
    }

    fn script(&self, name: &str, args: &[&str]) -> Command {
        let mut c = self.cmd("bash");
        c.arg(format!("scripts/{name}")).args(args);
        c
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn line_of(out: &str, needle: &str) -> usize {
    out.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{out}"))
}

/// The release's one ordering promise: nothing public (tag, main, push, release) until the
/// notarized build has produced its artifacts, and the assets publish in the same step as the
/// notes. A dry run prints the side effects in execution order and performs none of them.
#[test]
fn release_builds_before_anything_public_and_publishes_assets_with_notes() {
    let f = Fixture::new("release-order");
    let out: Output = f
        .script("release.sh", &[])
        .env("DRY_RUN", "1")
        .output()
        .unwrap();
    let stdout = text(&out.stdout);
    assert!(
        out.status.success(),
        "dry run refused:\n{stdout}\n{}",
        text(&out.stderr)
    );

    let build = line_of(&stdout, "cargo tauri build --target universal-apple-darwin");
    let manifest = line_of(&stdout, "gen-latest-json.sh 1.2.3 latest.json");
    let tag = line_of(&stdout, "+ git tag -a v1.2.3");
    let ff = line_of(&stdout, "+ git fetch -q . dev:main");
    let push = line_of(&stdout, "+ git push --atomic origin main refs/tags/v1.2.3");
    let publish = line_of(&stdout, "+ gh release create v1.2.3");
    assert!(build < manifest && manifest < tag && tag < ff && ff < push && push < publish);

    let gh = stdout.lines().nth(publish).unwrap();
    for asset in [
        "demo-1.2.3-macos.zip",
        "demo.app.tar.gz",
        "demo.app.tar.gz.sig",
        "latest.json",
        "--notes-file",
        "--verify-tag",
    ] {
        assert!(gh.contains(asset), "publish step lacks {asset}: {gh}");
    }
    // Default notes: the commit subjects since the previous tag, plus the compare link.
    assert!(stdout.contains("- Add the widget"), "{stdout}");
    assert!(stdout.contains("compare/v1.2.2...v1.2.3"), "{stdout}");

    // Dry means dry: no tag, main unmoved.
    assert_eq!(f.git(&["tag", "-l", "v1.2.3"]), "");
    assert_eq!(
        f.git(&["rev-parse", "main"]),
        f.git(&["rev-parse", "v1.2.2^{commit}"])
    );
}

/// Every refusal precedes the build: an untracked file (the artifact must hold only committed
/// code) and an unpushed dev each stop a real run with no tag created.
#[test]
fn release_refuses_dirty_or_unpushed_dev_before_any_side_effect() {
    let f = Fixture::new("release-refuse");
    f.write("stray.txt", "x");
    let out = f.script("release.sh", &[]).output().unwrap();
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("working tree is dirty"));
    std::fs::remove_file(f.app.join("stray.txt")).unwrap();

    f.write("src-tauri/Cargo.toml", "[package]\nversion = \"1.2.4\"\n");
    f.git(&["commit", "-q", "-am", "Unpushed"]);
    let out = f.script("release.sh", &[]).output().unwrap();
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("not in sync with origin/dev"));
    assert_eq!(f.git(&["tag", "-l", "v1.2.*"]), "v1.2.2\n");
}

/// `dev` activates exactly the named core's [patch]; `guard` then blocks it, naming the pin
/// recipes from tooling.env's CORES.
#[test]
fn core_patch_dev_activates_the_patch_and_guard_blocks_it() {
    let f = Fixture::new("core-patch");
    let run = |args: &[&str]| f.script("core-patch.sh", args).output().unwrap();

    assert!(
        run(&["guard"]).status.success(),
        "guard fails on a clean manifest"
    );

    let missing = run(&["dev", "widget"]);
    assert!(!missing.status.success());
    assert!(text(&missing.stderr).contains("../widget-core not found"));

    let unknown = run(&["dev", "gadget"]);
    assert!(!unknown.status.success());
    assert!(text(&unknown.stderr).contains("unknown core 'gadget'"));

    std::fs::create_dir_all(f.root.join("widget-core")).unwrap();
    assert!(run(&["dev", "widget"]).status.success());
    let manifest = std::fs::read_to_string(f.app.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("\n[patch.\"https://github.com/Lockyc/widget-core\"]\n"));
    assert!(!manifest.contains("#PATCH:widget#"));

    let guard = run(&["guard"]);
    assert!(!guard.status.success());
    assert!(text(&guard.stderr).contains("'just widget-pin'"));
}

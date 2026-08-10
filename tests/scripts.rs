//! Smoke tests for the embedded release scripts + materialization. Guards two things the
//! embed-and-materialize pattern depends on: the scripts are actually embedded (non-empty, generic),
//! and `materialize_scripts` writes an executable copy.

#[test]
fn scripts_are_embedded_and_generic() {
    for (label, body) in [
        ("release", shell_core::RELEASE_SH),
        ("gen-latest", shell_core::GEN_LATEST_SH),
        ("install", shell_core::INSTALL_APP_SH),
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

    for name in ["release.sh", "gen-latest-json.sh", "install-app.sh"] {
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

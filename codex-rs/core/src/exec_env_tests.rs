use super::*;
use codex_protocol::config_types::ShellEnvironmentPolicyInherit;
use maplit::hashmap;
use pretty_assertions::assert_eq;

#[test]
fn session_tmp_is_prepared_private_and_reused() -> std::io::Result<()> {
    let home = tempfile::tempdir()?;
    let session_id = SessionId::from(ThreadId::new());
    let path = session_tmp_path(session_id);
    prepare_session_tmp(session_id, home.path())?;
    std::fs::write(path.join("retained"), "scratch")?;
    prepare_session_tmp(session_id, home.path())?;
    let mut env = HashMap::from([("SESSION_TMP".to_string(), "stale".to_string())]);
    inject_session_env(&mut env, session_id);
    assert_eq!(env["SESSION_TMP"], path.to_string_lossy());
    assert_eq!(std::fs::read_to_string(path.join("retained"))?, "scratch");
    assert_ne!(path, session_tmp_path(SessionId::from(ThreadId::new())));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o700
        );
    }
    std::fs::remove_file(path.join("retained"))?;
    std::fs::remove_dir(path)
}

#[cfg(unix)]
#[test]
fn session_tmp_rejects_symlinks_and_non_private_directories() -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir()?;
    let session_id = SessionId::from(ThreadId::new());
    let path = session_tmp_path(session_id);
    symlink(home.path(), &path)?;
    assert_eq!(
        prepare_session_tmp(session_id, home.path())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    std::fs::remove_file(&path)?;
    prepare_session_tmp(session_id, home.path())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    assert_eq!(
        prepare_session_tmp(session_id, home.path())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o755
    );
    std::fs::remove_dir(path)
}

fn make_vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn inject_permission_profile_env_overrides_policy_value() {
    let mut env = HashMap::from([(
        CODEX_PERMISSION_PROFILE_ENV_VAR.to_string(),
        "stale-profile".to_string(),
    )]);

    inject_permission_profile_env(
        &mut env,
        Some(&ActivePermissionProfile::new("current-profile")),
    );

    assert_eq!(
        env.get(CODEX_PERMISSION_PROFILE_ENV_VAR)
            .map(String::as_str),
        Some("current-profile")
    );
}

#[test]
fn inject_permission_profile_env_removes_stale_value_without_active_profile() {
    let mut env = HashMap::from([(
        CODEX_PERMISSION_PROFILE_ENV_VAR.to_string(),
        "stale-profile".to_string(),
    )]);

    inject_permission_profile_env(&mut env, /*active_permission_profile*/ None);

    assert_eq!(env.get(CODEX_PERMISSION_PROFILE_ENV_VAR), None);
}

#[test]
fn inject_apply_patch_env_enables_older_executables_despite_stale_values() {
    let mut env = HashMap::from([(
        CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR.to_ascii_lowercase(),
        "stale".to_string(),
    )]);
    inject_apply_patch_env(&mut env);
    assert_eq!(
        env,
        HashMap::from([(
            CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR.to_string(),
            "1".to_string(),
        )])
    );
}

#[cfg(target_os = "windows")]
#[test]
fn inject_permission_profile_env_replaces_differently_cased_windows_key() {
    let mut env = HashMap::from([(
        "codex_permission_profile".to_string(),
        "stale-profile".to_string(),
    )]);

    inject_permission_profile_env(
        &mut env,
        Some(&ActivePermissionProfile::new("current-profile")),
    );

    assert_eq!(
        env,
        HashMap::from([(
            CODEX_PERMISSION_PROFILE_ENV_VAR.to_string(),
            "current-profile".to_string(),
        )])
    );
}

#[test]
fn test_core_inherit_defaults_keep_sensitive_vars() {
    let vars = make_vars(&[
        ("PATH", "/usr/bin"),
        ("HOME", "/home/user"),
        ("API_KEY", "secret"),
        ("SECRET_TOKEN", "t"),
    ]);

    let policy = ShellEnvironmentPolicy::default(); // inherit All, default excludes ignored
    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));

    let mut expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
        "HOME".to_string() => "/home/user".to_string(),
        "API_KEY".to_string() => "secret".to_string(),
        "SECRET_TOKEN".to_string() => "t".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());

    assert_eq!(result, expected);
}

#[test]
fn test_core_inherit_with_default_excludes_enabled() {
    let vars = make_vars(&[
        ("PATH", "/usr/bin"),
        ("HOME", "/home/user"),
        ("API_KEY", "secret"),
        ("SECRET_TOKEN", "t"),
    ]);

    let policy = ShellEnvironmentPolicy {
        ignore_default_excludes: false, // apply KEY/SECRET/TOKEN filter
        ..Default::default()
    };
    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));

    let mut expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
        "HOME".to_string() => "/home/user".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());

    assert_eq!(result, expected);
}

#[test]
fn test_include_only() {
    let vars = make_vars(&[("PATH", "/usr/bin"), ("FOO", "bar")]);

    let policy = ShellEnvironmentPolicy {
        // skip default excludes so nothing is removed prematurely
        ignore_default_excludes: true,
        include_only: vec![EnvironmentVariablePattern::new_case_insensitive("*PATH")],
        ..Default::default()
    };

    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));

    let mut expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());

    assert_eq!(result, expected);
}

#[test]
fn test_set_overrides() {
    let vars = make_vars(&[("PATH", "/usr/bin")]);

    let mut policy = ShellEnvironmentPolicy {
        ignore_default_excludes: true,
        ..Default::default()
    };
    policy.r#set.insert("NEW_VAR".to_string(), "42".to_string());

    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));

    let mut expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
        "NEW_VAR".to_string() => "42".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());

    assert_eq!(result, expected);
}

#[test]
fn populate_env_inserts_thread_id() {
    let vars = make_vars(&[("PATH", "/usr/bin")]);
    let policy = ShellEnvironmentPolicy::default();
    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));

    let mut expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());

    assert_eq!(result, expected);
}

#[test]
fn populate_env_omits_thread_id_when_missing() {
    let vars = make_vars(&[("PATH", "/usr/bin")]);
    let policy = ShellEnvironmentPolicy::default();
    let result = populate_env(vars, &policy, /*thread_id*/ None);

    let expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
    };

    assert_eq!(result, expected);
}

#[test]
fn test_inherit_all() {
    let vars = make_vars(&[("PATH", "/usr/bin"), ("FOO", "bar")]);

    let policy = ShellEnvironmentPolicy {
        inherit: ShellEnvironmentPolicyInherit::All,
        ignore_default_excludes: true, // keep everything
        ..Default::default()
    };

    let thread_id = ThreadId::new();
    let result = populate_env(vars.clone(), &policy, Some(thread_id));
    let mut expected: HashMap<String, String> = vars.into_iter().collect();
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());
    assert_eq!(result, expected);
}

#[test]
fn test_inherit_all_with_default_excludes() {
    let vars = make_vars(&[("PATH", "/usr/bin"), ("API_KEY", "secret")]);

    let policy = ShellEnvironmentPolicy {
        inherit: ShellEnvironmentPolicyInherit::All,
        ignore_default_excludes: false,
        ..Default::default()
    };

    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));
    let mut expected: HashMap<String, String> = hashmap! {
        "PATH".to_string() => "/usr/bin".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());
    assert_eq!(result, expected);
}

#[test]
#[cfg(target_os = "windows")]
fn test_core_inherit_respects_case_insensitive_names_on_windows() {
    let vars = make_vars(&[
        ("Path", "C:\\Windows\\System32"),
        ("PathExt", ".COM;.EXE;.BAT;.CMD"),
        ("TEMP", "C:\\Temp"),
        ("FOO", "bar"),
    ]);

    let policy = ShellEnvironmentPolicy {
        inherit: ShellEnvironmentPolicyInherit::Core,
        ignore_default_excludes: true,
        ..Default::default()
    };

    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));
    let mut expected: HashMap<String, String> = hashmap! {
        "Path".to_string() => "C:\\Windows\\System32".to_string(),
        "PathExt".to_string() => ".COM;.EXE;.BAT;.CMD".to_string(),
        "TEMP".to_string() => "C:\\Temp".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());

    assert_eq!(result, expected);
}

#[test]
#[cfg(target_os = "windows")]
fn create_env_inserts_pathext_on_windows_when_missing() {
    let vars = make_vars(&[]);

    let policy = ShellEnvironmentPolicy {
        inherit: ShellEnvironmentPolicyInherit::None,
        ignore_default_excludes: true,
        ..Default::default()
    };

    let result = create_env_from_vars(vars, &policy, /*thread_id*/ None);

    let expected: HashMap<String, String> = hashmap! {
        "PATHEXT".to_string() => ".COM;.EXE;.BAT;.CMD".to_string(),
    };
    assert_eq!(result, expected);
}

#[test]
#[cfg(target_os = "windows")]
fn create_env_preserves_existing_pathext_case_insensitively_on_windows() {
    let vars = make_vars(&[("PathExt", ".COM;.EXE;.BAT;.CMD;.PS1")]);

    let policy = ShellEnvironmentPolicy {
        inherit: ShellEnvironmentPolicyInherit::Core,
        ignore_default_excludes: true,
        ..Default::default()
    };

    let result = create_env_from_vars(vars, &policy, /*thread_id*/ None);

    let pathext_vars = result
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("PATHEXT"))
        .collect::<Vec<_>>();

    assert_eq!(pathext_vars.len(), 1);
    assert_eq!(pathext_vars[0].1, ".COM;.EXE;.BAT;.CMD;.PS1");
}

#[test]
fn test_inherit_none() {
    let vars = make_vars(&[("PATH", "/usr/bin"), ("HOME", "/home")]);

    let mut policy = ShellEnvironmentPolicy {
        inherit: ShellEnvironmentPolicyInherit::None,
        ignore_default_excludes: true,
        ..Default::default()
    };
    policy
        .r#set
        .insert("ONLY_VAR".to_string(), "yes".to_string());

    let thread_id = ThreadId::new();
    let result = populate_env(vars, &policy, Some(thread_id));
    let mut expected: HashMap<String, String> = hashmap! {
        "ONLY_VAR".to_string() => "yes".to_string(),
    };
    expected.insert(CODEX_THREAD_ID_ENV_VAR.to_string(), thread_id.to_string());
    assert_eq!(result, expected);
}

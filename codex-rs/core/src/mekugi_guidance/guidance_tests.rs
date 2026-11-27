use std::os::unix::fs::PermissionsExt;

use pretty_assertions::assert_eq;

use super::shell_guidance;

#[test]
fn missing_helpers_leave_native_shell_description_alone() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    assert_eq!(shell_guidance(directory.path().as_os_str()), "");
    Ok(())
}

#[test]
fn available_helpers_use_the_native_command_entry() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mcat = directory.path().join("mcat");
    let mfinspect = directory.path().join("mfinspect");
    let mread = directory.path().join("mread");
    std::fs::write(&mcat, "")?;
    std::fs::set_permissions(&mcat, std::fs::Permissions::from_mode(0o700))?;
    std::fs::write(&mfinspect, "")?;
    std::fs::set_permissions(&mfinspect, std::fs::Permissions::from_mode(0o700))?;
    std::fs::write(&mread, "")?;
    std::fs::set_permissions(&mread, std::fs::Permissions::from_mode(0o700))?;
    std::fs::write(directory.path().join("mrun"), "")?;
    let mut description = "Runs a shell command.".to_string();
    description.push_str(&shell_guidance(directory.path().as_os_str()));
    let entry = codex_code_mode::render_code_mode_sample(
        &description,
        "exec_command",
        "args",
        "{cmd: string}".to_string(),
        "string".to_string(),
    );

    assert!(entry.starts_with("Runs a shell command.\n\n### Session shell helpers"));
    assert!(entry.contains("$SESSION_TMP/truncated_output/{id}"));
    assert!(!entry.contains("write_stdin"));
    assert!(entry.contains("#### `mfinspect`\n\nOutline source structure"));
    assert!(entry.contains("#### `mcat`\n\nRead UTF-8 files or inclusive line ranges."));
    assert!(!entry.contains("#### `mread`"));
    assert!(!entry.contains("mread"));
    assert!(!entry.contains("#### `mrun`"));
    assert!(entry.contains("### Source references\n\nSource references"));
    assert!(entry.contains("@path#Lstart-Lend,Lstart2-Lend2"));
    assert!(entry.contains("exec_command(args: {cmd: string}): Promise<string>"));
    Ok(())
}

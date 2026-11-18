use super::*;
use pretty_assertions::assert_eq;

fn frame(reference: &str, body: &str) -> String {
    let reference = serde_json::to_string(reference).unwrap();
    let payload = format!("\nSource {reference}:\n{body}\n");
    let start = payload.len() - body.len() - 1;
    format!(
        "{OPEN}{}\" bodies=\"{start}:{}\">\n{payload}{CLOSE}",
        payload.len(),
        payload.len() - 1,
    )
}

#[test]
fn output_deduplication_projects_structured_bodies_once_and_updates_bounds() {
    let nested = frame("$nested", "nested body");
    let text = format!("before\n{}\nafter", frame("@a.rs#L10-L12", &nested));
    let mut seen = Vec::new();
    let projected = project(&text, |reference, body| {
        seen.push((reference.to_owned(), body.to_owned()));
        "short α".to_owned()
    })
    .unwrap();
    assert_eq!(seen, vec![("@a.rs#L10-L12".to_owned(), nested)]);
    assert_eq!(
        projected,
        format!("before\n{}\nafter", frame("@a.rs#L10-L12", "short α"))
    );
    assert_eq!(
        project(&projected, |_, body| body.to_owned()),
        Some(projected)
    );
}

#[test]
fn output_deduplication_rejects_incomplete_and_invalid_source_frames() {
    let valid = frame("@a", "α source");
    for invalid in [
        valid.replace("bytes=\"", "bytes=\"999"),
        valid.replace("bodies=\"", "bodies=\"999:"),
        valid.trim_end_matches(CLOSE).to_owned(),
        format!("{OPEN}1\" bodies=\"\">\n{}", frame("$nested", "body")),
    ] {
        assert_eq!(
            project(&invalid, |_, _| panic!("invalid source was projected")),
            None
        );
    }
    assert_eq!(project("ordinary user text", |_, _| panic!()), None);
}

#[test]
fn output_deduplication_leaves_failure_only_frames_unchanged() {
    let payload = "\nSource \"@missing\": CONTENT NOT ATTACHED: \"failure\"\n";
    let text = format!("{OPEN}{}\" bodies=\"\">\n{payload}{CLOSE}", payload.len());
    assert_eq!(project(&text, |_, _| panic!()), Some(text));
}

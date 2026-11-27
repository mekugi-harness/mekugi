//! Shared source acquisition for messages, preload and context reload. Files and skills use
//! the selected native filesystem, and captured changes use the family's retained store.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use codex_file_system::EnvironmentAccess;
use codex_file_system::GetMetadataOptions;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::{AcquiredSource, ReloadSelection, SourceAcquisitionResult};
use codex_skills_extension::HostSkillsSnapshot;
use codex_utils_path_uri::PathUri;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::tools::handlers::resolve_tool_environment;

#[path = "context_sources_directory.rs"]
mod directory;

#[path = "context_sources_framing.rs"]
mod framing;
pub(crate) use framing::contains_envelope as contains_source_envelope;
pub(crate) use framing::project as project_source_bodies;

#[path = "context_sources_skills.rs"]
mod skills;
use skills::SkillSources;

const MAX_REFERENCE_DEPTH: usize = 3;
// Match retained capture capacity; charge cached reads, output and pending references.
pub(crate) const MAX_SOURCE_BYTES: usize = 32 << 20;

enum Target {
    File(String),
    Skill(String),
    Changes(Vec<String>),
}

struct Source {
    target: Target,
    ranges: Vec<(usize, usize)>,
}

struct SourceBody {
    text: String,
    relative_to: Option<PathUri>,
    expand_references: bool,
}

enum SourceReadError {
    NotFound(String),
    Other(String),
}

impl SourceReadError {
    fn message(&self) -> &str {
        match self {
            Self::NotFound(message) | Self::Other(message) => message,
        }
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
enum ResolvedSource {
    File(PathUri),
    ManagedSkill(String),
}

/// Read selected sources without changing the recipient's reset selection.
pub(crate) async fn acquire_selection(
    session: &Arc<Session>,
    step: &Arc<StepContext>,
    selection: &ReloadSelection,
    cancellation: &CancellationToken,
    outcomes: Option<&mut Vec<SourceAcquisitionResult>>,
) -> CodexResult<Option<String>> {
    acquire_sources(
        session,
        step,
        selection.sources.clone(),
        false,
        cancellation,
        outcomes,
    )
    .await
}

async fn acquire_sources(
    session: &Arc<Session>,
    step: &Arc<StepContext>,
    sources: Vec<String>,
    recipient_message: bool,
    cancellation: &CancellationToken,
    outcomes: Option<&mut Vec<SourceAcquisitionResult>>,
) -> CodexResult<Option<String>> {
    if sources.is_empty() {
        return Ok(None);
    }
    check_capacity(sources.iter().map(String::len).sum())?;
    // Dropping a native read cannot install a partially acquired replacement context.
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(CodexErr::TurnAborted),
        body = read_sources(session, step, sources, recipient_message, cancellation, outcomes) => {
            let body = body?;
            if body.is_empty() { return Ok(None); }
            let body = format!(
                "Source content (tool output). These source bodies are already read context; reread only missing or changed content. Failed reads are not complete context.\n{body}"
            );
            check_capacity(body.len())?;
            Ok(Some(body))
        }
    }
}

async fn read_sources(
    session: &Arc<Session>,
    step: &Arc<StepContext>,
    sources: Vec<String>,
    recipient_message: bool,
    cancellation: &CancellationToken,
    mut outcomes: Option<&mut Vec<SourceAcquisitionResult>>,
) -> CodexResult<String> {
    let environment = resolve_tool_environment(&step.environments, None);
    let fallback_cwd = PathUri::from(step.turn.config.cwd.clone());
    let cwd = environment.as_ref().map_or(&fallback_cwd, |env| env.cwd());
    let skills = HostSkillsSnapshot::for_step(&step.extension_data, &step.turn.extension_data);
    let file_sources = environment.as_ref().map(|environment| {
        (
            step.fs_accessor(environment),
            SkillSources::new(step, environment, skills.as_deref()),
        )
    });
    let mut pending_bytes: usize = sources.iter().map(String::len).sum();
    let mut pending = sources
        .into_iter()
        .map(|source| (source, cwd.clone(), 1))
        .collect::<VecDeque<_>>();
    let mut cached_bytes = 0usize;
    let mut files: HashMap<ResolvedSource, Result<SourceBody, SourceReadError>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut changes_seen = HashSet::new();
    let mut omitted_change = false;
    let mut output = String::new();
    let mut bodies = Vec::new();
    // Breadth-first acquisition encounters the shortest reference depth first, so cycles and
    // repeated references need neither rereads nor a second expansion pass.
    while let Some((reference, base, depth)) = pending.pop_front() {
        pending_bytes -= reference.len();
        if cancellation.is_cancelled() {
            return Err(CodexErr::TurnAborted);
        }
        let parsed = parse_reference(&reference);
        let target = parsed.as_ref().ok().map(|source| &source.target);
        let resolved_file = match target {
            Some(Target::File(path)) => Some(base.join(path)),
            _ => None,
        };
        let mut append = |result: Result<&str, &str>| -> CodexResult<()> {
            match result {
                Ok(body) => {
                    append_body(&mut output, &reference, body)?;
                    bodies.push((output.len() - body.len() - 1, output.len() - 1));
                }
                Err(error) => append_failure(&mut output, &reference, error)?,
            }
            if let Some(outcomes) = outcomes.as_deref_mut() {
                let target = match target {
                    Some(Target::File(path)) => AcquiredSource::File {
                        path: resolved_file
                            .as_ref()
                            .and_then(|resolved| resolved.as_ref().ok())
                            .map(|path| {
                                path.to_abs_path()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .unwrap_or_else(|_| path.to_string())
                            })
                            .unwrap_or_else(|| path.clone()),
                    },
                    Some(Target::Skill(name)) => AcquiredSource::Skill { name: name.clone() },
                    Some(Target::Changes(selection)) => AcquiredSource::Changes {
                        selection: selection.clone(),
                    },
                    None => AcquiredSource::Selection,
                };
                outcomes.push(SourceAcquisitionResult {
                    reference: reference.clone(),
                    target,
                    success: result.is_ok(),
                    diagnostic: result.err().map(str::to_string),
                });
            }
            Ok(())
        };
        let source = match &parsed {
            Ok(source) => source,
            Err(error) => {
                append(Err(error))?;
                continue;
            }
        };
        if let Target::Changes(ids) = &source.target {
            if !changes_seen.insert(ids.clone()) {
                continue;
            }
            if depth > MAX_REFERENCE_DEPTH {
                append(Err("nested source depth limit 3 exceeded"))?;
                continue;
            }
            match crate::session::capture_delivery::read_change_source(
                session.session_id(),
                ids.clone(),
                recipient_message.then_some(session.thread_id),
            )
            .await
            {
                Ok(body) if recipient_message && body.is_empty() => omitted_change = true,
                Ok(body) => {
                    append(Ok(&body))?;
                }
                Err(error) => {
                    append(Err(&error))?;
                }
            }
            continue;
        }
        let (access, skill_sources) = file_sources
            .as_ref()
            .map_err(|error| CodexErr::Fatal(format!("Reload acquisition: {error}")))?;
        let is_skill = matches!(&source.target, Target::Skill(_));
        // Reject deep skill references before resolving them.
        if is_skill && depth > MAX_REFERENCE_DEPTH {
            append(Err("nested source depth limit 3 exceeded"))?;
            continue;
        }
        let resolved = if let Some(path) = &resolved_file {
            path.as_ref()
                .map(|path| ResolvedSource::File(path.clone()))
                .map_err(|error| error.to_string())
        } else {
            let Target::Skill(skill) = &source.target else {
                unreachable!("changes were handled above");
            };
            if skill
                .split_once('/')
                .is_some_and(|(_, path)| path.rsplit('/').next() == Some("AGENTS.md"))
            {
                continue;
            }
            skill_sources.resolve(access, skill).await
        };
        let resolved = match resolved {
            Ok(path) => path,
            Err(error) => {
                append(Err(&error))?;
                continue;
            }
        };
        if matches!(&resolved, ResolvedSource::File(path) if path.basename().as_deref() == Some("AGENTS.md"))
            || !seen.insert((resolved.clone(), is_skill, source.ranges.clone()))
        {
            continue;
        }
        if depth > MAX_REFERENCE_DEPTH {
            append(Err("nested source depth limit 3 exceeded"))?;
            continue;
        }
        let body = match files.entry(resolved.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let result = match &resolved {
                    ResolvedSource::ManagedSkill(skill) => skill_sources
                        .read(access, skill)
                        .await
                        .map_err(SourceReadError::Other),
                    ResolvedSource::File(path) => read_file_checked(access, path).await,
                };
                cached_bytes = cached_bytes.saturating_add(match &result {
                    Ok(body) => body.text.len(),
                    Err(error) => error.message().len(),
                });
                check_capacity(cached_bytes)?;
                entry.insert(result)
            }
        };
        let acquired = match body {
            Ok(body) => body,
            Err(SourceReadError::NotFound(_)) if depth > 1 => {
                continue;
            }
            Err(error) => {
                append(Err(error.message()))?;
                continue;
            }
        };
        let body = match selected_lines(&acquired.text, &source.ranges) {
            Ok(body) => body,
            Err(error) => {
                append(Err(&error))?;
                continue;
            }
        };
        append(Ok(&body))?;
        let nested_base = acquired.relative_to.as_ref().unwrap_or(cwd);
        for nested in if acquired.expand_references {
            references(&body)
        } else {
            Vec::new()
        } {
            pending_bytes = pending_bytes.saturating_add(nested.len());
            check_capacity(pending_bytes)?;
            pending.push_back((nested, nested_base.clone(), depth + 1));
        }
    }
    if omitted_change && output.is_empty() {
        return Ok(output);
    }
    let result = framing::envelope(&output, &bodies);
    check_capacity(result.len())?;
    Ok(result)
}

fn expands_references(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        ["md", "txt", "json"]
            .iter()
            .any(|kind| ext.eq_ignore_ascii_case(kind))
    })
}

async fn read_file(access: &dyn EnvironmentAccess, path: &PathUri) -> Result<SourceBody, String> {
    read_file_checked(access, path)
        .await
        .map_err(|error| error.message().to_string())
}

async fn read_file_checked(
    access: &dyn EnvironmentAccess,
    path: &PathUri,
) -> Result<SourceBody, SourceReadError> {
    let metadata = access
        .get_metadata(path, GetMetadataOptions::default())
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SourceReadError::NotFound(error.to_string())
            } else {
                SourceReadError::Other(error.to_string())
            }
        })?;
    if metadata.is_directory {
        return directory::read(access, path)
            .await
            .map(|text| SourceBody {
                text,
                relative_to: None,
                expand_references: false,
            })
            .map_err(SourceReadError::Other);
    }
    if !metadata.is_file {
        return Err(SourceReadError::Other(
            "source is not a regular file".to_string(),
        ));
    }
    if metadata.size > MAX_SOURCE_BYTES as u64 {
        return Err(SourceReadError::Other(
            "source exceeds retained capture capacity".to_string(),
        ));
    }
    let mut stream = access
        .read_file_stream(path)
        .await
        .map_err(|error| SourceReadError::Other(error.to_string()))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| SourceReadError::Other(error.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
            return Err(SourceReadError::Other(
                "source exceeds retained capture capacity".to_string(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes)
        .map(|text| SourceBody {
            text,
            relative_to: path.parent(),
            expand_references: path.basename().as_deref().is_some_and(expands_references),
        })
        .map_err(|error| SourceReadError::Other(error.to_string()))
}

fn selected_lines<'a>(
    body: &'a str,
    ranges: &[(usize, usize)],
) -> Result<std::borrow::Cow<'a, str>, String> {
    if ranges.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(body));
    }
    // Store only requested boundaries, not one pointer for every line in a large file.
    let mut offsets = BTreeMap::new();
    let mut last = 0;
    for &(start, end) in ranges {
        offsets.insert(start - 1, 0);
        offsets.insert(end, 0);
        last = last.max(end);
    }
    let mut position = 0;
    let mut lines = 0;
    for line in body.split_inclusive('\n').take(last) {
        position += line.len();
        lines += 1;
        if let Some(offset) = offsets.get_mut(&lines) {
            *offset = position;
        }
    }
    let mut selected = String::new();
    for &(start, end) in ranges {
        if end > lines {
            return Err(format!(
                "requested lines {start}-{end}; source has {lines} lines"
            ));
        }
        if !selected.is_empty() && !selected.ends_with('\n') {
            selected.push('\n');
        }
        let slice = &body[offsets[&(start - 1)]..offsets[&end]];
        if selected.len().saturating_add(slice.len()) > MAX_SOURCE_BYTES {
            return Err("selected source ranges exceed retained capture capacity".to_string());
        }
        selected.push_str(slice);
    }
    Ok(std::borrow::Cow::Owned(selected))
}

pub(crate) fn frame_source_body(source: &str, body: &str) -> CodexResult<String> {
    let mut output = String::new();
    append_body(&mut output, source, body)?;
    let end = output.len() - 1;
    Ok(framing::envelope(&output, &[(end - body.len(), end)]))
}

fn append_body(output: &mut String, source: &str, body: &str) -> CodexResult<()> {
    check_capacity(
        output
            .len()
            .saturating_add(source.len() * 6)
            .saturating_add(body.len())
            .saturating_add(16),
    )?;
    let source = serde_json::to_string(source).expect("source reference is a string");
    output.push_str(&format!("\nSource {source}:\n{body}\n"));
    Ok(())
}

fn append_failure(output: &mut String, source: &str, error: &str) -> CodexResult<()> {
    check_capacity(
        output
            .len()
            .saturating_add(source.len() * 6)
            .saturating_add(error.len())
            .saturating_add(48),
    )?;
    let source = serde_json::to_string(source).expect("source reference is a string");
    let error = serde_json::to_string(error).expect("source failure is a string");
    output.push_str(&format!(
        "\nSource {source}: CONTENT NOT ATTACHED: {error}\n"
    ));
    Ok(())
}

pub(crate) fn first_invalid_reference(sources: &[String]) -> Option<usize> {
    sources
        .iter()
        .position(|reference| parse_reference(reference).is_err())
}

fn parse_reference(reference: &str) -> Result<Source, String> {
    let invalid = || format!("invalid source reference {reference:?}");
    if reference.contains(['\0', '\r', '\n']) {
        return Err(invalid());
    }
    if let Some(changes) = reference.strip_prefix("mchanges:") {
        let ids = changes.split(',').map(str::to_string).collect::<Vec<_>>();
        for id in &ids {
            let (first, last) = id
                .split_once("..")
                .map_or((id.as_str(), None), |(first, last)| (first, Some(last)));
            if !change_id(first, false) || last.is_some_and(|last| !change_id(last, true)) {
                return Err(invalid());
            }
        }
        return Ok(Source {
            target: Target::Changes(ids),
            ranges: Vec::new(),
        });
    }
    let (target, suffix) = if let Some(path) = reference.strip_prefix('@') {
        let (path, suffix) = if path.starts_with(['\'', '"']) {
            let (decoded, end) = quoted(path)?;
            (decoded, &path[end..])
        } else {
            let (path, suffix) = split_ranges(path);
            (path.to_string(), suffix)
        };
        if path.is_empty() || path.starts_with('@') || path.contains(['\0', '\r', '\n']) {
            return Err(invalid());
        }
        (Target::File(path), suffix)
    } else if let Some(skill) = reference.strip_prefix('$') {
        let (skill, suffix) = split_ranges(skill);
        if skill.is_empty()
            || skill.starts_with('-')
            || skill
                .chars()
                .any(|c| c.is_whitespace() || "\\\"'`$#()[]{}<>,;".contains(c))
            || skill.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(invalid());
        }
        (Target::Skill(skill.to_string()), suffix)
    } else {
        return Err(invalid());
    };
    let mut ranges = Vec::new();
    if !suffix.is_empty() {
        for span in suffix.strip_prefix('#').ok_or_else(invalid)?.split(',') {
            let span = span.strip_prefix('L').ok_or_else(invalid)?;
            let (first, last) = span.split_once("-L").unwrap_or((span, span));
            let start = first.parse::<usize>().map_err(|_| invalid())?;
            let end = last.parse::<usize>().map_err(|_| invalid())?;
            if start == 0 || end < start || start.to_string() != first || end.to_string() != last {
                return Err(invalid());
            }
            ranges.push((start, end));
        }
    }
    Ok(Source { target, ranges })
}

fn split_ranges(reference: &str) -> (&str, &str) {
    reference
        .rfind("#L")
        .map_or((reference, ""), |index| reference.split_at(index))
}

fn change_id(id: &str, abbreviated: bool) -> bool {
    let prefix = id.bytes().take_while(u8::is_ascii_lowercase).count();
    let number = &id[prefix..];
    (prefix > 0 || abbreviated)
        && number.starts_with(|c: char| ('1'..='9').contains(&c))
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn quoted(text: &str) -> Result<(String, usize), String> {
    if let Some(rest) = text.strip_prefix('\'') {
        let end = rest
            .find('\'')
            .ok_or_else(|| "unclosed source path quote".to_string())?;
        return Ok((rest[..end].to_string(), end + 2));
    }
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<String>();
    let value = stream
        .next()
        .ok_or_else(|| "missing quoted source path".to_string())?
        .map_err(|error| error.to_string())?;
    Ok((value, stream.byte_offset()))
}

// In scanned text, reserve shell-style uppercase names without changing explicit selections.
fn is_environment_reference(reference: &str) -> bool {
    let Some(skill) = reference.strip_prefix('$') else {
        return false;
    };
    let (skill, _) = split_ranges(skill);
    let name = skill.split('/').next().unwrap_or_default();
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Recognize source references in text, not bare paths, escaped markers, email addresses or shell vars.
fn references(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let tail = &text[index..];
        let marker = text.as_bytes()[index];
        let changes = tail.starts_with("mchanges:");
        if !matches!(marker, b'@' | b'$') && !changes {
            index += tail.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if let Some(previous) = text[..index].chars().next_back()
            && !previous.is_whitespace()
            && !"\"'`([{ :".contains(previous)
        {
            index += 1;
            continue;
        }
        let mut exact_json = false;
        if index > 0
            && text.as_bytes()[index - 1] == b'"'
            && let Ok((decoded, size)) = quoted(&text[index - 1..])
        {
            exact_json = true;
            if parse_reference(&decoded).is_ok() {
                if !is_environment_reference(&decoded) {
                    found.push(decoded);
                }
                index += size - 1;
                continue;
            }
        }
        let start = index;
        let mut end = index + if changes { "mchanges:".len() } else { 1 };
        if end == text.len()
            || marker == b'@' && text.as_bytes()[end] == b'@'
            || marker == b'$' && b"${(".contains(&text.as_bytes()[end])
        {
            index = end;
            continue;
        }
        if marker == b'@' && matches!(text.as_bytes()[end], b'\'' | b'"') {
            match quoted(&text[end..]) {
                Ok((_, size)) => end += size,
                Err(_) => {
                    index = end + 1;
                    continue;
                }
            }
            if !text[end..].starts_with("#L") {
                found.push(text[start..end].to_string());
                index = end;
                continue;
            }
        }
        let mut ranged = false;
        while end < text.len() {
            ranged |= text[end..].starts_with("#L");
            let Some(character) = text[end..].chars().next() else {
                break;
            };
            if character.is_whitespace()
                || "\"'`()[]{}<>;".contains(character)
                || character == ',' && !changes && !ranged
            {
                break;
            }
            end += character.len_utf8();
        }
        index = end.max(start + 1);
        while !exact_json && end > start + 1 && b".,:!?".contains(&text.as_bytes()[end - 1]) {
            if matches!(&text[start..end], "@." | "@..") {
                break;
            }
            if changes && (text[start..end].ends_with("..") || end == start + "mchanges:".len()) {
                break;
            }
            end -= 1;
        }
        if end > start + 1 && !is_environment_reference(&text[start..end]) {
            found.push(text[start..end].to_string());
        }
    }
    found
}

fn check_capacity(bytes: usize) -> CodexResult<()> {
    if bytes > MAX_SOURCE_BYTES {
        return Err(CodexErr::InvalidRequest(
            "source acquisition exceeds retained capture capacity; select smaller ranges"
                .to_string(),
        ));
    }
    Ok(())
}

/// Preserve the original attributed message and append its referenced source content.
pub(crate) async fn acquire_message(
    session: &Arc<Session>,
    step: &Arc<StepContext>,
    message: &str,
    cancellation: &CancellationToken,
) -> CodexResult<String> {
    let Some(sources) =
        acquire_sources(session, step, references(message), true, cancellation, None).await?
    else {
        return Ok(message.to_string());
    };
    check_capacity(
        message
            .len()
            .saturating_add(sources.len())
            .saturating_add(2),
    )?;
    Ok(format!("{message}\n\n{sources}"))
}

pub(crate) fn has_message_sources(message: &str) -> bool {
    !references(message).is_empty()
}

pub(crate) fn message_skill_sources(message: &str) -> impl Iterator<Item = String> {
    references(message).into_iter().filter_map(|reference| {
        match parse_reference(&reference).ok()?.target {
            Target::Skill(skill) => Some(skill),
            Target::File(_) | Target::Changes(_) => None,
        }
    })
}

/// Acquire sources at accepted user-input recording, separately from user-authored text.
#[tracing::instrument(level = "trace", skip_all)]
pub(crate) fn record_user_sources<'a>(
    session: &'a Arc<Session>,
    turn: &'a Arc<crate::session::turn_context::TurnContext>,
    message: &'a str,
) -> futures::future::BoxFuture<'a, CodexResult<()>> {
    Box::pin(async move {
        let cancellation = CancellationToken::new();
        let _cancel_on_drop = cancellation.clone().drop_guard();
        let step = session
            .capture_speculative_step_context(Arc::clone(turn), &cancellation)
            .await?;
        let selection = ReloadSelection {
            sources: references(message),
        };
        if let Some(body) =
            acquire_selection(session, &step, &selection, &cancellation, None).await?
        {
            record_source_output(session, &step, "attachments", body).await;
        }
        Ok(())
    })
}

/// Final replies select retained changes, not another acquisition of file/skill sources.
pub(crate) fn completion_sources(message: &str) -> Vec<String> {
    references(message)
        .into_iter()
        .filter(|source| source.starts_with("mchanges:"))
        .collect()
}

/// Retain preload as source output, not user text or a replacement reset selection.
pub(crate) async fn preload(
    session: &Arc<Session>,
    step: &Arc<StepContext>,
    selection: &ReloadSelection,
    cancellation: &CancellationToken,
) -> CodexResult<()> {
    let Some(body) = acquire_selection(session, step, selection, cancellation, None).await? else {
        return Ok(());
    };
    record_source_output(session, step, "preload", body).await;
    Ok(())
}

async fn record_source_output(
    session: &Arc<Session>,
    step: &Arc<StepContext>,
    name: &str,
    body: String,
) {
    use codex_history::ResponseItemEnvelope;
    use codex_protocol::items::FunctionCallOutputItem;
    use codex_protocol::items::TurnItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ResponseItem;

    let tokens = codex_utils_output_truncation::approx_token_count(&body);
    let id = codex_protocol::ResponseItemId::with_suffix("context", uuid::Uuid::new_v4());
    let output = FunctionCallOutputPayload::from_text(body);
    let lifecycle = TurnItem::FunctionCallOutput(FunctionCallOutputItem {
        id: id.to_string(),
        name: name.to_string(),
        namespace: Some("context".to_string()),
        output: output.body.clone(),
    });
    let mut item = ResponseItemEnvelope::new(ResponseItem::FunctionCallOutput {
        id: Some(id),
        call_id: None,
        name: Some(name.to_string()),
        namespace: Some("context".to_string()),
        output,
        internal_chat_message_metadata_passthrough: None,
    });
    item.metadata
        .get_or_insert_default()
        .history_truncation_token_limit = Some(tokens);
    session
        .record_annotated_conversation_items(&step.turn, &step.settings.model_info, vec![item])
        .await;
    session.emit_turn_item_started(&step.turn, &lifecycle).await;
    session
        .emit_turn_item_completed(&step.turn, lifecycle)
        .await;
}

#[cfg(test)]
#[path = "context_sources_tests.rs"]
mod recording_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use codex_history::RetainedContextEvent;
    use codex_login::CodexAuth;
    use codex_utils_absolute_path::AbsolutePathBuf;

    pub(super) async fn fixture() -> (tempfile::TempDir, Arc<Session>, Arc<StepContext>) {
        let directory = tempfile::tempdir().unwrap();
        let (session, turn, _events) =
            crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
                CodexAuth::from_api_key("test"),
                Vec::new(),
                |config| {
                    config.cwd = AbsolutePathBuf::from_absolute_path(directory.path()).unwrap();
                },
            )
            .await;
        (directory, session, StepContext::for_test(turn))
    }

    #[test]
    fn message_references_leave_escaped_and_bare_paths_literal() {
        assert_eq!(
            references(
                r#"Read @a.rs#L2-L4,L8 @"with spaces.txt" $skill/references/a.md mchanges:amber1..3. \@literal \$literal bare.rs user@example.com @@other"#
            ),
            vec![
                "@a.rs#L2-L4,L8",
                "@\"with spaces.txt\"",
                "$skill/references/a.md",
                "mchanges:amber1..3"
            ]
        );
        assert!(!has_message_sources(
            r"Use bare.rs and \@literal and \$skill"
        ));
    }

    #[test]
    fn message_references_leave_uppercase_environment_variables_literal() {
        let text = r#"Read $SESSION_TMP/check.rs. $HOME `$_CACHE2/log` $ROOT#L2 and $preview/references/A.md $Mixed_2 ["$SESSION_TMP/check.rs","$PATH","$guide/references/a.md"]"#;
        assert_eq!(
            references(text),
            [
                "$preview/references/A.md",
                "$Mixed_2",
                "$guide/references/a.md"
            ]
        );
        assert!(!has_message_sources(r#"["$SESSION_TMP/check.rs"]"#));
        assert!(matches!(
            parse_reference("$SESSION_TMP/check.rs").unwrap().target,
            Target::Skill(name) if name == "SESSION_TMP/check.rs"
        ));
    }

    #[test]
    fn completion_sources_select_only_explicit_retained_ranges() {
        assert_eq!(
            completion_sources(
                r"Delivered mchanges:amber1,amber3..7 and @file $skill; \mchanges:literal1 is literal."
            ),
            vec!["mchanges:amber1,amber3..7"],
        );
        assert_eq!(
            completion_sources("mchanges:amber1.."),
            vec!["mchanges:amber1.."]
        );
        assert!(completion_sources("No captures: amber1 @file $skill").is_empty());
    }

    #[tokio::test]
    async fn explicit_acquisition_freezes_sources_errors_and_keeps_reset_selection() {
        let (directory, session, step) = fixture().await;
        std::fs::write(
            directory.path().join("first.json"),
            "FIRST @second.md @AGENTS.md",
        )
        .unwrap();
        std::fs::write(directory.path().join("second.md"), "SECOND @third.txt").unwrap();
        std::fs::write(directory.path().join("third.txt"), "THIRD @fourth").unwrap();
        std::fs::write(directory.path().join("fourth"), "EXCLUDED_DEPTH").unwrap();
        std::fs::write(directory.path().join("AGENTS.md"), "HOST_ONLY").unwrap();
        let retained = ReloadSelection {
            sources: vec!["@retained".to_string()],
        };
        session
            .record_retained_context(RetainedContextEvent::ReloadSelection {
                selection: retained.clone(),
            })
            .await;
        let message = "Read @first.json @missing. Keep \\@literal and bare.rs unchanged.";
        let frozen = acquire_message(&session, &step, message, &CancellationToken::new())
            .await
            .unwrap();
        std::fs::write(directory.path().join("first.json"), "CHANGED").unwrap();
        std::fs::write(directory.path().join("missing"), "LATER").unwrap();
        assert!(frozen.starts_with(message));
        for expected in [
            "FIRST",
            "SECOND",
            "THIRD",
            "CONTENT NOT ATTACHED",
            "depth limit",
        ] {
            assert!(frozen.contains(expected), "{frozen}");
        }
        for excluded in ["CHANGED", "LATER", "EXCLUDED_DEPTH", "HOST_ONLY"] {
            assert!(!frozen.contains(excluded));
        }
        assert_eq!(session.reload_selection().await, Some(retained));
    }

    #[tokio::test]
    async fn selected_lines_resolve_nested_sources_beside_the_selected_file() {
        let (directory, session, step) = fixture().await;
        std::fs::create_dir(directory.path().join("sub")).unwrap();
        std::fs::write(
            directory.path().join("sub/first.md"),
            "EXCLUDED @not-selected\nINCLUDED @second.rs\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("sub/second.rs"),
            "BESIDE_SOURCE @ignored",
        )
        .unwrap();
        let text = acquire_message(
            &session,
            &step,
            "Read @sub/first.md#L2 @sub/first.md#L9",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(text.contains("INCLUDED"));
        assert!(text.contains("BESIDE_SOURCE"));
        assert!(text.contains("requested lines 9-9"));
        assert!(!text.contains(r#"Source "@ignored""#));
        assert!(!text.contains("EXCLUDED"));
        assert!(!text.contains("not-selected"));
    }

    #[tokio::test]
    async fn nested_missing_at_tokens_in_markdown_are_silently_skipped() {
        let (directory, session, step) = fixture().await;
        std::fs::write(
            directory.path().join("guide.md"),
            "Use @workspace, @types/react, @starting-style, and @real.txt.\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("real.txt"), "REAL_CONTENT").unwrap();
        let text = acquire_message(
            &session,
            &step,
            "Read @guide.md @explicit-missing.txt",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(text.contains("REAL_CONTENT"));
        assert!(text.contains(r#"Source "@explicit-missing.txt": CONTENT NOT ATTACHED"#));
        for ignored in ["@workspace", "@types/react", "@starting-style"] {
            assert!(
                !text.contains(&format!(r#"Source "{ignored}""#)),
                "unexpected nested failure for {ignored}: {text}"
            );
        }
    }

    #[tokio::test]
    async fn cancellation_and_capacity_failure_publish_no_partial_sources() {
        let (directory, session, step) = fixture().await;
        std::fs::write(directory.path().join("first"), "SOURCE").unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(
            acquire_message(&session, &step, "Read @first", &cancelled)
                .await
                .is_err()
        );
        assert!(check_capacity(MAX_SOURCE_BYTES + 1).is_err());
        let mut output = "x".repeat(MAX_SOURCE_BYTES - 10);
        assert!(append_body(&mut output, "@first", "SOURCE").is_err());
        assert_eq!(output.len(), MAX_SOURCE_BYTES - 10);
        assert_eq!(session.reload_selection().await, None);
    }

    #[tokio::test]
    async fn preload_is_source_output_and_keeps_large_content_in_native_history() {
        let (directory, session, step) = fixture().await;
        let content = format!("{}END_OF_PRELOAD", "content ".repeat(5000));
        std::fs::write(directory.path().join("large"), &content).unwrap();
        preload(
            &session,
            &step,
            &ReloadSelection {
                sources: vec!["@large".to_string()],
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let history = session.clone_history().await;
        let serialized = serde_json::to_string(&history.raw_items().collect::<Vec<_>>()).unwrap();
        assert!(serialized.contains("END_OF_PRELOAD"));
        assert!(serialized.contains("preload"));
        assert_eq!(session.reload_selection().await, None);
    }
    #[tokio::test]
    async fn recipient_permissions_fail_closed_without_sandbox_runtime_paths() {
        use crate::config::PermissionProfileSnapshot;
        use crate::environment_selection::TurnEnvironmentState;
        use codex_protocol::models::PermissionProfile;
        use codex_protocol::permissions::FileSystemAccessMode;
        use codex_protocol::permissions::FileSystemPath;
        use codex_protocol::permissions::FileSystemSandboxEntry;
        let (directory, session, mut step) = fixture().await;
        std::fs::write(directory.path().join("secret.env"), "DENIED_BYTES").unwrap();
        let profile = PermissionProfile::read_only();
        let mut policy = profile.file_system_sandbox_policy();
        policy.entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::GlobPattern {
                pattern: format!("{}/*.env", directory.path().display()),
            },
            access: FileSystemAccessMode::Deny,
            missing_path_behavior: None,
        });
        let TurnEnvironmentState::Ready(environment) =
            &mut Arc::get_mut(&mut step).unwrap().environments.environments[0]
        else {
            panic!("ready recipient environment");
        };
        environment.config_mut().permission_profile = PermissionProfileSnapshot::legacy(
            PermissionProfile::from_runtime_permissions_with_enforcement(
                profile.enforcement(),
                &policy,
                profile.network_sandbox_policy(),
            ),
        );
        let frozen = acquire_message(
            &session,
            &step,
            "Read @secret.env",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(frozen.contains("CONTENT NOT ATTACHED"));
        assert!(!frozen.contains("DENIED_BYTES"));
    }
}

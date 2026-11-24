//! Derive optional recall at window installation, never on the per-request path.
//! The native turn projection respects rewind and fork boundaries in saved history.
use super::CompactedHistoryInstallation;
use super::Session;
use codex_app_server_protocol::ThreadItemPayload;
use codex_history::AttachmentRecallCheckpoint;
use codex_history::ResponseItemEnvelope;
use codex_protocol::SessionId;
use codex_protocol::models::{ContentItem, ImageReference, ResponseItem, input_text};
use codex_protocol::user_input::UserInput;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

const SELECTIONS: &str = "\n\n---\nUser selections:\n";

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Recall {
    #[serde(skip_serializing_if = "IndexSet::is_empty")]
    selections: IndexSet<String>,
    #[serde(skip_serializing_if = "IndexSet::is_empty")]
    sources: IndexSet<String>,
    #[serde(skip_serializing_if = "IndexSet::is_empty")]
    images: IndexSet<String>,
}

impl Recall {
    fn text(&mut self, text: &str, multipart: bool) {
        if let Some(frames) = crate::context_manager::attachment_frames(text) {
            for frame in frames {
                if frame.starts_with("Selected text for ") || frame.starts_with("<selected-text ") {
                    self.selections.insert(frame);
                } else if let Some(path) = frame.strip_prefix("Attached file ").and_then(quoted) {
                    self.sources.insert(format!("@{path}"));
                } else if let Some(name) = crate::context_manager::legacy_skill_name(&frame) {
                    self.sources.insert(format!("${name}"));
                }
            }
        } else if multipart && text.starts_with(SELECTIONS) {
            self.selections.insert(text.to_owned());
        } else {
            self.sources.extend(
                crate::context_sources::references(text)
                    .into_iter()
                    .filter(|source| source.starts_with(['@', '$'])),
            );
        }
    }

    fn image(&mut self, image: &ImageReference) {
        let source = match image {
            ImageReference::File { file_id } => file_id.clone(),
            ImageReference::Inline { image_url } if image_url.starts_with("data:") => {
                let mime = image_url
                    .split_once(',')
                    .map(|(header, _)| header.split(';').next().unwrap_or("data:"))
                    .unwrap_or("data:");
                format!("{mime} (embedded image; pixels not retained)")
            }
            ImageReference::Inline { image_url } => image_url.clone(),
        };
        self.images.insert(source);
    }

    fn user(&mut self, parts: Vec<UserInput>) {
        let multipart = parts.len() > 1;
        for part in parts {
            match part {
                UserInput::Text { text, .. } => self.text(&text, multipart),
                UserInput::LocalImage { path, .. } => {
                    self.images.insert(path.display().to_string());
                }
                UserInput::Image { image, .. } => self.image(&image),
                UserInput::Skill { name, .. } => {
                    self.sources.insert(format!("${name}"));
                }
                _ => {}
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.selections.is_empty() && self.sources.is_empty() && self.images.is_empty()
    }

    fn publish(
        mut self,
        previous: Option<AttachmentRecallCheckpoint>,
        session_id: SessionId,
    ) -> anyhow::Result<(AttachmentRecallCheckpoint, Option<ResponseItemEnvelope>)> {
        if let Some(AttachmentRecallCheckpoint::Published {
            session_id: owner,
            digest,
        }) = previous
        {
            let path = recall_path(owner, &digest);
            if self.is_empty() {
                return Ok((
                    AttachmentRecallCheckpoint::Published {
                        session_id: owner,
                        digest,
                    },
                    Some(recall_hint(&path)),
                ));
            }
            let bytes = std::fs::read(&path)?;
            anyhow::ensure!(
                <[u8; 32]>::from(Sha256::digest(&bytes)) == digest,
                "attachment recall changed"
            );
            let mut retained: Self = serde_yaml::from_slice(&bytes)?;
            retained.selections.extend(self.selections);
            retained.sources.extend(self.sources);
            retained.images.extend(self.images);
            self = retained;
        }
        if self.is_empty() {
            return Ok((AttachmentRecallCheckpoint::Empty, None));
        }
        let body = serde_yaml::to_string(&self)?;
        let digest = Sha256::digest(body.as_bytes()).into();
        let path = recall_path(session_id, &digest);
        let directory = crate::exec_env::session_tmp_path(session_id);
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(body.as_bytes())?;
        let mut permissions = file.as_file().metadata()?.permissions();
        permissions.set_readonly(true);
        file.as_file().set_permissions(permissions)?;
        match file.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
        Ok((
            AttachmentRecallCheckpoint::Published { session_id, digest },
            Some(recall_hint(&path)),
        ))
    }
}

fn recall_path(session_id: SessionId, digest: &[u8; 32]) -> PathBuf {
    let id: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    crate::exec_env::session_tmp_path(session_id).join(format!("attachments-{id}.yaml"))
}

fn recall_hint(path: &std::path::Path) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Message {
        id: None,
        role: "user".to_owned(),
        content: vec![input_text(format!(
            "Optional attachment recall (read with mcat): {}",
            path.display()
        ))],
        status: None,
        encrypted_content: None,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

fn quoted(text: &str) -> Option<String> {
    serde_json::Deserializer::from_str(text)
        .into_iter::<String>()
        .next()?
        .ok()
}

impl Session {
    #[tracing::instrument(level = "trace", skip_all)]
    pub(super) async fn attachment_recall(
        &self,
        installation: &CompactedHistoryInstallation<'_>,
        items: &mut Vec<ResponseItemEnvelope>,
    ) -> anyhow::Result<()> {
        let (history, len, was_compacted) = match installation {
            CompactedHistoryInstallation::Complete(metadata, trace) => {
                let history = match trace {
                    Some((_, captured)) => (*captured).clone(),
                    None => self.clone_history().await,
                };
                let len = history.annotated_items().len();
                (history, len, metadata.window_number > 1)
            }
            CompactedHistoryInstallation::Prefix(prefix) => {
                let state = self.state.lock().await;
                (
                    state.clone_history(),
                    prefix.prefix.len,
                    state.auto_compact_window_number() > 0,
                )
            }
        };
        let Some(prefix) = history.annotated_items().get(..len) else {
            return Ok(());
        };
        let mut previous = prefix
            .iter()
            .find_map(|item| item.metadata.as_ref()?.attachment_recall.clone());
        // Session scratch may have been cleaned between runs. A missing file
        // is rebuilt from admitted history, never by opening the source paths.
        if let Some(AttachmentRecallCheckpoint::Published { session_id, digest }) = &previous
            && !tokio::fs::try_exists(recall_path(*session_id, digest)).await?
        {
            previous = None;
        }
        let mut recall = Recall::default();
        // Only older checkpoints lack recall receipts. Use native rollback-aware
        // history for their one-time upgrade, never for ordinary new windows.
        if previous.is_none()
            && was_compacted
            && let Some(thread) = self.live_thread()
        {
            thread.flush().await?;
            let history = thread.load_history(false).await?;
            for turn in codex_app_server_protocol::build_turns_from_rollout_items(&history.items) {
                for item in turn.items {
                    if let ThreadItemPayload::UserMessage { content, .. } = item.payload {
                        recall.user(content.into_iter().map(|part| part.into_core()).collect());
                    }
                }
            }
        }
        {
            // Legacy UI events flatten text parts. The admitted model history
            // preserves their exact boundaries and also covers ephemeral forks.
            for envelope in prefix {
                if let ResponseItem::Message {
                    role,
                    content,
                    internal_chat_message_metadata_passthrough: Some(metadata),
                    ..
                } = &envelope.item
                    && role == "user"
                    && metadata.content_item_kinds.as_ref().is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|kind| kind.0 == "user.text" || kind.0 == "user.image")
                    })
                {
                    for (index, part) in content.iter().enumerate() {
                        match part {
                            ContentItem::InputText { text } => recall.text(text, content.len() > 1),
                            ContentItem::InputImage { image, .. } => {
                                let local_path = index.checked_sub(1).and_then(|i| match &content[i] {
                                    ContentItem::InputText { text } if codex_protocol::models::is_local_image_open_tag_text(text) =>
                                        text.split_once(" path=\"")?.1.strip_suffix("\">"),
                                    _ => None,
                                });
                                if let Some(path) = local_path {
                                    recall.images.insert(path.to_owned());
                                } else {
                                    recall.image(image);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        let (checkpoint, hint) = if recall.is_empty()
            && !matches!(previous, Some(AttachmentRecallCheckpoint::Published { .. }))
        {
            (AttachmentRecallCheckpoint::Empty, None)
        } else {
            let session_id = self.session_id();
            tokio::task::spawn_blocking(move || recall.publish(previous, session_id)).await??
        };
        let cutoff = crate::context_manager::sampled_through(prefix);
        items.retain_mut(|envelope| {
            let metadata = envelope.metadata.as_ref();
            if matches!(metadata.and_then(|m| m.attachment_recall.as_ref()), Some(AttachmentRecallCheckpoint::Published { .. })) {
                return false;
            }
            if metadata.and_then(|m| m.history_sequence).is_some_and(|n| n <= cutoff)
                && let ResponseItem::Message { role, content, internal_chat_message_metadata_passthrough, .. } = &mut envelope.item
                && role == "user" {
                let len = content.len();
                content.retain(|part| !matches!(part, ContentItem::InputText { text } if crate::context_manager::attachment_frames(text).is_some() || (len > 1 && text.starts_with(SELECTIONS))));
                if len != content.len() {
                    *internal_chat_message_metadata_passthrough = None;
                    return !content.is_empty();
                }
            }
            true
        });
        if let Some(hint) = hint {
            items.push(hint);
        }
        if let Some(item) = items.last_mut() {
            item.metadata.get_or_insert_default().attachment_recall = Some(checkpoint);
        }
        Ok(())
    }
}

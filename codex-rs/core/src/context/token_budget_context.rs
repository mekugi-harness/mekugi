use super::ContextualUserFragment;
use super::world_state::PreviousSectionState;
use super::world_state::WorldStateSection;
use crate::context::world_state::SectionTransition;
use crate::context::world_state::WorldStateUpdate;
use codex_context_fragments::AnnotatedContent;
use codex_context_fragments::message_from_parts;
use codex_prompts::ResolvedModelMessages;
use codex_protocol::AgentPath;
use codex_protocol::ContextWindowId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::ContentItemMetadata;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::input_text;
use codex_protocol::protocol::CONTEXT_WINDOW_CLOSE_TAG;
use codex_protocol::protocol::CONTEXT_WINDOW_GUIDANCE_CLOSE_TAG;
use codex_protocol::protocol::CONTEXT_WINDOW_GUIDANCE_OPEN_TAG;
use codex_protocol::protocol::CONTEXT_WINDOW_OPEN_TAG;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenBudgetContext {
    agent_path: AgentPath,
    first_window_id: ContextWindowId,
    previous_window_id: Option<ContextWindowId>,
    window_id: ContextWindowId,
}

impl TokenBudgetContext {
    pub(crate) fn new(
        agent_path: AgentPath,
        first_window_id: ContextWindowId,
        previous_window_id: Option<ContextWindowId>,
        window_id: ContextWindowId,
    ) -> Self {
        Self {
            agent_path,
            first_window_id,
            previous_window_id,
            window_id,
        }
    }

    /// Wraps tool hints without losing their source or changing the concatenated prompt text.
    pub(crate) fn render_with_hints(&self, hints: Vec<AnnotatedContent>) -> ResponseItem {
        let (open, close) = self.markers();
        let annotate = |content| {
            AnnotatedContent::from_parts(content, self.content_kind(), self.content_metadata())
        };
        let mut content = vec![annotate(input_text(open)), annotate(self.body())];
        for hint in hints {
            content.push(hint);
            content.push(annotate(input_text("\n")));
        }
        content.push(annotate(input_text(close)));
        message_from_parts(self.role(), content)
    }
}

impl ContextualUserFragment for TokenBudgetContext {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("token_budget.context_window".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (CONTEXT_WINDOW_OPEN_TAG, CONTEXT_WINDOW_CLOSE_TAG)
    }

    fn body(&self) -> ContentItem {
        let first_window_id = &self.first_window_id;
        let window_id = &self.window_id;
        let mut lines = vec![
            format!("Agent name: {}", self.agent_path),
            format!("First context window id: {first_window_id}"),
            format!("Current context window id: {window_id}"),
        ];
        if let Some(previous_window_id) = &self.previous_window_id {
            lines.push(format!("Previous context window id: {previous_window_id}"));
        }
        input_text(format!("\n{}\n", lines.join("\n")))
    }
}

impl WorldStateSection for TokenBudgetContext {
    const ID: &'static str = "context_window";
    type Snapshot = AgentPath;

    fn render_diff(
        &self,
        previous: PreviousSectionState<'_, Self::Snapshot>,
    ) -> SectionTransition<Self::Snapshot> {
        let current = self.agent_path.clone();
        let fragment = matches!(previous, PreviousSectionState::Known(agent_path) if agent_path != &self.agent_path)
            .then(|| WorldStateUpdate::fragment(self.clone()).standalone());
        (Some(current), fragment.into_iter().collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextWindowGuidance {
    message: String,
}

impl ContextWindowGuidance {
    pub(crate) fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

impl ContextualUserFragment for ContextWindowGuidance {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("token_budget.context_window_guidance".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            CONTEXT_WINDOW_GUIDANCE_OPEN_TAG,
            CONTEXT_WINDOW_GUIDANCE_CLOSE_TAG,
        )
    }

    fn body(&self) -> ContentItem {
        input_text(format!("\n{}\n", self.message))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenBudgetRemainingContext {
    tokens_left: Option<i64>,
}

impl TokenBudgetRemainingContext {
    pub(crate) fn new(tokens_left: i64) -> Self {
        Self {
            tokens_left: Some(tokens_left),
        }
    }

    pub(crate) fn unknown() -> Self {
        Self { tokens_left: None }
    }
}

impl ContextualUserFragment for TokenBudgetRemainingContext {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("token_budget.remaining_tokens".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> ContentItem {
        input_text(match self.tokens_left {
            Some(tokens_left) => {
                format!("You have {tokens_left} tokens left in this context window.")
            }
            None => "You have unknown tokens left in this context window.".to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenBudgetReminder {
    message: String,
    usage_level: Option<i64>,
}

impl TokenBudgetReminder {
    pub(crate) fn new(message_template: &str, n_remaining: i64) -> Self {
        Self {
            message: message_template.replace("{n_remaining}", &n_remaining.to_string()),
            usage_level: None,
        }
    }

    pub(crate) const USAGE_KIND_PREFIX: &str = "context_usage.notice.";

    pub(crate) fn context_usage(percent: i64, tokens: i64, window: i64) -> Self {
        let urgent = if percent >= 90 {
            let reminder = Self::new(
                ResolvedModelMessages::bundled().token_budget_reminder_template(),
                window.saturating_sub(tokens).max(0),
            );
            format!(" {} You must compact now.", reminder.message)
        } else {
            String::new()
        };
        Self {
            message: format!(
                "Mekugi context usage: {percent}% ({tokens}/{window} tokens).{urgent}"
            ),
            usage_level: Some(percent / 10 * 10),
        }
    }
}

impl ContextualUserFragment for TokenBudgetReminder {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind(match self.usage_level {
            Some(level) => format!("{}{level}", Self::USAGE_KIND_PREFIX),
            None => "token_budget.reminder".to_string(),
        })
    }

    fn content_metadata(&self) -> ContentItemMetadata {
        if self.usage_level.is_some() {
            ContentItemMetadata::developer_instructions(false)
        } else {
            ContentItemMetadata::harness()
        }
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> ContentItem {
        input_text(self.message.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoCompactFallbackPrompt {
    message: String,
}

impl AutoCompactFallbackPrompt {
    pub(crate) fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

impl ContextualUserFragment for AutoCompactFallbackPrompt {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("compaction.auto_fallback_prompt".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> ContentItem {
        input_text(self.message.clone())
    }
}

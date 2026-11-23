use super::PreviousSectionState;
use super::SectionTransition;
use super::WorldStateSection;
use super::WorldStateUpdate;
use crate::agents_md::LoadedAgentsMd;
use crate::context::ContextualUserFragment;
use crate::context::UserInstructions;
use serde::Deserialize;
use serde::Serialize;

const REPLACEMENT_NOTICE: &str =
    "These AGENTS.md instructions replace all previously provided AGENTS.md instructions.";
const DIFF_NOTICE: &str = "Apply this diff to the previously provided AGENTS.md instructions:";
const UNCHANGED_NOTICE: &str = "The previously provided AGENTS.md instructions are unchanged.";
const REMOVAL_NOTICE: &str = "The previously provided AGENTS.md instructions no longer apply.";
const MIN_DIFF_SAVING_BYTES: usize = 128;

/// The AGENTS.md instructions currently visible to the model.
#[derive(Clone, Debug, Default)]
pub(crate) struct AgentsMdState {
    instructions: Option<UserInstructions>,
}

/// Persisted model-visible AGENTS.md state, without filesystem provenance.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct AgentsMdSnapshot {
    directory: Option<String>,
    text: Option<String>,
}

impl AgentsMdState {
    pub(crate) fn new(loaded: Option<&LoadedAgentsMd>) -> Self {
        Self {
            instructions: loaded.map(LoadedAgentsMd::contextual_user_fragment),
        }
    }
}

impl WorldStateSection for AgentsMdState {
    const ID: &'static str = "agents_md";
    type Snapshot = AgentsMdSnapshot;

    fn matches_legacy_fragment(role: &str, text: &str) -> bool {
        role == "user" && UserInstructions::matches_text(text)
    }

    fn render_diff(
        &self,
        previous: PreviousSectionState<'_, Self::Snapshot>,
    ) -> SectionTransition<Self::Snapshot> {
        let current = match &self.instructions {
            Some(instructions) => AgentsMdSnapshot {
                directory: instructions.directory.clone(),
                text: Some(instructions.text.clone()),
            },
            None => AgentsMdSnapshot::default(),
        };
        if matches!(previous, PreviousSectionState::Known(previous) if previous == &current) {
            return (None, Vec::new());
        }

        let (previous_may_contain_instructions, previous_text) = match previous {
            PreviousSectionState::Known(previous) => {
                (previous.text.is_some(), previous.text.as_deref())
            }
            PreviousSectionState::Unknown => (true, None),
            PreviousSectionState::Absent => (false, None),
        };
        let instructions = match (&self.instructions, previous_may_contain_instructions) {
            (Some(instructions), true) => UserInstructions {
                directory: instructions.directory.clone(),
                text: render_updated_instructions(previous_text, &instructions.text),
            },
            (Some(instructions), false) => instructions.clone(),
            (None, true) => UserInstructions {
                directory: None,
                text: REMOVAL_NOTICE.to_string(),
            },
            (None, false) => return (Some(current), Vec::new()),
        };
        (
            Some(current),
            vec![WorldStateUpdate::fragment(instructions)],
        )
    }
}

fn render_updated_instructions(previous_text: Option<&str>, current_text: &str) -> String {
    if previous_text == Some(current_text) {
        return UNCHANGED_NOTICE.to_string();
    }
    let replacement = format!("{REPLACEMENT_NOTICE}\n\n{current_text}");
    let Some(previous_text) = previous_text else {
        return replacement;
    };
    let diff = similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Patience)
        .diff_lines(previous_text, current_text);
    let changes = diff.unified_diff().context_radius(1).to_string();
    let changes = changes.trim_end_matches('\n');
    if changes.is_empty() {
        return replacement;
    }
    let diff_body = format!("{DIFF_NOTICE}\n\n```diff\n{changes}\n```");
    if diff_body.len().saturating_add(MIN_DIFF_SAVING_BYTES) <= replacement.len() {
        diff_body
    } else {
        replacement
    }
}

#[cfg(test)]
#[path = "agents_md_tests.rs"]
mod tests;

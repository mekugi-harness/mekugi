use codex_protocol::config_types::ModeKind;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_protocol::request_user_input::RequestUserInputQuestionOption;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const REQUEST_USER_INPUT_TOOL_NAME: &str = "request_user_input";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestUserInputToolArgs {
    pub questions: Vec<QuestionInput>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct QuestionInput {
    pub question: String,
    pub header: Option<String>,
    pub options: Option<Vec<RequestUserInputQuestionOption>>,
}

impl QuestionInput {
    pub(crate) fn with_id(self, id: String) -> RequestUserInputQuestion {
        RequestUserInputQuestion {
            id,
            header: self.header,
            question: self.question,
            options: self.options,
            is_other: true,
            is_secret: false,
        }
    }
}

pub(crate) fn question_parameters() -> JsonSchema {
    let option = JsonSchema::object(
        BTreeMap::from([
            ("label".into(), JsonSchema::string(Some("Short user-facing label.".into()))),
            ("description".into(), JsonSchema::string(Some("Impact or tradeoff of this choice.".into()))),
            ("preview".into(), JsonSchema::string(Some("Inline plain text or Markdown preview; no HTML.".into()))),
            ("recommended".into(), JsonSchema::boolean(Some("Mark at most one option recommended. Preserve option order; do not alter labels.".into()))),
        ]), Some(vec!["label".into()]), Some(false.into()),
    );
    let mut options = JsonSchema::array(option, Some("Suggested choices in original order. The UI adds Other; omit options for free text only.".into()));
    options.min_items = Some(1);
    let question = JsonSchema::object(
        BTreeMap::from([
            (
                "question".into(),
                JsonSchema::string(Some("Complete question shown to the user.".into())),
            ),
            (
                "header".into(),
                JsonSchema::string(Some("Optional short UI heading.".into())),
            ),
            ("options".into(), options),
        ]),
        Some(vec!["question".into()]),
        Some(false.into()),
    );
    let mut questions = JsonSchema::array(
        question,
        Some("One or more questions. Runtime assigns session-unique IDs.".into()),
    );
    questions.min_items = Some(1);
    JsonSchema::object(
        BTreeMap::from([("questions".into(), questions)]),
        Some(vec!["questions".into()]),
        Some(false.into()),
    )
}

pub fn create_request_user_input_tool(description: String) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: REQUEST_USER_INPUT_TOOL_NAME.into(),
        description,
        strict: false,
        defer_loading: None,
        parameters: question_parameters(),
        output_schema: None,
    })
}

pub fn request_user_input_unavailable_message(
    mode: ModeKind,
    available_modes: &[ModeKind],
) -> Option<String> {
    if available_modes.contains(&mode) {
        None
    } else {
        let mode_name = mode.display_name();
        Some(format!(
            "request_user_input is unavailable in {mode_name} mode"
        ))
    }
}

pub(crate) fn normalize_request_user_input_tool_args(
    args: RequestUserInputToolArgs,
) -> Result<RequestUserInputToolArgs, String> {
    if args.questions.is_empty() {
        return Err("questions must not be empty".into());
    }
    for question in &args.questions {
        if question.question.trim().is_empty() {
            return Err("questions must contain non-empty question text".into());
        }
        if let Some(options) = &question.options {
            if options.is_empty() || options.iter().any(|option| option.label.trim().is_empty()) {
                return Err("options must contain at least one non-empty label".into());
            }
            if options.iter().filter(|option| option.recommended).count() > 1 {
                return Err("at most one option may be recommended per question".into());
            }
        }
    }
    Ok(args)
}

pub fn request_user_input_tool_description(available_modes: &[ModeKind]) -> String {
    let modes = available_modes
        .iter()
        .map(|mode| mode.display_name())
        .collect::<Vec<_>>()
        .join(" or ");
    format!(
        "Ask questions and wait for answers. Available in {modes} mode. Mark recommendations with recommended: true. Returns ordered compact answers: {{id,choice,note?}} (1-based choice), {{id,other}}, or {{id,skipped:true}}."
    )
}

#[cfg(test)]
#[path = "request_user_input_spec_tests.rs"]
mod tests;

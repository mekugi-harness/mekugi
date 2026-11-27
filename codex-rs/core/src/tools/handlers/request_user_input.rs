use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::request_user_input_spec::REQUEST_USER_INPUT_TOOL_NAME;
use crate::tools::handlers::request_user_input_spec::RequestUserInputToolArgs;
use crate::tools::handlers::request_user_input_spec::create_request_user_input_tool;
use crate::tools::handlers::request_user_input_spec::normalize_request_user_input_tool_args;
use crate::tools::handlers::request_user_input_spec::request_user_input_tool_description;
use crate::tools::handlers::request_user_input_spec::request_user_input_unavailable_message;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_features::Feature;
use codex_history::RetainedContextEvent;
use codex_history::VerifiedAnswer;
use codex_history::VerifiedQuestionAnswer;
use codex_protocol::config_types::ModeKind;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageReference;
use codex_protocol::request_user_input::RequestUserInputArgs;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

pub struct RequestUserInputHandler {
    pub available_modes: Vec<ModeKind>,
}

const ANSWER_IMAGE_UPLOAD_LIMIT: usize = 32 << 20;

impl ToolExecutor<ToolInvocation> for RequestUserInputHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(REQUEST_USER_INPUT_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_request_user_input_tool(request_user_input_tool_description(&self.available_modes))
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl RequestUserInputHandler {
    #[tracing::instrument(skip_all)]
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{REQUEST_USER_INPUT_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        if turn.root_capabilities_restricted {
            return Err(FunctionCallError::RespondToModel(
                "request_user_input can only be used by the root thread".to_string(),
            ));
        }

        let mode = turn.collaboration_mode().mode;
        if let Some(message) = request_user_input_unavailable_message(mode, &self.available_modes) {
            return Err(FunctionCallError::RespondToModel(message));
        }

        let args: RequestUserInputToolArgs = parse_arguments(&arguments)?;
        let args = normalize_request_user_input_tool_args(args)
            .map_err(FunctionCallError::RespondToModel)?;
        let ids = session.allocate_question_ids(args.questions.len()).await;
        let args = RequestUserInputArgs {
            questions: args
                .questions
                .into_iter()
                .zip(ids)
                .map(|(question, id)| question.with_id(id))
                .collect(),
            is_blocking: mode == ModeKind::Plan,
            auto_resolution_ms: None,
        };
        let questions = args.questions.clone();
        let accepted = session
            .request_user_input(turn.as_ref(), call_id.clone(), args)
            .await
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "{REQUEST_USER_INPUT_TOOL_NAME} was cancelled before receiving a response"
                ))
            })?;

        let mut submitted = accepted.response.answers;
        let mut answers = questions
            .iter()
            .map(|question| {
                submitted
                    .iter_mut()
                    .find(|answer| answer.is_valid_for(question))
                    .map(std::mem::take)
                    .unwrap_or_else(
                        || codex_protocol::request_user_input::RequestUserInputAnswer {
                            id: question.id.clone(),
                            skipped: true,
                            ..Default::default()
                        },
                    )
            })
            .collect::<Vec<_>>();

        // Serialize only the compact answers. Attach the image content in
        // question order with its own label rather than putting bytes in text.
        let mut image_content = Vec::new();
        let mut remaining_bytes = ANSWER_IMAGE_UPLOAD_LIMIT;
        for answer in &mut answers {
            let Some(images) = answer.images.take().filter(|images| !images.is_empty()) else {
                continue;
            };
            image_content.push(FunctionCallOutputContentItem::InputText {
                text: format!("Images for answer {}:", answer.id),
            });
            for image_url in images {
                let bytes = inline_answer_image_bytes(&image_url).ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "Question answer images must be inline base64 uploads.".to_string(),
                    )
                })?;
                remaining_bytes = remaining_bytes.checked_sub(bytes).ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "Question answer uploads exceed the 32 MiB limit.".to_string(),
                    )
                })?;
                image_content.push(FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline { image_url },
                    detail: None,
                });
            }
        }
        let content = serde_json::to_string(&answers).map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to serialize {REQUEST_USER_INPUT_TOOL_NAME} response: {err}"
            ))
        })?;
        // Persist answers even while an older checkpoint needs compatibility review.
        if turn.config.features.enabled(Feature::GuardianApproval) {
            let user_input = questions
                .iter()
                .filter_map(|question| {
                    let response = answers.iter().find(|answer| answer.id == question.id)?;
                    let answer = response.readable_answer(question)?;
                    if answer.trim().is_empty() {
                        return None;
                    }
                    let mut question_text = question.question.clone();
                    if let Some(option) = response.selected_option(question) {
                        question_text.push_str(&format!("\n{}", option.label));
                        if let Some(description) = &option.description {
                            question_text.push_str(&format!(": {description}"));
                        }
                    }
                    Some(VerifiedQuestionAnswer {
                        question: question_text,
                        answer,
                    })
                })
                .collect::<Vec<_>>();
            if !user_input.is_empty() {
                session
                    .record_retained_context(RetainedContextEvent::VerifiedAnswer {
                        answer: VerifiedAnswer {
                            turn_id: turn.sub_id.clone(),
                            call_id,
                            questions: user_input,
                        },
                        acceptance_order: Some(accepted.acceptance_order),
                    })
                    .await;
            }
        }

        let mut output = vec![FunctionCallOutputContentItem::InputText { text: content }];
        output.append(&mut image_content);
        Ok(boxed_tool_output(FunctionToolOutput::from_content(
            output,
            /*success*/ Some(true),
        )))
    }
}

fn inline_answer_image_bytes(image_url: &str) -> Option<usize> {
    let (metadata, payload) = image_url.strip_prefix("data:")?.split_once(',')?;
    let mime = metadata.strip_suffix(";base64")?;
    if !(mime.starts_with("image/") || mime == "application/octet-stream")
        || payload.is_empty()
        || !payload.len().is_multiple_of(4)
    {
        return None;
    }
    // The existing image preparation decodes and verifies the actual file.
    let padding = payload.ends_with('=') as usize + payload.ends_with("==") as usize;
    Some(payload.len() / 4 * 3 - padding)
}

impl CoreToolRuntime for RequestUserInputHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "request_user_input_tests.rs"]
mod tests;

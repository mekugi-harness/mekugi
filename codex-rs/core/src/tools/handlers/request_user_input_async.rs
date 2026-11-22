use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::request_user_input_spec::RequestUserInputToolArgs;
use crate::tools::handlers::request_user_input_spec::normalize_request_user_input_tool_args;
use crate::tools::handlers::request_user_input_spec::question_parameters;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::AgentMessageDelivery;
use codex_protocol::items::AgentMessageItem;
use codex_protocol::items::AsyncUserInputQuestion;
use codex_protocol::items::TurnItem;
use codex_protocol::models::MessagePhase;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

const TOOL_NAME: &str = "request_user_input_async";

pub struct RequestUserInputAsyncHandler {
    pub description: String,
}

impl ToolExecutor<ToolInvocation> for RequestUserInputAsyncHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(TOOL_NAME)
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: TOOL_NAME.into(),
            description: self.description.clone(),
            strict: false,
            defer_loading: None,
            parameters: question_parameters(),
            output_schema: None,
        })
    }
    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl RequestUserInputAsyncHandler {
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
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{TOOL_NAME} handler received unsupported payload"
            )));
        };
        let args: RequestUserInputToolArgs = parse_arguments(&arguments)?;
        let args = normalize_request_user_input_tool_args(args)
            .map_err(FunctionCallError::RespondToModel)?;
        let ids = session.allocate_question_ids(args.questions.len()).await;
        let questions = args
            .questions
            .into_iter()
            .zip(&ids)
            .map(|(question, id)| AsyncUserInputQuestion {
                id: id.clone(),
                header: question.header,
                question: question.question,
                options: question.options,
            })
            .collect::<Vec<_>>();
        let text = questions
            .iter()
            .map(|question| {
                let mut lines = vec![question.question.clone()];
                if let Some(options) = &question.options {
                    lines.extend(options.iter().map(|option| format!("- {}", option.label)));
                }
                lines.join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let item = TurnItem::AgentMessage(AgentMessageItem {
            moderation: None,
            id: call_id,
            content: vec![AgentMessageContent::Text { text }],
            phase: Some(MessagePhase::FinalAnswer),
            memory_citation: None,
            delivery: Some(AgentMessageDelivery::Async),
            questions: Some(questions),
        });
        session.emit_turn_item_started(turn.as_ref(), &item).await;
        session.emit_turn_item_completed(turn.as_ref(), item).await;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            serde_json::json!({"ids": ids}).to_string(),
            Some(true),
        )))
    }
}

impl CoreToolRuntime for RequestUserInputAsyncHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

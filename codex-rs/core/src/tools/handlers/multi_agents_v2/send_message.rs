use super::analytics::ToolCallAnalytics;
use super::message_tool::SendMessageArgs;
use super::message_tool::handle_message_string_tool;
use super::message_tool::message_content;
use super::*;
use crate::agent::types::MessageDeliveryMode;
use crate::tools::context::FunctionToolOutput;
use crate::tools::handlers::multi_agents_spec::create_send_message_tool;
use codex_tools::ToolSpec;

pub(crate) struct Handler;

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("send_message")
    }

    fn spec(&self) -> ToolSpec {
        create_send_message_tool()
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let mut analytics = ToolCallAnalytics::new(&invocation, CollabAgentTool::SendMessage);
            let result = self.handle_call(invocation, &mut analytics).await;
            analytics.finish(&result);
            result
        })
    }
}

impl Handler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
        analytics: &mut ToolCallAnalytics,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let arguments = function_arguments(invocation.payload.clone())?;
        let args: SendMessageArgs = parse_arguments(&arguments)?;
        if let Some(target) = super::foreign::target(&invocation, &args.target) {
            use codex_app_server_protocol::{
                AgentForeignOperation as Operation, AgentForeignResponse as Response,
            };
            let message = message_content(args.message)?;
            let sources = crate::compact_token_budget::reload::has_message_sources(&message);
            analytics.set_receiver(target);
            let operation = if args.btw {
                Operation::SideQuestion { question: message }
            } else {
                Operation::Message { message }
            };
            let response = super::foreign::route(&invocation, target, operation).await?;
            let text = match response {
                Response::Message {
                    target_thread_id,
                    accepted: true,
                    ..
                } if !args.btw && target_thread_id == target => String::new(),
                Response::SideQuestion {
                    target_thread_id,
                    answer,
                } if args.btw && target_thread_id == target => answer,
                _ => return Err(super::foreign::invalid_response()),
            };
            invocation
                .session
                .services
                .local_agent_runtime
                .record_feature_use(
                    invocation.session.thread_id,
                    invocation.call_id.clone(),
                    codex_protocol::feature_use::FeatureUseEvent::MessageAccepted {
                        sources,
                        btw: args.btw,
                    },
                );
            return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                Some(true),
            )));
        }
        if args.btw {
            use codex_protocol::feature_use::FeatureUseEvent;
            let runtime = &invocation.session.services.local_agent_runtime;
            let prepared = async {
                let question = message_content(args.message)?;
                let sources = crate::compact_token_budget::reload::has_message_sources(&question);
                let target =
                    resolve_agent_target(&invocation.session, &invocation.turn, &args.target)
                        .await?;
                analytics.set_receiver(target);
                let question = runtime
                    .acquire_recipient_message(target, &question, &invocation.cancellation_token)
                    .await
                    .map_err(|error| collab_v2_agent_error(target, error))?;
                Ok::<_, FunctionCallError>((target, question, sources))
            }
            .await;
            let (target, question, sources) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    runtime.record_feature_use(
                        invocation.session.thread_id,
                        invocation.call_id.clone(),
                        FeatureUseEvent::SideQuestionRejected,
                    );
                    return Err(error);
                }
            };
            runtime.record_feature_use(
                invocation.session.thread_id,
                invocation.call_id.clone(),
                FeatureUseEvent::MessageAccepted { sources, btw: true },
            );
            let answer = runtime
                .answer_recipient_side_question(
                    target,
                    question,
                    None,
                    invocation.cancellation_token,
                )
                .await
                .map_err(|error| {
                    runtime.record_feature_use(
                        invocation.session.thread_id,
                        invocation.call_id.clone(),
                        FeatureUseEvent::SideQuestionFailed,
                    );
                    collab_v2_agent_error(target, error)
                })?;
            return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                answer,
                Some(true),
            )));
        }
        handle_message_string_tool(
            invocation,
            MessageDeliveryMode::QueueOnly,
            args.target,
            args.message,
            analytics,
        )
        .await
        .map(boxed_tool_output)
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

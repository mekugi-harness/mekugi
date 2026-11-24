use super::*;
use crate::tools::handlers::multi_agents_spec::create_inspect_agent_tool;
use codex_app_server_protocol::Thread;
use codex_tools::ToolSpec;

pub(crate) struct Handler;

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("inspect_agent")
    }
    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }
    fn spec(&self) -> ToolSpec {
        create_inspect_agent_tool()
    }
    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let args: InspectAgentArgs =
                parse_arguments(&function_arguments(invocation.payload.clone())?)?;
            if let Some(recipient) = super::foreign::target(&invocation, &args.target).await? {
                let thread = recipient
                    .inspect()
                    .await
                    .map_err(|e| collab_v2_agent_error(recipient.id(), e))?;
                return Ok(boxed_tool_output(InspectAgentResult { thread }));
            }
            let target =
                resolve_agent_target(&invocation.session, &invocation.turn, &args.target).await?;
            let thread = invocation
                .session
                .services
                .local_agent_runtime
                .read_thread_details(target)
                .await
                .map_err(|error| collab_v2_agent_error(target, error))?;
            Ok(boxed_tool_output(InspectAgentResult { thread }))
        })
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectAgentArgs {
    target: String,
}

#[derive(Debug, Serialize)]
struct InspectAgentResult {
    thread: Thread,
}

impl ToolOutput for InspectAgentResult {
    fn log_output(&self) -> String {
        tool_output_json_text(self, "inspect_agent")
    }
    fn success_for_logging(&self) -> bool {
        true
    }
    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "inspect_agent")
    }
    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "inspect_agent")
    }
}

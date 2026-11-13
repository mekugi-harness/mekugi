use super::analytics::ToolCallAnalytics;
use super::*;
use crate::tools::handlers::multi_agents_spec::create_list_agents_tool;
use codex_tools::ToolSpec;

pub(crate) struct Handler;

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("list_agents")
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn spec(&self) -> ToolSpec {
        create_list_agents_tool()
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let analytics = ToolCallAnalytics::new(&invocation, CollabAgentTool::ListAgents);
            let result = self.handle_call(invocation).await;
            analytics.finish(&result);
            result
        })
    }
}

impl Handler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let args: ListAgentsArgs =
            parse_arguments(&function_arguments(invocation.payload.clone())?)?;
        if let Some(target) = args
            .path_prefix
            .as_deref()
            .and_then(|value| super::foreign::target(&invocation, value))
        {
            use codex_app_server_protocol::{AgentForeignOperation, AgentForeignResponse};
            let response = super::foreign::route(
                &invocation,
                target,
                AgentForeignOperation::List { path_prefix: None },
            )
            .await?;
            let AgentForeignResponse::List { threads } = response else {
                return Err(super::foreign::invalid_response());
            };
            let threads: Vec<codex_app_server_protocol::Thread> =
                serde_json::from_value(threads).map_err(|_| super::foreign::invalid_response())?;
            if !threads.iter().any(|agent| agent.id == target.to_string()) {
                return Err(super::foreign::invalid_response());
            }
            let agents = threads
                .into_iter()
                .filter_map(|thread| {
                    thread.agent_status.map(|agent_status| ListedAgent {
                        agent_name: thread.id,
                        agent_status,
                    })
                })
                .collect();
            return Ok(boxed_tool_output(ListAgentsResult { agents }));
        }
        let ToolInvocation { session, turn, .. } = invocation;
        let agents = session
            .services
            .agent_control
            .list(
                session.thread_id,
                turn.parent_thread_id,
                &turn.session_source,
                args.path_prefix.as_deref(),
            )
            .await
            .map_err(collab_spawn_error)?;

        let agents = agents
            .into_iter()
            .map(|agent| ListedAgent {
                agent_name: agent
                    .metadata
                    .agent_path
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| agent.thread_id.to_string()),
                agent_status: agent.status,
            })
            .collect();
        Ok(boxed_tool_output(ListAgentsResult { agents }))
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAgentsArgs {
    path_prefix: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListedAgent {
    agent_name: String,
    agent_status: AgentStatus,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListAgentsResult {
    agents: Vec<ListedAgent>,
}

impl ToolOutput for ListAgentsResult {
    fn log_output(&self) -> String {
        tool_output_json_text(self, "list_agents")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "list_agents")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "list_agents")
    }
}

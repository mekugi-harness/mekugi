use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

/// Lightweight pre-execution observation barrier emitted immediately before
/// dispatching local `exec_command` (`Bash`) or `apply_patch` calls when a
/// capture observer is registered for the session.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ExecutionBeforeToolParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub tool: String,
    pub cwd: String,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_tmp: Option<String>,
}

/// Empty acknowledgement releasing the pre-execution observation barrier early.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ExecutionBeforeToolResponse {}

//! Passive facts about an admitted native tree. Missing observations are null.

use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AgentEndParams {
    pub root_thread_id: String,
    pub thread_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AgentEndResponse {
    pub thread_ids: Vec<String>,
}

/// Authenticated by the originating native runtime, then by the owning host connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AgentForeignOrigin {
    pub root_thread_id: codex_protocol::ThreadId,
    pub actor_id: codex_protocol::ThreadId,
    pub agent_path: codex_protocol::AgentPath,
    pub turn_id: String,
    pub call_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub enum AgentForeignOperation {
    Message {
        message: String,
    },
    SideQuestion {
        question: String,
    },
    List {
        #[serde(rename = "pathPrefix")]
        path_prefix: Option<codex_protocol::AgentPath>,
    },
    Inspect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct AgentForeignRequest {
    pub source: AgentForeignOrigin,
    pub target_thread_id: codex_protocol::ThreadId,
    pub operation: AgentForeignOperation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum AgentForeignResponse {
    Message {
        #[serde(rename = "targetThreadId")]
        target_thread_id: codex_protocol::ThreadId,
        #[serde(rename = "deliveryId")]
        delivery_id: String,
        accepted: bool,
    },
    SideQuestion {
        #[serde(rename = "targetThreadId")]
        target_thread_id: codex_protocol::ThreadId,
        answer: String,
    },
    List {
        /// Native thread records forwarded unchanged; the receiving tool validates them.
        threads: serde_json::Value,
    },
    Inspect {
        /// Native thread record forwarded unchanged; the receiving tool validates it.
        thread: serde_json::Value,
    },
}

#[cfg(test)]
mod foreign_tests {
    use super::*;
    #[test]
    fn foreign_operations_do_not_decode_control_authority() {
        for operation in ["spawn", "followup", "interrupt", "reset", "end"] {
            assert!(
                serde_json::from_value::<AgentForeignOperation>(
                    serde_json::json!({"type": operation})
                )
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<AgentForeignOperation>(
                serde_json::json!({"type":"message", "message":"hello", "triggerTurn":true})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::from_value::<AgentForeignOperation>(
                serde_json::json!({"type":"sideQuestion", "question":"hello"})
            )
            .unwrap(),
            AgentForeignOperation::SideQuestion {
                question: "hello".into()
            }
        );
    }
}

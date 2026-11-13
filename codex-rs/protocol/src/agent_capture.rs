//! Host-owned retained capture selections and bounded output, not coordination state.

use crate::ThreadId;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ChildHostAdmissionRequest {
    pub root_thread_id: ThreadId,
    pub parent_thread_id: ThreadId,
    pub thread_id: ThreadId,
    pub local_cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ChildHostAdmissionResponse {
    pub thread_id: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CaptureCursor {
    pub key: String,
    #[ts(type = "number")]
    pub through_chunk: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CaptureInterval {
    pub key: String,
    pub change_id: String,
    #[ts(type = "number")]
    pub record_order: u64,
    pub legacy_record_key: Option<String>,
    #[ts(type = "number")]
    pub first_chunk: u64,
    #[ts(type = "number")]
    pub last_chunk: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct CaptureCoverage {
    pub status: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ProgressActor {
    pub thread_id: ThreadId,
    pub task_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct FrozenAgentCapture {
    pub actor: ProgressActor,
    pub intervals: Vec<CaptureInterval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PreparedProgressAgent {
    pub actor: ProgressActor,
    pub intervals: Vec<CaptureInterval>,
    pub summary: SummaryResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ProgressFreezeRequest {
    pub root_thread_id: ThreadId,
    pub operation_id: String,
    pub caller_thread_id: ThreadId,
    pub recipient_context_id: String,
    pub selected: Vec<ProgressActor>,
    pub seen: Vec<CaptureCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ProgressFreezeResponse {
    pub selection_ref: String,
    #[ts(type = "number")]
    pub publication_revision: u64,
    pub agents: Vec<FrozenAgentCapture>,
    pub coverage: CaptureCoverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ProgressOutputRequest {
    pub root_thread_id: ThreadId,
    pub operation_id: String,
    pub caller_thread_id: ThreadId,
    pub recipient_context_id: String,
    pub selection_ref: String,
    pub agents: Vec<PreparedProgressAgent>,
    pub coverage: CaptureCoverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ProgressOutputResponse {
    pub text: String,
    pub recovery_ref: Option<String>,
    pub delivered_intervals: Vec<CaptureInterval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct FinalCaptureRequest {
    pub root_thread_id: ThreadId,
    pub thread_id: ThreadId,
    pub operation_id: String,
    pub recipient_thread_id: ThreadId,
    pub recipient_cwd: Option<String>,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct FinalCaptureResponse {
    pub capture: Option<FrozenFinalCapture>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct FrozenFinalCapture {
    pub resource_ref: String,
    pub full_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct FinalCaptureProjectRequest {
    pub root_thread_id: ThreadId,
    pub thread_id: ThreadId,
    pub operation_id: String,
    pub recipient_thread_id: ThreadId,
    pub recipient_cwd: Option<String>,
    pub recipient_context_id: String,
    pub resource_ref: String,
    pub seen: Vec<CaptureCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct FinalCaptureProjectResponse {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum SummaryResult {
    Available { text: String },
    Unavailable { reason: String },
}

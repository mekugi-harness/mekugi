//! Retained change selections and sampled delivery receipts.

use crate::ThreadId;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

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
pub struct ProgressOutputResponse {
    pub text: String,
    pub delivered_intervals: Vec<CaptureInterval>,
}

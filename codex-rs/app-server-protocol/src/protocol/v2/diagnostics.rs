use std::path::PathBuf;

use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ServerDiagnosticsParams {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ServerDiagnosticsResponse {
    pub process: ServerDiagnosticsProcess,
    pub gauges: Vec<ServerDiagnosticsGauge>,
    /// The active native diagnostic log database, if open.
    pub debug_path: Option<PathBuf>,
    /// Model transport attempts across all threads since this process started.
    #[serde(default)]
    #[ts(type = "number")]
    pub provider_requests: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ServerDiagnosticsProcess {
    pub id: u32,
    #[ts(type = "number | null")]
    pub resident_memory_bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub physical_footprint_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ServerDiagnosticsGauge {
    pub name: String,
    #[ts(type = "number")]
    pub value: u64,
}

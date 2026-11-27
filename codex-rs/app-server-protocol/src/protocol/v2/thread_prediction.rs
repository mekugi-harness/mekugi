//! App-server API types for predictions.

use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPredictionRequestParams {
    pub thread_id: String,
    pub source_turn_id: String,
    /// Automatic suggestions require the OpenAI provider and a ChatGPT account.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automatic: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[ts(export_to = "v2/")]
pub struct ThreadPredictionRequestResponse {
    pub text: Option<String>,
}

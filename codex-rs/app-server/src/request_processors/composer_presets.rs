//! The browser can edit a numbered draft, not the configuration that stores it.

use super::ConfigRequestProcessor;
use super::config_write_error;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::ComposerPreset;
use codex_app_server_protocol::ConfigValueWriteParams;
use codex_app_server_protocol::ConfigWriteErrorCode;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::MergeStrategy;
use codex_app_server_protocol::ThreadPresetWriteParams;
use codex_app_server_protocol::ThreadPresetWriteResponse;
use codex_app_server_protocol::ThreadPresetsListParams;
use codex_app_server_protocol::ThreadPresetsListResponse;
use codex_config::types::QuickDial;
use codex_core::config::Config;
use codex_protocol::protocol::ThreadSource;

impl ConfigRequestProcessor {
    #[tracing::instrument(skip_all)]
    pub(crate) async fn list_presets(
        &self,
        params: ThreadPresetsListParams,
    ) -> Result<ThreadPresetsListResponse, JSONRPCErrorError> {
        let config = self.preset_config(&params.thread_id).await?;
        Ok(ThreadPresetsListResponse {
            presets: config
                .tui_quick_dials
                .into_iter()
                .map(|(slot, preset)| (slot, to_preset(preset)))
                .collect(),
        })
    }

    #[tracing::instrument(skip_all)]
    pub(crate) async fn write_preset(
        &self,
        params: ThreadPresetWriteParams,
    ) -> Result<ThreadPresetWriteResponse, JSONRPCErrorError> {
        let ThreadPresetWriteParams {
            thread_id,
            slot,
            expected,
            preset,
        } = params;
        if slot > 9 {
            return Err(invalid_request("Choose a preset slot from 0 to 9."));
        }
        let preset = preset
            .map(|ComposerPreset { input, label }| {
                let label = label.trim();
                if label.contains(['\r', '\n']) {
                    return Err(invalid_request("Preset label must be a single line."));
                }
                let dial = QuickDial {
                    input,
                    label: Some(label.to_owned()),
                };
                dial.validate().map_err(invalid_request)?;
                Ok(dial)
            })
            .transpose()?;

        // Both RPCs share the config queue with the existing configuration writer.
        let config = self.preset_config(&thread_id).await?;
        let key = slot.to_string();
        if config.tui_quick_dials.get(&key).cloned().map(to_preset) != expected {
            return Err(config_write_error(
                ConfigWriteErrorCode::ConfigVersionConflict,
                format!("Preset {slot} changed. Reopen Presets to review it."),
            ));
        }
        let layers = &config.config_layer_stack;
        let user = layers
            .get_active_user_layer()
            .filter(|layer| !layer.is_disabled())
            .ok_or_else(|| invalid_request("There is no active user configuration."))?;
        for layer in layers.layers_high_to_low().filter(|layer| {
            layer
                .config
                .get("tui")
                .and_then(|tui| tui.get("presets"))
                .and_then(|slots| slots.get(&key))
                .is_some()
        }) {
            // A protected slot cannot be overwritten, or exposed by deleting its override.
            if !std::ptr::eq(user, layer) {
                return Err(config_write_error(
                    ConfigWriteErrorCode::ConfigLayerReadonly,
                    format!("Preset {slot} is defined outside the active user configuration."),
                ));
            }
            if preset.is_some() {
                break;
            }
        }
        self.config_manager
            .write_value(ConfigValueWriteParams {
                key_path: format!("tui.presets.{slot}"),
                value: serde_json::json!(preset),
                merge_strategy: MergeStrategy::Replace,
                file_path: None,
                expected_version: Some(user.version.clone()),
            })
            .await
            .map_err(|error| {
                tracing::warn!(%error, "could not write composer preset");
                match error.write_error_code() {
                    Some(ConfigWriteErrorCode::ConfigVersionConflict) => config_write_error(
                        ConfigWriteErrorCode::ConfigVersionConflict,
                        format!("Preset {slot} changed. Reopen Presets to review it."),
                    ),
                    _ => internal_error(format!("Could not update preset {slot}.")),
                }
            })?;
        Ok(ThreadPresetWriteResponse {
            preset: preset.map(to_preset),
        })
    }

    #[tracing::instrument(skip_all)]
    async fn preset_config(&self, thread_id: &str) -> Result<Config, JSONRPCErrorError> {
        let thread = super::super::lookup_loaded_thread(&self.thread_manager, thread_id).await?;
        if thread.session_source().is_non_root_agent()
            || !matches!(
                thread.config_snapshot().await.thread_source,
                None | Some(ThreadSource::User)
            )
        {
            return Err(invalid_request("Open Presets from Main."));
        }
        self.config_manager
            .load_latest_config_with_session_layers(thread.config().await.as_ref())
            .await
            .map_err(|error| {
                tracing::warn!(%error, "could not load composer presets");
                internal_error("Could not read presets from the active configuration.")
            })
    }
}

fn to_preset(dial: QuickDial) -> ComposerPreset {
    ComposerPreset {
        input: dial.input,
        label: dial.label.unwrap_or_default(),
    }
}

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_core::config::Config;
use codex_core::config::CurrentTimeReminderConfig;
use codex_core::config::RolloutBudgetConfig;
use codex_core::config::TokenBudgetConfig;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ExtensionWarning;
use codex_features::Feature;
use codex_protocol::config_types::TrustLevel;
use codex_protocol::models::AudioReference;
use codex_protocol::models::ImageReference;
use codex_protocol::openai_models::InputModality;
use codex_protocol::protocol::AdditionalContextEntry;
use codex_protocol::protocol::AdditionalContextKind;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GuardianAssessmentAction;
use codex_protocol::protocol::GuardianAssessmentEvent;
use codex_protocol::protocol::GuardianAssessmentStatus;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use codex_skills_extension::SkillsExtensionConfig;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_custom_tool_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_skill_identities_are_native_and_not_duplicated() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const BODY: &str = "native selected skill body";
    const IDENTITY: &str = "<skill name=\"native-selection\"/>";
    for extension in [false, true] {
        for managed in [false, true] {
            let server = start_mock_server().await;
            let response = mount_sse_once(&server, sse(vec![ev_completed("resp")])).await;
            let mut builder = test_codex()
                .with_config(|config| {
                    config.include_skill_instructions = false;
                })
                .with_workspace_setup(move |cwd, _| async move {
                    let directory = cwd.join(".agents/skills/native-selection");
                    std::fs::create_dir_all(&directory)?;
                    std::fs::write(
                        directory.join("SKILL.md"),
                        format!(
                            "---\nname: native-selection\ndescription: fixture skill\n---\n{BODY}\n"
                        ),
                    )?;
                    std::fs::write(
                        cwd.join(".skills-mgr.json"),
                        format!(
                            r#"{{"schema_revision":1,"skills":{{"native-selection":{managed}}}}}"#
                        ),
                    )?;
                    Ok(())
                });
            if extension {
                let mut extensions = ExtensionRegistryBuilder::<Config>::new();
                codex_skills_extension::install(&mut extensions, |config| SkillsExtensionConfig {
                    include_instructions: config.include_skill_instructions,
                    catalog_deltas_enabled: false,
                    omit_descriptions: false,
                    max_context_tokens: None,
                    bundled_skills_enabled: false,
                    cloud_skill_enabled: false,
                    shadow_selection_enabled: false,
                });
                builder = builder.with_extensions(Arc::new(extensions.build()));
            }
            let test = builder.build_with_auto_env(&server).await?;
            test.codex
                .start_or_steer_turn(TurnInputRequest::user_input(vec![
                    UserInput::Text {
                        text: "Use the selected skill.".to_owned(),
                        text_elements: Vec::new(),
                    },
                    UserInput::Skill {
                        name: "native-selection".to_owned(),
                        path: test.workspace_path(".agents/skills/native-selection/SKILL.md"),
                    },
                ]))
                .await?;
            let mut recorded = Vec::new();
            wait_for_event(&test.codex, |event| {
                if let EventMsg::RawResponseItem(event) = event {
                    recorded.push(event.item.clone());
                }
                matches!(event, EventMsg::TurnComplete(_))
            })
            .await;
            let request = response.single_request();
            let text = request.message_input_texts("user");
            assert_eq!(
                text.iter().filter(|text| text.as_str() == IDENTITY).count(),
                usize::from(managed)
            );
            assert_eq!(
                text.iter().filter(|text| text.contains(BODY)).count(),
                usize::from(!managed)
            );
            assert!(request.has_content_kinds(&["skills.selected_skill_instructions"]));
            let recorded = serde_json::to_string(&recorded)?;
            assert_eq!(recorded.matches(BODY).count(), usize::from(!managed));
            assert_eq!(
                recorded
                    .matches(r#"<skill name=\"native-selection\"/>"#)
                    .count(),
                usize::from(managed)
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_managed_skill_is_not_replaced_by_a_native_selection() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response = mount_sse_once(&server, sse(vec![ev_completed("resp")])).await;
    let test = test_codex().with_config(|config| {
        config.active_project.trust_level = Some(TrustLevel::Trusted);
    }).with_workspace_setup(|cwd, _| async move {
        for root in [".agents/skills/preferred", ".codex/skills/alternate"] {
            let path = cwd.join(root);
            std::fs::create_dir_all(&path)?;
            std::fs::write(path.join("SKILL.md"),
                "---\nname: scoped-skill\ndescription: alternate description\n---\nalternate instructions\n")?;
            std::fs::write(path.join("guide.md"), "alternate reference")?;
        }
        std::fs::set_permissions(cwd.join(".agents/skills/preferred/SKILL.md"), std::fs::Permissions::from_mode(0))?;
        std::fs::write(cwd.join(".skills-mgr.json"), r#"{"schema_revision":1,"skills":{"scoped-skill":true}}"#)?;
        Ok(())
    }).build_with_auto_env(&server).await?;
    if std::fs::File::open(test.cwd_path().join(".agents/skills/preferred/SKILL.md")).is_ok() {
        return Ok(()); // Privileged hosts can still open a mode-000 file.
    }

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![
            UserInput::Text {
                text: "Read $scoped-skill/guide.md".into(),
                text_elements: Vec::new(),
            },
            UserInput::Skill {
                name: "scoped-skill".into(),
                path: test.workspace_path(".codex/skills/alternate/SKILL.md"),
            },
        ]))
        .await?;
    let mut saw_warning = false;
    wait_for_event(&test.codex, |event| {
        if let EventMsg::Warning(warning) = event {
            saw_warning |= warning.message.contains("Could not resolve selected skill")
                && warning.message.contains("Permission denied");
        }
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let request = response.single_request().body_json().to_string();
    assert!(saw_warning, "no diagnostic for the typed selection");
    assert!(
        request.contains("Permission denied"),
        "missing explicit source failure"
    );
    assert!(!request.contains("alternate reference"));
    assert!(!request.contains("alternate instructions"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_skill_catalog_replaces_stock_without_hook() -> Result<()> {
    check_managed_skill_catalog(TrustLevel::Trusted, /*max_context_tokens*/ None).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_skill_catalog_rejects_overflow() -> Result<()> {
    check_managed_skill_catalog(TrustLevel::Trusted, NonZeroUsize::new(128)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_skill_catalog_does_not_run_untrusted_conditions() -> Result<()> {
    check_managed_skill_catalog(TrustLevel::Untrusted, /*max_context_tokens*/ None).await
}

struct CatalogWarningSink(std::sync::mpsc::Sender<ExtensionWarning>);

impl ExtensionEventSink for CatalogWarningSink {
    fn emit(&self, _event: Event) {}

    fn emit_warning(&self, warning: ExtensionWarning) {
        let _ = self.0.send(warning);
    }
}

async fn check_managed_skill_catalog(
    trust: TrustLevel,
    max_context_tokens: Option<NonZeroUsize>,
) -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        (0..if trust == TrustLevel::Trusted && max_context_tokens.is_none() {
            3
        } else {
            1
        })
            .map(|_| sse(vec![ev_completed("catalog-response")]))
            .collect(),
    )
    .await;
    let (warning_tx, warnings) = std::sync::mpsc::channel();
    let mut extensions = ExtensionRegistryBuilder::<Config>::with_event_sink(Arc::new(
        CatalogWarningSink(warning_tx),
    ));
    codex_skills_extension::install(&mut extensions, move |config| SkillsExtensionConfig {
        include_instructions: config.include_skill_instructions,
        catalog_deltas_enabled: false,
        omit_descriptions: false,
        max_context_tokens,
        bundled_skills_enabled: false,
        cloud_skill_enabled: false,
        shadow_selection_enabled: false,
    });
    let test = test_codex()
        .with_config(move |config| {
            config.active_project.trust_level = Some(trust);
            config.features.enable(Feature::TokenBudget).unwrap();
        })
        .with_workspace_setup(move |cwd, _| async move {
            let stock = cwd.join(".codex/skills/stale-stock");
            std::fs::create_dir_all(&stock)?;
            std::fs::write(
                stock.join("SKILL.md"),
                "---\nname: stale-stock\ndescription: stock catalog description\n---\nstale local body\n",
            )?;
            let current = cwd.join(".agents/skills/stale-stock");
            std::fs::create_dir_all(&current)?;
            std::fs::write(
                current.join("SKILL.md"),
                "---\nname: stale-stock\ndescription: managed description\n---\ncurrent managed body\n",
            )?;
            let exclusive = cwd.join(".agents/skills/manager-only");
            std::fs::create_dir_all(exclusive.join("references"))?;
            let description = if max_context_tokens.is_some() {
                format!("exclusive description {}", "long ".repeat(200))
            } else {
                "exclusive description".to_owned()
            };
            std::fs::write(
                exclusive.join("SKILL.md"),
                format!("---\nname: manager-only\ndescription: {description}\n---\n"),
            )?;
            std::fs::write(
                exclusive.join("references/sample.md"),
                "manager-only reference body\n",
            )?;
            std::fs::write(exclusive.join("references/other.md"), "another reference\n")?;
            let conditional = cwd.join(".agents/skills/conditional-catalog");
            std::fs::create_dir_all(&conditional)?;
            std::fs::write(
                conditional.join("SKILL.md"),
                "---\nname: conditional-catalog\ndescription: command-enabled skill\n---\n",
            )?;
            let condition = if cfg!(unix) {
                "true && touch managed-condition-ran"
            } else {
                "true"
            };
            std::fs::write(
                cwd.join(".skills-mgr.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema_revision": 3,
                    "skills": {
                        "manager-only": {"enabled": true},
                        "conditional-catalog": {"enabled": condition},
                    },
                }))?,
            )?;
            Ok(())
        })
        .with_extensions(Arc::new(extensions.build()))
        .build(&server)
        .await?;
    if trust == TrustLevel::Untrusted {
        test.submit_turn("Continue without selecting a skill.")
            .await?;
        assert!(!test.cwd_path().join("managed-condition-ran").exists());
        let requests = responses.requests();
        assert_eq!(requests.len(), 1);
        let developers = requests[0].message_input_texts("developer").join("\n");
        assert!(developers.contains("### Available skills"));
        assert!(!developers.contains("## Host skills (replaces any previous host catalog)"));
        return Ok(());
    }
    if max_context_tokens.is_some() {
        test.submit_turn("Continue without selecting a skill.")
            .await?;
        let warnings = warnings.try_iter().collect::<Vec<_>>();
        assert!(
            warnings.iter().any(|warning| warning
                .message
                .contains("skills-mgr catalog exceeds the skills context budget")
                && warning.message.contains("No managed catalog was included")),
            "missing budget warning: {warnings:?}"
        );
        let requests = responses.requests();
        assert_eq!(requests.len(), 1);
        let developers = requests[0].message_input_texts("developer").join("\n");
        assert!(!developers.contains("<skills>"));
        assert!(!developers.contains("managed description"));
        assert!(!developers.contains("stock catalog description"));
        return Ok(());
    }

    test.submit_turn("Read $stale-stock and $manager-only/references/sample.md.")
        .await?;
    test.submit_turn("Continue without selecting any skill.")
        .await?;
    test.codex.submit(Op::Compact { reload: None }).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    test.submit_turn("Continue after reset.").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        let developers = request.message_input_texts("developer").join("\n");
        assert_eq!(developers.matches("exclusive description").count(), 1);
        assert!(developers.contains("<references>references/{other,sample}.md</references>"));
        assert!(developers.contains("command-enabled skill"));
        assert!(developers.contains("`skills-mgr get NAME`"));
        assert!(developers.contains("`skills-mgr get NAME/FILE`"));
        assert!(developers.contains("`skills-mgr run NAME/SCRIPT [args...]`"));
        assert!(!developers.contains("stock catalog description"));
    }
    if cfg!(unix) {
        assert!(test.cwd_path().join("managed-condition-ran").exists());
    }
    let selected = requests[0].body_json();
    let outputs = selected["input"]
        .as_array()
        .expect("responses input")
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .filter_map(|item| item["output"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(outputs.matches("current managed body").count(), 1);
    assert_eq!(outputs.matches("manager-only reference body").count(), 1);
    assert!(!selected.to_string().contains("stale local body"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_deduplication_projects_line_runs_within_one_tool_result() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let block: String = (0..6)
        .map(|line| format!("entry {line}: {}\n", "a stable literal".repeat(3)))
        .collect();
    let body = format!("{block}separator\n{block}");
    let code = format!("text({});", serde_json::to_string(&body)?);
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_custom_tool_call("repeated", "exec", &code),
                ev_completed("resp-1"),
            ]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.features.enable(Feature::CodeMode).unwrap();
        })
        .build_with_auto_env(&server)
        .await?;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Print the repeated entries.".to_owned(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut raw = Vec::new();
    wait_for_event(&test.codex, |event| {
        if let EventMsg::RawResponseItem(event) = event {
            raw.push(event.item.clone());
        }
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let raw = serde_json::to_value(raw)?;
    let raw = raw
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "custom_tool_call_output" && item["call_id"] == "repeated")
        .expect("raw output remains available");
    let projected = requests[1].custom_tool_call_output("repeated");
    let original = raw["output"].to_string();
    let projected = projected["output"].to_string();
    assert_eq!(original.matches("entry 0:").count(), 2);
    assert!(!original.contains("[identical to"));
    assert_eq!(projected.matches("entry 0:").count(), 1);
    assert!(projected.contains("[identical to call c1#L"), "{projected}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_deduplication_projects_sources_only_in_model_requests() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let body = "original source evidence must stay available in raw history\n".repeat(16);
    for name in ["first.txt", "second.txt"] {
        std::fs::write(test.workspace_path(name), &body)?;
    }
    let input = "Read @first.txt and @second.txt.";
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: input.to_owned(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let mut raw = Vec::new();
    wait_for_event(&test.codex, |event| {
        if let EventMsg::RawResponseItem(event) = event {
            raw.push(event.item.clone());
        }
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let raw = serde_json::to_value(raw)?;
    let originals = raw
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.pointer("/output").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(originals.matches(&body).count(), 2);
    assert!(!originals.contains("[identical to"));

    std::fs::write(
        test.workspace_path("first.txt"),
        "not reacquired on continuation",
    )?;
    test.submit_text_turn("Continue without new references.")
        .await?;
    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let items = request.input();
        assert!(
            items
                .iter()
                .any(|item| item["content"].as_array().is_some_and(|parts| {
                    parts
                        .iter()
                        .any(|part| part["text"].as_str() == Some(input))
                }))
        );
        let sources = items
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(sources.matches(&body).count(), 1);
        assert!(sources.contains("[identical to @first.txt above]"));
        assert!(!sources.contains("not reacquired"));
        assert!(!sources.contains("[call"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_request_item_types_roles_and_content_annotations() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;
    let test = test_codex()
        .with_model_info_override("gpt-5.5", |model_info| {
            model_info.input_modalities.push(InputModality::Audio);
        })
        .with_config(|config| {
            config.update_plan_enabled = true;
            config.developer_instructions = Some("Keep world-state annotations aligned.".into());
            config.model_context_window = Some(128_000);
            config.current_time_reminder = Some(CurrentTimeReminderConfig::default());
            config.token_budget = Some(TokenBudgetConfig {
                guidance_message: Some("Preserve important context.".into()),
                ..TokenBudgetConfig::default()
            });
            config.rollout_budget = Some(RolloutBudgetConfig {
                enforce_limit: true,
                limit_tokens: 100,
                reminder_at_remaining_tokens: Vec::new(),
                sampling_token_weight: 1.0,
                prefill_token_weight: 1.0,
            });
            config.multi_agent_v2.root_agent_usage_hint_text =
                Some("Coordinate available subagents.".into());
            config.multi_agent_v2.multi_agent_mode_hint_text =
                Some("Delegate independent work.".into());
            for feature in [
                Feature::CurrentTimeReminder,
                Feature::DeferredExecutor,
                Feature::MultiAgentV2,
                Feature::TokenBudget,
            ] {
                config
                    .features
                    .enable(feature)
                    .expect("test config should allow feature update");
            }
        })
        .build_with_auto_env(&server)
        .await?;

    test.codex
        .submit(Op::ApproveGuardianDeniedAction {
            event: GuardianAssessmentEvent {
                review_reason: None,
                model_context: None,
                id: "guardian-review".to_string(),
                target_item_id: None,
                plugin_id: None,
                script_path: None,
                turn_id: "guardian-turn".to_string(),
                started_at_ms: 0,
                completed_at_ms: Some(1),
                status: GuardianAssessmentStatus::Denied,
                risk_level: None,
                user_authorization: None,
                rationale: None,
                decision_source: None,
                action: GuardianAssessmentAction::McpToolCall {
                    server: "example".to_string(),
                    tool_name: "write".to_string(),
                    connector_id: None,
                    connector_name: None,
                    tool_title: None,
                },
            },
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::RawResponseItem(_))
    })
    .await;

    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![
                UserInput::Text {
                    text: "inspect world state".to_string(),
                    text_elements: Vec::new(),
                },
                UserInput::Image {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==".to_string(),
                    },
                    detail: None,
                },
                UserInput::Audio { audio: AudioReference::Inline { audio_url: "data:audio/wav;base64,AAAA".to_string() } },
            ])
            .with_additional_context(BTreeMap::from([
                (
                    "browser_info".to_string(),
                    AdditionalContextEntry::new(
                        "tab one".to_string(),
                        AdditionalContextKind::Untrusted,
                    ),
                ),
                (
                    "automation_info".to_string(),
                    AdditionalContextEntry::new(
                        "run one".to_string(),
                        AdditionalContextKind::Application,
                    ),
                ),
            ])),
        )
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = response.single_request();
    assert!(request.has_content_kinds(&["guardian.approved_action"]));
    let mut guardian_item = request
        .input()
        .into_iter()
        .nth(1)
        .expect("guardian approval should follow the base instructions");
    guardian_item
        .as_object_mut()
        .expect("guardian approval should be an object")
        .remove("id");
    let guardian_metadata = guardian_item["internal_chat_message_metadata_passthrough"]
        .as_object_mut()
        .expect("guardian approval should have passthrough metadata");
    guardian_metadata.remove("turn_id");
    guardian_metadata.remove("create_time");
    let approved_action = serde_json::to_string_pretty(&serde_json::json!({
        "action": {
            "type": "mcp_tool_call",
            "server": "example",
            "tool_name": "write",
            "connector_id": null,
            "connector_name": null,
            "tool_title": null,
        },
        "outcome": "allowed",
    }))?;
    assert_eq!(
        guardian_item,
        serde_json::json!({
            "type": "message",
            "role": "developer",
            "content": [{
                "type": "input_text",
                "text": format!(
                    "The user has manually approved a specific action that was previously `Rejected`.\n\n\
                     Treat this as approval to perform that exact action in the same context in which it was originally requested.\n\
                     Do not assume this also authorizes similar operations with different payloads.\n\n\
                     Approved action:\n\
                     {approved_action}"
                ),
            }],
            "internal_chat_message_metadata_passthrough": {
                "content_item_kinds": ["guardian.approved_action"],
            },
        })
    );

    let items = request
        .input()
        .into_iter()
        .map(|item| {
            let item_type = item["type"].as_str().expect("response item type");
            let role = item["role"].as_str().unwrap_or("-");
            let content_annotations =
                &item["internal_chat_message_metadata_passthrough"]["content_item_kinds"];
            format!("{item_type} {role} {content_annotations}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(items, @r#"
    message developer ["model.base_instructions"]
    message developer ["guardian.approved_action"]
    message developer ["generic.developer_instructions","token_budget.context_window_guidance","permissions.instructions","environments.instructions"]
    message developer ["token_budget.context_window","token_budget.context_window","token_budget.context_window"]
    message developer ["multi_agent.usage_hint"]
    message developer ["multi_agent.mode_instructions"]
    message user ["environments.environment_context"]
    message developer ["additional_content.automation_info"]
    function_call_output - null
    message user ["user.text","user.image","user.audio"]
    message developer ["rollout_budget.remaining_tokens"]
    message developer ["current_time.reminder"]
    "#);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_item_kinds_are_omitted_when_feature_disabled() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.developer_instructions = Some("Keep other metadata intact.".into());
            config
                .features
                .disable(Feature::ContentItemKinds)
                .expect("test config should allow ContentItemKinds override");
        })
        .build_with_auto_env(&server)
        .await?;

    test.submit_text_turn("inspect request metadata").await?;

    let input = response.single_request().input();
    assert!(input.iter().all(|item| {
        item.pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
            .is_none()
    }));
    assert!(input.iter().any(|item| {
        item.pointer("/internal_chat_message_metadata_passthrough/turn_id")
            .is_some()
    }));

    Ok(())
}

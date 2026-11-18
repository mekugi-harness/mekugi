use super::*;
use codex_tools::JsonSchemaPrimitiveType;
use codex_tools::JsonSchemaType;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn spawn_agent_tool_v2_requires_task_name() {
    let tool = create_spawn_agent_tool_v2(
        SpawnAgentToolOptions {
            agent_type_description: "role help".to_string(),
            expose_agent_type: true,
            hide_agent_type_model_reasoning: false,
            expose_spawn_agent_model_overrides: true,
            usage_hint_text: None,
            ..Default::default()
        },
        /*description_override*/ None,
    );

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");
    assert!(description.contains("Spawns an agent to work on the specified task."));
    assert!(description.contains("The spawned agent will have the same tools as you"));
    assert!(!description.contains("max_concurrent_threads_per_session"));
    assert!(description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE));
    assert!(properties.contains_key("task_name"));
    assert!(properties.contains_key("message"));
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        Some(true)
    );
    assert!(properties.contains_key("fork_turns"));
    assert!(properties.contains_key("cwd"));
    let preload = properties
        .get("preload")
        .expect("fresh-child preload schema");
    assert_eq!(
        preload.required.as_deref(),
        Some(["sources".to_string()].as_slice())
    );
    let sources = preload.properties.as_ref().unwrap().get("sources").unwrap();
    assert_eq!(
        sources.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Array))
    );
    assert!(!properties.contains_key("items"));
    assert!(!properties.contains_key("fork_context"));
    assert_eq!(
        properties
            .get("model")
            .and_then(|schema| schema.description.as_deref()),
        Some(SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION)
    );
    assert_eq!(
        properties
            .get("reasoning_effort")
            .and_then(|schema| schema.description.as_deref()),
        Some("Reasoning effort override for the new agent. Omit to inherit the parent effort.")
    );
    assert!(!properties.contains_key("service_tier"));
    assert_eq!(
        parameters.required.as_deref(),
        Some(["task_name".to_string(), "message".to_string()].as_slice())
    );
    assert_eq!(
        output_schema.expect("spawn_agent output schema").to_value()["required"],
        json!(["task_name", "nickname"])
    );
}

#[test]
fn spawn_agent_catalog_description_preserves_generated_context() {
    let options = SpawnAgentToolOptions {
        agent_type_description: "Available agent roles: explorer".to_string(),
        expose_spawn_agent_model_overrides: true,
        usage_hint_text: Some("Local usage hint.".to_string()),
        ..Default::default()
    };
    let ToolSpec::Function(default_tool) =
        create_spawn_agent_tool_v2(options.clone(), /*description_override*/ None)
    else {
        panic!("spawn_agent should be a function tool");
    };
    let ToolSpec::Function(mut configured_tool) =
        create_spawn_agent_tool_v2(options, Some("Catalog spawning guidance."))
    else {
        panic!("spawn_agent should be a function tool");
    };
    assert!(
        configured_tool
            .description
            .contains("Catalog spawning guidance.")
    );
    assert!(
        configured_tool
            .description
            .contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE)
    );
    assert!(configured_tool.description.ends_with("Local usage hint."));
    assert!(
        !configured_tool
            .description
            .contains("Spawns an agent to work on the specified task.")
    );
    configured_tool.description = default_tool.description.clone();
    assert_eq!(configured_tool, default_tool);
}

#[test]
fn spawn_agent_tool_v1_keeps_legacy_fork_context_field() {
    let tool = create_spawn_agent_tool_v1(SpawnAgentToolOptions {
        agent_type_description: "role help".to_string(),
        expose_agent_type: true,
        hide_agent_type_model_reasoning: false,
        expose_spawn_agent_model_overrides: true,
        usage_hint_text: None,
        ..Default::default()
    });

    let ToolSpec::Namespace(namespace) = tool else {
        panic!("spawn_agent v1 should be a namespace tool");
    };
    assert_eq!(namespace.name, MULTI_AGENT_V1_NAMESPACE);
    let Some(ResponsesApiNamespaceTool::Function(ResponsesApiTool { parameters, .. })) =
        namespace.tools.first()
    else {
        panic!("spawn_agent should be a namespace function tool");
    };
    assert_eq!(
        parameters.schema_type.clone(),
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    assert!(properties.contains_key("fork_context"));
    assert!(!properties.contains_key("fork_turns"));
    assert_eq!(
        properties.get("agent_type"),
        Some(&JsonSchema::string(Some(format!(
            "{SPAWN_AGENT_TYPE_OVERRIDE_DESCRIPTION_V1}\nrole help"
        ))))
    );
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        None
    );
    assert_eq!(
        properties
            .get("model")
            .and_then(|schema| schema.description.as_deref()),
        Some(SPAWN_AGENT_MODEL_OVERRIDE_DESCRIPTION)
    );
    assert!(!properties.contains_key("service_tier"));
}

#[test_case::test_case(false; "inline_catalog")]
#[test_case::test_case(true; "context_catalog")]
fn spawn_agent_tool_keeps_model_controls_when_spawn_metadata_is_hidden(
    model_catalog_in_context: bool,
) {
    let tool = create_spawn_agent_tool_v2(
        SpawnAgentToolOptions {
            agent_type_description: "role help".to_string(),
            expose_agent_type: false,
            hide_agent_type_model_reasoning: true,
            expose_spawn_agent_model_overrides: true,
            model_catalog_in_context,
            usage_hint_text: None,
            ..Default::default()
        },
        /*description_override*/ None,
    );

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    assert!(!properties.contains_key("agent_type"));
    assert!(properties.contains_key("model"));
    assert!(properties.contains_key("reasoning_effort"));
    assert!(!properties.contains_key("service_tier"));
    assert!(!description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE));
    assert_eq!(
        description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE_V2),
        model_catalog_in_context
    );
}

#[test]
fn spawn_agent_tool_hides_model_controls_without_override_exposure() {
    let tool = create_spawn_agent_tool_v2(
        SpawnAgentToolOptions {
            agent_type_description: "role help".to_string(),
            expose_agent_type: false,
            hide_agent_type_model_reasoning: true,
            expose_spawn_agent_model_overrides: false,
            usage_hint_text: None,
            ..Default::default()
        },
        Some(""),
    );

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    for property in ["agent_type", "model", "reasoning_effort", "service_tier"] {
        assert!(!properties.contains_key(property));
    }
    assert!(!description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE));
    assert!(!description.contains("Available model overrides"));
}

#[test]
fn send_message_tool_requires_message_and_has_no_output_schema() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_send_message_tool()
    else {
        panic!("send_message should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("send_message should use object params");
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("message"));
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        Some(true)
    );
    assert!(!properties.contains_key("interrupt"));
    assert!(!properties.contains_key("items"));
    assert_eq!(
        properties
            .get("target")
            .and_then(|schema| schema.description.as_deref()),
        Some(
            "Relative or canonical task name to message (from spawn_agent). For another active session, use its exact thread ID."
        )
    );
    assert_eq!(
        parameters.required.as_deref(),
        Some(["target".to_string(), "message".to_string()].as_slice())
    );
    assert_eq!(output_schema, None);
}

#[test]
fn followup_task_tool_requires_message_and_has_no_output_schema() {
    let ToolSpec::Function(ResponsesApiTool {
        name,
        description,
        parameters,
        output_schema,
        ..
    }) = create_followup_task_tool()
    else {
        panic!("followup_task should be a function tool");
    };
    assert_eq!(name, "followup_task");
    assert_eq!(
        description,
        "Send a follow-up task to an existing non-root target agent and trigger a turn if it is idle. If the target is already running, deliver the task promptly at message boundaries while sampling, or after the pending tool call completes."
    );
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("followup_task should use object params");
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("message"));
    assert_eq!(
        properties
            .get("message")
            .and_then(|schema| schema.encrypted),
        Some(true)
    );
    assert!(!properties.contains_key("items"));
    assert_eq!(
        parameters.required.as_deref(),
        Some(["target".to_string(), "message".to_string()].as_slice())
    );
    assert_eq!(output_schema, None);
}

#[test]
fn wait_agent_v2_has_optional_any_all_and_progress_without_reset_or_timeout() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_wait_agent_tool_v2(WaitAgentTimeoutOptions::default())
    else {
        panic!("wait_agent should be a function tool");
    };
    let properties = parameters.properties.as_ref().expect("object params");
    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["wait_mode", "yield_time_ms"]
    );
    assert!(
        parameters
            .required
            .as_ref()
            .is_none_or(|required| required.is_empty())
    );
    let output = output_schema.expect("wait output schema").to_value();
    assert!(output["properties"].get("timed_out").is_none());
    assert!(output["properties"].get("completed_agents").is_some());
}

#[test]
fn list_agents_tool_includes_path_prefix_and_agent_fields() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("list_agents should use object params");
    assert!(properties.contains_key("path_prefix"));
    assert_eq!(
        properties
            .get("path_prefix")
            .and_then(|schema| schema.description.as_deref()),
        Some(
            "Relative or canonical task-path prefix without a trailing slash, or exact foreign agent ID to list its tree. Omit to list local live agents."
        )
    );
    assert_eq!(
        output_schema.expect("list_agents output schema").to_value()["properties"]["agents"]["items"]
            ["required"],
        json!(["agent_name", "agent_status"])
    );
}

#[test]
fn list_agents_tool_status_schema_includes_interrupted() {
    let ToolSpec::Function(ResponsesApiTool { output_schema, .. }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };

    assert_eq!(
        output_schema.expect("list_agents output schema").to_value()["properties"]["agents"]["items"]
            ["properties"]["agent_status"]["allOf"][0]["oneOf"][0]["enum"],
        json!([
            "pending_init",
            "running",
            "interrupted",
            "shutdown",
            "not_found"
        ])
    );
}

#[test]
fn independent_question_is_additive_and_inspection_is_passive() {
    let ToolSpec::Function(send) = create_send_message_tool() else {
        panic!("function tool")
    };
    assert_eq!(
        send.parameters.required.as_deref(),
        Some(["target".to_string(), "message".to_string()].as_slice())
    );
    assert!(send.parameters.properties.unwrap().contains_key("btw"));
    let ToolSpec::Function(inspect) = create_inspect_agent_tool() else {
        panic!("function tool")
    };
    assert_eq!(
        inspect.parameters.required.as_deref(),
        Some(["target".to_string()].as_slice())
    );
    assert_eq!(inspect.parameters.properties.unwrap().len(), 1);
}

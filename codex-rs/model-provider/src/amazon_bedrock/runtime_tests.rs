use codex_aws_auth::AwsAuthConfig;
use codex_model_provider_info::ModelProviderAwsAuthInfo;
use pretty_assertions::assert_eq;

use super::aws_auth_config;
use super::base_url;

#[test]
fn base_url_uses_regional_runtime_endpoint() {
    assert_eq!(
        base_url("us-west-2"),
        "https://bedrock-runtime.us-west-2.amazonaws.com/openai/v1"
    );
}

#[test]
fn aws_auth_config_uses_bedrock_service() {
    assert_eq!(
        aws_auth_config(&ModelProviderAwsAuthInfo {
            profile: Some("runtime-profile".to_string()),
            region: Some(" eu-west-1 ".to_string()),
            credential_export: None,
            auth_refresh: None,
        }),
        AwsAuthConfig {
            profile: Some("runtime-profile".to_string()),
            region: Some("eu-west-1".to_string()),
            service: "bedrock".to_string(),
        }
    );
}

#[tokio::test]
async fn rejects_invalid_caller_region_in_runtime_base_url() {
    use super::BedrockAuthSource;
    use super::bedrock_runtime_base_url;
    use codex_http_client::HttpClientFactory;
    use codex_http_client::OutboundProxyPolicy;
    use codex_protocol::config_types::CallerEnvironment;
    use std::collections::HashMap;

    let caller_env = CallerEnvironment::new(HashMap::from([(
        "AWS_REGION".to_string(),
        "attacker.example/v1#".to_string(),
    )]));
    let err = bedrock_runtime_base_url(
        BedrockAuthSource::CommandBearerToken,
        None,
        &ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
            credential_export: None,
            auth_refresh: None,
        },
        Some(&caller_env),
        &HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
    )
    .await
    .expect_err("URL delimiter in caller AWS_REGION must be rejected");
    assert!(
        err.to_string()
            .contains("Amazon Bedrock region `attacker.example/v1#` is invalid"),
        "unexpected error: {err:?}"
    );
}

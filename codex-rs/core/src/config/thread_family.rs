use super::Config;
use codex_login::AuthManager;
use codex_protocol::config_types::CallerEnvironment;
use codex_protocol::config_types::ProfileV2Name;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Frozen invocation inputs and credential owners shared by one thread family.
pub struct ThreadFamily {
    pub environment: CallerEnvironment,
    pub profile: Option<ProfileV2Name>,
    pub caller_home: Option<PathBuf>,
    require_custom_provider: bool,
    pub tracking_command: Option<String>,
    pub flags: Vec<(String, toml::Value)>,
    credentials: OnceCell<Arc<AuthManager>>,
    pub(crate) runtime_auth: crate::runtime_auth::RuntimeAuthSlot,
}

impl ThreadFamily {
    pub fn new(
        environment: CallerEnvironment,
        profile: Option<ProfileV2Name>,
        caller_home: Option<PathBuf>,
        flags: Vec<(String, toml::Value)>,
        require_custom_provider: bool,
        tracking_command: Option<String>,
    ) -> Self {
        Self {
            environment,
            profile,
            caller_home,
            flags,
            require_custom_provider,
            tracking_command,
            credentials: OnceCell::new(),
            runtime_auth: Default::default(),
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn initialize_credentials(&self, config: &Config) -> std::io::Result<()> {
        self.credentials
            .get_or_try_init(|| async {
                if self.require_custom_provider
                    && !config.model_provider.base_url.as_deref().is_some_and(|base| {
                        url::Url::parse(base).is_ok_and(|url| {
                            matches!(url.scheme(), "http" | "https") && url.has_host()
                        }) && codex_http_client::classify_openai_api_url(base)
                            == codex_http_client::OpenAiDestination::Other
                    })
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "mekugi requires a custom Responses provider: configure NAME.config.toml and run mekugi -p NAME; use mekugi codex for native OpenAI",
                    ));
                }
                let mut auth = config.auth_config();
                if let Some(home) = &self.caller_home {
                    auth.codex_home.clone_from(home);
                } else {
                    auth.auth_credentials_store_mode =
                        codex_config::types::AuthCredentialsStoreMode::Ephemeral;
                }
                AuthManager::for_caller(auth, self.environment.clone())
                    .await
                    .map_err(std::io::Error::other)
            })
            .await?;
        Ok(())
    }

    pub(crate) fn credentials(&self) -> Arc<AuthManager> {
        Arc::clone(
            self.credentials
                .get()
                .expect("admitted family credentials are initialized before thread startup"),
        )
    }

    pub fn overrides(&self, key: &str) -> bool {
        self.flags.iter().any(|(name, _)| name == key)
    }
}

impl PartialEq for ThreadFamily {
    fn eq(&self, other: &Self) -> bool {
        self.environment == other.environment
    }
}

impl fmt::Debug for ThreadFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadFamily")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

use std::path::PathBuf;
use std::sync::Arc;

use codex_exec_server::LOCAL_ENVIRONMENT_ID;
use codex_extension_api::ExtensionData;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_utils_absolute_path::AbsolutePathBuf;
use skills_mgr::Error;
use skills_mgr::SkillsManager;
use tokio::sync::OnceCell;

/// The original local workspace whose project configuration the host explicitly trusts.
pub struct TrustedManagedSkillsWorkspace(pub AbsolutePathBuf);

pub(crate) struct ManagedHostCatalog;

const BASE_SKILL_GUIDANCE: &str = "\
Skill descriptions in the available catalog own applicability. Use enabled
skills for the operations their descriptions match or the user invokes them
for. Instructions remain usable without any skills. Explicit reviews retain
their requested operation and scope. The skill inventory lists each skill's
references, so a reference whose name matches the current operation needs no
routing from the skill body: read it in the same shell call as the body. Follow
selected methods. If a required tool/method is unavailable, stop only dependent
work; reuse substitution or install approval, or ask. Missing automatic skills
alone do not block routine work.";

/// The optional local inventory shared by accepted sources and host catalog rendering.
pub struct ManagedSkills {
    project: PathBuf,
    reader: OnceCell<Result<(Arc<SkillsManager>, bool), String>>,
    catalog: OnceCell<Result<String, String>>,
}

impl ManagedSkills {
    /// Adapt Codex-owned base instructions to the host catalog selected for this step.
    pub fn apply_base_guidance(step: &ExtensionData, instructions: &mut String) {
        const HEADING: &str = "\n# Using skills\n\n";
        if step.get::<ManagedHostCatalog>().is_none() {
            return;
        }
        if let Some(start) = instructions.find(HEADING) {
            let start = start + HEADING.len();
            let body = &instructions[start..];
            let end = body.find("\n# ").unwrap_or(body.len());
            let body_len = body[..end].trim_end().len();
            instructions.replace_range(start..start + body_len, BASE_SKILL_GUIDANCE);
        }
    }

    pub fn for_turn(
        turn: &ExtensionData,
        environment: &TurnEnvironmentSelection,
    ) -> Option<Arc<Self>> {
        if environment.environment_id != LOCAL_ENVIRONMENT_ID {
            return None;
        }
        #[allow(deprecated)]
        let project = environment.cwd.to_abs_path().ok()?.into();
        if let Some(current) = turn.get::<Self>()
            && current.project == project
        {
            return Some(current);
        }
        let skills = Arc::new(Self {
            project,
            reader: OnceCell::new(),
            catalog: OnceCell::new(),
        });
        turn.insert_arc(Arc::clone(&skills));
        Some(skills)
    }

    async fn reader(&self) -> Result<&(Arc<SkillsManager>, bool), String> {
        self.reader
            .get_or_init(|| async {
                let project = self.project.clone();
                tokio::task::spawn_blocking(move || {
                    let manager = SkillsManager::for_project(project)?.with_strict_discovery();
                    let enabled = manager.is_enabled()?;
                    Ok((Arc::new(manager), enabled))
                })
                .await
                .map_err(|error| format!("skills-mgr is_enabled: {error}"))?
                .map_err(|error: Error| format!("skills-mgr is_enabled: {error}"))
            })
            .await
            .as_ref()
            .map_err(Clone::clone)
    }

    pub async fn is_enabled(&self) -> Result<bool, String> {
        Ok(self.reader().await?.1)
    }

    async fn lookup(&self, target: &str) -> Result<skills_mgr::Result<Vec<u8>>, String> {
        let manager = Arc::clone(&self.reader().await?.0);
        let target = target.to_owned();
        tokio::task::spawn_blocking(move || manager.get(&target))
            .await
            .map_err(|error| format!("skills-mgr get: {error}"))
    }

    pub async fn get(&self, target: &str) -> Result<Option<Vec<u8>>, String> {
        match self.lookup(target).await? {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(error) => Err(format!("skills-mgr get: {error}")),
        }
    }

    /// A typed native selection may name an unmanaged or explicitly disabled
    /// skill. An unreadable manager candidate is not evidence of either.
    pub async fn resolves_selection(&self, name: &str) -> Result<bool, String> {
        if !self.is_enabled().await? {
            return Ok(false);
        }
        match self.lookup(name).await? {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_) | Error::NotEnabled(_)) => Ok(false),
            Err(error) => Err(format!("skills-mgr get: {error}")),
        }
    }

    #[tracing::instrument(skip_all)]
    pub(crate) async fn catalog(
        &self,
        trusted: &TrustedManagedSkillsWorkspace,
    ) -> Result<Option<&str>, String> {
        // Unlike structured reads, XML listing may run workspace commands.
        if self.project.as_path() != trusted.0.as_path() {
            return Ok(None);
        }
        let (manager, enabled) = self.reader().await?;
        if !enabled {
            return Ok(None);
        }
        self.catalog
            .get_or_init(|| async {
                let manager = Arc::clone(manager);
                tokio::task::spawn_blocking(move || {
                    let xml = manager.list_xml(&[])?;
                    Ok::<_, Error>(format!(
                        "\n## Host skills (replaces any previous host catalog)\n{xml}Use `skills-mgr get NAME` to read a skill, or `skills-mgr get NAME/FILE` for a reference. Expand grouped reference paths first. Append `start:end` for an inclusive, 1-based line range.\nUse `skills-mgr run NAME/SCRIPT [args...]` to run a skill script.\n"
                    ))
                })
                    .await
                    .map_err(|error| format!("skills-mgr list: {error}"))?
                    .map_err(|error| format!("skills-mgr list: {error}"))
            })
            .await
            .as_ref()
            .map(|catalog| Some(catalog.as_str()))
            .map_err(Clone::clone)
    }
}

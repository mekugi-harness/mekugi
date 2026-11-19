//! Resolve explicit native skills and read managed sources in process.

use std::sync::Arc;

use codex_file_system::EnvironmentAccess;
use codex_file_system::GetMetadataOptions;
use codex_skills_extension::HostSkillsSnapshot;
use codex_skills_extension::ManagedSkills;
use codex_utils_path_uri::PathUri;

use super::ResolvedSource;
use super::SourceBody;
use super::check_capacity;
use super::read_file;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnEnvironment;

pub(super) struct SkillSources<'a> {
    managed: Option<Arc<ManagedSkills>>,
    native: Option<&'a HostSkillsSnapshot>,
}

impl<'a> SkillSources<'a> {
    pub(super) fn new(
        step: &StepContext,
        environment: &TurnEnvironment,
        native: Option<&'a HostSkillsSnapshot>,
    ) -> Self {
        Self {
            managed: ManagedSkills::for_turn(&step.turn.extension_data, &environment.selection),
            native,
        }
    }

    pub(super) async fn resolve(
        &self,
        access: &dyn EnvironmentAccess,
        reference: &str,
    ) -> Result<ResolvedSource, String> {
        if let Some(manager) = &self.managed
            && manager.is_enabled().await?
        {
            return Ok(ResolvedSource::ManagedSkill(reference.to_string()));
        }
        match self.native_path(access, reference).await? {
            Some(path) => Ok(ResolvedSource::File(path)),
            None => Ok(ResolvedSource::ManagedSkill(reference.to_string())),
        }
    }

    // A marker reserves the name for the manager even if it is hidden from the catalog.
    async fn native_path(
        &self,
        access: &dyn EnvironmentAccess,
        reference: &str,
    ) -> Result<Option<PathUri>, String> {
        const MANAGED_PLACEHOLDER: &str = "skills-mgr remote placeholder\n";
        let (name, relative) = reference
            .split_once('/')
            .map_or((reference, None), |(name, path)| (name, Some(path)));
        let Some(snapshot) = self.native else {
            return Err(format!("native skill {name:?} is unavailable"));
        };
        let mut native = None;
        let mut ambiguous = false;
        let outcome = snapshot.outcome();
        for skill in outcome
            .skills
            .iter()
            .filter(|skill| skill.name == name && outcome.is_skill_enabled(skill))
        {
            let path = &skill.path_to_skills_md;
            if native == Some(path) {
                continue;
            }
            let marker = path
                .parent()
                .ok_or_else(|| "skill has no parent directory".to_string())?
                .join_descendant(".skills-mgr-placeholder")
                .map_err(|error| error.to_string())?;
            match access
                .get_metadata(&marker, GetMetadataOptions::default())
                .await
            {
                Ok(metadata)
                    if metadata.is_file && metadata.size == MANAGED_PLACEHOLDER.len() as u64 =>
                {
                    if read_file(access, &marker).await?.text == MANAGED_PLACEHOLDER {
                        return Ok(None);
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            ambiguous |= native.replace(path).is_some();
        }
        if ambiguous {
            return Err(format!(
                "ambiguous native skill {name:?}; select its file explicitly"
            ));
        }
        let path = native.ok_or_else(|| format!("native skill {name:?} is unavailable"))?;
        let path = match relative {
            None => path.clone(),
            Some(relative) => path
                .parent()
                .ok_or_else(|| "skill has no parent directory".to_string())?
                .join_descendant(relative)
                .map_err(|error| error.to_string())?,
        };
        Ok(Some(path))
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub(super) async fn read(
        &self,
        access: &dyn EnvironmentAccess,
        reference: &str,
    ) -> Result<SourceBody, String> {
        let manager = self
            .managed
            .as_ref()
            .ok_or_else(|| "skills-mgr is unavailable in the selected environment".to_string())?;
        match manager.get(reference).await? {
            Some(bytes) => {
                check_capacity(bytes.len()).map_err(|error| error.to_string())?;
                Ok(SourceBody {
                    text: String::from_utf8(bytes).map_err(|error| error.to_string())?,
                    relative_to: None,
                    expand_references: reference.split_once('/').is_none_or(|(_, path)| {
                        super::expands_references(path.rsplit('/').next().unwrap_or(path))
                    }),
                })
            }
            None => match self.native_path(access, reference).await? {
                Some(path) => read_file(access, &path).await,
                None => Err(format!(
                    "skills-mgr get: skill {reference:?} was not discovered"
                )),
            },
        }
    }
}

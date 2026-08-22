use android_manager_adapter::{
    AdapterError, AdapterErrorCode, AdapterResult, DelegateProvider, DelegateRequest,
};
use android_manager_model::{
    manual_resource_id, package_resource_id, AdapterCapability, AdapterDescriptor, AdapterKind,
    DelegatePlan, DelegatedAction, PrerequisiteResource, SourceIntent,
};
use std::collections::BTreeSet;

pub const OBTAINIUM_PACKAGE: &str = "dev.imranr.obtainium";

pub struct ObtainiumProvider;

pub fn source_url(source: &SourceIntent) -> Option<&str> {
    source.url.as_deref().or(source.repository.as_deref())
}

fn instruction(package_name: &str, source: &SourceIntent) -> String {
    match source_url(source) {
        Some(url) => format!("Add {package_name} to Obtainium from {url}"),
        None => format!("Add {package_name} to Obtainium"),
    }
}

fn obtainium_install_instruction() -> String {
    format!("Install Obtainium ({OBTAINIUM_PACKAGE}) to manage delegated apps")
}

impl DelegateProvider for ObtainiumProvider {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            id: "obtainium".into(),
            kind: AdapterKind::Delegate,
            capabilities: BTreeSet::from([AdapterCapability::PlanDelegate]),
        }
    }

    fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()> {
        let Some(url) = source_url(source) else {
            return Err(AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "obtainium",
                "source.url or source.repository is required",
            ));
        };
        if url.split_once("://").is_some_and(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .is_some_and(|authority| authority.contains('@'))
        }) {
            return Err(AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "obtainium",
                "credential-bearing source URLs are forbidden",
            ));
        }
        Ok(())
    }

    fn supports(&self, source: &SourceIntent) -> bool {
        source.kind == "obtainium"
    }

    fn plan(&self, request: DelegateRequest<'_>) -> AdapterResult<DelegatePlan> {
        let source = request.app.source.as_ref().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "obtainium",
                format!("{} has no source", request.package_name),
            )
        })?;
        self.validate_config(source)?;
        let prerequisite =
            package_resource_id(request.device_name, request.user_id, OBTAINIUM_PACKAGE);
        let obtainium_installed =
            request.snapshot.packages.iter().any(|package| {
                package.name == OBTAINIUM_PACKAGE && package.user_id == request.user_id
            });
        Ok(DelegatePlan {
            action: DelegatedAction {
                resource: manual_resource_id(
                    request.device_name,
                    request.user_id,
                    request.package_name,
                ),
                instruction: instruction(request.package_name, source),
                depends_on: if obtainium_installed {
                    Vec::new()
                } else {
                    vec![prerequisite.clone()]
                },
            },
            prerequisites: if obtainium_installed {
                Vec::new()
            } else {
                vec![PrerequisiteResource {
                    resource: prerequisite,
                    instruction: obtainium_install_instruction(),
                }]
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: Option<&str>, repository: Option<&str>) -> SourceIntent {
        let mut source = SourceIntent::new("obtainium");
        source.url = url.map(str::to_string);
        source.repository = repository.map(str::to_string);
        source
    }

    #[test]
    fn instruction_includes_url() {
        let text = instruction(
            "org.example.app",
            &source(Some("https://github.com/ex/app"), None),
        );
        assert!(text.contains("org.example.app"));
        assert!(text.contains("https://github.com/ex/app"));
    }

    #[test]
    fn provider_plans_user_scoped_prerequisite() {
        let app = android_manager_model::DesiredApp {
            state: android_manager_model::AppState::Present,
            management: android_manager_model::ManagementMode::Delegated,
            source: Some(source(None, Some("https://github.com/ex/app"))),
            version_policy: android_manager_model::VersionPolicy::Presence,
            verification: android_manager_model::Verification::default(),
        };
        let snapshot = android_manager_model::DeviceSnapshot {
            transport_id: "adb:test".into(),
            serial: "test".into(),
            manufacturer: None,
            sdk: 35,
            abis: Vec::new(),
            users: vec![10],
            packages: Vec::new(),
            capabilities: android_manager_model::DeviceCapabilities::default(),
        };
        let plan = ObtainiumProvider
            .plan(DelegateRequest {
                device_name: "phone",
                user_id: 10,
                package_name: "org.example.app",
                app: &app,
                snapshot: &snapshot,
            })
            .unwrap();
        assert_eq!(plan.prerequisites.len(), 1);
        assert_eq!(
            plan.action.depends_on,
            [plan.prerequisites[0].resource.clone()]
        );
    }

    #[test]
    fn credential_bearing_url_is_rejected() {
        assert!(ObtainiumProvider
            .validate_config(&source(Some("https://user:token@example.test/app"), None))
            .is_err());
    }
}

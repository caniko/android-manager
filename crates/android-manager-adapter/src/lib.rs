use android_manager_model::{
    AdapterCapability, AdapterDescriptor, AdapterKind, DelegatePlan, DesiredApp, DeviceSnapshot,
    LockEntry, SourceIntent,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterErrorCode {
    DuplicateProvider,
    UnknownProvider,
    WrongAdapterKind,
    CapabilityMismatch,
    IncompatibleSource,
    InvalidConfiguration,
    ResolutionFailed,
    DelegatePlanningFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterError {
    pub code: AdapterErrorCode,
    pub adapter: String,
    pub message: String,
}

impl AdapterError {
    pub fn new(
        code: AdapterErrorCode,
        adapter: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            adapter: adapter.into(),
            message: message.into(),
        }
    }
}

impl Display for AdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "adapter {} {:?}: {}",
            self.adapter, self.code, self.message
        )
    }
}

impl Error for AdapterError {}

pub type AdapterResult<T> = Result<T, AdapterError>;

pub struct ResolveRequest<'a> {
    pub package_name: &'a str,
    pub app: &'a DesiredApp,
}

pub struct DelegateRequest<'a> {
    pub device_name: &'a str,
    pub user_id: u32,
    pub package_name: &'a str,
    pub app: &'a DesiredApp,
    pub snapshot: &'a DeviceSnapshot,
}

pub trait SourceProvider: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()>;
    fn supports(&self, source: &SourceIntent) -> bool;
    fn resolve(&self, request: ResolveRequest<'_>) -> AdapterResult<(String, LockEntry)>;
}

pub trait DelegateProvider: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()>;
    fn supports(&self, source: &SourceIntent) -> bool;
    fn plan(&self, request: DelegateRequest<'_>) -> AdapterResult<DelegatePlan>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn SourceProvider>>,
}

impl ProviderRegistry {
    pub fn register(&mut self, provider: Arc<dyn SourceProvider>) -> AdapterResult<()> {
        let descriptor = provider.descriptor();
        validate_descriptor(&descriptor, AdapterKind::Source, AdapterCapability::Resolve)?;
        if self.providers.contains_key(&descriptor.id) {
            return Err(AdapterError::new(
                AdapterErrorCode::DuplicateProvider,
                &descriptor.id,
                "provider is already registered",
            ));
        }
        self.providers.insert(descriptor.id, provider);
        Ok(())
    }

    pub fn resolve(&self, source: &SourceIntent) -> AdapterResult<Arc<dyn SourceProvider>> {
        let id = source.provider_id().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "external",
                "external source requires adapter",
            )
        })?;
        let provider = self.providers.get(id).cloned().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorCode::UnknownProvider,
                id,
                "provider is not registered",
            )
        })?;
        if !provider.supports(source) {
            return Err(AdapterError::new(
                AdapterErrorCode::IncompatibleSource,
                id,
                format!("source type {} is incompatible", source.kind),
            ));
        }
        provider.validate_config(source)?;
        Ok(provider)
    }
}

#[derive(Default)]
pub struct DelegateRegistry {
    providers: BTreeMap<String, Arc<dyn DelegateProvider>>,
}

impl DelegateRegistry {
    pub fn register(&mut self, provider: Arc<dyn DelegateProvider>) -> AdapterResult<()> {
        let descriptor = provider.descriptor();
        validate_descriptor(
            &descriptor,
            AdapterKind::Delegate,
            AdapterCapability::PlanDelegate,
        )?;
        if self.providers.contains_key(&descriptor.id) {
            return Err(AdapterError::new(
                AdapterErrorCode::DuplicateProvider,
                &descriptor.id,
                "provider is already registered",
            ));
        }
        self.providers.insert(descriptor.id, provider);
        Ok(())
    }

    pub fn resolve(&self, source: &SourceIntent) -> AdapterResult<Arc<dyn DelegateProvider>> {
        let id = source.provider_id().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "external",
                "external source requires adapter",
            )
        })?;
        let provider = self.providers.get(id).cloned().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorCode::UnknownProvider,
                id,
                "provider is not registered",
            )
        })?;
        if !provider.supports(source) {
            return Err(AdapterError::new(
                AdapterErrorCode::IncompatibleSource,
                id,
                format!("source type {} is incompatible", source.kind),
            ));
        }
        provider.validate_config(source)?;
        Ok(provider)
    }
}

fn validate_descriptor(
    descriptor: &AdapterDescriptor,
    kind: AdapterKind,
    capability: AdapterCapability,
) -> AdapterResult<()> {
    if descriptor.kind != kind {
        return Err(AdapterError::new(
            AdapterErrorCode::WrongAdapterKind,
            &descriptor.id,
            format!("expected {kind:?}, got {:?}", descriptor.kind),
        ));
    }
    if !descriptor.capabilities.contains(&capability) {
        return Err(AdapterError::new(
            AdapterErrorCode::CapabilityMismatch,
            &descriptor.id,
            format!("missing capability {capability:?}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use android_manager_model::{Constraints, LockedSource};

    struct FixtureSource(&'static str);

    impl SourceProvider for FixtureSource {
        fn descriptor(&self) -> AdapterDescriptor {
            AdapterDescriptor {
                id: self.0.into(),
                kind: AdapterKind::Source,
                capabilities: [AdapterCapability::Resolve].into(),
            }
        }

        fn validate_config(&self, _source: &SourceIntent) -> AdapterResult<()> {
            Ok(())
        }

        fn supports(&self, source: &SourceIntent) -> bool {
            source.kind == self.0
        }

        fn resolve(&self, request: ResolveRequest<'_>) -> AdapterResult<(String, LockEntry)> {
            Ok((
                format!("{}:{}", self.0, request.package_name),
                LockEntry {
                    source: LockedSource {
                        kind: self.0.into(),
                        path: None,
                        url: None,
                        repository: None,
                        repository_identity: None,
                        adapter: None,
                    },
                    package_name: request.package_name.into(),
                    version_name: None,
                    version_code: 1,
                    signers: vec!["sha256:aa".into()],
                    artifacts: Vec::new(),
                    constraints: Constraints::default(),
                },
            ))
        }
    }

    #[test]
    fn registry_lookup_and_duplicate_rejection() {
        let mut registry = ProviderRegistry::default();
        registry.register(Arc::new(FixtureSource("local"))).unwrap();
        assert!(registry.resolve(&SourceIntent::new("local")).is_ok());
        let duplicate = registry
            .register(Arc::new(FixtureSource("local")))
            .unwrap_err();
        assert_eq!(duplicate.code, AdapterErrorCode::DuplicateProvider);
        let unknown = match registry.resolve(&SourceIntent::new("missing")) {
            Ok(_) => panic!("missing provider resolved"),
            Err(error) => error,
        };
        assert_eq!(unknown.code, AdapterErrorCode::UnknownProvider);
    }

    struct MissingCapability;

    impl SourceProvider for MissingCapability {
        fn descriptor(&self) -> AdapterDescriptor {
            AdapterDescriptor {
                id: "broken".into(),
                kind: AdapterKind::Source,
                capabilities: Default::default(),
            }
        }

        fn validate_config(&self, _source: &SourceIntent) -> AdapterResult<()> {
            Ok(())
        }

        fn supports(&self, _source: &SourceIntent) -> bool {
            true
        }

        fn resolve(&self, _request: ResolveRequest<'_>) -> AdapterResult<(String, LockEntry)> {
            unreachable!()
        }
    }

    #[test]
    fn registration_rejects_capability_mismatch() {
        let mut registry = ProviderRegistry::default();
        let error = registry.register(Arc::new(MissingCapability)).unwrap_err();
        assert_eq!(error.code, AdapterErrorCode::CapabilityMismatch);
    }
}

use android_manager_model::SourceIntent;
use anyhow::{bail, Result};
use serde::Serialize;

pub const OBTAINIUM_PACKAGE: &str = "dev.imranr.obtainium";

pub fn kind() -> &'static str {
    "obtainium"
}

pub fn source_url(source: &SourceIntent) -> Option<&str> {
    source.url.as_deref().or(source.repository.as_deref())
}

pub fn instruction(package_name: &str, source: &SourceIntent) -> String {
    match source_url(source) {
        Some(url) => format!("Add {package_name} to Obtainium from {url}"),
        None => format!("Add {package_name} to Obtainium"),
    }
}

pub fn obtainium_install_instruction() -> String {
    format!("Install Obtainium ({OBTAINIUM_PACKAGE}) to manage delegated apps")
}

#[derive(Debug, Serialize)]
pub struct ObtainiumApp {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct ObtainiumExport {
    pub apps: Vec<ObtainiumApp>,
}

pub fn export_app(package_name: &str, source: &SourceIntent) -> Result<ObtainiumApp> {
    let Some(url) = source_url(source) else {
        bail!("obtainium {package_name} needs source.url or source.repository");
    };
    Ok(ObtainiumApp {
        id: package_name.to_string(),
        url: url.to_string(),
    })
}

pub fn export_json(apps: &[(String, SourceIntent)]) -> Result<String> {
    let apps = apps
        .iter()
        .map(|(package, source)| export_app(package, source))
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_string_pretty(&ObtainiumExport { apps })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: Option<&str>, repository: Option<&str>) -> SourceIntent {
        SourceIntent {
            kind: kind().into(),
            path: None,
            url: url.map(str::to_string),
            sha256: None,
            repository: repository.map(str::to_string),
            identity: None,
            channel: None,
        }
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
    fn export_prefers_url_and_requires_one() {
        let app = export_app(
            "org.example.app",
            &source(
                Some("https://f-droid.org/packages/org.example.app"),
                Some("ignored"),
            ),
        )
        .unwrap();
        assert_eq!(app.id, "org.example.app");
        assert_eq!(app.url, "https://f-droid.org/packages/org.example.app");
        assert!(export_app("org.example.app", &source(None, None)).is_err());
    }

    #[test]
    fn export_json_lists_apps() {
        let json = export_json(&[(
            "org.example.app".into(),
            source(None, Some("https://github.com/ex/app")),
        )])
        .unwrap();
        assert!(json.contains("org.example.app"));
        assert!(json.contains("https://github.com/ex/app"));
    }
}

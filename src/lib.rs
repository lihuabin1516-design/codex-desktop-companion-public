pub mod audit;
pub mod config;
pub mod diagnostics;
pub mod discovery;
pub mod environment;
pub mod hashing;
pub mod launcher;
pub mod logging;

use serde::Serialize;

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PRODUCT_NAME: &str = "Codex + CC-Panes Companion";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductDescriptor {
    pub schema_version: u32,
    pub name: &'static str,
    pub version: &'static str,
    pub role: &'static str,
    pub upstream_policy: &'static str,
    pub liveagent_runtime_dependency: bool,
    pub sanitized_environment: Vec<&'static str>,
    pub preserved_environment: Vec<&'static str>,
}

pub fn descriptor() -> ProductDescriptor {
    ProductDescriptor {
        schema_version: 1,
        name: PRODUCT_NAME,
        version: PRODUCT_VERSION,
        role: "update-transparent-launcher-and-diagnostics",
        upstream_policy: "official-external-binaries",
        liveagent_runtime_dependency: false,
        sanitized_environment: vec![
            "CODEX_INTERNAL_*",
            "CODEX_THREAD_ID",
            "CODEX_PERMISSION_PROFILE",
            "WORKFLOW_FABRIC_CONTROLLED_CODEX_LAUNCHER_ERROR",
        ],
        preserved_environment: vec!["CODEX_HOME", "CODEX_API_KEY", "OPENAI_API_KEY", "PATH"],
    }
}

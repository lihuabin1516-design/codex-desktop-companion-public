use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub ticks: u64,
    pub idle_ticks: u64,
    pub dispatch_claims: u64,
    pub dispatch_frozen: u64,
    pub backpressure_rejections: u64,
    pub cancellations: u64,
    pub aggregation_queries: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuredLogRecord {
    pub schema_version: String,
    pub level: LogLevel,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

impl StructuredLogRecord {
    pub fn new(level: LogLevel, event: impl Into<String>) -> Self {
        Self {
            schema_version: "coordination.coordinator-log/v1beta1".to_owned(),
            level,
            event: event.into(),
            correlation_id: None,
            fields: BTreeMap::new(),
        }
    }

    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

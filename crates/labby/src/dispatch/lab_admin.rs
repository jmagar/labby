//! Shared dispatch layer for the internal `lab_admin` tool.
//!
//! This module is the single authoritative owner of the `lab_admin` action
//! catalog and shared dispatch semantics. No HTTP client is needed — the
//! underlying `audit_services` call is a local filesystem scan.

mod catalog;
mod client;
mod dispatch;
mod params;

pub use catalog::ACTIONS;
pub use dispatch::dispatch;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn help_lists_onboarding_audit() {
        let value = dispatch("help", serde_json::json!({})).await.unwrap();
        let names: Vec<String> = value["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|action| action["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"onboarding.audit".to_string()));
    }

    #[test]
    fn catalog_has_onboarding_audit() {
        let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
        assert!(
            names.contains(&"onboarding.audit"),
            "onboarding.audit missing from catalog"
        );
    }
}

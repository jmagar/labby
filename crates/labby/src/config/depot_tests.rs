use super::LabConfig;

#[test]
fn opaque_epochs_reject_numbers_and_unbounded_strings() {
    use super::depot::OpaqueEpoch;
    assert!(serde_json::from_str::<OpaqueEpoch>("9007199254740993").is_err());
    assert!(serde_json::from_str::<OpaqueEpoch>("\"\"").is_err());
    assert!(serde_json::from_value::<OpaqueEpoch>(serde_json::json!("x".repeat(129))).is_err());
    let epoch: OpaqueEpoch = serde_json::from_str("\"9007199254740993\"").unwrap();
    assert_eq!(
        serde_json::to_string(&epoch).unwrap(),
        "\"9007199254740993\""
    );
}

#[test]
fn fresh_depot_is_public_and_explicit_disable_survives_roundtrip() {
    let fresh = LabConfig::default();
    assert!(fresh.depot.public_enabled);
    let disabled: LabConfig = toml::from_str("[depot]\npublic_enabled = false").unwrap();
    assert!(!disabled.depot.public_enabled);
    let roundtrip: LabConfig = toml::from_str(&toml::to_string(&disabled).unwrap()).unwrap();
    assert!(!roundtrip.depot.public_enabled);
}

#[test]
fn managed_mode_never_silently_authorizes_stale_or_unknown_protocol() {
    use super::depot::DepotControlMode;
    let mut config = LabConfig::default();
    assert_eq!(config.depot.control_mode, DepotControlMode::Standalone);
    assert!(!config.depot.managed_mutations_ready(true, 1));

    config.depot.control_mode = DepotControlMode::LabbyManaged;
    assert!(config.depot.managed_mutations_ready(true, 1));
    assert!(!config.depot.managed_mutations_ready(false, 1));
    assert!(!config.depot.managed_mutations_ready(true, 2));
    config.depot.managed_authority_kill_switch = true;
    assert!(!config.depot.managed_mutations_ready(true, 1));
}

#[test]
fn invalid_provider_is_quarantined_without_losing_healthy_sibling() {
    let config: LabConfig = toml::from_str(
        r#"
        [[depot.providers]]
        id = "bad"
        enabled = "wrong type"
        [[depot.providers]]
        id = "team"
        name = "Team Depot"
        endpoint = "https://depot.example.com"
        enabled = true
        auth_mode = "anonymous"
        [depot.providers.future]
        nested = "preserved"
    "#,
    )
    .unwrap();
    let resolved = config.depot.resolve(&Default::default());
    assert_eq!(resolved.providers.len(), 2);
    assert_eq!(resolved.diagnostics.len(), 1);
    assert!(toml::to_string(&config).unwrap().contains("preserved"));
}

#[test]
fn duplicate_ids_quarantine_every_collision() {
    let entry = "[[depot.providers]]\nid='team'\nname='Team'\nendpoint='https://example.com'\nenabled=true\nauth_mode='anonymous'\n";
    let config: LabConfig = toml::from_str(&entry.repeat(2)).unwrap();
    let resolved = config.depot.resolve(&Default::default());
    assert_eq!(resolved.providers.len(), 1);
    assert_eq!(resolved.diagnostics.len(), 2);
}

#[test]
fn legacy_never_downgrades_to_anonymous_or_resurrects() {
    use super::depot::LegacyDepot;
    let legacy = LegacyDepot {
        url: Some("https://legacy.example".into()),
        enabled: None,
        token_present: false,
    };
    let mut config = LabConfig::default();
    let resolved = config.depot.resolve(&legacy);
    assert_eq!(resolved.providers.len(), 1);
    assert_eq!(resolved.diagnostics.len(), 1);
    let with_token = LegacyDepot {
        token_present: true,
        ..legacy
    };
    assert_eq!(config.depot.resolve(&with_token).providers.len(), 2);
    config.depot.legacy_migrated = true;
    assert_eq!(config.depot.resolve(&with_token).providers.len(), 1);
}

#[test]
fn exact_identity_and_safe_counts_are_lossless() {
    use super::depot::{ArtifactRef, safe_total};
    for raw in [" a+%/é ", "é", "e\u{301}"] {
        let reference = ArtifactRef::new("public", raw).unwrap();
        assert_eq!(reference.artifact_id, raw);
    }
    assert!(ArtifactRef::new("all", "a").is_err());
    assert!(ArtifactRef::new("public", "").is_err());
    assert!(ArtifactRef::new("public", &"a".repeat(2049)).is_err());
    assert_eq!(
        safe_total(9_007_199_254_740_991),
        Some(9_007_199_254_740_991)
    );
    assert_eq!(safe_total(9_007_199_254_740_992), None);
}

#[test]
fn diagnostics_and_debug_do_not_expose_raw_invalid_values() {
    let config: LabConfig =
        toml::from_str("[[depot.providers]]\nid='bad'\npassword='super-secret'").unwrap();
    assert!(!format!("{:?}", config.depot).contains("super-secret"));
    assert!(
        !serde_json::to_string(&config.depot.resolve(&Default::default()))
            .unwrap()
            .contains("super-secret")
    );
}

#[test]
fn disabled_legacy_remains_disabled_without_a_secret() {
    let config = LabConfig::default();
    let legacy = super::depot::LegacyDepot {
        url: Some("https://old.example".into()),
        enabled: Some(false),
        token_present: false,
    };
    let resolved = config.depot.resolve(&legacy);
    assert_eq!(resolved.providers.len(), 2);
    assert!(!resolved.providers[1].enabled);
}

#[test]
fn provider_slots_and_tombstones_never_recycle_identity() {
    let entries = (0..20).map(|i| format!("[[depot.providers]]\nid='p{i}'\nname='P'\nendpoint='https://example.com'\nenabled=true\nauth_mode='anonymous'\n")).collect::<String>();
    let mut config: LabConfig = toml::from_str(&entries).unwrap();
    assert_eq!(
        config.depot.resolve(&Default::default()).providers.len(),
        16
    );
    config.depot.tombstones.insert("p0".into());
    let resolved = config.depot.resolve(&Default::default());
    assert!(!resolved.providers.iter().any(|p| p.id == "p0"));
    assert!(!resolved.providers.iter().any(|p| p.id == "p15"));
}

#[test]
fn credential_references_and_endpoint_authority_are_constrained() {
    use super::depot::{allowed_secret_reference, canonical_endpoint};
    assert!(allowed_secret_reference("LABBY_DEPOT_TEAM_TOKEN"));
    assert!(!allowed_secret_reference("AWS_SECRET_ACCESS_KEY"));
    for endpoint in [
        "http://example.com",
        "https://u:p@example.com",
        "https://example.com?q=1",
        "https://example.com/#x",
    ] {
        assert!(canonical_endpoint(endpoint).is_err());
    }
    assert_ne!(
        canonical_endpoint("https://example.com/a").unwrap(),
        canonical_endpoint("https://example.com/b").unwrap()
    );
}

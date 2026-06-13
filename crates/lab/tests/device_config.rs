#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
#[test]
fn parses_device_master_config_block() {
    let raw = r#"
        [device]
        master = "tootie"
    "#;

    let parsed: labby::config::LabConfig = toml::from_str(raw).unwrap();
    assert_eq!(
        parsed.device.as_ref().unwrap().master.as_deref(),
        Some("tootie")
    );
}

#[test]
fn defaults_device_config_when_block_missing() {
    let parsed: labby::config::LabConfig = toml::from_str("").unwrap();
    assert!(parsed.device.is_none());
}

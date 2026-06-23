use labby_apis::acp::AcpEvent;

#[test]
fn acp_message_chunk_round_trips_provider_owner() {
    let event = AcpEvent::MessageChunk {
        id: "evt-1".into(),
        created_at: "2026-05-05T00:00:00Z".into(),
        session_id: "session-1".into(),
        seq: 1,
        provider: "claude-acp".into(),
        role: "assistant".into(),
        text: "I can continue from the handoff.".into(),
        message_id: "msg-1".into(),
    };

    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["provider"], "claude-acp");
    let decoded: AcpEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.provider_id(), Some("claude-acp"));
}

#[test]
fn acp_provider_switch_event_round_trips_visible_continuity() {
    let event = AcpEvent::ProviderSwitch {
        id: "evt-switch".into(),
        created_at: "2026-05-05T00:00:00Z".into(),
        session_id: "session-1".into(),
        seq: 2,
        from_provider: "codex-acp".into(),
        to_provider: "claude-acp".into(),
        continuity_mode: "handoff".into(),
        message: "Continuing with Claude ACP using a bounded transcript handoff.".into(),
    };

    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["kind"], "provider_switch");
    assert_eq!(value["to_provider"], "claude-acp");
    let decoded: AcpEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.seq(), 2);
}

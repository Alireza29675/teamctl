use super::*;

#[test]
fn protocol_roundtrip_defaults_and_pairing_debug_are_safe() {
    use super::protocol::*;
    let message = decode_adapter(br#"{"type":"message","external_id":"1","peer_id":"opaque:peer","sender_id":"human","text":"hello"}"#).unwrap();
    assert!(
        matches!(message, AdapterFrame::Message { ref kind, payload: None, .. } if kind == "text")
    );
    assert_eq!(
        decode_adapter(&encode_adapter(&message).unwrap()).unwrap(),
        message
    );
    let secret = AdapterFrame::Pairing {
        qr: "ephemeral pairing secret".into(),
    };
    assert!(!format!("{secret:?}").contains("ephemeral pairing secret"));
    let outbound = HostFrame::Send {
        request_id: "delivery:1".into(),
        peer_id: "peer".into(),
        message: OutboundMessage {
            text: "سلام é 📎\nline two".into(),
            kind: "text".into(),
            payload: None,
            reply_to_external_id: Some("source:7".into()),
        },
    };
    assert_eq!(
        decode_host(&encode_host(&outbound).unwrap()).unwrap(),
        outbound
    );
}

#[test]
fn protocol_rejects_malformed_oversize_unknown_and_privileged_frames() {
    use super::protocol::*;
    for input in [
        r#"{"type":"ready","version":2,"capabilities":["text"],"display_name":"Fixture"}"#,
        r#"{"type":"message","external_id":"1","peer_id":"peer","sender_id":"human","text":"hi","kind":"system"}"#,
        r#"{"type":"message","external_id":"1","peer_id":"peer","sender_id":"human","text":"hi","unexpected":true}"#,
        r#"{"type":"message","external_id":"1","peer_id":"","sender_id":"human","text":"hi"}"#,
        r#"{"type":"unknown"}"#,
        "not-json",
        "{\n\"type\":\"connection\",\"state\":\"connected\"}",
    ] {
        assert!(decode_adapter(input.as_bytes()).is_err(), "{input}");
    }
    assert!(decode_adapter(&vec![b' '; MAX_FRAME_BYTES + 1]).is_err());
    let oversized = AdapterFrame::Pairing {
        qr: "x".repeat(MAX_TEXT_BYTES + 1),
    };
    assert!(encode_adapter(&oversized).is_err());
    let mut text = inbound("peer", "1");
    text.text = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(Repository::new(&connection()).ingest(text).is_err());
}

#[test]
fn malformed_pairing_frame_error_does_not_echo_secret_keys() {
    let frame = br#"{"type":"pairing","qr":"secret","ephemeral-secret-key":true}"#;
    let error = super::protocol::decode_adapter(frame).unwrap_err();
    assert!(!format!("{error:?}").contains("ephemeral-secret-key"));
    assert!(!error.to_string().contains("secret"));
}

#[test]
fn capability_bounds_and_control_direction_match_repository() {
    use super::protocol::*;
    let conn = connection();
    let repo = Repository::new(&conn);
    let capabilities: Vec<_> = (0..32).map(|i| format!("cap{i}")).collect();
    repo.register_capabilities("fixture", &capabilities)
        .unwrap();
    let mut frame = AdapterFrame::Ready {
        version: VERSION,
        capabilities,
        display_name: "Fixture".into(),
    };
    assert!(encode_adapter(&frame).is_ok());
    if let AdapterFrame::Ready { capabilities, .. } = &mut frame {
        capabilities.push("extra".into());
        assert!(repo.register_capabilities("fixture", capabilities).is_err());
    }
    assert!(encode_adapter(&frame).is_err());
    for kind in ["typing", "reaction"] {
        let input = serde_json::json!({"type":"message","external_id":"1","peer_id":"p","sender_id":"s","text":"","kind":kind});
        assert!(decode_adapter(input.to_string().as_bytes()).is_err());
    }
}

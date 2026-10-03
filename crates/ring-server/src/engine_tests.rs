use super::*;
use tempfile::TempDir;

struct Fixture {
    engine: Engine,
    _dir: TempDir,
    alice: Session,
    bob: Session,
    carol: Session,
    outsider: Session,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = Engine::open(dir.path()).unwrap();
        let mut login = |actor: &str, org: &str, admin: bool| {
            let result = engine
                .login(
                    Identity {
                        actor: actor.into(),
                        org_id: org.into(),
                        realm: "test".into(),
                        display_name: actor.into(),
                        admin,
                    },
                    None,
                )
                .unwrap();
            engine
                .session(result["session_token"].as_str().unwrap())
                .unwrap()
        };
        let alice = login("si:alice", "org", true);
        let bob = login("si:bob", "org", false);
        let carol = login("si:carol", "org", false);
        let outsider = login("si:alice", "other", true);
        Self {
            engine,
            _dir: dir,
            alice,
            bob,
            carol,
            outsider,
        }
    }
}
fn op(e: &mut Engine, s: &Session, m: &str, p: Value) -> Value {
    let response = e.request(s, &id("test_request"), m, p);
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}
fn fail(e: &mut Engine, s: &Session, m: &str, p: Value) -> String {
    let response = e.request(s, &id("test_request"), m, p);
    assert_eq!(response["ok"], false, "{response}");
    response["error"]["code"].as_str().unwrap().into()
}
fn dial(e: &mut Engine, s: &Session, target: &str) -> String {
    op(e, s, "calls.init", json!({"target":target}))["ringid"]
        .as_str()
        .unwrap()
        .into()
}
fn voicemail_off(e: &mut Engine, s: &Session) {
    op(
        e,
        s,
        "config.set",
        json!({"scope":"actor","values":{"voicemail.enabled":false}}),
    );
}

#[test]
fn acceptance_preparation_freezes_offer_and_checks_actor_and_deadline() {
    let Fixture {
        mut engine,
        alice,
        bob,
        carol,
        ..
    } = Fixture::new();
    let ring = dial(&mut engine, &alice, "si:bob");
    let preview = op(
        &mut engine,
        &bob,
        "calls.prepare",
        json!({"action":"accept","ringid":ring,"context":"Reviewed"}),
    );
    assert_eq!(
        preview["invitation_id"],
        engine.state.calls[&ring].invitations[0].invitation_id
    );
    assert_eq!(
        fail(
            &mut engine,
            &carol,
            "calls.prepare",
            json!({"action":"accept","ringid":ring})
        ),
        "FORBIDDEN"
    );
    assert_eq!(
        fail(
            &mut engine,
            &alice,
            "calls.prepare",
            json!({"action":"accept","ringid":ring})
        ),
        "INVITATION_UNAVAILABLE"
    );
    let old = engine.state.calls[&ring].invitations[0].clone();
    {
        let call = engine.state.calls.get_mut(&ring).unwrap();
        call.invitations[0].state = "declined".into();
        let mut replacement = old.clone();
        replacement.invitation_id = id("offer");
        call.invitations.push(replacement);
    }
    assert_eq!(
        fail(
            &mut engine,
            &bob,
            "calls.accept",
            json!({"ringid":ring,"preparation_id":preview["preparation_id"]})
        ),
        "INVALID_INPUT"
    );
    {
        let call = engine.state.calls.get_mut(&ring).unwrap();
        call.invitations = vec![old];
        call.invitations[0].expires_at = after(-1);
    }
    assert_eq!(
        fail(
            &mut engine,
            &bob,
            "calls.prepare",
            json!({"action":"accept","ringid":ring})
        ),
        "INVITATION_EXPIRED"
    );
    engine.state.calls.get_mut(&ring).unwrap().invitations[0].expires_at = after(60);
    let result = op(
        &mut engine,
        &bob,
        "calls.accept",
        json!({"ringid":ring,"preparation_id":preview["preparation_id"]}),
    );
    assert_eq!(result["state"], "active");
}

#[test]
fn disabled_voicemail_ends_busy_declined_and_timed_out_direct_calls() {
    for outcome in ["busy", "declined", "timeout"] {
        let Fixture {
            mut engine,
            alice,
            bob,
            carol,
            ..
        } = Fixture::new();
        voicemail_off(&mut engine, &bob);
        if outcome == "busy" {
            dial(&mut engine, &bob, "si:carol");
        }
        let ring = dial(&mut engine, &alice, "si:bob");
        if outcome == "declined" {
            op(
                &mut engine,
                &bob,
                "calls.decline",
                json!({"ringid":ring,"give_no_reason":true}),
            );
        }
        if outcome == "timeout" {
            engine.state.calls.get_mut(&ring).unwrap().invitations[0].expires_at = after(-1);
            assert!(engine.tick(false));
        }
        let call = &engine.state.calls[&ring];
        assert_eq!(call.state, "ended", "{outcome}");
        assert!(call.ended_at.is_some());
        assert!(engine
            .state
            .events
            .iter()
            .any(|e| e.kind == "call.ended" && e.data["ringid"] == ring));
        assert!(!engine
            .state
            .events
            .iter()
            .any(|e| e.kind == "voicemail.offered" && e.data["ringid"] == ring));
        assert_eq!(
            fail(
                &mut engine,
                &alice,
                "voicemail.begin",
                json!({"ringid":ring,"format":"text"})
            ),
            "VOICEMAIL_DISABLED"
        );
        // Failed invitations do not affect an existing independent call or its reservation.
        if outcome == "busy" {
            assert!(engine.state.busy(&bob.identity.actor, "test", None));
        }
        let _ = carol;
    }
}

#[test]
fn one_failed_offer_does_not_end_other_pending_or_connected_participants() {
    let Fixture {
        mut engine,
        alice,
        bob,
        carol,
        ..
    } = Fixture::new();
    voicemail_off(&mut engine, &bob);
    voicemail_off(&mut engine, &carol);
    let ring = dial(&mut engine, &alice, "si:bob");
    op(
        &mut engine,
        &alice,
        "calls.invite",
        json!({"ringid":ring,"target":"si:carol"}),
    );
    op(
        &mut engine,
        &bob,
        "calls.decline",
        json!({"ringid":ring,"give_no_reason":true}),
    );
    assert_eq!(engine.state.calls[&ring].state, "ringing");
    op(&mut engine, &carol, "calls.accept", json!({"ringid":ring}));
    assert_eq!(engine.state.calls[&ring].state, "active");
    op(
        &mut engine,
        &alice,
        "calls.invite",
        json!({"ringid":ring,"target":"si:bob"}),
    );
    let call = engine.state.calls.get_mut(&ring).unwrap();
    call.invitations.last_mut().unwrap().expires_at = after(-1);
    engine.tick(false);
    assert_eq!(engine.state.calls[&ring].state, "active");
}

#[test]
fn asset_metadata_and_provider_config_preserve_boundaries() {
    let Fixture {
        mut engine,
        alice,
        bob,
        outsider,
        ..
    } = Fixture::new();
    let asset = op(
        &mut engine,
        &alice,
        "assets.begin",
        json!({"purpose":"profile_photo","mime_type":"image/png","size_bytes":8}),
    );
    assert!(asset.get("path").is_none());
    assert!(asset.get("owner").is_none());
    assert_eq!(
        fail(
            &mut engine,
            &bob,
            "assets.get",
            json!({"asset_id":asset["asset_id"]})
        ),
        "FORBIDDEN"
    );
    assert_eq!(
        fail(
            &mut engine,
            &outsider,
            "assets.get",
            json!({"asset_id":asset["asset_id"]})
        ),
        "FORBIDDEN"
    );
    let provider = json!({"scope":"org","values":{"providers.live":{"api_key":"sealed-example","model":"published-model"}}});
    assert_eq!(
        fail(&mut engine, &bob, "config.set", provider.clone()),
        "FORBIDDEN"
    );
    let result = op(&mut engine, &alice, "config.set", provider);
    assert_eq!(result["values"]["providers.live"]["api_key"], "[redacted]");
    assert_eq!(
        engine.state.configs[&key("test", "org", "*")].values["providers.live"]["api_key"],
        "sealed-example"
    );
    assert_eq!(
        fail(
            &mut engine,
            &alice,
            "config.set",
            json!({"scope":"actor","values":{"providers.live":{"api_key":"x"}}})
        ),
        "FORBIDDEN"
    );
    assert_eq!(
        fail(
            &mut engine,
            &alice,
            "config.set",
            json!({"scope":"org","values":{"providers.live":{"endpoint":"https://evil.invalid"}}})
        ),
        "INVALID_INPUT"
    );
    let effective = op(&mut engine, &bob, "config.get", json!({"scope":"actor"}));
    assert_eq!(
        effective["effective"]["providers.live"]["api_key"],
        "[redacted]"
    );
}

#[test]
fn failed_persistence_restores_exact_preoperation_memory() {
    let Fixture {
        mut engine, alice, ..
    } = Fixture::new();
    let before = serde_json::to_value(&engine.state).unwrap();
    engine.db.execute_batch("PRAGMA query_only=ON").unwrap();
    assert_eq!(
        fail(
            &mut engine,
            &alice,
            "profile.update",
            json!({"display_name":"Must roll back"})
        ),
        "STORAGE_FAILED"
    );
    assert_eq!(serde_json::to_value(&engine.state).unwrap(), before);
    engine.db.execute_batch("PRAGMA query_only=OFF").unwrap();
    assert_ne!(
        op(&mut engine, &alice, "profile.get", json!({}))["display_name"],
        "Must roll back"
    );
}

#[test]
fn recordings_report_real_gap_coverage() {
    let Fixture {
        mut engine,
        alice,
        bob,
        ..
    } = Fixture::new();
    let ring = dial(&mut engine, &alice, "si:bob");
    op(&mut engine, &bob, "calls.accept", json!({"ringid":ring}));
    engine.state.recording_gaps.insert(
        ring.clone(),
        vec![json!({"start_ms":0,"end_ms":20,"reason":"network_gap"})],
    );
    let recording = op(
        &mut engine,
        &alice,
        "recordings.get",
        json!({"ringid":ring}),
    );
    assert_eq!(recording["coverage"], "partial");
    assert_eq!(recording["missing_intervals"].as_array().unwrap().len(), 1);
    assert!(recording["duration_ms"].is_number());
}

#[test]
fn encrypted_retry_fingerprints_and_push_tokens_remain_private() {
    let Fixture {
        mut engine,
        alice,
        bob,
        ..
    } = Fixture::new();
    let fingerprint = digest("original plaintext request fingerprint");
    let one = engine.request_with_fingerprint(
        &alice,
        "sealed-retry",
        "config.set",
        json!({"scope":"org","values":{"providers.live":{"api_key":"nonce-one-ciphertext"}}}),
        fingerprint.clone(),
    );
    let two = engine.request_with_fingerprint(
        &alice,
        "sealed-retry",
        "config.set",
        json!({"scope":"org","values":{"providers.live":{"api_key":"nonce-two-ciphertext"}}}),
        fingerprint,
    );
    assert_eq!(one, two);
    assert_eq!(one["ok"], true);
    let result = op(
        &mut engine,
        &alice,
        "devices.update",
        json!({"device_id":alice.device_id,"push_platform":"apns_voip","push_token":"private-APNS-token"}),
    );
    assert_eq!(result["push_enabled"], true);
    assert!(result.get("push_token").is_none());
    let listed = op(&mut engine, &alice, "devices.list", json!({}));
    assert!(!listed.to_string().contains("private-APNS-token"));
    assert_eq!(
        fail(
            &mut engine,
            &bob,
            "devices.update",
            json!({"device_id":alice.device_id,"push_platform":"fcm","push_token":"other"})
        ),
        "FORBIDDEN"
    );
    assert_eq!(
        fail(
            &mut engine,
            &alice,
            "devices.update",
            json!({"device_id":alice.device_id,"push_platform":"unsupported"})
        ),
        "INVALID_INPUT"
    );
}

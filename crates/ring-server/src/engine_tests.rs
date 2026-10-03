use super::*;
use tempfile::TempDir;

#[test]
fn logout_clears_push_registration_and_pending_delivery() {
    let mut f = Fixture::new();
    op(
        &mut f.engine,
        &f.alice,
        "devices.update",
        json!({"device_id":f.alice.device_id,"push_platform":"apns_voip","push_environment":"sandbox","push_token":"private-device-token"}),
    );
    f.engine.state.pushes.insert(
        "queued".into(),
        json!({"device_id":f.alice.device_id,"status":"pending"}),
    );
    op(&mut f.engine, &f.alice, "auth.logout", json!({}));
    assert!(f.engine.state.devices[&f.alice.device_id]
        .push_token
        .is_none());
    assert!(f.engine.state.pushes.is_empty());
}

#[test]
fn delayed_captions_are_scoped_to_the_audio_participation_window() {
    let mut f = Fixture::new();
    let ring = dial(&mut f.engine, &f.alice, "si:bob");
    op(
        &mut f.engine,
        &f.bob,
        "calls.accept",
        json!({"ringid":ring}),
    );
    let call = f.engine.state.calls.get_mut(&ring).unwrap();
    call.answered_at = Some("2026-01-01T00:00:00Z".into());
    let bob = call
        .participants
        .iter_mut()
        .find(|p| p.actor == "si:bob")
        .unwrap();
    bob.joined_at = "2026-01-01T00:00:10Z".into();
    bob.left_at = Some("2026-01-01T00:00:20Z".into());
    let mut caption = Entry {
        seq: 999,
        occurred_at: "2026-01-01T00:00:30Z".into(),
        kind: "speech".into(),
        actor: None,
        data: json!({"start_ms":12000,"end_ms":14000}),
        private_to: None,
    };
    assert!(entry_visible(call, &caption, "si:bob"));
    caption.data = json!({"start_ms":2000,"end_ms":4000});
    assert!(!entry_visible(call, &caption, "si:bob"));
    caption.data = json!({"start_ms":9000,"end_ms":12000});
    assert!(!entry_visible(call, &caption, "si:bob"));
    caption.data = json!({"start_ms":19000,"end_ms":21000});
    assert!(!entry_visible(call, &caption, "si:bob"));
}

#[test]
fn event_cursors_stay_monotonic_after_retention_and_outbox_sends_recover() {
    let mut f = Fixture::new();
    f.engine.state.event(
        &f.alice.identity,
        vec!["si:alice".into()],
        "call.incoming",
        json!({"ringid":"example"}),
    );
    let first = f.engine.state.latest_event_seq();
    f.engine.state.events.clear();
    f.engine.state.event(
        &f.alice.identity,
        vec!["si:alice".into()],
        "call.ended",
        json!({"ringid":"example"}),
    );
    assert_eq!(f.engine.state.latest_event_seq(), first + 1);
    let notification = f.engine.state.publications.values_mut().next().unwrap();
    notification.status = "sending".into();
    let id = notification.notification_id.clone();
    f.engine.persist().unwrap();
    let reopened = Engine::open(f._dir.path()).unwrap();
    assert_eq!(reopened.state.publications[&id].status, "pending");
}

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
        let outsider = login("si:eve", "other", true);
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

#[test]
fn global_ids_use_participant_access_and_pinned_own_credentials() {
    let mut f = Fixture::new();
    let mut other = f.bob.identity.clone();
    other.org_id = "bobs-own-org".into();
    let token = f.engine.login(other.clone(), None).unwrap();
    let bob = f
        .engine
        .session(token["session_token"].as_str().unwrap())
        .unwrap();
    let ring = dial(&mut f.engine, &f.alice, "si:bob[legacy-other-org]");
    assert_eq!(
        f.engine.state.calls[&ring].identity("si:bob").org_id,
        "bobs-own-org"
    );
    assert!(f
        .engine
        .state
        .publications
        .values()
        .any(|n| n.actor == "si:bob"
            && n.org_id == "bobs-own-org"
            && n.event_type == "call.incoming"));
    op(&mut f.engine, &bob, "calls.accept", json!({"ringid":ring}));
    // Logging in through a different own organization never changes this call's credentials.
    f.engine.login(f.bob.identity.clone(), None).unwrap();
    f.engine.tick(true);
    assert!(f
        .engine
        .state
        .publications
        .values()
        .any(|n| n.actor == "si:bob"
            && n.org_id == "bobs-own-org"
            && n.event_type == "transcript.batch"));
    assert_eq!(
        op(&mut f.engine, &bob, "calls.get", json!({"ringid":ring}))["state"],
        "active"
    );
    assert_eq!(
        op(&mut f.engine, &f.bob, "calls.get", json!({"ringid":ring}))["state"],
        "active"
    );
    let mut alternate_realm = bob.clone();
    alternate_realm.identity.realm = "production".into();
    assert_eq!(
        fail(
            &mut f.engine,
            &alternate_realm,
            "calls.get",
            json!({"ringid":ring})
        ),
        "FORBIDDEN"
    );
    assert_eq!(
        fail(
            &mut f.engine,
            &f.outsider,
            "calls.get",
            json!({"ringid":ring})
        ),
        "FORBIDDEN"
    );
    assert!(op(&mut f.engine, &bob, "calls.list", json!({}))["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["ringid"] == ring));
    assert_eq!(
        actor_id("si:bob[]", "org").unwrap_err().code,
        "INVALID_INPUT"
    );
    assert_eq!(
        actor_id("si:bob[x][y]", "org").unwrap_err().code,
        "INVALID_INPUT"
    );
    assert_eq!(
        actor_id("si:bob[x]tail", "org").unwrap_err().code,
        "INVALID_INPUT"
    );
}

#[test]
fn foreign_recipient_voicemail_preferences_and_notification_route_are_preserved() {
    let mut f = Fixture::new();
    let mut identity = f.bob.identity.clone();
    identity.org_id = "bobs-own-org".into();
    let token = f.engine.login(identity, None).unwrap();
    let bob = f
        .engine
        .session(token["session_token"].as_str().unwrap())
        .unwrap();
    voicemail_off(&mut f.engine, &bob);
    let ring = dial(&mut f.engine, &f.alice, "si:bob");
    op(
        &mut f.engine,
        &bob,
        "calls.decline",
        json!({"ringid":ring,"give_no_reason":true}),
    );
    assert_eq!(f.engine.state.calls[&ring].state, "ended");
    assert_eq!(
        fail(
            &mut f.engine,
            &f.alice,
            "voicemail.begin",
            json!({"ringid":ring,"format":"text"})
        ),
        "VOICEMAIL_DISABLED"
    );
    op(
        &mut f.engine,
        &bob,
        "config.set",
        json!({"scope":"actor","values":{"voicemail.enabled":true}}),
    );
    let draft = op(
        &mut f.engine,
        &f.alice,
        "voicemail.begin",
        json!({"ringid":ring,"format":"text"}),
    );
    let vid = draft["voicemail_id"].as_str().unwrap();
    assert_eq!(draft["recipient_org_id"], "bobs-own-org");
    assert_eq!(draft["org_id"], "org");
    assert_eq!(
        fail(
            &mut f.engine,
            &bob,
            "voicemail.get",
            json!({"voicemail_id":vid})
        ),
        "FORBIDDEN"
    );
    // Delivery itself is covered with real recorded PCM by test-server.mjs.
    let vm = f.engine.state.voicemails.get_mut(vid).unwrap();
    vm.complete_audio = true;
    vm.audio_asset_id = Some("recorded-asset".into());
    vm.text = Some("Private message".into());
    f.engine.login(f.bob.identity.clone(), None).unwrap();
    op(
        &mut f.engine,
        &f.alice,
        "voicemail.commit",
        json!({"voicemail_id":vid}),
    );
    assert_eq!(
        op(
            &mut f.engine,
            &bob,
            "voicemail.get",
            json!({"voicemail_id":vid})
        )["state"],
        "delivered"
    );
    assert!(f
        .engine
        .state
        .publications
        .values()
        .any(|n| n.actor == "si:bob"
            && n.org_id == "bobs-own-org"
            && n.event_type == "voicemail.received"));
    assert_eq!(
        fail(
            &mut f.engine,
            &f.outsider,
            "voicemail.get",
            json!({"voicemail_id":vid})
        ),
        "FORBIDDEN"
    );
}

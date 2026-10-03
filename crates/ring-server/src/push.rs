use crate::{model::*, App};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use std::time::Duration;
struct Apple {
    key: EncodingKey,
    key_id: String,
    team_id: String,
    topic: String,
    sandbox: bool,
    token: Option<(i64, String)>,
}
struct Firebase {
    key: EncodingKey,
    email: String,
    project: String,
    token: Option<(i64, String)>,
}
pub struct Push {
    client: reqwest::Client,
    apple: Option<Apple>,
    firebase: Option<Firebase>,
}
fn config_error(message: &str) -> Fault {
    Fault::new(
        "PUSH_CONFIGURATION",
        message,
        "Configure the APNs signing key or Firebase service account on the server.",
    )
}
fn network() -> Fault {
    let mut e = Fault::new(
        "PUSH_UNAVAILABLE",
        "The platform push service did not confirm delivery.",
        "The server will retry while the incoming offer is still valid.",
    );
    e.retryable = true;
    e
}
fn required_env(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| config_error(&format!("Set {name}")))
}
impl Push {
    pub fn new() -> Result<Self> {
        ring_providers::init_tls();
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| network())?;
        let apple = if std::env::var("RING_APNS_KEY_FILE").is_ok()
            || std::env::var("RING_APNS_KEY_BASE64").is_ok()
        {
            let bytes = if let Ok(path) = std::env::var("RING_APNS_KEY_FILE") {
                std::fs::read(path)
                    .map_err(|_| config_error("Could not read RING_APNS_KEY_FILE"))?
            } else {
                STANDARD
                    .decode(required_env("RING_APNS_KEY_BASE64")?)
                    .map_err(|_| config_error("APNs key is not valid base64"))?
            };
            let key = EncodingKey::from_ec_pem(&bytes)
                .map_err(|_| config_error("APNs requires the Apple .p8 EC private key"))?;
            let bundle = std::env::var("RING_APNS_BUNDLE_ID")
                .unwrap_or_else(|_| "com.teamofsilicons.ring".into());
            Some(Apple {
                key,
                key_id: required_env("RING_APNS_KEY_ID")?,
                team_id: required_env("RING_APNS_TEAM_ID")?,
                topic: format!("{bundle}.voip"),
                sandbox: std::env::var("RING_APNS_SANDBOX").as_deref() == Ok("true"),
                token: None,
            })
        } else {
            None
        };
        let firebase = if std::env::var("RING_FCM_SERVICE_ACCOUNT_FILE").is_ok()
            || std::env::var("RING_FCM_SERVICE_ACCOUNT_BASE64").is_ok()
        {
            let bytes = if let Ok(path) = std::env::var("RING_FCM_SERVICE_ACCOUNT_FILE") {
                std::fs::read(path)
                    .map_err(|_| config_error("Could not read Firebase service-account JSON"))?
            } else {
                STANDARD
                    .decode(required_env("RING_FCM_SERVICE_ACCOUNT_BASE64")?)
                    .map_err(|_| config_error("Firebase credentials are not valid base64"))?
            };
            let v: Value = serde_json::from_slice(&bytes)
                .map_err(|_| config_error("Invalid Firebase service-account JSON"))?;
            if v["type"] != "service_account" {
                return Err(config_error(
                    "Use a Firebase service account key, not google-services.json",
                ));
            }
            let key = EncodingKey::from_rsa_pem(required(&v, "private_key")?.as_bytes())
                .map_err(|_| config_error("Firebase service-account RSA key is invalid"))?;
            Some(Firebase {
                key,
                email: required(&v, "client_email")?.into(),
                project: required(&v, "project_id")?.into(),
                token: None,
            })
        } else {
            None
        };
        Ok(Self {
            client,
            apple,
            firebase,
        })
    }
    pub async fn send(
        &mut self,
        platform: &str,
        token: &str,
        environment: Option<&str>,
        payload: &Value,
        delivery_id: &str,
        expires: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        if platform == "apns_voip" {
            let apple = self
                .apple
                .as_mut()
                .ok_or_else(|| config_error("APNs is not configured"))?;
            if apple.token.as_ref().is_none_or(|(at, _)| now - at > 1800) {
                let mut header = Header::new(Algorithm::ES256);
                header.kid = Some(apple.key_id.clone());
                let signed = encode(&header, &json!({"iss":apple.team_id,"iat":now}), &apple.key)
                    .map_err(|_| config_error("APNs JWT signing failed"))?;
                apple.token = Some((now, signed));
            }
            let host = if environment.map_or(apple.sandbox, |e| e == "sandbox") {
                "api.sandbox.push.apple.com"
            } else {
                "api.push.apple.com"
            };
            let response = self
                .client
                .post(format!("https://{host}/3/device/{token}"))
                .bearer_auth(&apple.token.as_ref().unwrap().1)
                .header("apns-push-type", "voip")
                .header("apns-topic", &apple.topic)
                .header("apns-priority", "10")
                .header("apns-expiration", expires.to_string())
                .header("apns-id", delivery_id)
                .json(payload)
                .send()
                .await
                .map_err(|_| network())?;
            let status = response.status();
            if status.is_success() {
                return Ok(());
            }
            let code = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["reason"].as_str().map(String::from))
                .unwrap_or_else(|| "Unknown".into());
            let mut fault = Fault::new(
                "PUSH_REJECTED",
                format!("APNs returned HTTP {} ({code}).", status.as_u16()),
                "Check the key environment, bundle ID, and device push token.",
            );
            fault.retryable = status.as_u16() == 429 || status.is_server_error();
            return Err(fault);
        }
        if platform == "fcm" {
            let firebase = self
                .firebase
                .as_mut()
                .ok_or_else(|| config_error("Firebase Cloud Messaging is not configured"))?;
            if firebase
                .token
                .as_ref()
                .is_none_or(|(expiry, _)| *expiry <= now + 60)
            {
                let assertion=encode(&Header::new(Algorithm::RS256),&json!({"iss":firebase.email,"scope":"https://www.googleapis.com/auth/firebase.messaging","aud":"https://oauth2.googleapis.com/token","iat":now,"exp":now+3600}),&firebase.key).map_err(|_|config_error("Firebase JWT signing failed"))?;
                let response = self
                    .client
                    .post("https://oauth2.googleapis.com/token")
                    .form(&[
                        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                        ("assertion", assertion.as_str()),
                    ])
                    .send()
                    .await
                    .map_err(|_| network())?;
                if !response.status().is_success() {
                    return Err(config_error(
                        "Google did not authorize the service account for Firebase messaging",
                    ));
                }
                let value: Value = response.json().await.map_err(|_| network())?;
                firebase.token = Some((
                    now + value["expires_in"].as_i64().unwrap_or(3600),
                    required(&value, "access_token")?.into(),
                ));
            }
            let data: serde_json::Map<String, Value> = payload
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| *k != "aps")
                .map(|(k, v)| {
                    (
                        k.clone(),
                        json!(v
                            .as_str()
                            .map(String::from)
                            .unwrap_or_else(|| v.to_string())),
                    )
                })
                .collect();
            let response=self.client.post(format!("https://fcm.googleapis.com/v1/projects/{}/messages:send",firebase.project)).bearer_auth(&firebase.token.as_ref().unwrap().1).json(&json!({"message":{"token":token,"data":data,"android":{"priority":"HIGH","ttl":format!("{}s",(expires-now).clamp(0,60)),"collapse_key":payload["call_uuid"]}}})).send().await.map_err(|_|network())?;
            if response.status().is_success() {
                return Ok(());
            }
            let mut fault = Fault::new(
                "PUSH_REJECTED",
                format!("Firebase returned HTTP {}.", response.status().as_u16()),
                "Check the service account's messaging permission and current device token.",
            );
            fault.retryable =
                response.status().as_u16() == 429 || response.status().is_server_error();
            return Err(fault);
        }
        Err(invalid("Unknown native push platform"))
    }
}
pub fn start(app: App) {
    if app.providers_disabled {
        return;
    }
    let mut push = match Push::new() {
        Ok(p) => p,
        Err(error) => {
            tracing::error!(code=%error.code,message=%error.message,"Native push configuration failed");
            return;
        }
    };
    let test_push_enabled = std::env::var("RING_TEST_NATIVE_PUSH_ENABLED").as_deref() == Ok("true");
    app.clone().spawn_draining(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! { biased; _ = app.activity.cancelled.cancelled() => break, _ = tick.tick() => {} }
            let tasks = {
                let mut e = app.engine.lock().unwrap();
                let events: Vec<_> = e
                    .state
                    .events
                    .iter()
                    .filter(|v| v.seq > e.state.push_cursor && v.kind == "call.incoming")
                    .cloned()
                    .collect();
                for event in events {
                    let Some(ring) = event.data["ringid"].as_str() else {
                        continue;
                    };
                    let Some(call) = e.state.calls.get(ring).cloned() else {
                        continue;
                    };
                    let Some(offer) = call
                        .invitations
                        .iter()
                        .find(|v| v.invitation_id == event.data["invitation_id"])
                        .cloned()
                    else {
                        continue;
                    };
                    if offer.state != "pending" || offer.expires_at <= now() {
                        continue;
                    }
                    let devices: Vec<_> = e
                        .state
                        .devices
                        .values()
                        .filter(|d| {
                            push_identity(
                                &e.state,
                                &call.realm,
                                &offer.target,
                                d,
                                test_push_enabled,
                            )
                            .is_some()
                        })
                        .cloned()
                        .collect();
                    let inviter = call.identity(&offer.inviter);
                    let name = e
                        .state
                        .profiles
                        .get(&key(&inviter.realm, &inviter.org_id, &inviter.actor))
                        .map(|p| p.display_name.clone())
                        .unwrap_or_else(|| offer.inviter.clone());
                    for device in devices {
                        let did = format!("{}:{}", offer.invitation_id, device.device_id);
                        let uuid = uuid::Uuid::parse_str(ring.trim_start_matches("ring_"))
                            .map(|u| u.to_string())
                            .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
                        let payload = json!({"aps":{"content-available":1},"type":"call.incoming","ringid":ring,"invitation_id":offer.invitation_id,"call_uuid":uuid,"caller":offer.inviter,"display_name":name,"expires_at":offer.expires_at,"participants_count":call.participants.len()});
                        e.state.pushes.entry(did).or_insert(json!({"device_id":device.device_id,"ringid":ring,"invitation_id":offer.invitation_id,"payload":payload,"apns_id":uuid::Uuid::new_v4().to_string(),"status":"pending","attempts":0,"retry_at":now()}));
                    }
                }
                let latest = e.state.latest_event_seq();
                let previous_cursor = e.state.push_cursor;
                let changed = latest != previous_cursor;
                e.state.push_cursor = latest;
                if changed && e.persist().is_err() {
                    // Reprocess these stable event/device IDs before sending anything.
                    e.state.push_cursor = previous_cursor;
                    continue;
                }
                e.state
                    .pushes
                    .iter()
                    .filter(|(_, v)| {
                        v["status"] == "pending"
                            && v["retry_at"].as_str().is_none_or(|t| t <= now().as_str())
                    })
                    .take(10)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            };
            for (id, task) in tasks {
                if app.activity.cancelled.is_cancelled() { break; }
                let device = {
                    let e = app.engine.lock().unwrap();
                    pending_push_device(&e.state, &task, test_push_enabled)
                };
                let outcome = if let Some((_, identity)) = device {
                    let expiry = task["payload"]["expires_at"]
                        .as_str()
                        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.timestamp())
                        .unwrap_or(0);
                    match crate::auth::verify(&app, &identity).await {
                        Ok(_) => {
                            // Logout, device revocation, or a cut may happen during introspection.
                            let current = {
                                let e = app.engine.lock().unwrap();
                                pending_push_device(&e.state, &task, test_push_enabled).filter(
                                    |(_, i)| {
                                        i.actor == identity.actor
                                            && i.realm == identity.realm
                                            && i.org_id == identity.org_id
                                    },
                                )
                            };
                            if app.activity.cancelled.is_cancelled() { break; }
                            if let Some((d, _)) = current {
                                push.send(
                                    d.push_platform.as_deref().unwrap_or(""),
                                    d.push_token.as_deref().unwrap_or(""),
                                    d.push_environment.as_deref(),
                                    &task["payload"],
                                    task["apns_id"].as_str().unwrap_or(""),
                                    expiry,
                                )
                                .await
                            } else {
                                Ok(())
                            }
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(())
                };
                let mut e = app.engine.lock().unwrap();
                if let Some(v) = e.state.pushes.get_mut(&id) {
                    let attempts = v["attempts"].as_u64().unwrap_or(0) + 1;
                    v["attempts"] = json!(attempts);
                    match outcome {
                        Ok(()) => {
                            v["status"] = json!("delivered");
                        }
                        Err(error) => {
                            v["status"] = json!(if error.retryable && attempts < 5 {
                                "pending"
                            } else {
                                "failed"
                            });
                            v["error"] = json!(error);
                            v["retry_at"] = json!(after(2i64.pow(attempts as u32)));
                        }
                    }
                }
                let _ = e.persist();
            }
        }
    });
}
fn pending_push_device(
    state: &State,
    task: &Value,
    test_push_enabled: bool,
) -> Option<(Device, Identity)> {
    let call = state.calls.get(task["ringid"].as_str()?)?;
    let offer = call.invitations.iter().find(|o| {
        o.invitation_id == task["invitation_id"]
            && o.state == "pending"
            && !o.silenced
            && o.expires_at > now()
    })?;
    let device = state.devices.get(task["device_id"].as_str()?)?;
    let identity = push_identity(state, &call.realm, &offer.target, device, test_push_enabled)?;
    Some((device.clone(), identity))
}
fn push_identity(
    state: &State,
    realm: &str,
    actor: &str,
    device: &Device,
    test_push_enabled: bool,
) -> Option<Identity> {
    if device.revoked
        || !device.ring_enabled
        || device.push_token.is_none()
        || !allowed_environment(realm, device, test_push_enabled)
    {
        return None;
    }
    state
        .sessions
        .values()
        .find(|s| {
            s.device_id == device.device_id
                && s.expires_at > now()
                && s.identity.realm == realm
                && s.identity.actor == actor
                && device.owner == key(&s.identity.realm, &s.identity.org_id, &s.identity.actor)
        })
        .map(|s| s.identity.clone())
}
fn allowed_environment(realm: &str, device: &Device, test_push_enabled: bool) -> bool {
    realm == "production"
        || (test_push_enabled
            && matches!(device.push_platform.as_deref(), Some("apns_voip" | "fcm"))
            && device.push_environment.as_deref() == Some("sandbox"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_push_target_requires_its_own_live_device_binding() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = crate::engine::Engine::open(dir.path()).unwrap();
        let mut login = |actor: &str, org: &str, realm: &str| {
            let value = engine
                .login(
                    Identity {
                        actor: actor.into(),
                        org_id: org.into(),
                        realm: realm.into(),
                        display_name: actor.into(),
                        admin: false,
                    },
                    None,
                )
                .unwrap();
            engine
                .session(value["session_token"].as_str().unwrap())
                .unwrap()
        };
        let caller = login("si:caller", "caller-org", "production");
        let recipient = login("c:recipient", "recipient-org", "production");
        let other = login("c:other", "recipient-org", "production");
        let isolated = login("c:recipient", "recipient-org", "test");
        for session in [&recipient, &other, &isolated] {
            let d = engine.state.devices.get_mut(&session.device_id).unwrap();
            d.push_token = Some("push-token".into());
            d.push_platform = Some("fcm".into());
        }
        let started = engine
            .dispatch(&caller, "calls.init", &json!({"target":"c:recipient"}))
            .unwrap();
        let ring = started["ringid"].as_str().unwrap();
        let offer = engine.state.calls[ring].invitations[0]
            .invitation_id
            .clone();
        let task = json!({"ringid":ring,"invitation_id":offer,"device_id":recipient.device_id});
        let (_, identity) = pending_push_device(&engine.state, &task, false).unwrap();
        assert_eq!(identity.org_id, "recipient-org");
        for device in [&other.device_id, &isolated.device_id] {
            let mut changed = task.clone();
            changed["device_id"] = json!(device);
            assert!(pending_push_device(&engine.state, &changed, false).is_none());
        }
        let owner = engine.state.devices[&recipient.device_id].owner.clone();
        engine
            .state
            .devices
            .get_mut(&recipient.device_id)
            .unwrap()
            .owner = key("production", "unverified-org", "c:recipient");
        assert!(pending_push_device(&engine.state, &task, false).is_none());
        engine
            .state
            .devices
            .get_mut(&recipient.device_id)
            .unwrap()
            .owner = owner;
        engine
            .state
            .devices
            .get_mut(&recipient.device_id)
            .unwrap()
            .revoked = true;
        assert!(pending_push_device(&engine.state, &task, false).is_none());
        engine
            .state
            .devices
            .get_mut(&recipient.device_id)
            .unwrap()
            .revoked = false;
        engine.state.calls.get_mut(ring).unwrap().invitations[0].silenced = true;
        assert!(pending_push_device(&engine.state, &task, false).is_none());
        engine.state.calls.get_mut(ring).unwrap().invitations[0].silenced = false;
        for session in engine.state.sessions.values_mut() {
            if session.device_id == recipient.device_id {
                session.expires_at = after(-1);
            }
        }
        assert!(pending_push_device(&engine.state, &task, false).is_none());
    }

    #[test]
    fn isolated_calls_never_wake_production_push_tokens() {
        let mut device = Device {
            device_id: "device".into(),
            owner: "test:org:c:alice".into(),
            name: "iPhone".into(),
            ring_enabled: true,
            revoked: false,
            push_token: Some("token".into()),
            push_platform: Some("apns_voip".into()),
            push_environment: Some("production".into()),
        };
        assert!(!allowed_environment("test", &device, false));
        assert!(!allowed_environment("test", &device, true));
        device.push_environment = Some("sandbox".into());
        assert!(!allowed_environment("test", &device, false));
        assert!(allowed_environment("test", &device, true));
        device.push_platform = Some("fcm".into());
        assert!(!allowed_environment("test", &device, false));
        assert!(allowed_environment("test", &device, true));
        device.push_environment = Some("production".into());
        assert!(!allowed_environment("test", &device, true));
        assert!(allowed_environment("production", &device, false));
    }
    #[test]
    fn uuid_of_call_is_stable() {
        let c = id("ring");
        let u = uuid::Uuid::parse_str(c.trim_start_matches("ring_")).unwrap();
        assert_eq!(
            u.to_string().replace('-', ""),
            c.trim_start_matches("ring_")
        );
    }
}

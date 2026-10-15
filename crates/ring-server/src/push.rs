//! Push to carbons: APNs VoIP (iOS/iPadOS ringing, CallKit), APNs alert (missed calls, voicemail,
//! and macOS ringing), FCM v1 data messages (Android) and Web Push (browsers).
//! VoIP pushes carry only `call.incoming` and `call.cancelled`.

use std::time::Duration;

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring_proto::{
    Platform, PushKind, PushParticipant, PushPayload, PushPerson, RingingStoppedReason, Voicemail,
};
use serde_json::{Map, Value, json};

use crate::app::AppState;
use crate::calls::{CallData, InvitationRow};
use crate::config::Config;
use crate::sessions::{PushTarget, push_targets};
use crate::util::now_ms;
use crate::webpush;

const APNS_JWT_TTL_SECS: i64 = 30 * 60;
const CALL_TTL_SECS: i64 = 60;
const ALERT_TTL_SECS: i64 = 24 * 3600;

struct Apns {
    key: EncodingKey,
    key_id: String,
    team_id: String,
    bundle_id: String,
    jwt: std::sync::Mutex<Option<(i64, String)>>,
}

struct Fcm {
    key: EncodingKey,
    email: String,
    project: String,
    token: tokio::sync::Mutex<Option<(i64, String)>>,
}

struct Vapid {
    private_key: [u8; 32],
    public_key: String,
    subject: String,
}

pub struct Push {
    http: reqwest::Client,
    apns: Option<Apns>,
    fcm: Option<Fcm>,
    vapid: Option<Vapid>,
}

enum Outcome {
    Sent,
    InvalidToken,
    Retry(String),
    Failed(String),
}

#[derive(Clone)]
enum Delivery {
    Apns {
        voip: bool,
        body: Value,
        expiration: i64,
        priority: u8,
        collapse: Option<String>,
    },
    Fcm {
        data: Map<String, Value>,
        ttl_secs: i64,
        collapse: Option<String>,
    },
    Web {
        body: Vec<u8>,
        ttl_secs: i64,
    },
}

struct Job {
    device_id: String,
    kind: PushKind,
    token: String,
    sandbox: bool,
    delivery: Delivery,
}

impl Push {
    pub fn new(config: &Config) -> Result<Push, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("ring-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        let apns = match &config.apns {
            Some(c) => Some(Apns {
                key: EncodingKey::from_ec_pem(&c.key_pem)
                    .map_err(|_| "RING_APNS key is not an Apple .p8 EC private key".to_string())?,
                key_id: c.key_id.clone(),
                team_id: c.team_id.clone(),
                bundle_id: c.bundle_id.clone(),
                jwt: std::sync::Mutex::new(None),
            }),
            None => None,
        };
        let fcm = match &config.fcm_service_account {
            Some(bytes) => {
                let v: Value = serde_json::from_slice(bytes)
                    .map_err(|_| "the FCM service account is not JSON".to_string())?;
                if v["type"] != "service_account" {
                    return Err(
                        "use a Firebase service account key, not google-services.json".into(),
                    );
                }
                let pem = v["private_key"]
                    .as_str()
                    .ok_or("the FCM service account has no private_key")?;
                Some(Fcm {
                    key: EncodingKey::from_rsa_pem(pem.as_bytes())
                        .map_err(|_| "the FCM service account key is invalid".to_string())?,
                    email: v["client_email"]
                        .as_str()
                        .ok_or("the FCM service account has no client_email")?
                        .to_string(),
                    project: v["project_id"]
                        .as_str()
                        .ok_or("the FCM service account has no project_id")?
                        .to_string(),
                    token: tokio::sync::Mutex::new(None),
                })
            }
            None => None,
        };
        let vapid = config.vapid.as_ref().map(|v| Vapid {
            private_key: v.private_key,
            public_key: v.public_key.clone(),
            subject: v.subject.clone(),
        });
        Ok(Push {
            http,
            apns,
            fcm,
            vapid,
        })
    }

    pub fn summary(&self) -> String {
        format!(
            "apns={} fcm={} webpush={}",
            self.apns.is_some(),
            self.fcm.is_some(),
            self.vapid.is_some()
        )
    }

    fn apns_jwt(&self, apns: &Apns, force: bool) -> Result<String, String> {
        let now = chrono::Utc::now().timestamp();
        let mut cached = apns.jwt.lock().unwrap_or_else(|p| p.into_inner());
        if !force
            && let Some((at, jwt)) = cached.as_ref()
            && now - at < APNS_JWT_TTL_SECS
        {
            return Ok(jwt.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(apns.key_id.clone());
        let jwt = jsonwebtoken::encode(
            &header,
            &json!({"iss": apns.team_id, "iat": now}),
            &apns.key,
        )
        .map_err(|_| "APNs JWT signing failed")?;
        *cached = Some((now, jwt.clone()));
        Ok(jwt)
    }

    async fn send_apns(
        &self,
        job: &Job,
        voip: bool,
        body: &Value,
        expiration: i64,
        priority: u8,
        collapse: Option<&str>,
    ) -> Outcome {
        let Some(apns) = &self.apns else {
            return Outcome::Failed("APNs is not configured".into());
        };
        let host = if job.sandbox {
            "api.sandbox.push.apple.com"
        } else {
            "api.push.apple.com"
        };
        let topic = if voip {
            format!("{}.voip", apns.bundle_id)
        } else {
            apns.bundle_id.clone()
        };
        let mut force = false;
        for _ in 0..2 {
            let jwt = match self.apns_jwt(apns, force) {
                Ok(j) => j,
                Err(e) => return Outcome::Failed(e),
            };
            let mut request = self
                .http
                .post(format!("https://{host}/3/device/{}", job.token))
                .bearer_auth(jwt)
                .header("apns-push-type", if voip { "voip" } else { "alert" })
                .header("apns-topic", topic.as_str())
                .header("apns-priority", priority.to_string())
                .header("apns-expiration", expiration.to_string())
                .json(body);
            if let Some(c) = collapse {
                request = request.header("apns-collapse-id", c);
            }
            let response = match request.send().await {
                Ok(r) => r,
                Err(e) => {
                    return Outcome::Retry(if e.is_timeout() {
                        "timeout".into()
                    } else {
                        "network".into()
                    });
                }
            };
            let status = response.status().as_u16();
            if status == 200 {
                return Outcome::Sent;
            }
            let reason = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["reason"].as_str().map(String::from))
                .unwrap_or_default();
            match (status, reason.as_str()) {
                (410, _) | (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => {
                    return Outcome::InvalidToken;
                }
                (403, "ExpiredProviderToken") if !force => force = true,
                (429, _) | (500..=599, _) => {
                    return Outcome::Retry(format!("APNs {status} {reason}"));
                }
                _ => return Outcome::Failed(format!("APNs {status} {reason}")),
            }
        }
        Outcome::Failed("APNs rejected the provider token".into())
    }

    async fn fcm_token(&self, fcm: &Fcm) -> Result<String, String> {
        let now = chrono::Utc::now().timestamp();
        let mut cached = fcm.token.lock().await;
        if let Some((expires, token)) = cached.as_ref()
            && *expires > now + 60
        {
            return Ok(token.clone());
        }
        let claims = json!({
            "iss": fcm.email,
            "scope": "https://www.googleapis.com/auth/firebase.messaging",
            "aud": "https://oauth2.googleapis.com/token",
            "iat": now,
            "exp": now + 3600,
        });
        let assertion = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &fcm.key)
            .map_err(|_| "FCM JWT signing failed")?;
        let response = self
            .http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|_| "Google OAuth unreachable".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "Google OAuth refused the service account ({})",
                response.status().as_u16()
            ));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| "Google OAuth answered oddly")?;
        let token = value["access_token"]
            .as_str()
            .ok_or("Google OAuth returned no token")?
            .to_string();
        *cached = Some((
            now + value["expires_in"].as_i64().unwrap_or(3600),
            token.clone(),
        ));
        Ok(token)
    }

    async fn send_fcm(
        &self,
        job: &Job,
        data: &Map<String, Value>,
        ttl_secs: i64,
        collapse: Option<&str>,
    ) -> Outcome {
        let Some(fcm) = &self.fcm else {
            return Outcome::Failed("FCM is not configured".into());
        };
        let access = match self.fcm_token(fcm).await {
            Ok(t) => t,
            Err(e) => return Outcome::Retry(e),
        };
        let mut android = json!({"priority": "HIGH", "ttl": format!("{ttl_secs}s")});
        if let Some(c) = collapse {
            android["collapse_key"] = json!(c);
        }
        let body = json!({"message": {"token": job.token, "data": data, "android": android}});
        let response = match self
            .http
            .post(format!(
                "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                fcm.project
            ))
            .bearer_auth(access)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Outcome::Retry(if e.is_timeout() {
                    "timeout".into()
                } else {
                    "network".into()
                });
            }
        };
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return Outcome::Sent;
        }
        let error: Value = response.json().await.unwrap_or(Value::Null);
        let code = error["error"]["status"].as_str().unwrap_or("").to_string();
        let unregistered = error["error"]["details"]
            .as_array()
            .is_some_and(|d| d.iter().any(|x| x["errorCode"] == "UNREGISTERED"));
        match status {
            404 => Outcome::InvalidToken,
            _ if unregistered => Outcome::InvalidToken,
            429 | 500..=599 => Outcome::Retry(format!("FCM {status} {code}")),
            _ => Outcome::Failed(format!("FCM {status} {code}")),
        }
    }

    async fn send_web(&self, job: &Job, body: &[u8], ttl_secs: i64) -> Outcome {
        let Some(vapid) = &self.vapid else {
            return Outcome::Failed("Web Push is not configured".into());
        };
        let Some(sub) = webpush::Subscription::parse(&job.token) else {
            return Outcome::InvalidToken;
        };
        let encrypted = match webpush::encrypt(&sub, body) {
            Ok(e) => e,
            Err(e) => return Outcome::Failed(e.into()),
        };
        let Some(auth) = webpush::vapid_header(
            &vapid.private_key,
            &vapid.public_key,
            &vapid.subject,
            &sub.endpoint,
        ) else {
            return Outcome::Failed("VAPID signing failed".into());
        };
        let response = match self
            .http
            .post(&sub.endpoint)
            .header("Authorization", auth)
            .header("TTL", ttl_secs.to_string())
            .header("Urgency", "high")
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .body(encrypted)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return Outcome::Retry(if e.is_timeout() {
                    "timeout".into()
                } else {
                    "network".into()
                });
            }
        };
        match response.status().as_u16() {
            200..=299 => Outcome::Sent,
            404 | 410 => Outcome::InvalidToken,
            s @ (429 | 500..=599) => Outcome::Retry(format!("Web Push {s}")),
            s => Outcome::Failed(format!("Web Push {s}")),
        }
    }

    async fn deliver(&self, job: &Job) -> Outcome {
        match &job.delivery {
            Delivery::Apns {
                voip,
                body,
                expiration,
                priority,
                collapse,
            } => {
                self.send_apns(
                    job,
                    *voip,
                    body,
                    *expiration,
                    *priority,
                    collapse.as_deref(),
                )
                .await
            }
            Delivery::Fcm {
                data,
                ttl_secs,
                collapse,
            } => {
                self.send_fcm(job, data, *ttl_secs, collapse.as_deref())
                    .await
            }
            Delivery::Web { body, ttl_secs } => self.send_web(job, body, *ttl_secs).await,
        }
    }
}

fn spawn_jobs(app: &AppState, jobs: Vec<Job>) {
    for job in jobs {
        let app = app.clone();
        tokio::spawn(async move {
            let kind = crate::calls::enum_str(&job.kind);
            for attempt in 0..3u32 {
                match app.push.deliver(&job).await {
                    Outcome::Sent => {
                        tracing::debug!(
                            device_id = job.device_id.as_str(),
                            kind = kind.as_str(),
                            "push sent"
                        );
                        return;
                    }
                    Outcome::InvalidToken => {
                        tracing::info!(
                            device_id = job.device_id.as_str(),
                            kind = kind.as_str(),
                            "push token is no longer valid; dropping it"
                        );
                        let _ = app.db.with(|c| {
                            c.execute("DELETE FROM push_tokens WHERE device_id = ?1 AND kind = ?2 AND token = ?3", rusqlite::params![job.device_id, kind, job.token])
                        });
                        return;
                    }
                    Outcome::Failed(error) => {
                        tracing::warn!(
                            device_id = job.device_id.as_str(),
                            kind = kind.as_str(),
                            error = error.as_str(),
                            "push failed"
                        );
                        return;
                    }
                    Outcome::Retry(error) => {
                        tracing::warn!(
                            device_id = job.device_id.as_str(),
                            kind = kind.as_str(),
                            error = error.as_str(),
                            attempt,
                            "push will be retried"
                        );
                        tokio::time::sleep(Duration::from_secs(1 + 2 * attempt as u64)).await;
                    }
                }
            }
        });
    }
}

fn person(account: &ring_proto::Account) -> PushPerson {
    PushPerson {
        uuid: account.uuid.clone(),
        id: account.id.clone(),
        display_name: account.display_name.clone(),
        pfp_url: account.pfp_url.clone(),
    }
}

fn payload_value(payload: &PushPayload) -> Value {
    serde_json::to_value(payload).unwrap_or(Value::Null)
}

/// FCM data values must all be strings, and FCM rejects the whole message (400 INVALID_ARGUMENT)
/// when a key is reserved (`from`, `message_type`, `google.*`, `gcm.*`), so the caller travels as
/// `caller`. Null values are left out rather than sent as the text "null".
fn fcm_data(payload: &Value) -> Map<String, Value> {
    payload
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| {
                    let key = if k == "from" { "caller".to_string() } else { k.clone() };
                    let value = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
                    (key, Value::String(value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn with_aps(payload: &Value, aps: Value) -> Value {
    let mut body = payload.clone();
    if let Value::Object(map) = &mut body {
        map.insert("aps".into(), aps);
    }
    body
}

fn targets(app: &AppState, uuid: &str) -> Vec<PushTarget> {
    app.db
        .with(|c| push_targets(c, uuid))
        .unwrap_or_else(|error| {
            tracing::error!(%error, "could not read push targets");
            vec![]
        })
}

fn job(t: &PushTarget, delivery: Delivery) -> Job {
    Job {
        device_id: t.device_id.clone(),
        kind: t.kind,
        token: t.token.clone(),
        sandbox: t.environment.as_deref() == Some("sandbox"),
        delivery,
    }
}

/// Rings the invitee's devices that have ringing enabled.
pub fn incoming(app: &AppState, data: &CallData, inv: &InvitationRow) {
    let from = data.account(&inv.from_uuid);
    let payload = payload_value(&PushPayload::CallIncoming {
        ringid: inv.ringid.clone(),
        invitation_id: inv.invitation_id.clone(),
        call_uuid: inv.call_uuid.clone(),
        from: person(&from),
        participants: data
            .joined()
            .map(|p| data.account(&p.account_uuid))
            .map(|a| PushParticipant {
                id: a.id,
                display_name: a.display_name,
            })
            .collect(),
        expires_at: crate::util::time(inv.expires_at),
    });
    let expiration = inv.expires_at / 1000;
    let mut jobs = Vec::new();
    for t in targets(app, &inv.to_uuid).iter().filter(|t| t.ring_enabled) {
        let online = app.hub.is_online(&t.device_id);
        let delivery = match t.kind {
            PushKind::ApnsVoip => Delivery::Apns {
                voip: true,
                body: with_aps(&payload, json!({"content-available": 1})),
                expiration,
                priority: 10,
                collapse: None,
            },
            PushKind::Apns if t.platform == Some(Platform::Macos) && !online => Delivery::Apns {
                voip: false,
                body: with_aps(
                    &payload,
                    json!({
                        "alert": {"title": from.label(), "subtitle": from.id, "body": "Incoming Ring call"},
                        "sound": "default",
                        "interruption-level": "time-sensitive",
                        "category": "RING_CALL",
                        "thread-id": inv.ringid,
                    }),
                ),
                expiration,
                priority: 10,
                collapse: Some(inv.call_uuid.clone()),
            },
            PushKind::Fcm => Delivery::Fcm {
                data: fcm_data(&payload),
                ttl_secs: CALL_TTL_SECS,
                collapse: Some(inv.call_uuid.clone()),
            },
            PushKind::Webpush if !online => Delivery::Web {
                body: payload.to_string().into_bytes(),
                ttl_secs: CALL_TTL_SECS,
            },
            _ => continue,
        };
        jobs.push(job(t, delivery));
    }
    spawn_jobs(app, jobs);
}

/// Stops ringing on the invitee's other devices.
pub fn cancel(
    app: &AppState,
    inv: &InvitationRow,
    reason: RingingStoppedReason,
    acting_device: Option<&str>,
) {
    let payload = payload_value(&PushPayload::CallCancelled {
        ringid: inv.ringid.clone(),
        invitation_id: inv.invitation_id.clone(),
        call_uuid: inv.call_uuid.clone(),
        reason,
    });
    let expiration = now_ms() / 1000 + CALL_TTL_SECS;
    let mut jobs = Vec::new();
    for t in targets(app, &inv.to_uuid)
        .iter()
        .filter(|t| Some(t.device_id.as_str()) != acting_device)
    {
        let delivery = match t.kind {
            PushKind::ApnsVoip => Delivery::Apns {
                voip: true,
                body: with_aps(&payload, json!({"content-available": 1})),
                expiration,
                priority: 10,
                collapse: None,
            },
            PushKind::Fcm => Delivery::Fcm {
                data: fcm_data(&payload),
                ttl_secs: CALL_TTL_SECS,
                collapse: Some(inv.call_uuid.clone()),
            },
            PushKind::Webpush => Delivery::Web {
                body: payload.to_string().into_bytes(),
                ttl_secs: CALL_TTL_SECS,
            },
            _ => continue,
        };
        jobs.push(job(t, delivery));
    }
    spawn_jobs(app, jobs);
}

fn alert_jobs(
    app: &AppState,
    uuid: &str,
    payload: &Value,
    title: &str,
    body: &str,
    thread: &str,
) -> Vec<Job> {
    let expiration = now_ms() / 1000 + ALERT_TTL_SECS;
    let mut jobs = Vec::new();
    for t in targets(app, uuid) {
        let delivery = match t.kind {
            PushKind::Apns => Delivery::Apns {
                voip: false,
                body: with_aps(
                    payload,
                    json!({"alert": {"title": title, "body": body}, "sound": "default", "thread-id": thread}),
                ),
                expiration,
                priority: 10,
                collapse: None,
            },
            PushKind::Fcm => Delivery::Fcm {
                data: fcm_data(payload),
                ttl_secs: ALERT_TTL_SECS,
                collapse: None,
            },
            PushKind::Webpush => Delivery::Web {
                body: payload.to_string().into_bytes(),
                ttl_secs: CALL_TTL_SECS,
            },
            PushKind::ApnsVoip => continue,
        };
        jobs.push(job(&t, delivery));
    }
    jobs
}

pub fn missed(app: &AppState, data: &CallData, inv: &InvitationRow) {
    let from = data.account(&inv.from_uuid);
    let payload = payload_value(&PushPayload::CallMissed {
        ringid: inv.ringid.clone(),
        from: person(&from),
    });
    let jobs = alert_jobs(
        app,
        &inv.to_uuid,
        &payload,
        "Missed call",
        from.label(),
        &inv.ringid,
    );
    spawn_jobs(app, jobs);
}

pub fn voicemail(app: &AppState, vm: &Voicemail) {
    let payload = payload_value(&PushPayload::VoicemailReceived {
        voicemail_id: vm.voicemail_id.clone(),
        from: person(&vm.from),
        transcript: vm.transcript.clone(),
    });
    let body: String = match &vm.transcript {
        Some(t) if !t.is_empty() => t.chars().take(180).collect(),
        _ => "New voicemail".into(),
    };
    let title = format!("Voicemail from {}", vm.from.label());
    let jobs = alert_jobs(app, &vm.to.uuid, &payload, &title, &body, &vm.ringid);
    spawn_jobs(app, jobs);
}

use crate::model::*;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Engine {
    pub state: State,
    db: Connection,
    pub data_dir: PathBuf,
}
impl Engine {
    pub fn open(dir: &Path) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(dir.join("assets"))?;
        let db = Connection::open(dir.join("ring.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);")?;
        let mut state: State = db
            .query_row("SELECT json FROM state WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        // A crash after publication may leave an uncertain send. Retry its same logical ID.
        for publication in state
            .publications
            .values_mut()
            .filter(|n| n.status == "sending")
        {
            publication.status = "pending".into();
        }
        Ok(Self {
            state,
            db,
            data_dir: dir.into(),
        })
    }
    pub fn persist(&self) -> Result<()> {
        let data = serde_json::to_string(&self.state).map_err(|_| {
            Fault::new(
                "STORAGE_FAILED",
                "Could not encode state.",
                "Check server diagnostics.",
            )
        })?;
        self.db.execute("INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",[data]).map_err(|_|Fault::new("STORAGE_FAILED","Could not persist the operation.","Check disk availability; retry with the same request ID."))?;
        Ok(())
    }
    pub fn login(&mut self, identity: Identity, device: Option<String>) -> Result<Value> {
        self.state.remember_identity(&identity);
        let owner = key(&identity.realm, &identity.org_id, &identity.actor);
        let device_id = device
            .filter(|d| {
                self.state
                    .devices
                    .get(d)
                    .is_some_and(|v| v.owner == owner && !v.revoked)
            })
            .unwrap_or_else(|| id("dev"));
        self.state
            .devices
            .entry(device_id.clone())
            .or_insert(Device {
                device_id: device_id.clone(),
                owner: owner.clone(),
                name: "Ring device".into(),
                ring_enabled: true,
                revoked: false,
                push_token: None,
                push_platform: None,
                push_environment: None,
            });
        let token = id("session");
        let s = Session {
            identity: identity.clone(),
            device_id: device_id.clone(),
            // Native incoming-call listeners must survive an idle night; IAM authority is
            // still checked online on every control operation and every 30 seconds.
            expires_at: after(30 * 24 * 3600),
        };
        self.state.profiles.entry(owner).or_insert_with(|| Profile {
            actor: identity.actor.clone(),
            display_name: identity.display_name.clone(),
            photo_asset_id: None,
            voice_id: "gleam".into(),
        });
        self.state.sessions.insert(digest(&token), s.clone());
        self.persist()?;
        Ok(
            json!({"authenticated":true,"session_token":token,"actor":identity.actor,"actor_id":identity.actor,"display_name":identity.display_name,"org_id":identity.org_id,"realm":identity.realm,"device_id":device_id,"expires_at":s.expires_at,"permissions":if identity.admin {vec!["org:admin"]}else{vec!["calls:write"]}}),
        )
    }
    pub fn session(&self, token: &str) -> Result<Session> {
        self.state
            .sessions
            .get(&digest(token))
            .filter(|s| {
                s.expires_at > now()
                    && self
                        .state
                        .devices
                        .get(&s.device_id)
                        .is_some_and(|d| !d.revoked)
            })
            .cloned()
            .ok_or_else(|| {
                Fault::new(
                    "AUTH_REQUIRED",
                    "Ring session is missing, expired, or revoked.",
                    "Run ring login with a new short-lived IAM token.",
                )
            })
    }
    pub fn request(&mut self, s: &Session, request_id: &str, method: &str, p: Value) -> Value {
        let fingerprint = digest(&format!("{method}:{p}"));
        self.request_with_fingerprint(s, request_id, method, p, fingerprint)
    }
    pub fn request_with_fingerprint(
        &mut self,
        s: &Session,
        request_id: &str,
        method: &str,
        p: Value,
        fingerprint: String,
    ) -> Value {
        let cache_key = format!(
            "{}|{}",
            key(&s.identity.realm, &s.identity.org_id, &s.identity.actor),
            request_id
        );
        if let Some(c) = self.state.requests.get(&cache_key) {
            return if c.fingerprint == fingerprint {
                c.response.clone()
            } else {
                json!({"id":request_id,"ok":false,"error":Fault::new("REQUEST_ID_REUSED","Request ID was already used with different input.","Use a new request ID for changed input.")})
            };
        }
        // Read-only requests neither clone all history nor force an SQLite fsync on the audio path.
        if read_only(method) {
            return match self.dispatch(s, method, &p) {
                Ok(result) => json!({"id":request_id,"ok":true,"result":result}),
                Err(error) => json!({"id":request_id,"ok":false,"error":error}),
            };
        }
        let before = self.state.clone();
        let result = self.dispatch(s, method, &p);
        let response = match result {
            Ok(r) => json!({"id":request_id,"ok":true,"result":r}),
            Err(e) => {
                self.state = before.clone();
                json!({"id":request_id,"ok":false,"error":e})
            }
        };
        if response["ok"] == true && !read_only(method) {
            self.state.requests.insert(
                cache_key,
                Cached {
                    fingerprint,
                    response: response.clone(),
                },
            );
        }
        if let Err(e) = self.persist() {
            self.state = before;
            return json!({"id":request_id,"ok":false,"error":e});
        }
        response
    }
    pub fn dispatch(&mut self, s: &Session, m: &str, p: &Value) -> Result<Value> {
        let i = &s.identity;
        let owner = key(&i.realm, &i.org_id, &i.actor);
        match m {
            "auth.status" => Ok(
                json!({"authenticated":true,"actor":i.actor,"display_name":i.display_name,"org_id":i.org_id,"realm":i.realm,"device_id":s.device_id,"expires_at":s.expires_at}),
            ),
            "auth.logout" => {
                let all = p["all_devices"].as_bool().unwrap_or(false);
                let mut removed = Vec::new();
                for device in self
                    .state
                    .devices
                    .values_mut()
                    .filter(|d| d.owner == owner && (all || d.device_id == s.device_id))
                {
                    device.push_token = None;
                    removed.push(device.device_id.clone());
                }
                self.state.pushes.retain(|_, task| {
                    !task["device_id"]
                        .as_str()
                        .is_some_and(|id| removed.iter().any(|d| d == id))
                });
                self.state.sessions.retain(|_, x| {
                    !(key(&x.identity.realm, &x.identity.org_id, &x.identity.actor) == owner
                        && (all || x.device_id == s.device_id))
                });
                Ok(json!({"authenticated":false}))
            }
            "profile.get" => {
                let actor = actor_id(p["actor"].as_str().unwrap_or(&i.actor), &i.org_id)?;
                if actor == i.actor {
                    return Ok(json!(self.state.profile(i)));
                }
                self.state
                    .profiles
                    .get(
                        &self
                            .state
                            .recipient_identity(&i.realm, &actor)
                            .map(|identity| key(&identity.realm, &identity.org_id, &identity.actor))
                            .ok_or_else(|| missing("Profile"))?,
                    )
                    .map(|v| json!(v))
                    .ok_or_else(|| missing("Profile"))
            }
            "profile.update" => {
                let mut v = self.state.profile(i);
                if let Some(name) = p.get("display_name") {
                    v.display_name = name
                        .as_str()
                        .filter(|n| !n.trim().is_empty() && n.chars().count() <= 100)
                        .ok_or_else(|| invalid("display_name must be 1–100 characters"))?
                        .into();
                }
                if let Some(voice) = p.get("voice_id") {
                    if !silicon(&i.actor) {
                        return Err(forbidden());
                    }
                    let voice = voice
                        .as_str()
                        .ok_or_else(|| invalid("voice_id must be a string"))?;
                    if !VOICES.contains(&voice) {
                        return Err(invalid("voice_id is not a supported Natural Voice"));
                    }
                    v.voice_id = voice.into();
                }
                if let Some(photo) = p.get("photo_asset_id") {
                    v.photo_asset_id = if photo.is_null() {
                        None
                    } else {
                        let a = self.owned_asset(
                            i,
                            photo
                                .as_str()
                                .ok_or_else(|| invalid("photo_asset_id must be an ID or null"))?,
                        )?;
                        if !a.complete || a.purpose != "profile_photo" {
                            return Err(invalid("Photo upload is not complete"));
                        }
                        Some(a.asset_id.clone())
                    };
                }
                self.state.profiles.insert(owner, json_value(v.clone())?);
                Ok(json!(v))
            }
            "voices.list" => Ok(
                json!({"items":VOICES.iter().map(|v|json!({"id":v,"voice_id":v,"name":v,"category":"Natural Voices"})).collect::<Vec<_>>(),"next_cursor":null}),
            ),
            "devices.list" => Ok(
                json!({"items":self.state.devices.values().filter(|d|d.owner==owner&&!d.revoked).map(|d|{let mut v=public_device(d);v["active_call"]=self.state.calls.values().find(|c|c.state=="active"&&c.participants.iter().any(|p|p.device_id.as_deref()==Some(&d.device_id)&&p.left_at.is_none())).map(|c|json!(c.ringid)).unwrap_or(Value::Null);v}).collect::<Vec<_>>(),"next_cursor":null}),
            ),
            "devices.update" | "devices.revoke" => {
                let d = self
                    .state
                    .devices
                    .get_mut(required(p, "device_id")?)
                    .filter(|d| d.owner == owner)
                    .ok_or_else(forbidden)?;
                if m == "devices.revoke" {
                    d.revoked = true;
                    d.push_token = None;
                } else {
                    if let Some(name) = p.get("name") {
                        d.name = name
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 120)
                            .ok_or_else(|| invalid("name must be 1–120 bytes"))?
                            .into()
                    }
                    if let Some(b) = p.get("ring_enabled") {
                        d.ring_enabled = b
                            .as_bool()
                            .ok_or_else(|| invalid("ring_enabled must be boolean"))?
                    }
                    if let Some(platform) = p.get("push_platform") {
                        d.push_platform = if platform.is_null() {
                            None
                        } else {
                            Some(
                                platform
                                    .as_str()
                                    .filter(|p| matches!(*p, "apns_voip" | "fcm"))
                                    .ok_or_else(|| {
                                        invalid("push_platform must be apns_voip or fcm")
                                    })?
                                    .into(),
                            )
                        };
                    }
                    if let Some(environment) = p.get("push_environment") {
                        d.push_environment = if environment.is_null() {
                            None
                        } else {
                            Some(
                                environment
                                    .as_str()
                                    .filter(|v| matches!(*v, "sandbox" | "production"))
                                    .ok_or_else(|| {
                                        invalid("push_environment must be sandbox or production")
                                    })?
                                    .into(),
                            )
                        };
                    }
                    if let Some(token) = p.get("push_token") {
                        d.push_token = if token.is_null() {
                            None
                        } else {
                            Some(
                                token
                                    .as_str()
                                    .filter(|t| !t.is_empty() && t.len() < 4096)
                                    .ok_or_else(|| invalid("push_token must be 1–4095 bytes"))?
                                    .into(),
                            )
                        };
                    }
                    if d.push_token.is_some() && d.push_platform.is_none() {
                        return Err(invalid("A push token requires push_platform"));
                    }
                }
                Ok(public_device(d))
            }
            "config.get" | "config.set" | "config.reset" => self.config_op(i, m, p),
            "calls.prepare" => self.prepare(i, p),
            "context.approve" => {
                let prep = self
                    .state
                    .preparations
                    .get(required(p, "preparation_id")?)
                    .filter(|x| x.owner == owner && x.expires_at > now())
                    .ok_or_else(|| missing("Current preparation"))?;
                let seconds = p["valid_for_seconds"].as_i64().unwrap_or(3600);
                if !(1..=86400).contains(&seconds) {
                    return Err(invalid("valid_for_seconds must be 1–86400"));
                }
                if self.state.config(i).revision != prep.default_revision {
                    return Err(Fault::new(
                        "CONTEXT_CHANGED",
                        "Default context changed after preview.",
                        "Prepare and inspect the new context.",
                    ));
                }
                let a = Approval {
                    approval_id: id("approval"),
                    owner,
                    default_revision: prep.default_revision,
                    context_mode: prep.context_mode.clone(),
                    expires_at: after(seconds),
                    revoked: false,
                };
                self.state
                    .approvals
                    .insert(a.approval_id.clone(), a.clone());
                Ok(json!(a))
            }
            "context.approvals.list" => paginate(
                self.state
                    .approvals
                    .values()
                    .filter(|a| a.owner == owner)
                    .map(|a| json!(a))
                    .collect(),
                p,
            ),
            "context.approvals.revoke" => {
                let a = self
                    .state
                    .approvals
                    .get_mut(required(p, "approval_id")?)
                    .filter(|a| a.owner == owner)
                    .ok_or_else(forbidden)?;
                a.revoked = true;
                Ok(json!(a))
            }
            "calls.init" => self.init(s, p),
            "calls.accept" | "calls.decline" | "calls.silence" | "calls.cut" | "calls.invite"
            | "calls.handoff" => self.call_mutation(s, m, p),
            "calls.get" => Ok(public_call(
                self.state.call(i, required(p, "ringid")?)?,
                &i.actor,
            )),
            "calls.list" => {
                let mut rows = Vec::new();
                for c in self
                    .state
                    .calls
                    .values()
                    .rev()
                    .filter(|c| c.realm == i.realm && c.visible(&i.actor))
                {
                    if p["state"].as_str().is_some_and(|state| state != c.state)
                        || !time_matches(&c.created_at, p)?
                    {
                        continue;
                    }
                    if p["direction"] == "outgoing" && c.caller != i.actor
                        || p["direction"] == "incoming" && c.caller == i.actor
                    {
                        continue;
                    }
                    if let Some(actor) = p["actor"].as_str() {
                        let a = actor_id(actor, &i.org_id)?;
                        if !c.visible(&a) {
                            continue;
                        }
                    }
                    rows.push(public_call(c, &i.actor))
                }
                rows.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
                paginate(rows, p)
            }
            "transcript.list" => {
                let c = self.state.call(i, required(p, "ringid")?)?;
                let entries = c
                    .transcript
                    .iter()
                    .filter(|e| entry_visible(c, e, &i.actor))
                    .filter(|e| {
                        p["after_seq"].as_u64().is_none_or(|n| e.seq > n)
                            && p["kind"].as_str().is_none_or(|k| e.kind == k)
                    })
                    .map(|e| json!(e))
                    .collect();
                let mut out = paginate(entries, p)?;
                out["latest_seq"] = json!(c.transcript.last().map_or(0, |e| e.seq));
                Ok(out)
            }
            "representative.send" => self.send(i, p),
            "delegations.list" => {
                self.state.call(i, required(p, "ringid")?)?;
                paginate(
                    self.state
                        .delegations
                        .values()
                        .filter(|d| {
                            d.realm == i.realm
                                && d.actor == i.actor
                                && d.ringid == p["ringid"]
                                && p["status"].as_str().is_none_or(|s| s == d.status)
                        })
                        .map(|d| json!(d))
                        .collect(),
                    p,
                )
            }
            "delegations.get" => self
                .state
                .delegations
                .get(required(p, "delegation_id")?)
                .filter(|d| d.actor == i.actor && d.realm == i.realm)
                .map(|d| json!(d))
                .ok_or_else(forbidden),
            "voicemail.begin" | "voicemail.send" | "voicemail.commit" | "voicemail.abort"
            | "voicemail.get" | "voicemail.list" | "voicemail.mark" | "voicemail.delete" => {
                self.voicemail(i, m, p)
            }
            "assets.begin" => {
                let purpose = required(p, "purpose")?;
                let mime = required(p, "mime_type")?;
                if !match purpose {
                    "profile_photo" => matches!(mime, "image/png" | "image/jpeg" | "image/webp"),
                    "voicemail_greeting" => matches!(
                        mime,
                        "audio/wav" | "audio/webm" | "audio/ogg" | "audio/mpeg" | "audio/mp4"
                    ),
                    _ => false,
                } {
                    return Err(invalid(
                        "Only photo and audio greeting uploads are supported",
                    ));
                }
                let size = p["size_bytes"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= 20 * 1024 * 1024)
                    .ok_or_else(|| invalid("size_bytes must be 1–20971520"))?
                    as usize;
                let aid = id("asset");
                let a = Asset {
                    asset_id: aid.clone(),
                    owner,
                    purpose: purpose.into(),
                    mime_type: mime.into(),
                    size_bytes: size,
                    received_bytes: 0,
                    next_seq: 0,
                    complete: false,
                    path: self
                        .data_dir
                        .join("assets")
                        .join(&aid)
                        .to_string_lossy()
                        .into(),
                    ringid: None,
                    voicemail_id: None,
                };
                self.state.assets.insert(aid, a.clone());
                Ok(
                    json!({"asset_id":a.asset_id,"purpose":a.purpose,"mime_type":a.mime_type,"size_bytes":a.size_bytes,"received_bytes":0,"next_seq":0,"complete":false}),
                )
            }
            "assets.complete" => {
                let aid = required(p, "asset_id")?;
                let a = self.owned_asset(i, aid)?;
                if a.received_bytes != a.size_bytes {
                    return Err(Fault::new(
                        "UPLOAD_INCOMPLETE",
                        "Upload bytes do not match the declared size.",
                        "Resume the upload before completing it.",
                    ));
                }
                validate_asset_header(a)?;
                self.state.assets.get_mut(aid).unwrap().complete = true;
                Ok(json!({"asset_id":aid,"complete":true}))
            }
            "assets.get" => {
                let a = self.authorized_asset(i, required(p, "asset_id")?)?;
                Ok(
                    json!({"asset_id":a.asset_id,"mime_type":a.mime_type,"size_bytes":a.size_bytes,"received_bytes":a.received_bytes,"next_seq":a.next_seq,"complete":a.complete,"transfer_id":id("transfer")}),
                )
            }
            "recordings.get" => {
                let c = self.state.call(i, required(p, "ringid")?)?;
                if !c.participants.iter().any(|p| p.actor == i.actor) {
                    return Err(forbidden());
                }
                let gaps = self
                    .state
                    .recording_gaps
                    .get(&c.ringid)
                    .cloned()
                    .unwrap_or_default();
                let duration_ms = c
                    .answered_at
                    .as_deref()
                    .and_then(|start| chrono::DateTime::parse_from_rfc3339(start).ok())
                    .map(|start| {
                        let end = c
                            .ended_at
                            .as_deref()
                            .and_then(|end| chrono::DateTime::parse_from_rfc3339(end).ok())
                            .unwrap_or_else(|| chrono::Utc::now().fixed_offset());
                        (end - start).num_milliseconds().max(0)
                    });
                Ok(
                    json!({"ringid":c.ringid,"status":c.recording_status,"audio_asset_id":c.recording_asset_id,"mime_type":"audio/wav","coverage":if gaps.is_empty()&&c.recording_status!="failed"{"full"}else{"partial"},"missing_intervals":gaps,"duration_ms":duration_ms}),
                )
            }
            "notifications.status" => {
                if !silicon(&i.actor) {
                    return Err(invalid("Ting notifications apply to silicon identities"));
                }
                let rows: Vec<_> = self
                    .state
                    .publications
                    .values()
                    .filter(|n| n.actor == i.actor && n.realm == i.realm)
                    .collect();
                Ok(
                    json!({"pending":rows.iter().filter(|n|n.status=="pending").count(),"failed":rows.iter().filter(|n|n.status=="failed").count(),"delivered":rows.iter().filter(|n|n.status=="delivered").count(),"errors":rows.iter().filter_map(|n|n.error.as_ref()).collect::<Vec<_>>()}),
                )
            }
            "notifications.retry" => {
                let mut n = 0;
                for v in self.state.publications.values_mut().filter(|v| {
                    v.actor == i.actor
                        && v.realm == i.realm
                        && v.status == "failed"
                        && p["notification_id"]
                            .as_str()
                            .is_none_or(|id| id == v.notification_id)
                }) {
                    v.status = "pending".into();
                    n += 1;
                }
                Ok(json!({"queued":n}))
            }
            "bugs.submit" => {
                let title = required(p, "title")?;
                let description = required(p, "description")?;
                if title.len() > 200 || description.len() > 20000 {
                    return Err(invalid(
                        "Bug title or description exceeds the allowed length",
                    ));
                }
                let id = id("bug");
                let mut report = p.clone();
                report["report_id"] = json!(id);
                report["actor"] = json!(i.actor);
                report["created_at"] = json!(now());
                self.state.bugs.insert(id.clone(), report);
                Ok(json!({"report_id":id,"status":"recorded","url":null}))
            }
            "release.info" => release_info(p),
            _ => Err(Fault::new(
                "METHOD_NOT_FOUND",
                format!("Unknown operation {m}."),
                "Use app.info to inspect supported capabilities.",
            )),
        }
    }
    fn config_op(&mut self, i: &Identity, m: &str, p: &Value) -> Result<Value> {
        let scope = p["scope"].as_str().unwrap_or("actor");
        if !matches!(scope, "actor" | "org") {
            return Err(invalid("scope must be actor or org"));
        }
        if scope == "org" && !i.admin {
            return Err(forbidden());
        }
        let k = key(
            &i.realm,
            &i.org_id,
            if scope == "org" { "*" } else { &i.actor },
        );
        let mut cfg = self.state.configs.get(&k).cloned().unwrap_or_default();
        if m == "config.set" {
            let values = p["values"]
                .as_object()
                .ok_or_else(|| invalid("values must be an object"))?;
            for (name, v) in values {
                match name.as_str() {
                    "representative.default_context" => {
                        if scope != "actor" || !silicon(&i.actor) {
                            return Err(forbidden());
                        }
                        text_limit(&json!({"context":v}), "context", 400)?
                    }
                    "representative.context_mode" => {
                        if !silicon(&i.actor)
                            || !matches!(v.as_str(), Some("append" | "prepend" | "overwrite"))
                        {
                            return Err(invalid(
                                "context_mode must be append, prepend or overwrite for a silicon",
                            ));
                        }
                    }
                    "representative.context_required"
                    | "voicemail.enabled"
                    | "telemetry.enabled" => {
                        if !v.is_boolean() {
                            return Err(invalid(format!("{name} must be boolean")));
                        }
                    }
                    "voicemail.greetings.busy"
                    | "voicemail.greetings.declined"
                    | "voicemail.greetings.timeout" => {
                        if v["text"]
                            .as_str()
                            .is_some_and(|s| !s.is_empty() && s.chars().count() <= 1000)
                        {
                        } else if let Some(a) = v["asset_id"].as_str() {
                            if silicon(&i.actor) {
                                return Err(invalid(
                                    "Silicon greetings must use text and its selected voice",
                                ));
                            }
                            let a = self.owned_asset(i, a)?;
                            if !a.complete || a.purpose != "voicemail_greeting" {
                                return Err(invalid("Greeting asset must be fully uploaded"));
                            }
                        } else {
                            return Err(invalid(
                                "Greeting requires text or a completed audio asset",
                            ));
                        }
                    }
                    "providers.live"
                    | "providers.tts"
                    | "providers.transcription"
                    | "providers.storage" => {
                        if scope != "org" {
                            return Err(forbidden());
                        }
                        let fields = v.as_object().filter(|v| !v.is_empty()).ok_or_else(|| {
                            invalid(format!("{name} must be a nonempty settings object"))
                        })?;
                        for (field, value) in fields {
                            let supported = if name == "providers.storage" {
                                matches!(
                                    field.as_str(),
                                    "bucket"
                                        | "region"
                                        | "prefix"
                                        | "access_key_id"
                                        | "secret_access_key"
                                        | "session_token"
                                )
                            } else {
                                matches!(field.as_str(), "api_key" | "model")
                            };
                            if !supported
                                || !value.as_str().is_some_and(|text| {
                                    text.len() <= 16384 && (!text.is_empty() || field == "prefix")
                                })
                            {
                                return Err(invalid(format!(
                                    "Unsupported or invalid provider setting: {name}.{field}"
                                )));
                            }
                        }
                    }
                    "retention.calls_days" | "retention.voicemail_days" => {
                        if scope != "org" || v.as_u64().is_none_or(|n| !(1..=3650).contains(&n)) {
                            return Err(invalid("Retention requires org scope and 1–3650 days"));
                        }
                    }
                    _ => {
                        return Err(invalid(format!(
                            "Unknown or unsupported configuration key {name}"
                        )))
                    }
                }
                cfg.values.insert(name.clone(), v.clone());
            }
            cfg.revision += 1;
            self.state.configs.insert(k, cfg.clone());
        } else if m == "config.reset" {
            for n in p["keys"]
                .as_array()
                .ok_or_else(|| invalid("keys must be an array"))?
            {
                cfg.values.remove(
                    n.as_str()
                        .ok_or_else(|| invalid("Each key must be a string"))?,
                );
            }
            cfg.revision += 1;
            self.state.configs.insert(k, cfg.clone());
        }
        let mut effective = defaults();
        if scope == "actor" {
            if let Some(org) = self.state.configs.get(&key(&i.realm, &i.org_id, "*")) {
                for (k, v) in &org.values {
                    effective[k] = v.clone();
                }
            }
        }
        for (k, v) in &cfg.values {
            effective[k] = v.clone();
        }
        let mut result = json!({"scope":scope,"values":cfg.values,"effective":effective,"defaults":defaults(),"revision":cfg.revision});
        redact_config(&mut result);
        Ok(result)
    }
    fn prepare(&mut self, i: &Identity, p: &Value) -> Result<Value> {
        if !silicon(&i.actor) {
            return Err(invalid("Representative context is silicon-only"));
        }
        text_limit(p, "context", 400)?;
        let action = required(p, "action")?;
        let mut invitation_id = None;
        let target = match action {
            "init" => actor_id(required(p, "target")?, &i.org_id)?,
            "accept" => {
                let r = required(p, "ringid")?;
                let call = self.state.call(i, r)?;
                let index = offer_index(call, &i.actor, p, true)?;
                let offer = &call.invitations[index];
                if offer.expires_at <= now() {
                    return Err(Fault::new(
                        "INVITATION_EXPIRED",
                        "The offer expired before preparation.",
                        "Start a new call or inspect current invitations.",
                    ));
                }
                invitation_id = Some(offer.invitation_id.clone());
                r.into()
            }
            _ => return Err(invalid("action must be init or accept")),
        };
        let cfg = self.state.config(i);
        let default = cfg
            .values
            .get("representative.default_context")
            .and_then(Value::as_str)
            .unwrap_or("");
        let supplied = p["context"].as_str().unwrap_or("");
        if cfg.values.get("representative.context_required") == Some(&Value::Bool(true))
            && supplied.trim().is_empty()
        {
            return Err(invalid(
                "Nonempty per-call context is required by your configuration",
            ));
        }
        let mode = p["context_mode"]
            .as_str()
            .or_else(|| {
                cfg.values
                    .get("representative.context_mode")
                    .and_then(Value::as_str)
            })
            .unwrap_or("append");
        let texts = match mode {
            "append" => vec![default, supplied],
            "prepend" => vec![supplied, default],
            "overwrite" => vec![supplied],
            _ => return Err(invalid("context_mode must be append, prepend or overwrite")),
        };
        let effective = texts
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if effective.chars().count() > 400 {
            return Err(invalid(
                "Merged context exceeds 400 Unicode characters including separator",
            ));
        }
        let owner = key(&i.realm, &i.org_id, &i.actor);
        let approval_required = !default.is_empty()
            && mode != "overwrite"
            && !self.state.approvals.values().any(|a| {
                a.owner == owner
                    && a.default_revision == cfg.revision
                    && a.context_mode == mode
                    && !a.revoked
                    && a.expires_at > now()
            });
        let prep = Preparation {
            preparation_id: id("prep"),
            owner,
            action: action.into(),
            target,
            invitation_id,
            default_text: default.into(),
            default_revision: cfg.revision,
            supplied_text: supplied.into(),
            context_mode: mode.into(),
            effective_text: effective,
            expires_at: after(300),
            approval_required,
        };
        self.state
            .preparations
            .insert(prep.preparation_id.clone(), prep.clone());
        Ok(json!(prep))
    }
    fn context(&self, i: &Identity, p: &Value, action: &str, target: &str) -> Result<String> {
        text_limit(p, "start", 100)?;
        if !silicon(&i.actor) {
            if p.get("start").is_some() || p.get("preparation_id").is_some() {
                return Err(invalid("start and context are silicon-only"));
            }
            return Ok(String::new());
        }
        let cfg = self.state.config(i);
        let Some(pid) = p["preparation_id"].as_str() else {
            if cfg.values.get("representative.context_required") == Some(&Value::Bool(true))
                || cfg
                    .values
                    .get("representative.default_context")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            {
                return Err(Fault::new("CONTEXT_PREPARATION_REQUIRED","Configured representative context must be reviewed.","Run calls.prepare, inspect its context, explicitly approve the default if required, then retry."));
            }
            return Ok(String::new());
        };
        let owner = key(&i.realm, &i.org_id, &i.actor);
        let prep = self
            .state
            .preparations
            .get(pid)
            .filter(|x| {
                x.owner == owner
                    && x.action == action
                    && x.target == target
                    && x.invitation_id
                        .as_deref()
                        .is_none_or(|id| Some(id) == p["invitation_id"].as_str())
            })
            .ok_or_else(|| invalid("preparation_id does not match this action and target"))?;
        if prep.expires_at <= now() {
            return Err(Fault::new(
                "CONTEXT_EXPIRED",
                "Preparation expired.",
                "Prepare and review context again.",
            ));
        }
        if prep.default_revision != cfg.revision {
            return Err(Fault::new(
                "CONTEXT_CHANGED",
                "Context defaults changed since preparation.",
                "Prepare and review the current default.",
            ));
        }
        let need = !prep.default_text.is_empty() && prep.context_mode != "overwrite";
        if need
            && !self.state.approvals.values().any(|a| {
                a.owner == owner
                    && a.default_revision == cfg.revision
                    && a.context_mode == prep.context_mode
                    && !a.revoked
                    && a.expires_at > now()
                    && p["approval_id"]
                        .as_str()
                        .is_none_or(|id| id == a.approval_id)
            })
        {
            let mut e = Fault::new(
                "CONTEXT_APPROVAL_REQUIRED",
                "The displayed default context needs explicit approval.",
                "Run ring context approve PREPARATION_ID --for 1h, then repeat the call command.",
            );
            e.details = Some(json!(prep));
            return Err(e);
        }
        Ok(prep.effective_text.clone())
    }
    fn init(&mut self, s: &Session, p: &Value) -> Result<Value> {
        let i = &s.identity;
        let target = actor_id(required(p, "target")?, &i.org_id)?;
        if target == i.actor {
            return Err(invalid(
                "Cannot call yourself; use device handoff for another device",
            ));
        }
        let recipient = self
            .state
            .recipient_identity(&i.realm, &target)
            .ok_or_else(|| {
                Fault::new(
                    "RECIPIENT_NOT_REGISTERED",
                    "Recipient has not registered with Ring.",
                    "Ask the recipient to log in to Ring with IAM first.",
                )
            })?;
        if self.state.busy(&i.actor, &i.realm, None) {
            return Err(Fault::new(
                "SILICON_BUSY",
                "You already have a reserved or active call.",
                "Cut your existing call or invite this person into it.",
            ));
        }
        let context = self.context(i, p, "init", &target)?;
        let device = self.call_device(s, p)?;
        let busy = self.state.busy(&target, &i.realm, None);
        let invitation = Invitation {
            invitation_id: id("offer"),
            inviter: i.actor.clone(),
            target: target.clone(),
            state: if busy { "busy" } else { "pending" }.into(),
            created_at: now(),
            expires_at: after(60),
            reason: None,
            silenced: false,
        };
        let mut call = Call {
            ringid: id("ring"),
            actor_orgs: [
                (i.actor.clone(), i.org_id.clone()),
                (target.clone(), recipient.org_id.clone()),
            ]
            .into_iter()
            .collect(),
            org_id: i.org_id.clone(),
            realm: i.realm.clone(),
            caller: i.actor.clone(),
            target: target.clone(),
            state: if busy { "voicemail" } else { "ringing" }.into(),
            created_at: now(),
            answered_at: None,
            ended_at: None,
            participants: vec![Participant {
                actor: i.actor.clone(),
                display_name: self.state.profile(i).display_name,
                device_id: device,
                joined_at: now(),
                left_at: None,
                context,
                start: p["start"].as_str().unwrap_or("").into(),
            }],
            invitations: vec![invitation.clone()],
            transcript: vec![],
            recording_status: "pending".into(),
            recording_asset_id: None,
        };
        call.entry(
            "call.created",
            Some(i.actor.clone()),
            json!({"target":target,"invitation_id":invitation.invitation_id}),
            None,
        );
        if busy && !self.voicemail_enabled(&i.realm, &recipient.org_id, &target) {
            end_call(&mut call);
        }
        let event = if call.state == "ended" {
            "call.ended"
        } else if busy {
            "voicemail.offered"
        } else {
            "call.incoming"
        };
        self.state.call_event(&call,event,json!({"ringid":call.ringid,"invitation_id":invitation.invitation_id,"caller":call.caller,"target":target,"expires_at":invitation.expires_at,"state":call.state,"outcome":if busy{Some("busy")}else{None}}));
        let result = public_call(&call, &i.actor);
        self.state.calls.insert(call.ringid.clone(), call);
        Ok(result)
    }
    fn voicemail_enabled(&self, realm: &str, org: &str, actor: &str) -> bool {
        self.state
            .configs
            .get(&key(realm, org, actor))
            .and_then(|c| c.values.get("voicemail.enabled"))
            .or_else(|| {
                self.state
                    .configs
                    .get(&key(realm, org, "*"))
                    .and_then(|c| c.values.get("voicemail.enabled"))
            })
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }
    fn settle_unanswered(&self, call: &mut Call) {
        if call.invitations.iter().any(|v| v.state == "pending") {
            return;
        }
        if call.state == "ringing" {
            if call.invitations.iter().any(|v| {
                matches!(v.state.as_str(), "busy" | "declined" | "timeout")
                    && self.voicemail_enabled(
                        &call.realm,
                        &call.identity(&v.target).org_id,
                        &v.target,
                    )
            }) {
                call.state = "voicemail".into();
            } else {
                end_call(call);
            }
        } else if call.state == "active"
            && call
                .participants
                .iter()
                .filter(|p| p.left_at.is_none())
                .count()
                < 2
        {
            end_call(call);
        }
    }
    fn call_device(&self, s: &Session, p: &Value) -> Result<Option<String>> {
        if silicon(&s.identity.actor) {
            return Ok(None);
        }
        let d = p["device_id"].as_str().unwrap_or(&s.device_id);
        if !self.state.devices.get(d).is_some_and(|x| {
            x.owner == key(&s.identity.realm, &s.identity.org_id, &s.identity.actor) && !x.revoked
        }) {
            return Err(forbidden());
        }
        Ok(Some(d.into()))
    }
    fn call_mutation(&mut self, s: &Session, m: &str, p: &Value) -> Result<Value> {
        let i = &s.identity;
        let ring = required(p, "ringid")?;
        let mut call = self.state.call(i, ring)?.clone();
        let event = match m {
            "calls.accept" => {
                let idx = offer_index(&call, &i.actor, p, true)?;
                if call.invitations[idx].expires_at <= now() {
                    return Err(Fault::new(
                        "INVITATION_EXPIRED",
                        "The offer expired.",
                        "Start a new call.",
                    ));
                }
                if self.state.busy(&i.actor, &i.realm, Some(ring)) {
                    return Err(Fault::new(
                        "SILICON_BUSY",
                        "You already have an active call.",
                        "Leave the existing call before accepting.",
                    ));
                }
                let mut context_params = p.clone();
                context_params["invitation_id"] = json!(call.invitations[idx].invitation_id);
                let context = self.context(i, &context_params, "accept", ring)?;
                let device = self.call_device(s, p)?;
                call.actor_orgs.insert(i.actor.clone(), i.org_id.clone());
                call.invitations[idx].state = "accepted".into();
                call.invitations[idx].silenced = true;
                call.participants.push(Participant {
                    actor: i.actor.clone(),
                    display_name: self.state.profile(i).display_name,
                    device_id: device,
                    joined_at: now(),
                    left_at: None,
                    context,
                    start: p["start"].as_str().unwrap_or("").into(),
                });
                if call.answered_at.is_none() {
                    call.answered_at = Some(now());
                }
                call.state = "active".into();
                call.recording_status = "recording".into();
                call.entry("participant.joined",Some(i.actor.clone()),json!({"inviter":call.invitations[idx].inviter,"invitation_id":call.invitations[idx].invitation_id}),None);
                "call.accepted"
            }
            "calls.decline" => {
                let idx = offer_index(&call, &i.actor, p, true)?;
                if p.get("reason").is_some() && p["give_no_reason"] == true {
                    return Err(invalid("reason and give_no_reason are mutually exclusive"));
                }
                if silicon(&i.actor)
                    && p["reason"].as_str().is_none_or(|r| r.trim().is_empty())
                    && p["give_no_reason"] != true
                {
                    return Err(invalid(
                        "Silicon decline requires reason or give_no_reason=true",
                    ));
                }
                text_limit(p, "reason", 1000)?;
                call.invitations[idx].state = "declined".into();
                call.invitations[idx].reason = p["reason"].as_str().map(String::from);
                call.invitations[idx].silenced = true;
                call.entry("call.declined",Some(i.actor.clone()),json!({"reason":p["reason"],"invitation_id":call.invitations[idx].invitation_id}),None);
                self.settle_unanswered(&mut call);
                "call.declined"
            }
            "calls.silence" => {
                let idx = offer_index(&call, &i.actor, p, true)?;
                call.invitations[idx].silenced = true;
                "call.silenced"
            }
            "calls.cut" => {
                if !call.active(&i.actor) {
                    return Err(forbidden());
                }
                for member in call
                    .participants
                    .iter_mut()
                    .filter(|p| p.actor == i.actor && p.left_at.is_none())
                {
                    member.left_at = Some(now());
                }
                for invitation in call
                    .invitations
                    .iter_mut()
                    .filter(|v| v.state == "pending" && v.inviter == i.actor)
                {
                    invitation.state = "canceled".into();
                }
                call.entry("participant.left", Some(i.actor.clone()), json!({}), None);
                let active = call
                    .participants
                    .iter()
                    .filter(|p| p.left_at.is_none())
                    .count();
                if active == 0
                    || (active == 1 && !call.invitations.iter().any(|v| v.state == "pending"))
                {
                    end_call(&mut call)
                }
                for d in
                    self.state.delegations.values_mut().filter(|d| {
                        d.ringid == ring && (d.actor == i.actor || call.state == "ended")
                    })
                {
                    d.status = "closed".into();
                }
                if call.state == "ended" {
                    "call.ended"
                } else {
                    "participant.left"
                }
            }
            "calls.invite" => {
                if !call.active(&i.actor) || !matches!(call.state.as_str(), "active" | "ringing") {
                    return Err(forbidden());
                }
                let target = actor_id(required(p, "target")?, &i.org_id)?;
                if call.active(&target)
                    || call
                        .invitations
                        .iter()
                        .any(|v| v.target == target && v.state == "pending")
                {
                    return Err(Fault::new(
                        "ALREADY_INVITED",
                        "Recipient is already in the call or has a pending offer.",
                        "Inspect the call roster.",
                    ));
                }
                let recipient = self
                    .state
                    .recipient_identity(&i.realm, &target)
                    .ok_or_else(|| missing("Registered recipient"))?;
                call.actor_orgs
                    .insert(target.clone(), recipient.org_id.clone());
                let busy = self.state.busy(&target, &i.realm, Some(ring));
                let offer = Invitation {
                    invitation_id: id("offer"),
                    inviter: i.actor.clone(),
                    target: target.clone(),
                    state: if busy { "busy" } else { "pending" }.into(),
                    created_at: now(),
                    expires_at: after(60),
                    reason: None,
                    silenced: false,
                };
                call.entry(
                    "participant.invited",
                    Some(i.actor.clone()),
                    json!({"target":target,"invitation_id":offer.invitation_id}),
                    None,
                );
                call.invitations.push(offer);
                if busy && self.voicemail_enabled(&i.realm, &recipient.org_id, &target) {
                    "voicemail.offered"
                } else if busy {
                    "call.invitation.busy"
                } else {
                    "call.incoming"
                }
            }
            "calls.handoff" => {
                if silicon(&i.actor) || !call.active(&i.actor) {
                    return Err(forbidden());
                }
                let d = required(p, "to_device_id")?;
                if !self
                    .state
                    .devices
                    .get(d)
                    .is_some_and(|v| owns_actor(&v.owner, &i.realm, &i.actor) && !v.revoked)
                {
                    return Err(forbidden());
                }
                let target_org = self.state.devices[d]
                    .owner
                    .splitn(3, '|')
                    .nth(1)
                    .unwrap()
                    .to_owned();
                call.actor_orgs.insert(i.actor.clone(), target_org);
                for member in call
                    .participants
                    .iter_mut()
                    .filter(|m| m.actor == i.actor && m.left_at.is_none())
                {
                    member.device_id = Some(d.into());
                }
                "call.handoff"
            }
            _ => unreachable!(),
        };
        self.state.call_event(&call,event,json!({"ringid":ring,"actor":i.actor,"state":call.state,"invitation_id":call.invitations.last().map(|v|&v.invitation_id),"target":call.invitations.last().map(|v|&v.target)}));
        if call.state == "ended" && event != "call.ended" {
            self.state
                .call_event(&call, "call.ended", json!({"ringid":ring,"state":"ended"}));
        }
        if call.state == "voicemail" && event == "call.declined" {
            self.state.call_event(
                &call,
                "voicemail.offered",
                json!({"ringid":ring,"state":"voicemail"}),
            );
        }
        let result = public_call(&call, &i.actor);
        self.state.calls.insert(ring.into(), call);
        Ok(result)
    }
    fn send(&mut self, i: &Identity, p: &Value) -> Result<Value> {
        if !silicon(&i.actor) {
            return Err(forbidden());
        }
        text_limit(p, "text", 160)?;
        let text = required(p, "text")?;
        let kind = required(p, "kind")?;
        if !matches!(kind, "thinking" | "commentary") {
            return Err(invalid("kind must be thinking or commentary"));
        }
        if p.get("delegation_id").is_none() {
            return Err(invalid(
                "delegation_id is required; send null when unrelated to delegation",
            ));
        }
        let ring = required(p, "ringid")?;
        let call = self.state.call(i, ring)?;
        if call.state != "active" || !call.active(&i.actor) {
            return Err(Fault::new(
                "NO_ACTIVE_REPRESENTATIVE",
                "You have no active representative in this call.",
                "Accept or initiate a connected call first.",
            ));
        }
        let msg = json!({"message_id":id("msg"),"kind":kind,"text":text,"delegation_id":p["delegation_id"],"occurred_at":now()});
        if let Some(d) = p["delegation_id"].as_str() {
            let delegation = self
                .state
                .delegations
                .get_mut(d)
                .filter(|d| {
                    d.ringid == ring
                        && d.actor == i.actor
                        && d.realm == i.realm
                        && d.status == "open"
                })
                .ok_or_else(forbidden)?;
            delegation.responses.push(msg.clone());
        } else if !p["delegation_id"].is_null() {
            return Err(invalid("delegation_id must be a string or null"));
        }
        let call = self.state.calls.get_mut(ring).unwrap();
        let entry = call.entry(
            kind,
            Some(i.actor.clone()),
            msg.clone(),
            Some(i.actor.clone()),
        );
        self.state.event(
            i,
            vec![i.actor.clone()],
            "representative.message",
            json!({"ringid":ring,"entry":entry}),
        );
        Ok(json!({"message_id":msg["message_id"],"transcript_seq":entry.seq}))
    }
    fn voicemail(&mut self, i: &Identity, m: &str, p: &Value) -> Result<Value> {
        if m == "voicemail.list" {
            let mut rows = Vec::new();
            for v in
                self.state.voicemails.values().filter(|v| {
                    v.recipient == i.actor && v.realm == i.realm && v.state == "delivered"
                })
            {
                if p["unread"] != false && v.read
                    || !time_matches(&v.created_at, p)?
                    || p["actor"]
                        .as_str()
                        .is_some_and(|a| a.trim_start_matches('@') != v.sender)
                {
                    continue;
                }
                rows.push(json!(v));
            }
            rows.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
            return paginate(rows, p);
        }
        if m == "voicemail.begin" {
            self.expire_voicemails(&now());
            let ring = required(p, "ringid")?;
            let c = self.state.call(i, ring)?;
            let offers: Vec<_> = c
                .invitations
                .iter()
                .filter(|v| {
                    v.inviter == i.actor
                        && matches!(v.state.as_str(), "busy" | "declined" | "timeout")
                        && p["invitation_id"]
                            .as_str()
                            .is_none_or(|x| x == v.invitation_id)
                })
                .collect();
            if offers.len() != 1 {
                return Err(invalid(
                    "Specify one eligible busy, declined or timed-out invitation",
                ));
            }
            let offer = offers[0];
            let recipient = c.identity(&offer.target);
            let cfg = self.state.config(&recipient);
            if !self.voicemail_enabled(&i.realm, &recipient.org_id, &recipient.actor) {
                return Err(Fault::new(
                    "VOICEMAIL_DISABLED",
                    "Recipient has disabled voicemail.",
                    "Try another call later.",
                ));
            }
            if self.state.voicemails.values().any(|v| {
                v.invitation_id == offer.invitation_id
                    && v.sender == i.actor
                    && !matches!(v.state.as_str(), "aborted" | "deleted" | "purged")
            }) {
                return Err(Fault::new(
                    "VOICEMAIL_EXISTS",
                    "This offer already has a voicemail draft or message.",
                    "Continue the existing draft, or abort it before recording again.",
                ));
            }
            let format = required(p, "format")?;
            if format != if silicon(&i.actor) { "text" } else { "audio" } {
                return Err(invalid(
                    "Silicons leave text voicemails; carbons leave audio",
                ));
            }
            let greeting=cfg.values.get(&format!("voicemail.greetings.{}",offer.state)).cloned().unwrap_or_else(||json!({"text":if offer.state=="busy"{"The Silicon you're trying to reach is currently talking to someone else, please wait or try again later. You can leave a message at the beep."}else{"Your call could not be answered. Please leave a message after the beep."}}));
            let v = Voicemail {
                recipient_org_id: recipient.org_id.clone(),
                voicemail_id: id("vm"),
                ringid: ring.into(),
                invitation_id: offer.invitation_id.clone(),
                org_id: i.org_id.clone(),
                realm: i.realm.clone(),
                sender: i.actor.clone(),
                recipient: offer.target.clone(),
                format: format.into(),
                state: "draft".into(),
                created_at: now(),
                expires_at: after(600),
                reason: offer.reason.clone(),
                text: None,
                transcript: None,
                audio_asset_id: None,
                read: false,
                complete_audio: false,
                transcription_status: "pending".into(),
                synthesis_status: "pending".into(),
                error: None,
            };
            let mut result = json!(v);
            result["greeting"] = greeting;
            result["beep"] = json!({"frequency_hz":1000,"duration_ms":250});
            result["max_duration_seconds"] = json!(180);
            self.state.voicemails.insert(v.voicemail_id.clone(), v);
            return Ok(result);
        }
        let vid = required(p, "voicemail_id")?;
        let v = self
            .state
            .voicemails
            .get(vid)
            .filter(|v| {
                v.realm == i.realm
                    && (v.sender == i.actor || v.recipient == i.actor && v.state == "delivered")
            })
            .ok_or_else(forbidden)?;
        if m == "voicemail.get" {
            return Ok(json!(v));
        }
        if matches!(m, "voicemail.mark" | "voicemail.delete") {
            if v.recipient != i.actor || v.state != "delivered" {
                return Err(forbidden());
            }
        } else if v.sender != i.actor
            || if m == "voicemail.abort" {
                !matches!(v.state.as_str(), "draft" | "aborted")
            } else {
                v.state != "draft" || v.expires_at <= now()
            }
        {
            return Err(Fault::new(
                "VOICEMAIL_NOT_DRAFT",
                "This draft is expired or already finalized.",
                "Start a new eligible voicemail draft.",
            ));
        }
        match m {
            "voicemail.mark" => {
                let read = p["read"]
                    .as_bool()
                    .ok_or_else(|| invalid("read must be boolean"))?;
                self.state.voicemails.get_mut(vid).unwrap().read = read;
            }
            "voicemail.delete" => {
                self.state.voicemails.get_mut(vid).unwrap().state = "deleted".into();
            }
            "voicemail.abort" => {
                self.state.voicemails.get_mut(vid).unwrap().state = "aborted".into();
            }
            "voicemail.send" => {
                if !silicon(&i.actor) || v.text.is_some() {
                    return Err(invalid("A silicon can fill a text draft only once"));
                }
                text_limit(p, "text", 4000)?;
                let text = required(p, "text")?.to_string();
                let v = self.state.voicemails.get_mut(vid).unwrap();
                v.text = Some(text);
                v.synthesis_status = "processing".into();
            }
            "voicemail.commit" => {
                if !v.complete_audio || v.audio_asset_id.is_none() {
                    return Err(Fault::new(
                        "AUDIO_NOT_READY",
                        "Complete audio or successful voice synthesis is required before commit.",
                        "Wait for synthesis, or detach recorded media and verify every chunk.",
                    ));
                }
                let v = self.state.voicemails.get_mut(vid).unwrap();
                v.state = "delivered".into();
                if v.format == "text" {
                    v.transcript = v.text.clone();
                    v.transcription_status = "ready".into();
                }
                let recipient = v.recipient.clone();
                let ring = v.ringid.clone();
                let metadata = json!({"voicemail_id":vid,"sender":v.sender,"ringid":ring,"transcript":v.transcript});
                self.state
                    .event(i, vec![recipient], "voicemail.received", metadata);
                if let Some(c) = self.state.calls.get_mut(&ring) {
                    if c.state == "voicemail" {
                        end_call(c);
                    }
                }
            }
            _ => unreachable!(),
        }
        Ok(json!(self.state.voicemails[vid]))
    }
    pub fn owned_asset(&self, i: &Identity, aid: &str) -> Result<&Asset> {
        self.state
            .assets
            .get(aid)
            .filter(|a| a.owner == key(&i.realm, &i.org_id, &i.actor))
            .ok_or_else(forbidden)
    }
    pub fn authorized_asset(&self, i: &Identity, aid: &str) -> Result<&Asset> {
        let a = self.state.assets.get(aid).ok_or_else(forbidden)?;
        if owns_actor(&a.owner, &i.realm, &i.actor) {
            return Ok(a);
        }
        if a.purpose == "voicemail_greeting"
            && a.complete
            && self.state.greetings.iter().any(|(vid, id)| {
                id == aid
                    && self.state.voicemails.get(vid).is_some_and(|v| {
                        v.sender == i.actor
                            && v.realm == i.realm
                            && matches!(v.state.as_str(), "draft" | "delivered")
                    })
            })
        {
            return Ok(a);
        }
        if a.purpose == "profile_photo"
            && self.state.profiles.iter().any(|(k, p)| {
                k.starts_with(&format!("{}|", i.realm)) && p.photo_asset_id.as_deref() == Some(aid)
            })
        {
            return Ok(a);
        }
        if a.ringid.as_ref().is_some_and(|ring| {
            self.state
                .call(i, ring)
                .is_ok_and(|c| c.participants.iter().any(|p| p.actor == i.actor))
        }) {
            return Ok(a);
        }
        if a.voicemail_id.as_ref().is_some_and(|vid| {
            self.state.voicemails.get(vid).is_some_and(|v| {
                v.realm == i.realm && v.recipient == i.actor && v.state == "delivered"
            })
        }) {
            return Ok(a);
        }
        Err(forbidden())
    }
    fn expire_voicemails(&mut self, time: &str) -> bool {
        let mut changed = false;
        for v in self
            .state
            .voicemails
            .values_mut()
            .filter(|v| v.state == "draft" && v.expires_at.as_str() <= time)
        {
            v.state = "aborted".into();
            changed = true;
        }
        changed
    }
    pub fn tick(&mut self, flush_transcripts: bool) -> bool {
        let time = now();
        let mut changed = self.expire_voicemails(&time);
        let ids: Vec<_> = self.state.calls.keys().cloned().collect();
        for id in ids {
            let mut c = self.state.calls[&id].clone();
            let mut expired = vec![];
            for offer in c
                .invitations
                .iter_mut()
                .filter(|v| v.state == "pending" && v.expires_at <= time)
            {
                offer.state = "timeout".into();
                offer.silenced = true;
                expired.push(offer.invitation_id.clone());
            }
            if !expired.is_empty() {
                changed = true;
                for offer in expired {
                    c.entry("call.timeout", None, json!({"invitation_id":offer}), None);
                }
                self.settle_unanswered(&mut c);
                self.state
                    .call_event(&c, "call.timeout", json!({"ringid":id,"state":c.state}));
                if c.state == "ended" {
                    self.state
                        .call_event(&c, "call.ended", json!({"ringid":id,"state":"ended"}));
                } else if c.state == "voicemail" {
                    self.state.call_event(
                        &c,
                        "voicemail.offered",
                        json!({"ringid":id,"state":"voicemail"}),
                    );
                }
                self.state.calls.insert(id.clone(), c.clone());
            }
            if c.state == "active" && flush_transcripts || c.state == "ended" {
                for p in c.participants.iter().filter(|p| silicon(&p.actor)) {
                    let cursor_key = format!("{}|{}", id, p.actor);
                    let last = self
                        .state
                        .transcript_cursors
                        .get(&cursor_key)
                        .copied()
                        .unwrap_or(0);
                    let entries: Vec<_> = c
                        .transcript
                        .iter()
                        .filter(|e| e.seq > last && entry_visible(&c, e, &p.actor))
                        .cloned()
                        .collect();
                    if !entries.is_empty() {
                        let upper = c.transcript.last().map_or(last, |e| e.seq);
                        let n = Publication {
                            notification_id: format!("transcript_{}_{}_{}", id, p.actor, upper),
                            actor: p.actor.clone(),
                            org_id: c.identity(&p.actor).org_id,
                            realm: c.realm.clone(),
                            event_type: "transcript.batch".into(),
                            data: json!({"ringid":id,"from_seq":entries[0].seq,"to_seq":upper,"entries":entries}),
                            status: "pending".into(),
                            attempts: 0,
                            retry_at: None,
                            error: None,
                        };
                        self.state.publications.insert(n.notification_id.clone(), n);
                        self.state.transcript_cursors.insert(cursor_key, upper);
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}
use rusqlite::OptionalExtension;
fn json_value<T>(v: T) -> Result<T> {
    Ok(v)
}
pub const VOICES: &[&str] = &[
    "ripple", "vesper", "willow", "stone", "gleam", "meridian", "bossa", "tempo",
];
fn read_only(m: &str) -> bool {
    m.ends_with(".get")
        || m.ends_with(".list")
        || matches!(m, "auth.status" | "notifications.status" | "release.info")
}
pub fn public_call(c: &Call, actor: &str) -> Value {
    let mut out = json!(c);
    out.as_object_mut().unwrap().remove("transcript");
    out.as_object_mut().unwrap().remove("actor_orgs");
    if let Some(rows) = out["participants"].as_array_mut() {
        for p in rows {
            p.as_object_mut().unwrap().remove("context");
            p.as_object_mut().unwrap().remove("start");
        }
    }
    out["invitation_id"] = c
        .invitations
        .iter()
        .rev()
        .find(|v| v.target == actor || v.inviter == actor)
        .map(|v| json!(v.invitation_id))
        .unwrap_or(Value::Null);
    out
}
pub fn entry_visible(c: &Call, e: &Entry, actor: &str) -> bool {
    if let Some(owner) = &e.private_to {
        return owner == actor;
    }
    if e.kind == "speech" {
        if let (Some(base), Some(from), Some(to)) = (
            c.answered_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
            e.data["start_ms"].as_u64(),
            e.data["end_ms"].as_u64(),
        ) {
            let from = base + chrono::Duration::milliseconds(from.min(i64::MAX as u64) as i64);
            let to = base + chrono::Duration::milliseconds(to.min(i64::MAX as u64) as i64);
            return c.participants.iter().any(|p| {
                p.actor == actor
                    && chrono::DateTime::parse_from_rfc3339(&p.joined_at)
                        .is_ok_and(|join| from >= join)
                    && p.left_at.as_deref().is_none_or(|s| {
                        chrono::DateTime::parse_from_rfc3339(s).is_ok_and(|left| to <= left)
                    })
            });
        }
    }
    c.participants.iter().any(|p| {
        p.actor == actor
            && e.occurred_at >= p.joined_at
            && p.left_at.as_ref().is_none_or(|t| e.occurred_at <= *t)
    }) || e.kind == "call.created" && c.caller == actor
}
fn offer_index(c: &Call, actor: &str, p: &Value, pending: bool) -> Result<usize> {
    let offers: Vec<_> = c
        .invitations
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            v.target == actor
                && (!pending || v.state == "pending")
                && p["invitation_id"]
                    .as_str()
                    .is_none_or(|id| id == v.invitation_id)
        })
        .map(|(n, _)| n)
        .collect();
    if offers.len() != 1 {
        return Err(Fault::new(
            "INVITATION_UNAVAILABLE",
            "No unique pending invitation is available for this identity.",
            "List calls and specify the current invitation ID.",
        ));
    }
    Ok(offers[0])
}
pub fn end_call(c: &mut Call) {
    c.state = "ended".into();
    c.ended_at = Some(now());
    for p in &mut c.participants {
        if p.left_at.is_none() {
            p.left_at = Some(now());
        }
    }
    for offer in c.invitations.iter_mut().filter(|i| i.state == "pending") {
        offer.state = "canceled".into();
    }
    if c.recording_status == "recording" {
        c.recording_status = "processing".into();
    }
    c.entry("call.ended", None, json!({}), None);
}
pub fn time_matches(t: &str, p: &Value) -> Result<bool> {
    let t = chrono::DateTime::parse_from_rfc3339(t)
        .map_err(|_| invalid("Stored timestamp is invalid"))?;
    for (name, lower) in [("since", true), ("until", false)] {
        if let Some(v) = p.get(name) {
            let s = v
                .as_str()
                .ok_or_else(|| invalid(format!("{name} must be RFC3339 text")))?;
            let bound = chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| invalid(format!("{name} must be RFC3339")))?;
            if lower && t < bound || !lower && t >= bound {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
pub fn paginate(rows: Vec<Value>, p: &Value) -> Result<Value> {
    let limit = match p.get("limit") {
        None => 50,
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0 && *n <= 200)
            .ok_or_else(|| invalid("limit must be 1–200"))? as usize,
    };
    let offset = match p.get("cursor") {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_str()
            .and_then(|s| s.strip_prefix("offset:"))
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| invalid("Invalid cursor"))?,
    };
    let next = if offset.saturating_add(limit) < rows.len() {
        Some(format!("offset:{}", offset + limit))
    } else {
        None
    };
    Ok(
        json!({"items":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"next_cursor":next}),
    )
}

fn redact_config(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if matches!(
                    name.as_str(),
                    "api_key" | "secret_access_key" | "session_token"
                ) {
                    *value = json!("[redacted]");
                } else {
                    redact_config(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_config(value);
            }
        }
        _ => {}
    }
}
#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;

fn public_device(device: &Device) -> Value {
    json!({"device_id":device.device_id,"name":device.name,"ring_enabled":device.ring_enabled,"revoked":device.revoked,"push_enabled":device.push_token.is_some(),"push_platform":device.push_platform,"push_environment":device.push_environment})
}

fn release_info(params: &Value) -> Result<Value> {
    let Some(path) = std::env::var_os("RING_RELEASE_INDEX_FILE") else {
        return Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"protocol_major":1,"channel":"stable","update_available":false,"url":null,"signature":null}),
        );
    };
    let bytes = std::fs::read(path).map_err(|_| {
        Fault::new(
            "RELEASE_METADATA_UNAVAILABLE",
            "Configured release index could not be read.",
            "Check RING_RELEASE_INDEX_FILE and deployment file permissions.",
        )
    })?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid("Release metadata exceeds one MiB"));
    }
    let index: Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("Configured release index is not valid JSON"))?;
    let platform = params["platform"].as_str().unwrap_or(std::env::consts::OS);
    let arch = params["arch"].as_str().unwrap_or(std::env::consts::ARCH);
    let channel = params["channel"].as_str().unwrap_or("stable");
    let release = index["releases"].as_array().and_then(|items| {
        items.iter().find(|r| {
            r["platform"] == platform
                && r["arch"] == arch
                && r["protocol_major"] == 1
                && r["channel"].as_str().unwrap_or("stable") == channel
        })
    });
    let Some(release) = release else {
        return Ok(
            json!({"protocol_major":1,"channel":channel,"platform":platform,"arch":arch,"update_available":false,"reason":"No compatible release is published for this target"}),
        );
    };
    if !release["url"]
        .as_str()
        .is_some_and(|url| url.starts_with("https://"))
        || release["signature"].as_str().is_none_or(|s| s.is_empty())
        || release["sha256"].as_str().is_none_or(|s| s.len() != 64)
    {
        return Err(invalid(
            "Published release metadata requires HTTPS, SHA-256 and signature fields",
        ));
    }
    let mut response = release.clone();
    response["update_available"] =
        json!(params["current_version"].as_str() != response["version"].as_str());
    Ok(response)
}

fn validate_asset_header(asset: &Asset) -> Result<()> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    let mut file =
        std::fs::File::open(&asset.path).map_err(|_| invalid("Uploaded file could not be read"))?;
    let length = file
        .read(&mut bytes)
        .map_err(|_| invalid("Uploaded file could not be read"))?;
    let b = &bytes[..length];
    let valid = match asset.mime_type.as_str() {
        "image/png" => b.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => b.starts_with(&[0xff, 0xd8, 0xff]),
        "image/webp" => b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP"),
        "audio/wav" => b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WAVE"),
        "audio/webm" => b.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        "audio/ogg" => b.starts_with(b"OggS"),
        "audio/mpeg" => {
            b.starts_with(b"ID3")
                || b.first() == Some(&0xff) && b.get(1).is_some_and(|n| n & 0xe0 == 0xe0)
        }
        "audio/mp4" => b.get(4..8) == Some(b"ftyp"),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "Uploaded bytes do not match the declared media type",
        ))
    }
}

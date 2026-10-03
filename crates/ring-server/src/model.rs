use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
pub fn after(seconds: i64) -> String {
    (Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub fn key(realm: &str, org: &str, actor: &str) -> String {
    format!("{realm}|{org}|{actor}")
}
pub fn silicon(actor: &str) -> bool {
    actor.starts_with("si:")
}
pub fn digest(s: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(s.as_bytes()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fault {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub next_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
impl Fault {
    pub fn new(code: &str, message: impl Into<String>, next: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            next_action: next.into(),
            details: None,
        }
    }
}
pub type Result<T> = std::result::Result<T, Fault>;
pub fn invalid(message: impl Into<String>) -> Fault {
    Fault::new(
        "INVALID_INPUT",
        message,
        "Check the command help and correct the indicated field.",
    )
}
pub fn forbidden() -> Fault {
    Fault::new(
        "FORBIDDEN",
        "This identity cannot access this resource.",
        "Use an identity invited to this resource.",
    )
}
pub fn missing(what: &str) -> Fault {
    Fault::new(
        "NOT_FOUND",
        format!("{what} was not found."),
        "List resources and use a current ID.",
    )
}
pub fn required<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(format!("{k} must be a nonempty string")))
}
pub fn text_limit(v: &Value, k: &str, max: usize) -> Result<()> {
    if let Some(x) = v.get(k) {
        let s = x
            .as_str()
            .ok_or_else(|| invalid(format!("{k} must be literal text")))?;
        if s.chars().count() > max {
            return Err(invalid(format!("{k} exceeds {max} Unicode characters")));
        }
    }
    Ok(())
}
pub fn actor_id(raw: &str, _org: &str) -> Result<String> {
    let raw = raw.trim_start_matches('@');
    let actor = if let Some((actor, suffix)) = raw.split_once('[') {
        let suffix = suffix
            .strip_suffix(']')
            .ok_or_else(|| invalid("Invalid identity suffix"))?;
        if suffix.is_empty()
            || suffix
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '|'))
        {
            return Err(invalid("Invalid identity suffix"));
        }
        actor
    } else {
        raw
    };
    if !(actor.starts_with("c:") || actor.starts_with("si:"))
        || actor.split_once(':').unwrap().1.is_empty()
        || actor
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '|' | '[' | ']'))
    {
        return Err(invalid("Expected a public c:handle or si:handle identity"));
    }
    Ok(actor.into())
}

pub fn owns_actor(owner: &str, realm: &str, actor: &str) -> bool {
    let mut parts = owner.splitn(3, '|');
    parts.next() == Some(realm) && parts.next().is_some() && parts.next() == Some(actor)
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Identity {
    pub actor: String,
    pub org_id: String,
    pub realm: String,
    pub display_name: String,
    #[serde(default)]
    pub admin: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub identity: Identity,
    pub device_id: String,
    pub expires_at: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub owner: String,
    pub name: String,
    pub ring_enabled: bool,
    pub revoked: bool,
    #[serde(default)]
    pub push_token: Option<String>,
    #[serde(default)]
    pub push_platform: Option<String>,
    #[serde(default)]
    pub push_environment: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub actor: String,
    pub display_name: String,
    pub photo_asset_id: Option<String>,
    pub voice_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Participant {
    pub actor: String,
    pub display_name: String,
    pub device_id: Option<String>,
    pub joined_at: String,
    pub left_at: Option<String>,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub start: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub invitation_id: String,
    pub inviter: String,
    pub target: String,
    pub state: String,
    pub created_at: String,
    pub expires_at: String,
    pub reason: Option<String>,
    pub silenced: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub occurred_at: String,
    pub kind: String,
    pub actor: Option<String>,
    pub data: Value,
    pub private_to: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Call {
    pub ringid: String,
    // IAM/provider context belongs to each actor; it is never a call access boundary.
    #[serde(default)]
    pub actor_orgs: BTreeMap<String, String>,
    pub org_id: String,
    pub realm: String,
    pub caller: String,
    pub target: String,
    pub state: String,
    pub created_at: String,
    pub answered_at: Option<String>,
    pub ended_at: Option<String>,
    pub participants: Vec<Participant>,
    pub invitations: Vec<Invitation>,
    pub transcript: Vec<Entry>,
    pub recording_status: String,
    pub recording_asset_id: Option<String>,
}
impl Call {
    pub fn credential_orgs(&self) -> BTreeMap<String, String> {
        let mut routes = self.actor_orgs.clone();
        for actor in self
            .participants
            .iter()
            .map(|p| &p.actor)
            .chain(self.invitations.iter().map(|v| &v.target))
        {
            routes
                .entry(actor.clone())
                .or_insert_with(|| self.org_id.clone());
        }
        routes
    }
    pub fn identity(&self, actor: &str) -> Identity {
        Identity {
            actor: actor.into(),
            org_id: self
                .actor_orgs
                .get(actor)
                .cloned()
                .unwrap_or_else(|| self.org_id.clone()),
            realm: self.realm.clone(),
            display_name: self
                .participants
                .iter()
                .find(|p| p.actor == actor)
                .map(|p| p.display_name.clone())
                .unwrap_or_else(|| actor.into()),
            admin: false,
        }
    }
    pub fn active(&self, actor: &str) -> bool {
        self.participants
            .iter()
            .any(|p| p.actor == actor && p.left_at.is_none())
    }
    pub fn visible(&self, actor: &str) -> bool {
        self.participants.iter().any(|p| p.actor == actor)
            || self.invitations.iter().any(|i| i.target == actor)
    }
    pub fn entry(
        &mut self,
        kind: &str,
        actor: Option<String>,
        data: Value,
        private: Option<String>,
    ) -> Entry {
        let e = Entry {
            seq: self.transcript.last().map_or(1, |x| x.seq + 1),
            occurred_at: now(),
            kind: kind.into(),
            actor,
            data,
            private_to: private,
        };
        self.transcript.push(e.clone());
        e
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Preparation {
    pub preparation_id: String,
    pub owner: String,
    pub action: String,
    pub target: String,
    pub invitation_id: Option<String>,
    pub default_text: String,
    pub default_revision: u64,
    pub supplied_text: String,
    pub context_mode: String,
    pub effective_text: String,
    pub expires_at: String,
    pub approval_required: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Approval {
    pub approval_id: String,
    pub owner: String,
    pub default_revision: u64,
    pub context_mode: String,
    pub expires_at: String,
    pub revoked: bool,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub revision: u64,
    pub values: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Voicemail {
    #[serde(default)]
    pub recipient_org_id: String,
    pub voicemail_id: String,
    pub ringid: String,
    pub invitation_id: String,
    pub org_id: String,
    pub realm: String,
    pub sender: String,
    pub recipient: String,
    pub format: String,
    pub state: String,
    pub created_at: String,
    pub expires_at: String,
    pub reason: Option<String>,
    pub text: Option<String>,
    pub transcript: Option<String>,
    pub audio_asset_id: Option<String>,
    pub read: bool,
    pub complete_audio: bool,
    pub transcription_status: String,
    pub synthesis_status: String,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Asset {
    pub asset_id: String,
    pub owner: String,
    pub purpose: String,
    pub mime_type: String,
    pub size_bytes: usize,
    pub received_bytes: usize,
    pub next_seq: u64,
    pub complete: bool,
    pub path: String,
    pub ringid: Option<String>,
    pub voicemail_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Delegation {
    pub delegation_id: String,
    pub ringid: String,
    pub actor: String,
    pub org_id: String,
    pub realm: String,
    pub request: String,
    pub context: Vec<Entry>,
    pub responses: Vec<Value>,
    pub status: String,
    pub created_at: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub occurred_at: String,
    pub data: Value,
    pub org_id: String,
    pub realm: String,
    pub recipients: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Publication {
    pub notification_id: String,
    pub actor: String,
    pub org_id: String,
    pub realm: String,
    pub event_type: String,
    pub data: Value,
    pub status: String,
    pub attempts: u32,
    #[serde(default)]
    pub retry_at: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Cached {
    pub fingerprint: String,
    pub response: Value,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub identities: BTreeMap<String, Identity>,
    #[serde(default)]
    pub recipient_routes: BTreeMap<String, String>,
    #[serde(default)]
    pub event_seq: u64,
    #[serde(default)]
    pub push_cursor: u64,
    #[serde(default)]
    pub pushes: BTreeMap<String, Value>,
    #[serde(default)]
    pub greetings: BTreeMap<String, String>,
    #[serde(default)]
    pub greeting_cache: BTreeMap<String, String>,
    #[serde(default)]
    pub sent_messages: BTreeMap<String, String>,
    #[serde(default)]
    pub recording_gaps: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub storage: BTreeMap<String, Value>,
    pub sessions: BTreeMap<String, Session>,
    pub devices: BTreeMap<String, Device>,
    pub profiles: BTreeMap<String, Profile>,
    pub configs: BTreeMap<String, Config>,
    pub calls: BTreeMap<String, Call>,
    pub preparations: BTreeMap<String, Preparation>,
    pub approvals: BTreeMap<String, Approval>,
    pub voicemails: BTreeMap<String, Voicemail>,
    pub assets: BTreeMap<String, Asset>,
    pub delegations: BTreeMap<String, Delegation>,
    pub events: Vec<Event>,
    pub publications: BTreeMap<String, Publication>,
    pub requests: BTreeMap<String, Cached>,
    pub bugs: BTreeMap<String, Value>,
    #[serde(default)]
    pub transcript_cursors: BTreeMap<String, u64>,
}
impl State {
    pub fn remember_identity(&mut self, identity: &Identity) {
        let owner = key(&identity.realm, &identity.org_id, &identity.actor);
        self.identities.insert(owner.clone(), identity.clone());
        self.recipient_routes
            .insert(format!("{}|{}", identity.realm, identity.actor), owner);
    }
    pub fn registered_identities(&self, realm: &str, actor: &str) -> Vec<Identity> {
        let mut candidates = BTreeMap::new();
        // Older snapshots predate the registry. Profiles provide candidate contexts;
        // auth verifies their own retained IAM credentials before routing production calls.
        for (owner, profile) in &self.profiles {
            if owns_actor(owner, realm, actor) {
                let org = owner.splitn(3, '|').nth(1).unwrap();
                candidates.insert(
                    owner.clone(),
                    Identity {
                        actor: actor.into(),
                        org_id: org.into(),
                        realm: realm.into(),
                        display_name: profile.display_name.clone(),
                        admin: false,
                    },
                );
            }
        }
        for session in self.sessions.values() {
            let i = &session.identity;
            if i.realm == realm && i.actor == actor {
                candidates.insert(key(realm, &i.org_id, actor), i.clone());
            }
        }
        for (owner, i) in &self.identities {
            if i.realm == realm && i.actor == actor {
                candidates.insert(owner.clone(), i.clone());
            }
        }
        let preferred = self.recipient_routes.get(&format!("{realm}|{actor}"));
        let mut result = Vec::new();
        if let Some(i) = preferred.and_then(|owner| candidates.remove(owner)) {
            result.push(i);
        }
        result.extend(candidates.into_values());
        result
    }
    pub fn recipient_identity(&self, realm: &str, actor: &str) -> Option<Identity> {
        self.registered_identities(realm, actor).into_iter().next()
    }
    pub fn latest_event_seq(&self) -> u64 {
        self.event_seq.max(self.events.last().map_or(0, |e| e.seq))
    }

    pub fn profile(&self, i: &Identity) -> Profile {
        self.profiles
            .get(&key(&i.realm, &i.org_id, &i.actor))
            .cloned()
            .unwrap_or(Profile {
                actor: i.actor.clone(),
                display_name: i.display_name.clone(),
                photo_asset_id: None,
                voice_id: "gleam".into(),
            })
    }
    pub fn config(&self, i: &Identity) -> Config {
        self.configs
            .get(&key(&i.realm, &i.org_id, &i.actor))
            .cloned()
            .unwrap_or_default()
    }
    pub fn event(&mut self, i: &Identity, recipients: Vec<String>, kind: &str, data: Value) {
        let mut routes = data["ringid"]
            .as_str()
            .and_then(|id| self.calls.get(id))
            .map(Call::credential_orgs)
            .unwrap_or_default();
        if let Some(vm) = data["voicemail_id"]
            .as_str()
            .and_then(|id| self.voicemails.get(id))
        {
            routes.insert(vm.sender.clone(), vm.org_id.clone());
            routes.insert(
                vm.recipient.clone(),
                if vm.recipient_org_id.is_empty() {
                    vm.org_id.clone()
                } else {
                    vm.recipient_org_id.clone()
                },
            );
        }
        self.event_routed(i, recipients, kind, data, &routes);
    }
    fn event_routed(
        &mut self,
        i: &Identity,
        recipients: Vec<String>,
        kind: &str,
        data: Value,
        routes: &BTreeMap<String, String>,
    ) {
        self.event_seq = self.latest_event_seq() + 1;
        let e = Event {
            event_id: id("evt"),
            seq: self.event_seq,
            kind: kind.into(),
            occurred_at: now(),
            data: data.clone(),
            org_id: i.org_id.clone(),
            realm: i.realm.clone(),
            recipients: recipients.clone(),
        };
        for actor in recipients
            .iter()
            .filter(|a| silicon(a) && kind != "transcript.delta")
        {
            let n = Publication {
                notification_id: format!("{}_{}", e.event_id, actor),
                actor: actor.clone(),
                org_id: routes
                    .get(actor)
                    .cloned()
                    .or_else(|| {
                        if actor == &i.actor {
                            Some(i.org_id.clone())
                        } else {
                            self.recipient_identity(&i.realm, actor).map(|i| i.org_id)
                        }
                    })
                    .unwrap_or_else(|| i.org_id.clone()),
                realm: i.realm.clone(),
                event_type: kind.into(),
                data: data.clone(),
                status: "pending".into(),
                attempts: 0,
                retry_at: None,
                error: None,
            };
            self.publications.insert(n.notification_id.clone(), n);
        }
        self.events.push(e);
    }
    pub fn call_event(&mut self, call: &Call, kind: &str, data: Value) {
        let mut recipients: Vec<_> = call
            .participants
            .iter()
            .map(|p| p.actor.clone())
            .chain(call.invitations.iter().map(|i| i.target.clone()))
            .collect();
        recipients.sort();
        recipients.dedup();
        let i = Identity {
            actor: call.caller.clone(),
            org_id: call.org_id.clone(),
            realm: call.realm.clone(),
            display_name: String::new(),
            admin: false,
        };
        self.event_routed(&i, recipients, kind, data, &call.credential_orgs());
    }
    pub fn busy(&self, actor: &str, realm: &str, except: Option<&str>) -> bool {
        silicon(actor)
            && self.calls.values().any(|c| {
                c.realm == realm
                    && Some(c.ringid.as_str()) != except
                    && matches!(c.state.as_str(), "ringing" | "connecting" | "active")
                    && c.active(actor)
            })
    }
    pub fn call(&self, i: &Identity, r: &str) -> Result<&Call> {
        self.calls
            .get(r)
            .filter(|c| c.realm == i.realm && c.visible(&i.actor))
            .ok_or_else(forbidden)
    }
}
pub fn defaults() -> Value {
    json!({"representative.default_context":"","representative.context_mode":"append","representative.context_required":false,"voicemail.enabled":true,"telemetry.enabled":true,"retention.calls_days":30,"retention.voicemail_days":30})
}

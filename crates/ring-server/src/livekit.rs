//! LiveKit: access tokens for devices and server participants, rooms, and webhook verification.

use std::time::Duration;

use livekit_api::access_token::{AccessToken, TokenVerifier, VideoGrants};
use livekit_api::services::room::{CreateRoomOptions, RoomClient};
use livekit_api::webhooks::WebhookReceiver;
use ring_proto::{Account, MediaGrant, Time};

use crate::config::Config;

/// Rooms close this long after the last participant leaves (a little over the ringing time, so
/// a carbon caller who reconnects while ringing keeps the room).
const EMPTY_TIMEOUT_SECS: u32 = 70;
const DEVICE_TOKEN_TTL: Duration = Duration::from_secs(10 * 60);
const SERVER_TOKEN_TTL: Duration = Duration::from_secs(6 * 3600);

pub struct LiveKit {
    api_key: String,
    api_secret: String,
    public_url: String,
    rooms: RoomClient,
    receiver: WebhookReceiver,
}

pub fn carbon_identity(uuid: &str, device_id: &str) -> String {
    format!("carbon:{uuid}:{device_id}")
}

pub fn representative_identity(uuid: &str) -> String {
    format!("rep:{uuid}")
}

pub const RECORDER_IDENTITY: &str = "ring:recorder";
pub const VOICEMAIL_IDENTITY: &str = "ring:voicemail";

/// `carbon:<uuid>:<device_id>` → (uuid, device_id).
pub fn parse_carbon_identity(identity: &str) -> Option<(&str, &str)> {
    let rest = identity.strip_prefix("carbon:")?;
    rest.split_once(':')
}

impl LiveKit {
    pub fn new(config: &Config) -> LiveKit {
        let rooms = RoomClient::with_api_key(
            &config.livekit_url,
            &config.livekit_api_key,
            &config.livekit_api_secret,
        )
        .with_request_timeout(Duration::from_secs(8));
        LiveKit {
            api_key: config.livekit_api_key.clone(),
            api_secret: config.livekit_api_secret.clone(),
            public_url: config.livekit_public_url.clone(),
            rooms,
            receiver: WebhookReceiver::new(TokenVerifier::with_api_key(
                &config.livekit_api_key,
                &config.livekit_api_secret,
            )),
        }
    }

    fn token(&self) -> AccessToken {
        AccessToken::with_api_key(&self.api_key, &self.api_secret)
    }

    /// A grant for one carbon device: join `room`, publish only a microphone, subscribe to audio.
    pub fn device_grant(
        &self,
        room: &str,
        account: &Account,
        device_id: &str,
    ) -> Result<MediaGrant, String> {
        let identity = carbon_identity(&account.uuid, device_id);
        let grants = VideoGrants {
            room_join: true,
            room: room.to_string(),
            can_publish: Some(true),
            can_subscribe: Some(true),
            can_publish_data: Some(false),
            can_publish_sources: vec!["microphone".into()],
            can_update_own_metadata: Some(false),
            ..Default::default()
        };
        let jwt = self
            .token()
            .with_identity(&identity)
            .with_name(account.label())
            .with_kind("standard")
            .with_ttl(DEVICE_TOKEN_TTL)
            .with_grants(grants)
            .with_attributes(attributes(account, "device", Some(device_id)))
            .to_jwt()
            .map_err(|e| e.to_string())?;
        Ok(MediaGrant {
            url: self.public_url.clone(),
            token: jwt,
            room: room.to_string(),
            identity,
            expires_at: Time(
                chrono::Utc::now()
                    + chrono::Duration::from_std(DEVICE_TOKEN_TTL).unwrap_or_default(),
            ),
        })
    }

    /// The token ring-media uses to join as a silicon's representative.
    pub fn representative_token(&self, room: &str, silicon: &Account) -> Result<String, String> {
        let grants = VideoGrants {
            room_join: true,
            room: room.to_string(),
            can_publish: Some(true),
            can_subscribe: Some(true),
            can_publish_data: Some(false),
            can_publish_sources: vec!["microphone".into()],
            agent: true,
            ..Default::default()
        };
        self.token()
            .with_identity(&representative_identity(&silicon.uuid))
            .with_name(silicon.label())
            .with_kind("agent")
            .with_ttl(SERVER_TOKEN_TTL)
            .with_grants(grants)
            .with_attributes(attributes(silicon, "representative", None))
            .to_jwt()
            .map_err(|e| e.to_string())
    }

    pub fn recorder_token(&self, room: &str) -> Result<String, String> {
        let grants = VideoGrants {
            room_join: true,
            room: room.to_string(),
            can_publish: Some(false),
            can_subscribe: Some(true),
            can_publish_data: Some(false),
            hidden: true,
            recorder: true,
            ..Default::default()
        };
        self.token()
            .with_identity(RECORDER_IDENTITY)
            .with_name("Ring recorder")
            .with_ttl(SERVER_TOKEN_TTL)
            .with_grants(grants)
            .with_attributes([("role", "recorder")])
            .to_jwt()
            .map_err(|e| e.to_string())
    }

    pub fn voicemail_token(&self, room: &str) -> Result<String, String> {
        let grants = VideoGrants {
            room_join: true,
            room: room.to_string(),
            can_publish: Some(true),
            can_subscribe: Some(true),
            can_publish_data: Some(false),
            can_publish_sources: vec!["microphone".into()],
            ..Default::default()
        };
        self.token()
            .with_identity(VOICEMAIL_IDENTITY)
            .with_name("Voicemail")
            .with_kind("agent")
            .with_ttl(SERVER_TOKEN_TTL)
            .with_grants(grants)
            .with_attributes([("role", "voicemail")])
            .to_jwt()
            .map_err(|e| e.to_string())
    }

    pub async fn create_room(&self, room: &str) -> Result<(), String> {
        let options = CreateRoomOptions {
            empty_timeout: EMPTY_TIMEOUT_SECS,
            departure_timeout: 20,
            max_participants: 32,
            ..Default::default()
        };
        self.rooms
            .create_room(room, options)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn delete_room(&self, room: &str) -> Result<(), String> {
        self.rooms
            .delete_room(room)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), String> {
        self.rooms
            .remove_participant(room, identity)
            .await
            .map_err(|e| e.to_string())
    }

    /// Whether `identity` is connected to `room` right now; `None` when LiveKit could not say.
    pub async fn has_participant(&self, room: &str, identity: &str) -> Option<bool> {
        match self.rooms.list_participants(room).await {
            Ok(participants) => Some(participants.iter().any(|p| p.identity == identity)),
            Err(error) => {
                let error = error.to_string();
                if error.contains("not_found") || error.contains("404") {
                    Some(false)
                } else {
                    tracing::warn!(%error, "could not list LiveKit participants");
                    None
                }
            }
        }
    }

    /// Verifies a webhook body against its `Authorization` JWT (signed with our API key).
    pub fn verify_webhook(&self, body: &str, authorization: &str) -> Result<WebhookEvent, String> {
        let token = authorization
            .strip_prefix("Bearer ")
            .unwrap_or(authorization)
            .trim();
        let event = self
            .receiver
            .receive(body, token)
            .map_err(|e| e.to_string())?;
        Ok(WebhookEvent {
            event: event.event,
            room: event.room.map(|r| r.name).unwrap_or_default(),
            identity: event.participant.map(|p| p.identity),
            id: event.id,
        })
    }
}

/// The parts of a LiveKit webhook Ring acts on.
pub struct WebhookEvent {
    pub event: String,
    pub room: String,
    pub identity: Option<String>,
    pub id: String,
}

fn attributes(account: &Account, role: &str, device_id: Option<&str>) -> Vec<(String, String)> {
    let mut out = vec![
        ("account_uuid".to_string(), account.uuid.clone()),
        ("account_id".to_string(), account.id.clone()),
        ("display_name".to_string(), account.label().to_string()),
        ("kind".to_string(), account.kind.as_str().to_string()),
        ("role".to_string(), role.to_string()),
    ];
    if let Some(device_id) = device_id {
        out.push(("device_id".to_string(), device_id.to_string()));
    }
    out
}

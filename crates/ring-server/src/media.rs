use crate::{engine::Engine, model::*};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    time::Instant,
};
use tokio::sync::mpsc;

pub const SAMPLES: usize = 480;
pub const RATE: u32 = 24000;
#[derive(Clone)]
pub enum Output {
    Socket(mpsc::Sender<Value>),
    Representative(mpsc::Sender<Vec<u8>>),
}
pub struct Stream {
    pub id: String,
    pub owner: String,
    pub actor: String,
    pub ringid: String,
    pub device_id: String,
    pub token_hash: String,
    pub voicemail_id: Option<String>,
    pub muted: bool,
    pub next_seq: u64,
    pub last_offset: u64,
    pub complete: bool,
    pub queue: VecDeque<i16>,
    pub output: Output,
    pub out_seq: u64,
    pub connected: bool,
    pub disconnected_at: Option<Instant>,
    pub speech: VecDeque<(u64, u64)>,
    pub last_seen: Instant,
    pub rep: bool,
    pub representative_generation: Option<usize>,
    pub asset_id: Option<String>,
}
pub struct Recording {
    file: File,
    pub asset_id: String,
    bytes: u64,
    start: Instant,
    pub failed: bool,
}
#[derive(Default)]
pub struct Media {
    pub transcribers: BTreeMap<String, mpsc::Sender<Vec<u8>>>,
    pub streams: BTreeMap<String, Stream>,
    recordings: BTreeMap<String, Recording>,
    pub ticks: u64,
}
impl Media {
    pub fn attach(
        &mut self,
        e: &mut Engine,
        s: &Session,
        token: &str,
        p: &Value,
        out: mpsc::Sender<Value>,
    ) -> Result<Value> {
        let i = &s.identity;
        let ring = required(p, "ringid")?;
        let device = required(p, "device_id")?;
        if device != s.device_id {
            return Err(forbidden());
        }
        let purpose = p["purpose"].as_str().unwrap_or("call");
        if !matches!(purpose, "call" | "voicemail") {
            return Err(invalid("purpose must be call or voicemail"));
        }
        let c = e.state.call(i, ring)?;
        let (standby, vm) = if purpose == "call" {
            if silicon(&i.actor) || c.state != "active" || !c.active(&i.actor) {
                return Err(Fault::new(
                    "CALL_NOT_ACTIVE",
                    "Only a connected carbon can attach a call microphone.",
                    "Accept a call first.",
                ));
            }
            let active = c
                .participants
                .iter()
                .find(|p| p.actor == i.actor && p.left_at.is_none())
                .and_then(|p| p.device_id.as_deref());
            (active != Some(device), None)
        } else {
            let vm = required(p, "voicemail_id")?;
            let v = e
                .state
                .voicemails
                .get(vm)
                .filter(|v| {
                    v.sender == i.actor
                        && v.realm == i.realm
                        && v.ringid == ring
                        && v.state == "draft"
                        && v.format == "audio"
                        && v.expires_at > now()
                })
                .ok_or_else(forbidden)?;
            if v.audio_asset_id.is_some() {
                return Err(Fault::new(
                    "AUDIO_EXISTS",
                    "This draft already contains recording audio.",
                    "Abort the draft and begin again to re-record.",
                ));
            }
            (false, Some(vm.to_owned()))
        };
        let stream_id = id("stream");
        let mut asset_id = None;
        if let Some(vm) = &vm {
            let aid = id("asset");
            let path = e.data_dir.join("assets").join(&aid);
            let mut file = File::create(&path).map_err(storage)?;
            file.write_all(&wav_header(0)).map_err(storage)?;
            let draft = &e.state.voicemails[vm];
            let a = Asset {
                asset_id: aid.clone(),
                owner: key(&draft.realm, &draft.org_id, &draft.sender),
                purpose: "voicemail".into(),
                mime_type: "audio/wav".into(),
                size_bytes: 44,
                received_bytes: 44,
                next_seq: 0,
                complete: false,
                path: path.to_string_lossy().into(),
                ringid: None,
                voicemail_id: Some(vm.clone()),
            };
            e.state.assets.insert(aid.clone(), a);
            e.state.voicemails.get_mut(vm).unwrap().audio_asset_id = Some(aid.clone());
            asset_id = Some(aid);
        }
        // A reconnect replaces this device's stale live media, never replays its queued speech.
        self.streams.retain(|_, st| {
            !(st.owner == key(&i.realm, &i.org_id, &i.actor)
                && st.device_id == device
                && st.ringid == ring
                && st.voicemail_id == vm)
        });
        self.streams.insert(
            stream_id.clone(),
            Stream {
                id: stream_id.clone(),
                owner: key(&i.realm, &i.org_id, &i.actor),
                actor: i.actor.clone(),
                ringid: ring.into(),
                device_id: device.into(),
                token_hash: digest(token),
                voicemail_id: vm,
                muted: false,
                next_seq: 0,
                last_offset: 0,
                complete: true,
                queue: VecDeque::new(),
                output: Output::Socket(out),
                out_seq: 0,
                connected: true,
                disconnected_at: None,
                speech: VecDeque::new(),
                last_seen: Instant::now(),
                rep: false,
                representative_generation: None,
                asset_id,
            },
        );
        e.persist()?;
        Ok(
            json!({"stream_id":stream_id,"state":if standby{"standby"}else{"active"},"sample_rate":RATE,"channels":1,"encoding":"pcm_s16le","frame_ms":20,"time_base":"milliseconds"}),
        )
    }
    pub fn representative(
        &mut self,
        call: &Call,
        actor: &str,
        out: mpsc::Sender<Vec<u8>>,
    ) -> String {
        let sid = id("rep");
        self.streams.entry(sid.clone()).or_insert(Stream {
            id: sid.clone(),
            owner: key(&call.realm, &call.identity(actor).org_id, actor),
            actor: actor.into(),
            ringid: call.ringid.clone(),
            device_id: String::new(),
            token_hash: String::new(),
            voicemail_id: None,
            muted: false,
            next_seq: 0,
            last_offset: 0,
            complete: true,
            queue: VecDeque::new(),
            output: Output::Representative(out),
            out_seq: 0,
            connected: true,
            disconnected_at: None,
            speech: VecDeque::new(),
            last_seen: Instant::now(),
            rep: true,
            representative_generation: call
                .participants
                .iter()
                .rposition(|p| p.actor == actor && p.left_at.is_none()),
            asset_id: None,
        });
        sid
    }
    pub fn handoff_ready(&self, i: &Identity, ring: &str, device: &str) -> bool {
        self.streams.values().any(|s| {
            owns_actor(&s.owner, &i.realm, &i.actor)
                && s.ringid == ring
                && s.device_id == device
                && s.connected
                && s.voicemail_id.is_none()
        })
    }
    pub fn state(&mut self, s: &Session, p: &Value) -> Result<Value> {
        let st = self.owned(s, required(p, "stream_id")?)?;
        st.muted = p["muted"]
            .as_bool()
            .ok_or_else(|| invalid("muted must be boolean"))?;
        st.queue.clear();
        Ok(json!({"stream_id":st.id,"muted":st.muted}))
    }
    pub fn detach(&mut self, e: &mut Engine, s: &Session, p: &Value) -> Result<Value> {
        let sid = required(p, "stream_id")?;
        let st = self.owned(s, sid)?;
        let last = p["last_seq"].as_u64();
        let complete = st.complete && st.next_seq > 0 && last == Some(st.next_seq - 1);
        if st.voicemail_id.is_some() {
            if let Some(aid) = &st.asset_id {
                let a = e
                    .state
                    .assets
                    .get_mut(aid)
                    .ok_or_else(|| missing("Recording asset"))?;
                let bytes = a.received_bytes.saturating_sub(44);
                let mut f = OpenOptions::new()
                    .write(true)
                    .open(&a.path)
                    .map_err(storage)?;
                f.write_all(&wav_header(bytes as u32)).map_err(storage)?;
                f.sync_all().map_err(storage)?;
                a.complete = complete;
                a.size_bytes = a.received_bytes;
                e.state
                    .voicemails
                    .get_mut(st.voicemail_id.as_ref().unwrap())
                    .unwrap()
                    .complete_audio = complete;
            }
        }
        let result = json!({"stream_id":sid,"detached":true,"complete":if st.voicemail_id.is_some(){complete}else{true},"next_seq":st.next_seq});
        self.streams.remove(sid);
        e.persist()?;
        Ok(result)
    }
    fn owned(&mut self, s: &Session, sid: &str) -> Result<&mut Stream> {
        self.streams
            .get_mut(sid)
            .filter(|st| {
                st.owner == key(&s.identity.realm, &s.identity.org_id, &s.identity.actor)
                    && st.device_id == s.device_id
                    && !st.rep
            })
            .ok_or_else(forbidden)
    }
    pub fn audio(&mut self, e: &mut Engine, s: &Session, p: &Value) -> Result<()> {
        let st = self.owned(s, required(p, "stream_id")?)?;
        let seq = p["seq"]
            .as_u64()
            .ok_or_else(|| invalid("Audio seq must be an unsigned integer"))?;
        let offset = p["offset_ms"]
            .as_u64()
            .ok_or_else(|| invalid("Audio offset_ms must be an unsigned integer"))?;
        let bytes = STANDARD
            .decode(required(p, "audio_base64")?)
            .map_err(|_| invalid("Audio is not valid base64"))?;
        if bytes.len() != SAMPLES * 2 {
            return Err(invalid(
                "Each PCM frame must have 480 signed 16-bit samples (20ms, 24kHz mono)",
            ));
        }
        if seq < st.next_seq {
            return Err(Fault::new(
                "STALE_AUDIO",
                "An old audio frame was dropped.",
                "Continue with the current live sequence; never replay stale audio.",
            ));
        }
        if st.next_seq > 0 && offset <= st.last_offset {
            return Err(invalid("Audio offset_ms must increase"));
        }
        if seq != st.next_seq {
            st.complete = false;
            st.queue.clear();
            if st.voicemail_id.is_none() {
                e.state.recording_gaps.entry(st.ringid.clone()).or_default().push(json!({"actor":st.actor,"occurred_at":now(),"start_ms":st.last_offset+20,"end_ms":offset,"reason":"input_frame_gap","time_base":"stream"}));
            }
        }
        st.last_offset = offset;
        st.next_seq = seq + 1;
        st.last_seen = Instant::now();
        if let Some(vm) = &st.voicemail_id {
            let v = e.state.voicemails.get(vm).ok_or_else(forbidden)?;
            if v.expires_at <= now() || v.state != "draft" || offset > 180000 {
                return Err(invalid("Voicemail draft expired or exceeds 180 seconds"));
            }
            let a = e
                .state
                .assets
                .get_mut(st.asset_id.as_ref().unwrap())
                .unwrap();
            let mut f = OpenOptions::new()
                .append(true)
                .open(&a.path)
                .map_err(storage)?;
            f.write_all(&bytes).map_err(storage)?;
            a.received_bytes += bytes.len();
            a.next_seq = st.next_seq;
            return Ok(());
        }
        let call = e.state.call(&s.identity, &st.ringid)?;
        if !call.active(&st.actor) || call.state != "active" {
            return Err(forbidden());
        }
        let active = call.participants.iter().any(|p| {
            p.actor == st.actor
                && p.device_id.as_deref() == Some(st.device_id.as_str())
                && p.left_at.is_none()
        });
        if !active || st.muted {
            st.queue.clear();
            return Ok(());
        }
        if st.queue.len() > SAMPLES * 10 {
            st.queue.clear();
        }
        st.queue.extend(
            bytes
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]])),
        );
        Ok(())
    }
    pub fn speech(&mut self, e: &Engine, s: &Session, p: &Value) -> Result<()> {
        let st = self.owned(s, required(p, "stream_id")?)?;
        let a = p["start_ms"]
            .as_u64()
            .ok_or_else(|| invalid("start_ms must be unsigned"))?;
        let b = p["end_ms"]
            .as_u64()
            .filter(|b| *b >= a && *b - a <= 30000)
            .ok_or_else(|| invalid("Speech interval must be ordered and at most 30 seconds"))?;
        let call = e.state.call(&s.identity, &st.ringid)?;
        let start = call
            .answered_at
            .as_deref()
            .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
            .ok_or_else(|| invalid("Call has no audio timeline"))?;
        let clock = (chrono::Utc::now() - start.with_timezone(&chrono::Utc))
            .num_milliseconds()
            .max(0) as u64;
        let shift = clock.saturating_sub(st.last_offset);
        st.speech.push_back((a + shift, b + shift));
        while st.speech.len() > 200 {
            st.speech.pop_front();
        }
        Ok(())
    }
    pub fn speakers(&self, ring: &str, start: u64, end: u64) -> Vec<String> {
        let mut actors: Vec<_> = self
            .streams
            .values()
            .filter(|s| {
                s.ringid == ring
                    && !s.rep
                    && s.voicemail_id.is_none()
                    && s.speech.iter().any(|(a, b)| *a < end && *b > start)
            })
            .map(|s| s.actor.clone())
            .collect();
        actors.sort();
        actors.dedup();
        actors
    }
    pub fn push_representative(&mut self, sid: &str, bytes: &[u8]) {
        if let Some(s) = self.streams.get_mut(sid) {
            if s.queue.len() > RATE as usize * 30 {
                s.queue.clear();
            }
            s.queue.extend(
                bytes
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]])),
            );
        }
    }
    pub fn disconnect(&mut self, out: &mpsc::Sender<Value>) {
        for s in self
            .streams
            .values_mut()
            .filter(|s| matches!(&s.output,Output::Socket(tx) if tx.same_channel(out)))
        {
            s.connected = false;
            s.disconnected_at = Some(Instant::now());
            s.queue.clear();
        }
    }
    pub fn disconnect_token(&mut self, token: &str) {
        for s in self
            .streams
            .values_mut()
            .filter(|s| s.token_hash == digest(token))
        {
            s.connected = false;
            s.disconnected_at = Some(Instant::now());
            s.queue.clear();
        }
    }
    pub fn tick(&mut self, e: &mut Engine) -> bool {
        self.ticks += 1;
        let timestamp = now();
        for s in self.streams.values_mut().filter(|s| !s.rep && s.connected) {
            let valid = e
                .state
                .sessions
                .get(&s.token_hash)
                .is_some_and(|v| v.expires_at > timestamp)
                && e.state
                    .devices
                    .get(&s.device_id)
                    .is_some_and(|d| !d.revoked);
            if !valid {
                s.connected = false;
                s.disconnected_at = Some(Instant::now());
                s.queue.clear();
            }
            if s.voicemail_id.is_none()
                && e.state.calls.get(&s.ringid).is_some_and(|call| {
                    !call.participants.iter().any(|p| {
                        p.actor == s.actor
                            && p.left_at.is_none()
                            && p.device_id.as_deref() == Some(s.device_id.as_str())
                    })
                })
            {
                // A device that hands off must not replay its buffered microphone on return.
                s.queue.clear();
            }
        }
        let mut changed = false;
        if self.ticks % 50 == 0 {
            let mut expired = Vec::new();
            for call in e.state.calls.values().filter(|c| c.state == "active") {
                for p in call
                    .participants
                    .iter()
                    .filter(|p| p.left_at.is_none() && !silicon(&p.actor))
                {
                    let stream = self.streams.values().find(|s| {
                        s.ringid == call.ringid
                            && s.actor == p.actor
                            && Some(&s.device_id) == p.device_id.as_ref()
                            && s.voicemail_id.is_none()
                    });
                    let missing_since = p
                        .joined_at
                        .clone()
                        .max(call.answered_at.clone().unwrap_or_default());
                    let overdue = match stream {
                        Some(s) if s.connected => !s.muted && s.last_seen.elapsed().as_secs() >= 30,
                        Some(s) => s
                            .disconnected_at
                            .is_some_and(|t| t.elapsed().as_secs() >= 30),
                        None => {
                            chrono::DateTime::parse_from_rfc3339(&missing_since).is_ok_and(|t| {
                                (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds()
                                    >= 30
                            })
                        }
                    };
                    if overdue {
                        expired.push((
                            call.ringid.clone(),
                            Session {
                                identity: call.identity(&p.actor),
                                device_id: p.device_id.clone().unwrap_or_default(),
                                expires_at: after(1),
                            },
                        ));
                    }
                }
            }
            for (ringid, session) in expired {
                if e.state
                    .calls
                    .get(&ringid)
                    .is_some_and(|c| c.state == "active" && c.active(&session.identity.actor))
                {
                    if let Some(call) = e.state.calls.get_mut(&ringid) {
                        call.entry(
                            "media.reconnect_timeout",
                            Some(session.identity.actor.clone()),
                            json!({"grace_seconds":30}),
                            None,
                        );
                    }
                    if e.dispatch(&session, "calls.cut", &json!({"ringid":ringid}))
                        .is_ok()
                    {
                        changed = true;
                    }
                }
            }
        }
        let calls: Vec<_> = e
            .state
            .calls
            .values()
            .filter(|c| c.state == "active" || self.recordings.contains_key(&c.ringid))
            .cloned()
            .collect();
        for c in calls {
            if c.state != "active" {
                if let Some(mut rec) = self.recordings.remove(&c.ringid) {
                    let finalized = (|| -> std::io::Result<()> {
                        rec.file.seek(SeekFrom::Start(0))?;
                        rec.file
                            .write_all(&wav_header(rec.bytes.min(u32::MAX as u64) as u32))?;
                        rec.file.sync_all()
                    })()
                    .is_ok()
                        && !rec.failed;
                    if let Some(asset) = e.state.assets.get_mut(&rec.asset_id) {
                        asset.size_bytes = rec.bytes as usize + 44;
                        asset.received_bytes = asset.size_bytes;
                        asset.complete = finalized;
                    }
                    if let Some(call) = e.state.calls.get_mut(&c.ringid) {
                        call.recording_status = if finalized { "ready" } else { "failed" }.into();
                    }
                    e.state.call_event(
                        &c,
                        "recording.updated",
                        json!({"ringid":c.ringid,"status":if finalized{"ready"}else{"failed"}}),
                    );
                    changed = true;
                }
                self.streams
                    .retain(|_, s| s.ringid != c.ringid || s.voicemail_id.is_some());
                continue;
            }
            if !self.recordings.contains_key(&c.ringid) {
                let aid = id("recording");
                let path = e.data_dir.join("assets").join(&aid);
                match File::create(&path).and_then(|mut f| {
                    f.write_all(&wav_header(0))?;
                    Ok(f)
                }) {
                    Ok(file) => {
                        e.state.assets.insert(
                            aid.clone(),
                            Asset {
                                asset_id: aid.clone(),
                                owner: key(&c.realm, &c.org_id, "*"),
                                purpose: "recording".into(),
                                mime_type: "audio/wav".into(),
                                size_bytes: 44,
                                received_bytes: 44,
                                next_seq: 0,
                                complete: false,
                                path: path.to_string_lossy().into(),
                                ringid: Some(c.ringid.clone()),
                                voicemail_id: None,
                            },
                        );
                        e.state.calls.get_mut(&c.ringid).unwrap().recording_asset_id =
                            Some(aid.clone());
                        self.recordings.insert(
                            c.ringid.clone(),
                            Recording {
                                file,
                                asset_id: aid,
                                bytes: 0,
                                start: Instant::now(),
                                failed: false,
                            },
                        );
                        changed = true
                    }
                    Err(_) => {
                        e.state.calls.get_mut(&c.ringid).unwrap().recording_status =
                            "failed".into();
                    }
                }
            }
            let private_actors: Vec<_> = self
                .streams
                .values()
                .filter(|s| {
                    s.voicemail_id.is_some()
                        && s.connected
                        && owns_actor(&s.owner, &c.realm, &s.actor)
                })
                .map(|s| s.actor.clone())
                .collect();
            let ids: Vec<_> = self
                .streams
                .iter()
                .filter(|(_, s)| {
                    s.ringid == c.ringid
                        && s.voicemail_id.is_none()
                        && s.connected
                        && c.participants.iter().enumerate().any(|(generation, p)| {
                            p.actor == s.actor
                                && p.left_at.is_none()
                                && if s.rep {
                                    s.representative_generation == Some(generation)
                                        && s.owner
                                            == key(&c.realm, &c.identity(&p.actor).org_id, &p.actor)
                                } else {
                                    p.device_id.as_deref() == Some(s.device_id.as_str())
                                }
                        })
                })
                .map(|(id, _)| id.clone())
                .collect();
            let mut audio = BTreeMap::new();
            let mut sum = vec![0i32; SAMPLES];
            for id in &ids {
                let s = self.streams.get_mut(id).unwrap();
                let mut frame = vec![0i16; SAMPLES];
                if !s.muted && !private_actors.contains(&s.actor) {
                    for sample in &mut frame {
                        *sample = s.queue.pop_front().unwrap_or(0)
                    }
                } else {
                    s.queue.clear();
                }
                for (n, value) in frame.iter().enumerate() {
                    sum[n] += *value as i32;
                }
                audio.insert(id, frame);
            }
            let carbon_mix = pcm(&(0..SAMPLES)
                .map(|n| {
                    ids.iter()
                        .filter(|id| !self.streams[*id].rep)
                        .map(|id| audio[id][n] as i32)
                        .sum()
                })
                .collect::<Vec<_>>());
            if let Some(tx) = self.transcribers.get(&c.ringid) {
                let _ = tx.try_send(carbon_mix);
            }
            let mixed = pcm(&sum);
            if let Some(rec) = self.recordings.get_mut(&c.ringid) {
                let expected = (rec.start.elapsed().as_millis() as u64 / 20) * 960;
                if expected > rec.bytes + 960 * 10 {
                    let gap = expected - rec.bytes;
                    let zeros = vec![0u8; gap.min(24000 * 2 * 30) as usize];
                    if rec.file.write_all(&zeros).is_err() {
                        rec.failed = true;
                    }
                    e.state.recording_gaps.entry(c.ringid.clone()).or_default().push(json!({"start_ms":rec.bytes/48,"end_ms":expected/48,"reason":"server_audio_clock_gap","time_base":"recording"}));
                    rec.bytes += zeros.len() as u64;
                    changed = true;
                }
                if rec.file.write_all(&mixed).is_err() {
                    rec.failed = true;
                }
                rec.bytes += mixed.len() as u64;
                if rec.bytes > u32::MAX as u64 - 44 {
                    rec.failed = true;
                }
            }
            for id in &ids {
                let s = self.streams.get_mut(id).unwrap();
                let own = &audio[id];
                let without = pcm(&sum
                    .iter()
                    .zip(own)
                    .map(|(a, b)| a - *b as i32)
                    .collect::<Vec<_>>());
                match &s.output {
                    Output::Socket(tx) => {
                        let value = json!({"type":"media.audio","data":{"stream_id":s.id,"seq":s.out_seq,"offset_ms":s.out_seq*20,"audio_base64":STANDARD.encode(&without)}});
                        if tx.try_send(value).is_err() {
                            s.complete = false;
                        }
                    }
                    Output::Representative(tx) => {
                        let _ = tx.try_send(without);
                    }
                }
                s.out_seq += 1;
            }
        }
        changed
    }
}
fn storage(_: std::io::Error) -> Fault {
    Fault::new(
        "STORAGE_FAILED",
        "Could not write audio.",
        "Check server disk availability.",
    )
}
pub fn pcm(samples: &[i32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|v| ((*v).clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes())
        .collect()
}
pub fn wav_header(bytes: u32) -> [u8; 44] {
    let mut h = [0; 44];
    h[..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&bytes.saturating_add(36).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&bytes.to_le_bytes());
    h
}
/// Snapshot authorized byte ranges under the state lock; read and transfer outside it.
pub struct Download {
    path: String,
    ranges: Vec<(u64, u64)>,
    header: Option<Vec<u8>>,
    pub size_bytes: u64,
}
pub fn authorized_download(e: &Engine, i: &Identity, a: &Asset) -> Result<Download> {
    let mut plan = Download {
        path: a.path.clone(),
        ranges: vec![(0, a.size_bytes as u64)],
        header: None,
        size_bytes: a.size_bytes as u64,
    };
    if let Some(ring) = &a.ringid {
        let c = e.state.call(i, ring)?;
        let start = c
            .answered_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .ok_or_else(|| missing("Recording start"))?;
        plan.ranges.clear();
        let mut length = 0u64;
        for p in c.participants.iter().filter(|p| p.actor == i.actor) {
            let join = chrono::DateTime::parse_from_rfc3339(&p.joined_at)
                .map_err(|_| invalid("Invalid participation time"))?;
            let left = p
                .left_at
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .unwrap_or_else(|| chrono::Utc::now().fixed_offset());
            let from = (44 + (join - start).num_milliseconds().max(0) as u64 * 48)
                .min(a.size_bytes as u64);
            let to = (44 + (left - start).num_milliseconds().max(0) as u64 * 48)
                .min(a.size_bytes as u64);
            if to > from {
                plan.ranges.push((from, to));
                length += to - from;
            }
        }
        plan.header = Some(
            wav_header(
                length
                    .try_into()
                    .map_err(|_| invalid("Recording exceeds WAV container limit"))?,
            )
            .to_vec(),
        );
        plan.size_bytes = 44 + length;
    }
    Ok(plan)
}
impl Download {
    pub async fn stream(self, out: mpsc::Sender<Value>, transfer: Value, asset: Value) {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let result = async {
            let mut file = tokio::fs::File::open(&self.path).await.map_err(storage)?;
            let mut sent = 0u64;
            let mut seq = 0u64;
            if let Some(header) = &self.header {
                sent += header.len() as u64;
                out.send(json!({"type":"assets.chunk","data":{"transfer_id":transfer,"asset_id":asset,"seq":seq,"data_base64":STANDARD.encode(header),"final":sent == self.size_bytes}})).await.map_err(|_| invalid("Transfer disconnected"))?;
                seq += 1;
            }
            let mut buffer = vec![0u8; 48 * 1024];
            for (from, to) in self.ranges {
                file.seek(SeekFrom::Start(from)).await.map_err(storage)?;
                let mut remaining = to - from;
                while remaining > 0 {
                    let length = remaining.min(buffer.len() as u64) as usize;
                    file.read_exact(&mut buffer[..length]).await.map_err(storage)?;
                    remaining -= length as u64;
                    sent += length as u64;
                    out.send(json!({"type":"assets.chunk","data":{"transfer_id":transfer,"asset_id":asset,"seq":seq,"data_base64":STANDARD.encode(&buffer[..length]),"final":sent == self.size_bytes}})).await.map_err(|_| invalid("Transfer disconnected"))?;
                    seq += 1;
                }
            }
            Ok::<(), Fault>(())
        }.await;
        if let Err(error) = result {
            let _ = out.send(json!({"type":"stream.error","data":{"transfer_id":transfer,"asset_id":asset,"error":error}})).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejoining_representative_never_mixes_or_hears_the_previous_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path()).unwrap();
        let mut login = |actor: &str, org: &str| {
            let value = e
                .login(
                    Identity {
                        actor: actor.into(),
                        org_id: org.into(),
                        realm: "test".into(),
                        display_name: actor.into(),
                        admin: false,
                    },
                    None,
                )
                .unwrap();
            e.session(value["session_token"].as_str().unwrap()).unwrap()
        };
        let alice = login("c:alice", "alice-org");
        let bob = login("si:bob", "old-org");
        let carol = login("si:carol", "carol-org");
        let ring = e
            .dispatch(&alice, "calls.init", &json!({"target":"si:bob"}))
            .unwrap()["ringid"]
            .as_str()
            .unwrap()
            .to_owned();
        e.dispatch(&bob, "calls.accept", &json!({"ringid":ring}))
            .unwrap();
        e.dispatch(
            &alice,
            "calls.invite",
            &json!({"ringid":ring,"target":"si:carol"}),
        )
        .unwrap();
        e.dispatch(&carol, "calls.accept", &json!({"ringid":ring}))
            .unwrap();
        let mut media = Media::default();
        let (old_tx, mut old_rx) = mpsc::channel(8);
        let old = media.representative(&e.state.calls[&ring], "si:bob", old_tx);
        media.push_representative(&old, &pcm(&vec![1000; SAMPLES]));
        e.dispatch(&bob, "calls.cut", &json!({"ringid":ring}))
            .unwrap();
        let mut alternate = bob.identity.clone();
        alternate.org_id = "new-org".into();
        let value = e.login(alternate, None).unwrap();
        let rejoined = e.session(value["session_token"].as_str().unwrap()).unwrap();
        e.dispatch(
            &alice,
            "calls.invite",
            &json!({"ringid":ring,"target":"si:bob"}),
        )
        .unwrap();
        e.dispatch(&rejoined, "calls.accept", &json!({"ringid":ring}))
            .unwrap();
        let (new_tx, mut new_rx) = mpsc::channel(8);
        let current = media.representative(&e.state.calls[&ring], "si:bob", new_tx);
        assert_ne!(old, current);
        media.push_representative(&current, &pcm(&vec![-200; SAMPLES]));
        media.tick(&mut e);
        assert!(old_rx.try_recv().is_err());
        assert!(new_rx.try_recv().unwrap().iter().all(|b| *b == 0));
        let asset = &e.state.assets[&media.recordings[&ring].asset_id];
        let recording = std::fs::read(&asset.path).unwrap();
        assert!(recording[44..]
            .chunks_exact(2)
            .all(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) == -200));
    }

    #[test]
    fn reconnect_replaces_stale_audio_and_timeout_leaves_the_room() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path()).unwrap();
        let mut sign_in = |actor: &str| {
            let login = e
                .login(
                    Identity {
                        actor: actor.into(),
                        org_id: "org".into(),
                        realm: "test".into(),
                        display_name: actor.into(),
                        admin: false,
                    },
                    None,
                )
                .unwrap();
            let token = login["session_token"].as_str().unwrap().to_owned();
            (e.session(&token).unwrap(), token)
        };
        let (a, token) = sign_in("c:alice");
        let (b, _) = sign_in("c:bob");
        let (a2, token2) = sign_in("c:alice");
        let call = e
            .dispatch(&a, "calls.init", &json!({"target":"c:bob"}))
            .unwrap();
        let ring = call["ringid"].as_str().unwrap();
        e.dispatch(&b, "calls.accept", &json!({"ringid":ring}))
            .unwrap();
        let mut media = Media::default();
        let (tx, _rx) = mpsc::channel(8);
        let attached = media
            .attach(
                &mut e,
                &a,
                &token,
                &json!({"ringid":ring,"device_id":a.device_id}),
                tx.clone(),
            )
            .unwrap();
        let old = attached["stream_id"].as_str().unwrap();
        media
            .streams
            .get_mut(old)
            .unwrap()
            .queue
            .extend([123i16; SAMPLES]);
        let (standby, _standby_rx) = mpsc::channel(8);
        media
            .attach(
                &mut e,
                &a2,
                &token2,
                &json!({"ringid":ring,"device_id":a2.device_id}),
                standby,
            )
            .unwrap();
        e.dispatch(
            &a,
            "calls.handoff",
            &json!({"ringid":ring,"to_device_id":a2.device_id}),
        )
        .unwrap();
        media.tick(&mut e);
        assert!(
            media.streams[old].queue.is_empty(),
            "handoff retained stale microphone samples"
        );
        e.dispatch(
            &a2,
            "calls.handoff",
            &json!({"ringid":ring,"to_device_id":a.device_id}),
        )
        .unwrap();
        media
            .streams
            .get_mut(old)
            .unwrap()
            .queue
            .extend([123i16; SAMPLES]);
        media.disconnect(&tx);
        assert!(media.streams[old].queue.is_empty());
        let (replacement, _rx) = mpsc::channel(8);
        let attached = media
            .attach(
                &mut e,
                &a,
                &token,
                &json!({"ringid":ring,"device_id":a.device_id}),
                replacement.clone(),
            )
            .unwrap();
        assert!(!media.streams.contains_key(old));
        media.ticks = 49;
        media.tick(&mut e);
        assert_eq!(e.state.calls[ring].state, "active");
        let sid = attached["stream_id"].as_str().unwrap();
        media.disconnect(&replacement);
        media.streams.get_mut(sid).unwrap().disconnected_at =
            Some(Instant::now() - std::time::Duration::from_secs(31));
        media.ticks = 99;
        media.tick(&mut e);
        assert_eq!(e.state.calls[ring].state, "ended");
        assert!(e.state.calls[ring]
            .transcript
            .iter()
            .any(|t| t.kind == "media.reconnect_timeout"));
    }
}

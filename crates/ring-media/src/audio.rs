//! PCM helpers: per-track jitter buffers mixed on a timeline that follows the audio, WAV files
//! and tones. All audio is mono signed 16-bit.

use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use livekit::{
    data_stream::api::ByteStreamReader,
    prelude::RemoteAudioTrack,
    webrtc::{
        audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions},
        stats::RtcStats,
    },
};
use tokio::task::JoinHandle;

use crate::preroll::{self, Converted, Preroll, Prerolls};

/// GPT-Live's audio format and the representative's published track.
pub const LIVE_RATE: u32 = 24_000;
/// Recordings, voicemail and Deepgram input: the rate Opus decodes at, so nothing the caller's
/// microphone sent is lost to resampling.
pub const RECORD_RATE: u32 = 48_000;
/// Mixer cadence.
pub const TICK: Duration = Duration::from_millis(20);

/// How each track is buffered before it is mixed.
///
/// libwebrtc decodes every track in 10 ms frames on one playout clock, so all tracks deliver at
/// the same steady rate; only when the frames reach the mixer varies. The mix therefore follows
/// the audio, not the wall clock: `prime` stays buffered per track to absorb ordinary delivery
/// jitter, and when frames run later than that (a busy host) the mix waits for them, up to
/// `patience` behind the wall clock, rather than filling the gap with silence. A stall then
/// delays the audio instead of cutting holes into words.
#[derive(Clone, Copy)]
pub struct Buffering {
    pub prime: Duration,
    /// How far the mix may fall behind the wall clock waiting for a late track before it plays
    /// silence in its place.
    pub patience: Duration,
    /// Extra latency a track may keep after such a gap; beyond it, quiet audio is skipped until
    /// the track is back within bounds.
    pub slack: Duration,
}

impl Buffering {
    /// For live conversation: little added latency.
    pub const INTERACTIVE: Self = Self {
        prime: Duration::from_millis(40),
        patience: Duration::from_millis(300),
        slack: Duration::from_millis(60),
    };
    /// For recordings and speech-to-text, where latency does not matter but gaps do.
    pub const RECORDING: Self = Self {
        prime: Duration::from_millis(200),
        patience: Duration::from_secs(3),
        slack: Duration::from_secs(1),
    };

    /// The most a track holds: a bound for a track nobody reads (one heard while a voicemail
    /// greeting plays), which the mix itself only reaches after stalls far beyond `patience`.
    fn cap(self) -> Duration {
        self.prime + self.patience + self.slack + Duration::from_millis(500)
    }
}

/// A 10 ms block below this peak (about -40 dBFS) is quiet enough to skip.
const QUIET: u16 = 330;

fn samples_for(rate: u32, d: Duration) -> usize {
    (rate as u128 * d.as_millis() / 1000) as usize
}

struct FeedBuffer {
    samples: VecDeque<i16>,
    primed: bool,
    prime: usize,
    slack: usize,
    cap: usize,
    block: usize,
    /// Samples mixed as silence because the track was later than `patience`.
    late: u64,
    /// Quiet samples skipped to shed latency.
    trimmed: u64,
    /// Samples dropped at `cap`.
    dropped: u64,
    /// libwebrtc's latest receive statistics for the track, when they are being reported.
    reception: Option<Reception>,
    /// Held back from the mix while the track's pre-roll is awaited, up to `hold_cap` samples.
    held: bool,
    hold_cap: usize,
    /// When the track's first audio arrived.
    first_at: Option<Instant>,
    /// Where on the timeline a held track would have started playing.
    due_at: Option<u64>,
}

impl FeedBuffer {
    fn new(rate: u32, buffering: Buffering) -> Self {
        Self {
            samples: VecDeque::new(),
            primed: false,
            prime: samples_for(rate, buffering.prime),
            slack: samples_for(rate, buffering.slack),
            cap: samples_for(rate, buffering.cap()),
            block: (rate / 100) as usize,
            late: 0,
            trimmed: 0,
            dropped: 0,
            reception: None,
            held: false,
            hold_cap: 0,
            first_at: None,
            due_at: None,
        }
    }

    fn push(&mut self, data: &[i16]) {
        self.first_at.get_or_insert_with(Instant::now);
        self.samples.extend(data);
        let cap = if self.held { self.hold_cap } else { self.cap };
        let excess = self.samples.len().saturating_sub(cap);
        self.samples.drain(..excess);
        self.dropped += excess as u64;
    }

    /// A track plays (and is waited for) once `prime` is buffered. A held track notes where it
    /// would have started, at timeline position `mixed`, and is not played yet.
    fn ready(&mut self, mixed: u64) -> Option<usize> {
        if self.held {
            if self.due_at.is_none() && self.samples.len() >= self.prime {
                self.due_at = Some(mixed);
            }
            return None;
        }
        if !self.primed && self.samples.len() >= self.prime {
            self.primed = true;
        }
        self.primed.then_some(self.samples.len())
    }

    /// Ends a hold. Returns the timeline position where `prefix` (the pre-roll that came before
    /// the live audio) starts, and `prefix` followed by the held audio that is already due at
    /// `mixed`. The rest stays queued and plays on from there.
    fn release(&mut self, prefix: Vec<i16>, mixed: u64) -> (i64, Vec<i16>) {
        self.held = false;
        let due = self.due_at.take().unwrap_or(mixed).min(mixed);
        let start = due as i64 - prefix.len() as i64;
        let overdue = ((mixed - due) as usize).min(self.samples.len());
        let mut samples = prefix;
        samples.extend(self.samples.drain(..overdue));
        self.primed = true;
        let excess = self.samples.len().saturating_sub(self.cap);
        self.samples.drain(..excess);
        self.dropped += excess as u64;
        (start, samples)
    }

    /// Adds the next `out.len()` samples into `out`. Returns whether any audio was taken.
    fn take_into(&mut self, out: &mut [i32]) -> bool {
        if !self.primed {
            return false;
        }
        let n = out.len().min(self.samples.len());
        for (acc, s) in out.iter_mut().zip(self.samples.drain(..n)) {
            *acc += s as i32;
        }
        if n < out.len() {
            // The mix waited `patience` and this track is still short: the rest is silence, and
            // the track is not waited for again until it has primed.
            self.primed = false;
            self.late += (out.len() - n) as u64;
        } else {
            self.trim();
        }
        n > 0
    }

    /// Sheds latency left over from a late stretch by skipping quiet 10 ms blocks.
    fn trim(&mut self) {
        while self.samples.len() > self.prime + self.slack
            && self
                .samples
                .range(..self.block)
                .all(|s| s.unsigned_abs() < QUIET)
        {
            self.samples.drain(..self.block);
            self.trimmed += self.block as u64;
        }
    }

    /// Discards the buffered audio on purpose, so whatever was dropped meanwhile is not a loss.
    fn clear(&mut self) {
        self.samples.clear();
        self.primed = false;
        self.dropped = 0;
    }

    fn take_all(&mut self) -> Vec<i16> {
        self.primed = false;
        self.samples.drain(..).collect()
    }
}

/// How a track's packets arrived and were decoded, from libwebrtc's receive statistics. Loss,
/// concealment and time stretching all happen before the mixer, and are what makes a voice
/// sound choppy when the network (or the sender) is at fault.
struct Reception {
    codec: String,
    /// Seconds of audio decoded; with `packets`, shows whether the sender uses DTX (Opus sends
    /// 50 packets a second without it).
    seconds: f64,
    /// Average payload bitrate.
    kbps: f64,
    packets: u64,
    lost: i64,
    jitter_ms: f64,
    jitter_buffer_ms: f64,
    concealed_pct: f64,
    concealment_events: u64,
    accelerated_ms: u64,
    decelerated_ms: u64,
    fec_packets: u64,
}

impl Reception {
    fn from_stats(stats: &[RtcStats]) -> Option<Self> {
        let rtp = stats.iter().find_map(|s| match s {
            RtcStats::InboundRtp(rtp) => Some(rtp),
            _ => None,
        })?;
        let codec = stats.iter().find_map(|s| match s {
            RtcStats::Codec(c) if c.rtc.id == rtp.stream.codec_id => Some(&c.codec),
            _ => None,
        });
        let clock_rate = codec.map_or(48_000, |c| c.clock_rate.max(1)) as u64;
        let decoded = &rtp.inbound;
        let seconds = decoded.total_samples_duration;
        Some(Self {
            codec: codec.map_or_else(|| "unknown".into(), |c| c.mime_type.clone()),
            seconds: seconds.round(),
            kbps: (decoded.bytes_received as f64 * 8.0 / 1000.0 / seconds.max(1.0)).round(),
            packets: rtp.received.packets_received,
            lost: rtp.received.packets_lost,
            jitter_ms: (rtp.received.jitter * 1000.0).round(),
            jitter_buffer_ms: (decoded.jitter_buffer_delay * 1000.0
                / decoded.jitter_buffer_emitted_count.max(1) as f64)
                .round(),
            concealed_pct: (decoded.concealed_samples as f64 * 1000.0
                / decoded.total_samples_received.max(1) as f64)
                .round()
                / 10.0,
            concealment_events: decoded.concealment_events,
            accelerated_ms: decoded.removed_samples_for_acceleration * 1000 / clock_rate,
            decelerated_ms: decoded.inserted_samples_for_deceleration * 1000 / clock_rate,
            fec_packets: decoded.fec_packets_received,
        })
    }
}

/// How often a reported track's receive statistics are refreshed.
const RECEPTION_INTERVAL: Duration = Duration::from_secs(5);

struct Feed {
    identity: String,
    buffer: Arc<Mutex<FeedBuffer>>,
    reader: JoinHandle<()>,
    /// Set while the track is held for its pre-roll.
    pending: Option<Pending>,
}

/// A track waiting for its pre-roll.
struct Pending {
    since: Instant,
    preroll: Option<Converted>,
    /// The pre-roll and live lengths at the last alignment attempt.
    tried: (usize, usize),
}

/// Where a session finds the pre-rolls of its call.
struct PrerollSource {
    prerolls: Arc<Prerolls>,
    ringid: String,
    /// The longest a track is held back waiting for its pre-roll.
    hold: Duration,
    /// The pre-rolls this session has put in front of a track already.
    used: Vec<Arc<Preroll>>,
}

impl PrerollSource {
    fn find(&mut self, identity: &str, track_sid: &str) -> Option<Arc<Preroll>> {
        let preroll = self
            .prerolls
            .find(&self.ringid, identity, track_sid, &self.used)?;
        self.used.push(preroll.clone());
        Some(preroll)
    }
}

/// Audio that belongs before what is already mixed: a track's pre-roll and the live audio held
/// back while it was awaited, which together start at `start` on the session's timeline
/// (negative when they begin before it).
pub struct Late {
    pub identity: String,
    pub start: i64,
    pub samples: Vec<i16>,
}

/// Every subscribed remote audio track of one room, decoded and resampled by libwebrtc to `rate`.
pub struct Feeds {
    rate: u32,
    buffering: Buffering,
    feeds: HashMap<String, Feed>,
    /// The call whose tracks' reception is logged when they go away.
    report: Option<String>,
    prerolls: Option<PrerollSource>,
}

impl Feeds {
    pub fn new(rate: u32, buffering: Buffering) -> Self {
        Self {
            rate,
            buffering,
            feeds: HashMap::new(),
            report: None,
            prerolls: None,
        }
    }

    /// Logs how each track's audio arrived when it goes away.
    pub fn reporting(mut self, ringid: &str) -> Self {
        self.report = Some(ringid.to_owned());
        self
    }

    /// Puts each new track's pre-roll in front of it, holding the track back for at most `hold`
    /// while its pre-roll is on its way (see [`Feeds::take_prerolls`]).
    pub fn with_prerolls(mut self, prerolls: Arc<Prerolls>, ringid: &str, hold: Duration) -> Self {
        self.prerolls = Some(PrerollSource {
            prerolls,
            ringid: ringid.to_owned(),
            hold,
            used: Vec::new(),
        });
        self
    }

    /// Files a pre-roll stream sent to this participant, for every session of the call.
    pub fn receive_preroll(&self, identity: &str, reader: ByteStreamReader) {
        if let Some(source) = &self.prerolls {
            source.prerolls.receive(&source.ringid, identity, reader);
        }
    }

    /// Adds a subscribed track. It is held back until its pre-roll is lined up with it when the
    /// pre-roll is already arriving, or when `announced`: its publisher said one comes with it
    /// (LiveKit's pre-connect buffer track feature).
    pub fn add(
        &mut self,
        track_sid: String,
        identity: String,
        track: RemoteAudioTrack,
        announced: bool,
    ) {
        self.remove(&track_sid);
        let mut buffer = FeedBuffer::new(self.rate, self.buffering);
        let rate = self.rate;
        let pending = self.prerolls.as_mut().and_then(|source| {
            let preroll = source.find(&identity, &track_sid);
            if preroll.is_none() && !announced {
                return None;
            }
            buffer.held = true;
            buffer.hold_cap = buffer.cap + samples_for(rate, source.hold);
            Some(Pending {
                since: Instant::now(),
                preroll: preroll.map(|p| Converted::new(p, rate)),
                tried: (0, 0),
            })
        });
        let buffer = Arc::new(Mutex::new(buffer));
        let sink = buffer.clone();
        // libwebrtc's own queue (10 ms frames, oldest dropped when full) holds as much as the
        // buffer does, so a burst after a stall reaches it whole.
        let options = NativeAudioStreamOptions {
            queue_size_frames: Some(self.buffering.cap().as_millis() as usize / 10),
        };
        let mut stream =
            NativeAudioStream::with_options(track.rtc_track(), self.rate as i32, 1, options);
        let reporting = self.report.is_some();
        let reader = tokio::spawn(async move {
            let mut stats = tokio::time::interval(RECEPTION_INTERVAL);
            stats.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    frame = stream.next() => match frame {
                        Some(frame) => sink.lock().unwrap().push(&frame.data),
                        None => break,
                    },
                    _ = stats.tick(), if reporting => {
                        let latest = track.get_stats().await.unwrap_or_default();
                        if let Some(reception) = Reception::from_stats(&latest) {
                            sink.lock().unwrap().reception = Some(reception);
                        }
                    }
                }
            }
        });
        tracing::debug!(%identity, track = %track_sid, held = pending.is_some(), "subscribed audio");
        self.feeds.insert(
            track_sid,
            Feed {
                identity,
                buffer,
                reader,
                pending,
            },
        );
    }

    /// Ends the holds whose pre-roll is lined up with the live audio, has finished without
    /// lining up, or ran out of time, and returns what each such track contributes before the
    /// audio already mixed: its pre-roll up to where the live audio starts, then the live audio
    /// held back meanwhile that is due on `clock`. Call once per tick, before [`Feeds::pull`].
    pub fn take_prerolls(&mut self, clock: &Clock) -> Vec<Late> {
        let Some(source) = self.prerolls.as_mut() else {
            return Vec::new();
        };
        let mut late = Vec::new();
        for (track_sid, feed) in &mut self.feeds {
            let Some(pending) = feed.pending.as_mut() else {
                continue;
            };
            if pending.preroll.is_none() {
                pending.preroll = source
                    .find(&feed.identity, track_sid)
                    .map(|p| Converted::new(p, self.rate));
            }
            let mut buffer = feed.buffer.lock().unwrap();
            let expired = pending.since.elapsed() >= source.hold;
            let mut aligned = None;
            let prefix = match pending.preroll.as_mut() {
                None => expired.then(Vec::new),
                Some(preroll) => {
                    let progress = preroll.update();
                    let lengths = (preroll.samples.len(), buffer.samples.len());
                    // Alignment is retried as either side grows by a tenth of a second.
                    let grown = lengths.0 >= pending.tried.0 + self.rate as usize / 10
                        || lengths.1 >= pending.tried.1 + self.rate as usize / 10;
                    if grown || progress.finished || expired {
                        pending.tried = lengths;
                        let live: Vec<i16> = buffer.samples.iter().copied().collect();
                        aligned = preroll::align(&preroll.samples, &live, self.rate);
                    }
                    match aligned {
                        Some((at, _)) => Some(preroll.samples[..at].to_vec()),
                        None if progress.finished || expired => {
                            // Unmatched (silence, or no overlap): the pre-roll ran until about
                            // as long after the live audio's first frame as its last chunk
                            // arrived after that frame.
                            let overlap = match (progress.last_at, buffer.first_at) {
                                (Some(last), Some(first)) => {
                                    samples_for(self.rate, last.saturating_duration_since(first))
                                }
                                _ => 0,
                            };
                            let at = preroll.samples.len().saturating_sub(overlap);
                            Some(preroll.samples[..at].to_vec())
                        }
                        None => None,
                    }
                }
            };
            let Some(prefix) = prefix else { continue };
            let prefix_ms = prefix.len() as u64 * 1000 / self.rate as u64;
            let (start, samples) = buffer.release(prefix, clock.mixed);
            // `correlation` is absent when the pre-roll was placed by timing, not by matching.
            tracing::info!(
                ringid = %source.ringid,
                identity = %feed.identity,
                prefix_ms,
                held_ms = pending.since.elapsed().as_millis() as u64,
                correlation = aligned.map(|(_, c)| (c * 100.0).round() / 100.0),
                "pre-roll placed"
            );
            feed.pending = None;
            if !samples.is_empty() {
                late.push(Late {
                    identity: feed.identity.clone(),
                    start,
                    samples,
                });
            }
        }
        late
    }

    pub fn remove(&mut self, track_sid: &str) {
        if let Some(feed) = self.feeds.remove(track_sid) {
            self.log_reception(&feed);
            feed.reader.abort();
        }
    }

    pub fn remove_identity(&mut self, identity: &str) {
        let sids: Vec<_> = self
            .feeds
            .iter()
            .filter(|(_, f)| f.identity == identity)
            .map(|(sid, _)| sid.clone())
            .collect();
        for sid in sids {
            self.remove(&sid);
        }
    }

    pub fn has_identity(&self, identity: &str) -> bool {
        self.feeds.values().any(|f| f.identity == identity)
    }

    pub fn identities(&self) -> impl Iterator<Item = &str> {
        self.feeds.values().map(|f| f.identity.as_str())
    }

    /// Advances the mix: takes the audio due on `clock` (at most `limit` samples) from every
    /// track, summed per participant identity, and returns how many samples that is. It is less
    /// than the wall clock asks for, or none, while a playing track's audio is late and the mix
    /// waits for it. Every track is drained on every call, used or not, so none of them ever
    /// goes stale.
    pub fn pull(&self, clock: &mut Clock, limit: usize) -> (usize, HashMap<String, Vec<i32>>) {
        let mut out: HashMap<String, Vec<i32>> = HashMap::new();
        let behind = clock.behind().min(limit);
        if behind == 0 {
            return (0, out);
        }
        let mut buffers: Vec<_> = self
            .feeds
            .values()
            .map(|f| (f, f.buffer.lock().unwrap()))
            .collect();
        // Follow the slowest playing track, but no further behind the wall clock than `patience`.
        let ready = buffers
            .iter_mut()
            .filter_map(|(_, b)| b.ready(clock.mixed))
            .min();
        let patience = samples_for(self.rate, self.buffering.patience);
        let n = match ready {
            Some(ready) => behind.min(ready).max(behind.saturating_sub(patience)),
            None => behind,
        };
        if n == 0 {
            return (0, out);
        }
        for (feed, buffer) in &mut buffers {
            let mut acc = vec![0i32; n];
            let late = buffer.late;
            let took = buffer.take_into(&mut acc);
            if buffer.late != late {
                let missing_ms = (buffer.late - late) * 1000 / self.rate as u64;
                tracing::debug!(identity = %feed.identity, missing_ms, "audio arrived too late; mixed silence");
            }
            if took {
                match out.get_mut(&feed.identity) {
                    Some(existing) => mix_into(existing, &acc),
                    None => {
                        out.insert(feed.identity.clone(), acc);
                    }
                }
            }
        }
        clock.mixed += n as u64;
        (n, out)
    }

    /// Everything still buffered, ignoring priming, summed per identity and padded to one
    /// length (returned first). Ends a recording without losing its last moments.
    pub fn drain(&self) -> (usize, HashMap<String, Vec<i32>>) {
        let taken: Vec<(&str, Vec<i16>)> = self
            .feeds
            .values()
            .map(|f| (f.identity.as_str(), f.buffer.lock().unwrap().take_all()))
            .collect();
        let n = taken.iter().map(|(_, s)| s.len()).max().unwrap_or(0);
        let mut out: HashMap<String, Vec<i32>> = HashMap::new();
        for (identity, samples) in taken.into_iter().filter(|(_, s)| !s.is_empty()) {
            let acc = out.entry(identity.to_owned()).or_insert_with(|| vec![0; n]);
            acc.iter_mut()
                .zip(samples)
                .for_each(|(a, s)| *a += s as i32);
        }
        (n, out)
    }

    /// Discards everything buffered so far (for example, audio heard while a greeting played).
    pub fn clear(&self) {
        for feed in self.feeds.values() {
            feed.buffer.lock().unwrap().clear();
        }
    }

    /// `late_ms`, `trimmed_ms` and `dropped_ms` are the mixer's own gaps and skips; the rest is
    /// libwebrtc's view of the network and the decoder, as of the last refresh.
    fn log_reception(&self, feed: &Feed) {
        let Some(ringid) = &self.report else { return };
        let ms = |samples: u64| samples * 1000 / self.rate as u64;
        let buffer = feed.buffer.lock().unwrap();
        let (late_ms, trimmed_ms, dropped_ms) =
            (ms(buffer.late), ms(buffer.trimmed), ms(buffer.dropped));
        match &buffer.reception {
            Some(r) => tracing::info!(
                ringid,
                identity = %feed.identity,
                codec = %r.codec,
                seconds = r.seconds,
                kbps = r.kbps,
                packets = r.packets,
                lost = r.lost,
                jitter_ms = r.jitter_ms,
                jitter_buffer_ms = r.jitter_buffer_ms,
                concealed_pct = r.concealed_pct,
                concealment_events = r.concealment_events,
                accelerated_ms = r.accelerated_ms,
                decelerated_ms = r.decelerated_ms,
                fec_packets = r.fec_packets,
                late_ms,
                trimmed_ms,
                dropped_ms,
                "audio reception"
            ),
            None => tracing::info!(
                ringid,
                identity = %feed.identity,
                late_ms,
                trimmed_ms,
                dropped_ms,
                "audio reception (no receive statistics yet)"
            ),
        }
    }
}

impl Drop for Feeds {
    fn drop(&mut self) {
        for feed in self.feeds.values() {
            self.log_reception(feed);
            feed.reader.abort();
        }
    }
}

/// Converts a mixed accumulator to samples, clipping instead of wrapping.
pub fn clip(acc: &[i32]) -> Vec<i16> {
    acc.iter()
        .map(|&s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
        .collect()
}

pub fn mix_into(acc: &mut [i32], samples: &[i32]) {
    acc.iter_mut().zip(samples).for_each(|(a, s)| *a += s);
}

pub fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

pub fn pcm_samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect()
}

/// A mix's timeline against the wall clock: how much audio is due, and how much was mixed.
/// Mixing may fall behind while it waits for late audio, then catches up, so the timeline
/// never drifts from real time for longer than `Buffering::patience`.
pub struct Clock {
    start: Instant,
    rate: u64,
    mixed: u64,
}

impl Clock {
    pub fn new(rate: u32) -> Self {
        Self {
            start: Instant::now(),
            rate: rate as u64,
            mixed: 0,
        }
    }

    /// Samples the wall clock has made due that are not mixed yet.
    fn behind(&self) -> usize {
        let due = (self.start.elapsed().as_nanos() * self.rate as u128 / 1_000_000_000) as u64;
        due.saturating_sub(self.mixed) as usize
    }

    /// Milliseconds of timeline mixed so far.
    pub fn position_ms(&self) -> i64 {
        (self.mixed * 1000 / self.rate) as i64
    }
}

pub fn tick_interval() -> tokio::time::Interval {
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval
}

/// Streams 16-bit mono PCM to a WAV file and patches the header sizes on `finish`.
pub struct WavWriter {
    file: BufWriter<File>,
    path: PathBuf,
    rate: u32,
    samples: u64,
}

impl WavWriter {
    pub fn create(path: &Path, rate: u32) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Readable too: audio that arrives late is mixed into what was already written.
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("cannot create {}", path.display()))?;
        let mut file = BufWriter::new(file);
        file.write_all(&wav_header(rate, 0))?;
        Ok(Self {
            file,
            path: path.to_owned(),
            rate,
            samples: 0,
        })
    }

    pub fn write(&mut self, samples: &[i16]) -> Result<()> {
        self.file.write_all(&pcm_bytes(samples))?;
        self.samples += samples.len() as u64;
        Ok(())
    }

    /// Mixes `samples` into the audio already written from sample `position` on.
    pub fn mix_at(&mut self, position: u64, samples: &[i16]) -> Result<()> {
        if position + samples.len() as u64 > self.samples {
            bail!("late audio runs past the end of the recording");
        }
        self.file.flush()?;
        let file = self.file.get_mut();
        let offset = 44 + position * 2;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0u8; samples.len() * 2];
        file.read_exact(&mut bytes)?;
        let mut mixed: Vec<i32> = pcm_samples(&bytes).into_iter().map(i32::from).collect();
        mixed
            .iter_mut()
            .zip(samples)
            .for_each(|(m, &s)| *m += s as i32);
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&pcm_bytes(&clip(&mixed)))?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Appends the audio of a file this writer produced earlier, including one whose header was
    /// never completed because the process stopped while recording.
    pub fn append_file(&mut self, path: &Path) -> Result<()> {
        let mut file = File::open(path)?;
        let mut header = [0u8; 44];
        file.read_exact(&mut header)?;
        if &header[0..4] != b"RIFF" || header[24..28] != self.rate.to_le_bytes() {
            bail!("{} is not a recording in this format", path.display());
        }
        let len = file.metadata()?.len().saturating_sub(44) & !1;
        let copied = std::io::copy(&mut file.take(len), &mut self.file)?;
        self.samples += copied / 2;
        Ok(())
    }

    pub fn duration_ms(&self) -> u64 {
        self.samples * 1000 / self.rate as u64
    }

    /// Completes the file and returns its path and duration in milliseconds.
    pub fn finish(mut self) -> Result<(PathBuf, u64)> {
        let data_len =
            u32::try_from(self.samples * 2).context("recording exceeds the WAV size limit")?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&wav_header(self.rate, data_len))?;
        let mut file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.flush()?;
        file.sync_all()?;
        Ok((self.path, self.samples * 1000 / self.rate as u64))
    }
}

pub fn wav_header(rate: u32, data_len: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

pub fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
    let pcm = pcm_bytes(samples);
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(&wav_header(rate, pcm.len() as u32));
    out.extend_from_slice(&pcm);
    out
}

/// Reads a 16-bit mono PCM WAV (as written by this module) into its rate and samples.
pub fn parse_wav(bytes: &[u8]) -> Result<(u32, Vec<i16>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let mut pos = 12;
    let mut format = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?) as usize;
        let body = bytes
            .get(pos + 8..pos + 8 + len)
            .unwrap_or(&bytes[pos + 8..]);
        match id {
            b"fmt " if body.len() >= 16 => {
                let tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into()?);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if tag != 1 || channels != 1 || bits != 16 {
                    bail!("unsupported WAV format");
                }
                format = Some(rate);
            }
            b"data" => {
                let rate = format.context("WAV data before fmt")?;
                return Ok((rate, pcm_samples(body)));
            }
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    bail!("WAV has no data chunk")
}

/// The voicemail beep: 1 kHz for 0.4 s with short fades so it does not click.
pub fn beep(rate: u32) -> Vec<i16> {
    let n = samples_for(rate, Duration::from_millis(400));
    let fade = samples_for(rate, Duration::from_millis(8));
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let envelope = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
            ((2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 0.5 * envelope * i16::MAX as f64)
                as i16
        })
        .collect()
}

pub fn silence(rate: u32, d: Duration) -> Vec<i16> {
    vec![0; samples_for(rate, d)]
}

/// Keeps `lead`/`tail` of quiet around the audible part of a clip.
pub fn trim_silence(
    samples: &[i16],
    rate: u32,
    threshold: i16,
    lead: Duration,
    tail: Duration,
) -> Vec<i16> {
    let loud = |s: &i16| s.unsigned_abs() > threshold as u16;
    let (Some(first), Some(last)) = (
        samples.iter().position(loud),
        samples.iter().rposition(loud),
    ) else {
        return Vec::new();
    };
    let start = first.saturating_sub(samples_for(rate, lead));
    let end = (last + samples_for(rate, tail)).min(samples.len());
    samples[start..end].to_vec()
}

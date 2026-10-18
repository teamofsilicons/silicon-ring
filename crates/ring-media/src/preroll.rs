//! Pre-roll: what a carbon's microphone heard before their track reached ring-media.
//!
//! A track only reaches ring-media after the client has connected, published it and LiveKit has
//! offered it to each subscriber: one to three seconds after a phone answers. Whatever the carbon
//! said meanwhile, typically their first words, never arrives as RTP. Clients therefore record
//! from the moment they answer and stream that audio to the recorder until their track is
//! subscribed, in the format of LiveKit's pre-connect audio buffer: a byte stream on [`TOPIC`]
//! with `sampleRate` and `channels` attributes, carrying 16-bit little-endian PCM.
//!
//! Ring's clients open the stream as soon as they are connected, before they publish, so the
//! pre-roll is known before its track arrives; it then belongs to the sender's next microphone
//! track. LiveKit's own SDKs open it after publishing and name the track (`trackId`) instead,
//! and flag the track as having a pre-roll.
//!
//! The recorder files each stream for every session of its ring ([`Prerolls`]). A session holds
//! a new track back while its pre-roll is expected, lines the pre-roll up with the start of the
//! live audio ([`align`]), and puts the part that came before it in front (see
//! [`crate::audio::Feeds::take_prerolls`]).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use livekit::{
    data_stream::api::{ByteStreamReader, StreamReader},
    webrtc::native::audio_resampler::AudioResampler,
};

/// LiveKit's topic for pre-connect audio.
pub const TOPIC: &str = "lk.agent.pre-connect-audio-buffer";
/// The most a pre-roll may hold; a client stops at its first subscriber, a few seconds in.
const MAX_DURATION: Duration = Duration::from_secs(15);
/// How long after it opened a pre-roll that names no track may still meet its track.
const UNNAMED_WINDOW: Duration = Duration::from_secs(20);
/// How long a pre-roll stays on file for the ring's sessions.
const KEEP: Duration = Duration::from_secs(300);

/// Every pre-roll received recently, for all calls.
#[derive(Default)]
pub struct Prerolls {
    entries: Mutex<Vec<Arc<Preroll>>>,
}

impl Prerolls {
    /// Files a pre-roll stream from `identity` and reads it in the background as it arrives.
    pub fn receive(&self, ringid: &str, identity: &str, reader: ByteStreamReader) {
        let info = reader.info();
        let attributes = info.attributes();
        let Some(preroll) = Preroll::new(ringid, identity, &info.mime_type, &attributes) else {
            tracing::debug!(ringid, identity, mime = %info.mime_type, ?attributes, "pre-roll ignored: not 16-bit PCM from a carbon");
            return;
        };
        let preroll = Arc::new(preroll);
        {
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|p| p.opened.elapsed() < KEEP);
            // A participant may send the same stream to several of ring-media's participants.
            if preroll.track_sid.is_some()
                && entries.iter().any(|p| {
                    p.ringid == preroll.ringid
                        && p.identity == preroll.identity
                        && p.track_sid == preroll.track_sid
                })
            {
                return;
            }
            entries.push(preroll.clone());
        }
        tracing::debug!(ringid, identity, track = ?preroll.track_sid, rate = preroll.rate, channels = preroll.channels, "pre-roll arriving");
        tokio::spawn(async move {
            let mut reader = reader;
            let read = async {
                while let Some(chunk) = reader.next().await {
                    match chunk {
                        Ok(bytes) if preroll.push(&bytes) => {}
                        Ok(_) => break,
                        Err(e) => {
                            tracing::debug!("pre-roll stream ended early: {e}");
                            break;
                        }
                    }
                }
            };
            let _ = tokio::time::timeout(MAX_DURATION + Duration::from_secs(5), read).await;
            preroll.received.lock().unwrap().finished = true;
        });
    }

    /// The pre-roll of a track `identity` just published in `ringid`: the one that names it, or
    /// else the latest one `identity` opened without naming a track that `used` does not hold.
    pub fn find(
        &self,
        ringid: &str,
        identity: &str,
        track_sid: &str,
        used: &[Arc<Preroll>],
    ) -> Option<Arc<Preroll>> {
        let entries = self.entries.lock().unwrap();
        let ours = entries
            .iter()
            .filter(|p| p.ringid == ringid && p.identity == identity);
        let named = ours
            .clone()
            .find(|p| p.track_sid.as_deref() == Some(track_sid));
        named
            .or_else(|| {
                ours.rev().find(|p| {
                    p.track_sid.is_none()
                        && p.opened.elapsed() < UNNAMED_WINDOW
                        && !used.iter().any(|u| Arc::ptr_eq(u, p))
                })
            })
            .cloned()
    }
}

/// One track's pre-roll as it streams in.
pub struct Preroll {
    ringid: String,
    /// The participant that sent it, who must also own the track.
    identity: String,
    /// The track it belongs to, when the sender named it.
    track_sid: Option<String>,
    rate: u32,
    channels: u32,
    opened: Instant,
    received: Mutex<Received>,
}

#[derive(Default)]
struct Received {
    /// Interleaved samples as sent.
    samples: Vec<i16>,
    /// The first byte of a sample split across chunks.
    odd: Option<u8>,
    /// When the latest chunk arrived.
    last_at: Option<Instant>,
    finished: bool,
}

impl Preroll {
    fn new(
        ringid: &str,
        identity: &str,
        mime_type: &str,
        attributes: &HashMap<String, String>,
    ) -> Option<Self> {
        let encoded = ["webm", "mp4", "ogg", "opus", "aac", "mpeg"]
            .iter()
            .any(|m| mime_type.contains(m));
        let track_sid = attributes.get("trackId").filter(|t| !t.is_empty());
        let rate: u32 = attributes.get("sampleRate")?.parse().ok()?;
        let channels: u32 = attributes
            .get("channels")
            .map_or(Some(1), |c| c.parse().ok())?;
        let valid = identity.starts_with("carbon:")
            && !encoded
            && track_sid.is_none_or(|t| t.starts_with("TR_"))
            && (8_000..=48_000).contains(&rate)
            && rate.is_multiple_of(100)
            && (1..=2).contains(&channels);
        valid.then(|| Self {
            ringid: ringid.to_owned(),
            identity: identity.to_owned(),
            track_sid: track_sid.cloned(),
            rate,
            channels,
            opened: Instant::now(),
            received: Mutex::default(),
        })
    }

    /// Appends a chunk. Returns false once the pre-roll is full.
    fn push(&self, bytes: &[u8]) -> bool {
        let max = (MAX_DURATION.as_secs() as u32 * self.rate * self.channels) as usize;
        let mut received = self.received.lock().unwrap();
        let mut bytes = bytes;
        if let (Some(low), Some((&high, rest))) = (received.odd, bytes.split_first()) {
            received.samples.push(i16::from_le_bytes([low, high]));
            received.odd = None;
            bytes = rest;
        }
        let (pairs, rest) = bytes.as_chunks::<2>();
        received
            .samples
            .extend(pairs.iter().map(|b| i16::from_le_bytes(*b)));
        received.odd = rest.first().copied();
        received.last_at = Some(Instant::now());
        if received.samples.len() >= max {
            received.samples.truncate(max);
            return false;
        }
        true
    }
}

/// A pre-roll converted to one session's mono rate as it arrives.
pub struct Converted {
    preroll: Arc<Preroll>,
    rate: u32,
    resampler: AudioResampler,
    /// Interleaved source samples converted so far.
    used: usize,
    pub samples: Vec<i16>,
}

/// How far a pre-roll has arrived.
pub struct Progress {
    pub finished: bool,
    pub last_at: Option<Instant>,
}

impl Converted {
    pub fn new(preroll: Arc<Preroll>, rate: u32) -> Self {
        Self {
            preroll,
            rate,
            resampler: AudioResampler::default(),
            used: 0,
            samples: Vec::new(),
        }
    }

    /// Converts whatever arrived since the last call, in the 10 ms frames libwebrtc resamples.
    pub fn update(&mut self) -> Progress {
        let received = self.preroll.received.lock().unwrap();
        let (rate, channels) = (self.preroll.rate, self.preroll.channels);
        let frame = (rate / 100 * channels) as usize;
        let mut chunk = vec![0i16; frame];
        while self.used < received.samples.len() {
            let available = received.samples.len() - self.used;
            if available < frame && !received.finished {
                break;
            }
            let n = available.min(frame);
            chunk[..n].copy_from_slice(&received.samples[self.used..self.used + n]);
            chunk[n..].fill(0);
            let out =
                self.resampler
                    .remix_and_resample(&chunk, rate / 100, channels, rate, 1, self.rate);
            self.samples.extend_from_slice(out);
            self.used += n;
        }
        Progress {
            finished: received.finished,
            last_at: received.last_at,
        }
    }
}

/// Alignment runs at this rate: speech's low band is all it needs, and it is cheap there.
const ALIGN_RATE: u32 = 4_000;
/// How much live audio one match takes. The jitter buffer stretches a new track's first second
/// by a few percent, which blurs longer windows.
const WINDOW: Duration = Duration::from_millis(300);
/// How far back from the pre-roll's end the match is searched for.
const SEARCH: Duration = Duration::from_secs(3);
/// The least correlation that counts as a match: Opus at 32 kbps keeps about 0.85 of a voice's
/// waveform, and the stretching takes some more.
const MIN_CORRELATION: f64 = 0.5;
/// How much better than any other lag a match must be, not counting lags within
/// [`NEIGHBOURHOOD`] of it (a voiced sound also matches itself a pitch period along). A match
/// less than [`SURE_MARGIN`] better is confirmed by a second window [`CONFIRM_AFTER`] later,
/// which must put the live audio within [`CONFIRM_TOLERANCE`] of where the first did.
const MIN_MARGIN: f64 = 0.1;
const SURE_MARGIN: f64 = 0.25;
const NEIGHBOURHOOD: Duration = Duration::from_millis(15);
const CONFIRM_AFTER: Duration = Duration::from_millis(100);
const CONFIRM_TOLERANCE: Duration = Duration::from_millis(30);
/// A window quieter than this (RMS, about -50 dBFS) has nothing to match on.
const MIN_LEVEL: f64 = 100.0;

fn samples(rate: u32, d: Duration) -> usize {
    (rate as u128 * d.as_millis() / 1000) as usize
}

/// Finds where `live` begins inside `pre`, both mono at `rate`: the pre-roll index of `live[0]`.
/// Matches the first audible window of `live` (it may start with silence), so it answers once
/// the pre-roll reaches past that window, and neither for silence alone nor for a match that is
/// not clearly the only one. Returns the index and the correlation found.
pub fn align(pre: &[i16], live: &[i16], rate: u32) -> Option<(usize, f64)> {
    let n = samples(rate, WINDOW);
    if pre.len() < n {
        return None;
    }
    let first = (0..=live.len().checked_sub(n)?)
        .step_by(samples(rate, Duration::from_millis(100)))
        .find(|&i| rms(&live[i..i + n]) >= MIN_LEVEL)?;
    let d = (rate / ALIGN_RATE).max(1) as usize;
    let from = pre.len().saturating_sub(samples(rate, SEARCH) + n);
    let low = decimate(&pre[from..], d);
    let neighbourhood = samples(ALIGN_RATE, NEIGHBOURHOOD);
    let (lag, correlation, runner_up) =
        best_match(&low, &decimate(&live[first..first + n], d), neighbourhood)?;
    if correlation < MIN_CORRELATION || correlation - runner_up < MIN_MARGIN {
        return None;
    }
    if correlation - runner_up < SURE_MARGIN {
        let step = samples(rate, CONFIRM_AFTER);
        let pattern = decimate(live.get(first + step..first + step + n)?, d);
        let expected = lag + step / d;
        let tolerance = samples(ALIGN_RATE, CONFIRM_TOLERANCE);
        let confirmed = (expected.saturating_sub(tolerance)..=expected + tolerance)
            .filter_map(|at| low.get(at..at + pattern.len()))
            .any(|segment| correlate(&pattern, segment) >= MIN_CORRELATION);
        if !confirmed {
            return None;
        }
    }
    // Refine at the full rate around the coarse match.
    let window: Vec<f64> = live[first..first + n].iter().map(|&s| s as f64).collect();
    let centre = from + lag * d;
    let hi = (centre + d).min(pre.len().checked_sub(n)?);
    let mut best = (centre, f64::MIN);
    for at in centre.saturating_sub(d)..=hi {
        let c = correlate_samples(&window, &pre[at..at + n]);
        if c > best.1 {
            best = (at, c);
        }
    }
    best.0.checked_sub(first).map(|at| (at, correlation))
}

fn rms(samples: &[i16]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

/// Averages each run of `d` samples.
fn decimate(samples: &[i16], d: usize) -> Vec<f64> {
    samples
        .chunks_exact(d)
        .map(|c| c.iter().map(|&s| s as f64).sum::<f64>() / d as f64)
        .collect()
}

/// The lag of `pattern` in `signal` with the highest normalised correlation, that correlation,
/// and the highest one more than `neighbourhood` lags away from it.
fn best_match(signal: &[f64], pattern: &[f64], neighbourhood: usize) -> Option<(usize, f64, f64)> {
    let n = pattern.len();
    if n == 0 || signal.len() < n {
        return None;
    }
    let pattern_energy: f64 = pattern.iter().map(|p| p * p).sum();
    let mut energy: f64 = signal[..n].iter().map(|s| s * s).sum();
    let mut correlations = Vec::with_capacity(signal.len() - n + 1);
    for lag in 0..=signal.len() - n {
        if lag > 0 {
            energy += signal[lag + n - 1].powi(2) - signal[lag - 1].powi(2);
        }
        let dot: f64 = pattern.iter().zip(&signal[lag..]).map(|(p, s)| p * s).sum();
        correlations.push(dot / (pattern_energy * energy.max(1e-9)).sqrt());
    }
    let (lag, &best) = correlations
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    let runner_up = correlations
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(lag) > neighbourhood)
        .map(|(_, &c)| c)
        .fold(f64::MIN, f64::max);
    Some((lag, best, runner_up))
}

fn correlate(pattern: &[f64], signal: &[f64]) -> f64 {
    let (mut dot, mut pe, mut se) = (0.0, 0.0, 0.0);
    for (p, s) in pattern.iter().zip(signal) {
        dot += p * s;
        pe += p * p;
        se += s * s;
    }
    dot / (pe * se).max(1e-9).sqrt()
}

fn correlate_samples(pattern: &[f64], signal: &[i16]) -> f64 {
    let signal: Vec<f64> = signal.iter().map(|&s| s as f64).collect();
    correlate(pattern, &signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise shaped like speech: bursts of varying loudness.
    fn speechlike(n: usize, seed: u64) -> Vec<i16> {
        let mut x = seed;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let envelope = ((i as f64 / 2400.0).sin().abs() * 0.9 + 0.1) * 8000.0;
                ((x % 2001) as f64 / 1000.0 - 1.0) * envelope
            })
            .map(|v| v as i16)
            .collect()
    }

    #[test]
    fn finds_where_the_live_audio_starts() {
        let rate = 48_000;
        let speech = speechlike(rate as usize * 4, 7);
        // The pre-roll runs to 3.5 s; the live track starts at 2.2 s.
        let pre = &speech[..rate as usize * 7 / 2];
        let start = rate as usize * 22 / 10;
        let live = &speech[start..];
        let (at, correlation) = align(pre, live, rate).expect("aligned");
        assert_eq!(at, start);
        assert!(correlation > 0.95);
    }

    #[test]
    fn skips_leading_silence_in_the_live_audio() {
        let rate = 24_000;
        let mut speech = speechlike(rate as usize * 3, 11);
        let start = rate as usize;
        speech[start..start + rate as usize / 2].fill(0);
        let pre = &speech[..rate as usize * 5 / 2];
        let (at, _) = align(pre, &speech[start..], rate).expect("aligned");
        assert_eq!(at, start);
    }

    #[test]
    fn does_not_match_before_the_preroll_reaches_the_live_audio() {
        let rate = 48_000;
        let speech = speechlike(rate as usize * 4, 3);
        let pre = &speech[..rate as usize * 2];
        assert!(align(pre, &speech[rate as usize * 3..], rate).is_none());
        assert!(align(pre, &vec![0; rate as usize], rate).is_none());
    }
}

//! PCM helpers: per-track jitter buffers, a wall-clock mixer timeline, WAV files and tones.
//! All audio is mono signed 16-bit.

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
    prelude::RemoteAudioTrack,
    webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions},
};
use tokio::task::JoinHandle;

/// GPT-Live's audio format and the representative's published track.
pub const LIVE_RATE: u32 = 24_000;
/// Recordings, voicemail and Deepgram input.
pub const RECORD_RATE: u32 = 16_000;
/// Mixer cadence.
pub const TICK: Duration = Duration::from_millis(20);

/// How much to buffer per track: `prime` before reading it (so delivery jitter never clicks),
/// at most `cap` (older audio is dropped so a stall never builds up lasting latency).
#[derive(Clone, Copy)]
pub struct Buffering {
    pub prime: Duration,
    pub cap: Duration,
}

impl Buffering {
    /// For live conversation: little added latency.
    pub const INTERACTIVE: Self = Self {
        prime: Duration::from_millis(40),
        cap: Duration::from_millis(400),
    };
    /// For recordings and speech-to-text, where latency does not matter but gaps do.
    pub const RECORDING: Self = Self {
        prime: Duration::from_millis(200),
        cap: Duration::from_millis(1500),
    };
}

/// libwebrtc's per-stream queue in 10 ms frames; generous, so bursts after a stall are kept.
const STREAM_QUEUE_FRAMES: usize = 50;

fn samples_for(rate: u32, d: Duration) -> usize {
    (rate as u128 * d.as_millis() / 1000) as usize
}

struct FeedBuffer {
    samples: VecDeque<i16>,
    primed: bool,
    prime: usize,
    cap: usize,
    underruns: u64,
}

impl FeedBuffer {
    fn new(rate: u32, buffering: Buffering) -> Self {
        Self {
            samples: VecDeque::new(),
            primed: false,
            prime: samples_for(rate, buffering.prime),
            cap: samples_for(rate, buffering.cap),
            underruns: 0,
        }
    }

    fn push(&mut self, data: &[i16]) {
        self.samples.extend(data);
        let excess = self.samples.len().saturating_sub(self.cap);
        self.samples.drain(..excess);
    }

    /// Adds up to `out.len()` samples into `out`. Returns whether any audio was taken.
    fn take_into(&mut self, out: &mut [i32]) -> bool {
        if !self.primed {
            if self.samples.len() < self.prime {
                return false;
            }
            self.primed = true;
        }
        let n = out.len().min(self.samples.len());
        if n < out.len() {
            // Underrun: play what is left and wait for the buffer to refill.
            self.primed = false;
            self.underruns += 1;
        }
        for (acc, s) in out.iter_mut().zip(self.samples.drain(..n)) {
            *acc += s as i32;
        }
        n > 0
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.primed = false;
    }

    fn take_all(&mut self) -> Vec<i16> {
        self.primed = false;
        self.samples.drain(..).collect()
    }
}

struct Feed {
    identity: String,
    buffer: Arc<Mutex<FeedBuffer>>,
    reader: JoinHandle<()>,
}

/// Every subscribed remote audio track of one room, decoded and resampled by libwebrtc to `rate`.
pub struct Feeds {
    rate: u32,
    buffering: Buffering,
    feeds: HashMap<String, Feed>,
}

impl Feeds {
    pub fn new(rate: u32, buffering: Buffering) -> Self {
        Self {
            rate,
            buffering,
            feeds: HashMap::new(),
        }
    }

    pub fn add(&mut self, track_sid: String, identity: String, track: RemoteAudioTrack) {
        self.remove(&track_sid);
        let buffer = Arc::new(Mutex::new(FeedBuffer::new(self.rate, self.buffering)));
        let sink = buffer.clone();
        let options = NativeAudioStreamOptions {
            queue_size_frames: Some(STREAM_QUEUE_FRAMES),
        };
        let mut stream =
            NativeAudioStream::with_options(track.rtc_track(), self.rate as i32, 1, options);
        let reader = tokio::spawn(async move {
            while let Some(frame) = stream.next().await {
                sink.lock().unwrap().push(&frame.data);
            }
        });
        tracing::debug!(%identity, track = %track_sid, "subscribed audio");
        self.feeds.insert(
            track_sid,
            Feed {
                identity,
                buffer,
                reader,
            },
        );
    }

    pub fn remove(&mut self, track_sid: &str) {
        if let Some(feed) = self.feeds.remove(track_sid) {
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

    /// Takes the next `n` samples from every track, summed per participant identity. Every
    /// track is drained on every call, used or not, so none of them ever goes stale.
    pub fn pull(&self, n: usize) -> HashMap<String, Vec<i32>> {
        let mut out: HashMap<String, Vec<i32>> = HashMap::new();
        for feed in self.feeds.values() {
            let mut acc = vec![0i32; n];
            let took = {
                let mut buffer = feed.buffer.lock().unwrap();
                let underruns = buffer.underruns;
                let took = buffer.take_into(&mut acc);
                if buffer.underruns != underruns {
                    tracing::trace!(identity = %feed.identity, "audio underrun");
                }
                took
            };
            if took {
                match out.get_mut(&feed.identity) {
                    Some(existing) => existing.iter_mut().zip(&acc).for_each(|(a, b)| *a += b),
                    None => {
                        out.insert(feed.identity.clone(), acc);
                    }
                }
            }
        }
        out
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
}

impl Drop for Feeds {
    fn drop(&mut self) {
        for feed in self.feeds.values() {
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

/// A sample timeline driven by the wall clock: each call returns how many samples are due
/// since the last one. Late ticks yield more samples, so a timeline built from it never drifts
/// from real time and gaps become silence.
pub struct Clock {
    start: Instant,
    rate: u64,
    emitted: u64,
}

impl Clock {
    pub fn new(rate: u32) -> Self {
        Self {
            start: Instant::now(),
            rate: rate as u64,
            emitted: 0,
        }
    }

    pub fn due(&mut self) -> usize {
        let target = (self.start.elapsed().as_nanos() * self.rate as u128 / 1_000_000_000) as u64;
        let n = target.saturating_sub(self.emitted);
        self.emitted = target;
        n as usize
    }

    /// Milliseconds of timeline emitted so far.
    pub fn position_ms(&self) -> i64 {
        (self.emitted * 1000 / self.rate) as i64
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
        let mut file = BufWriter::new(
            File::create(path).with_context(|| format!("cannot create {}", path.display()))?,
        );
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

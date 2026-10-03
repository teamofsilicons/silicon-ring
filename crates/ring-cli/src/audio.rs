//! Native PCM audio. The OS owns permission prompts; denied devices are reported explicitly.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ring_client::{Result, RingError};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    thread,
};
use tokio::sync::mpsc::UnboundedSender;
const RATE: u32 = 24_000;

pub struct Audio {
    stop: Option<mpsc::Sender<()>>,
    pub output: Arc<Mutex<VecDeque<i16>>>,
}
impl Drop for Audio {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}
fn error(e: impl std::fmt::Display) -> RingError {
    RingError::new("AUDIO_UNAVAILABLE",e.to_string(),"native audio","Grant microphone permission and select working devices with ring audio devices / ring config set --scope local.")
}
pub fn devices() -> Result<Value> {
    let host = cpal::default_host();
    let input = host.default_input_device().and_then(|d| d.name().ok());
    let output = host.default_output_device().and_then(|d| d.name().ok());
    let ins=host.input_devices().map_err(error)?.enumerate().map(|(i,d)|json!({"id":format!("input:{i}"),"name":d.name().ok(),"default":d.name().ok()==input})).collect::<Vec<_>>();
    let outs=host.output_devices().map_err(error)?.enumerate().map(|(i,d)|json!({"id":format!("output:{i}"),"name":d.name().ok(),"default":d.name().ok()==output})).collect::<Vec<_>>();
    Ok(
        json!({"inputs":ins,"outputs":outs,"permission":"requested when a stream opens; enumeration does not prove microphone permission"}),
    )
}
fn device(host: &cpal::Host, input: bool, selected: Option<&str>) -> Result<cpal::Device> {
    if let Some(selected) = selected {
        let devices = if input {
            host.input_devices()
        } else {
            host.output_devices()
        }
        .map_err(error)?;
        for (i, d) in devices.enumerate() {
            if selected == format!("{}:{i}", if input { "input" } else { "output" })
                || d.name().ok().as_deref() == Some(selected)
            {
                return Ok(d);
            }
        }
        return Err(error("Selected audio device is not connected"));
    }
    (if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    })
    .ok_or_else(|| error("No default audio device"))
}
impl Audio {
    pub fn start(config: &Value, sender: UnboundedSender<Vec<i16>>) -> Result<Self> {
        let input = config["audio.input"].as_str().map(str::to_owned);
        let output = config["audio.output"].as_str().map(str::to_owned);
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let buffer = Arc::new(Mutex::new(VecDeque::new()));
        let out = buffer.clone();
        thread::spawn(move || {
            let setup = (|| {
                let host = cpal::default_host();
                let input = device(&host, true, input.as_deref())?;
                let output = device(&host, false, output.as_deref())?;
                let ic = input.default_input_config().map_err(error)?;
                let oc = output.default_output_config().map_err(error)?;
                let incfg: cpal::StreamConfig = ic.clone().into();
                let outcfg: cpal::StreamConfig = oc.clone().into();
                let input = match ic.sample_format() {
                    cpal::SampleFormat::F32 => input_stream::<f32>(&input, &incfg, sender),
                    cpal::SampleFormat::I16 => input_stream::<i16>(&input, &incfg, sender),
                    cpal::SampleFormat::U16 => input_stream::<u16>(&input, &incfg, sender),
                    format => Err(error(format!(
                        "Unsupported microphone sample format {format}"
                    ))),
                }?;
                let output = match oc.sample_format() {
                    cpal::SampleFormat::F32 => output_stream::<f32>(&output, &outcfg, out),
                    cpal::SampleFormat::I16 => output_stream::<i16>(&output, &outcfg, out),
                    cpal::SampleFormat::U16 => output_stream::<u16>(&output, &outcfg, out),
                    format => Err(error(format!("Unsupported output sample format {format}"))),
                }?;
                input.play().map_err(error)?;
                output.play().map_err(error)?;
                Ok((input, output))
            })();
            match setup {
                Ok(streams) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv();
                    drop(streams);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(15))
            .map_err(error)??;
        Ok(Self {
            stop: Some(stop_tx),
            output: buffer,
        })
    }
}
fn input_stream<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    tx: UnboundedSender<Vec<i16>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::Sample,
    f32: cpal::FromSample<T>,
{
    let channels = cfg.channels as usize;
    let rate = cfg.sample_rate.0;
    let mut phase = 0u64;
    let mut chunk = Vec::with_capacity(480);
    device
        .build_input_stream(
            cfg,
            move |data: &[T], _| {
                for frame in data.chunks(channels) {
                    let sample =
                        frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32;
                    phase += RATE as u64;
                    while phase >= rate as u64 {
                        phase -= rate as u64;
                        chunk.push((sample.clamp(-1., 1.) * 32767.) as i16);
                        if chunk.len() == 480 {
                            let _ = tx.send(std::mem::replace(&mut chunk, Vec::with_capacity(480)));
                        }
                    }
                }
            },
            |e| eprintln!("Audio input interrupted: {e}"),
            None,
        )
        .map_err(error)
}
fn output_stream<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    buffer: Arc<Mutex<VecDeque<i16>>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = cfg.channels as usize;
    let rate = cfg.sample_rate.0;
    let mut phase = 0u64;
    let mut sample = 0f32;
    device
        .build_output_stream(
            cfg,
            move |data: &mut [T], _| {
                let mut queue = buffer.lock().unwrap_or_else(|e| e.into_inner());
                for frame in data.chunks_mut(channels) {
                    phase += RATE as u64;
                    if phase >= rate as u64 {
                        phase -= rate as u64;
                        sample = queue.pop_front().unwrap_or(0) as f32 / 32768.;
                    }
                    for out in frame {
                        *out = T::from_sample(sample);
                    }
                }
                // ponytail: bound playout latency to one second; stale live audio must never replay after reconnect.
                if queue.len() > RATE as usize {
                    let excess = queue.len() - RATE as usize;
                    queue.drain(..excess);
                }
            },
            |e| eprintln!("Audio output interrupted: {e}"),
            None,
        )
        .map_err(error)
}

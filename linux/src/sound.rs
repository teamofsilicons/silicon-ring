//! Local call sounds (`assets/sounds`), played through GTK's media backend (GStreamer).
//!
//! The ringtone and ringback loop until stopped; the connect and end ticks play once.

use std::cell::RefCell;
use std::path::PathBuf;

use gtk::prelude::*;

use crate::config;

#[derive(Clone, Copy, Debug)]
pub enum Cue {
    Connected,
    Ended,
}

#[derive(Default)]
pub struct Sounds {
    dir: Option<PathBuf>,
    ringtone: RefCell<Option<gtk::MediaFile>>,
    ringback: RefCell<Option<gtk::MediaFile>>,
    cue: RefCell<Option<gtk::MediaFile>>,
}

impl Sounds {
    pub fn new() -> Self {
        let dir = config::data_dir("sounds");
        if dir.is_none() {
            tracing::warn!("call sounds not found; ringing will be silent");
        }
        Sounds {
            dir,
            ..Default::default()
        }
    }

    fn open(&self, name: &str) -> Option<gtk::MediaFile> {
        let path = self.dir.as_ref()?.join(name);
        path.is_file().then(|| gtk::MediaFile::for_filename(path))
    }

    fn start_loop(&self, slot: &RefCell<Option<gtk::MediaFile>>, name: &str) {
        if slot
            .borrow()
            .as_ref()
            .is_some_and(|media| media.is_playing())
        {
            return;
        }
        if let Some(media) = self.open(name) {
            media.set_loop(true);
            media.play();
            slot.replace(Some(media));
        }
    }

    fn stop_loop(slot: &RefCell<Option<gtk::MediaFile>>) {
        if let Some(media) = slot.take() {
            media.pause();
            media.clear();
        }
    }

    pub fn start_ringtone(&self) {
        self.start_loop(&self.ringtone, "ringtone.wav");
    }

    pub fn stop_ringtone(&self) {
        Self::stop_loop(&self.ringtone);
    }

    /// The caller-side "tring tring, pause" cycle.
    pub fn start_ringback(&self) {
        self.start_loop(&self.ringback, "ringback.wav");
    }

    pub fn stop_ringback(&self) {
        Self::stop_loop(&self.ringback);
    }

    pub fn play(&self, cue: Cue) {
        let name = match cue {
            Cue::Connected => "connected.wav",
            Cue::Ended => "ended.wav",
        };
        if let Some(media) = self.open(name) {
            media.play();
            // Keep the stream alive until it finishes; replacing it drops the previous one.
            self.cue.replace(Some(media));
        }
    }
}

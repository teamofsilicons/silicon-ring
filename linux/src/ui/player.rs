//! Playback of recordings and voicemail. Presigned URLs are downloaded to a private cache file
//! first, then played with `gtk::MediaFile` (seekable, unlike a stream).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::app::App;
use crate::{APP_ID, config, runtime};

fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join(APP_ID).join("audio")
}

/// Removes audio left behind by a previous run.
pub fn clear_cache() {
    let _ = std::fs::remove_dir_all(cache_dir());
}

fn extension(mime: Option<&str>) -> &'static str {
    match mime.unwrap_or_default() {
        "audio/mpeg" => "mp3",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mp4" | "audio/aac" => "m4a",
        "audio/webm" => "webm",
        _ => "wav",
    }
}

struct State {
    file: Option<PathBuf>,
    media: Option<gtk::MediaFile>,
    generation: u64,
}

/// A media control bar with a status line, bound to one downloaded file at a time.
#[derive(Clone)]
pub struct Player {
    root: gtk::Box,
    controls: gtk::MediaControls,
    status: gtk::Label,
    state: Rc<RefCell<State>>,
}

impl Player {
    pub fn new() -> Player {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let controls = gtk::MediaControls::new(None::<&gtk::MediaStream>);
        controls.set_hexpand(true);
        controls.set_visible(false);
        let status = gtk::Label::builder().xalign(0.0).build();
        status.add_css_class("pixel");
        status.add_css_class("muted");
        root.append(&controls);
        root.append(&status);
        Player {
            root,
            controls,
            status,
            state: Rc::new(RefCell::new(State {
                file: None,
                media: None,
                generation: 0,
            })),
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_status(&self, text: &str) {
        self.status.set_label(&text.to_uppercase());
        self.status.set_visible(!text.is_empty());
    }

    pub fn is_playing(&self) -> bool {
        self.state
            .borrow()
            .media
            .as_ref()
            .is_some_and(|m| m.is_playing())
    }

    /// Downloads `url` (named after `key`) and starts playing it.
    pub fn load(&self, key: &str, url: String, mime: Option<&str>, autoplay: bool) {
        self.stop();
        let generation = {
            let mut state = self.state.borrow_mut();
            state.generation += 1;
            state.generation
        };
        self.set_status("Loading…");
        let path = cache_dir().join(format!(
            "{}.{}",
            key.replace(['/', '.'], "_"),
            extension(mime)
        ));
        let client = App::get().client();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let target = path.clone();
            let result = runtime::run(async move {
                let bytes = client.download(&url).await.map_err(|e| e.to_string())?;
                config::write_private(&target, &bytes).map_err(|e| e.to_string())
            })
            .await;
            if this.state.borrow().generation != generation {
                let _ = std::fs::remove_file(&path);
                return;
            }
            match result {
                Ok(()) => {
                    let media = gtk::MediaFile::for_filename(&path);
                    this.controls.set_media_stream(Some(&media));
                    this.controls.set_visible(true);
                    this.set_status("");
                    if autoplay {
                        media.play();
                    }
                    let mut state = this.state.borrow_mut();
                    state.file = Some(path);
                    state.media = Some(media);
                }
                Err(err) => {
                    tracing::warn!("audio download failed: {err}");
                    this.set_status("Couldn't load the audio");
                }
            }
        });
    }

    pub fn toggle(&self) {
        if let Some(media) = self.state.borrow().media.as_ref() {
            if media.is_playing() {
                media.pause()
            } else {
                media.play()
            }
        }
    }

    pub fn media(&self) -> Option<gtk::MediaFile> {
        self.state.borrow().media.clone()
    }

    /// Stops playback and deletes the downloaded file.
    pub fn stop(&self) {
        let mut state = self.state.borrow_mut();
        state.generation += 1;
        if let Some(media) = state.media.take() {
            media.pause();
            media.clear();
        }
        self.controls.set_media_stream(None::<&gtk::MediaStream>);
        self.controls.set_visible(false);
        if let Some(file) = state.file.take() {
            let _ = std::fs::remove_file(file);
        }
    }
}

//! The call screen: outgoing ringing, voicemail recording and the live call, with the transcript
//! moving up and fading as it goes.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use ring_core::media::{self, AudioDevice, MediaState};
use ring_proto::{
    Account, AccountKind, Call, CallState, EntryKind, InvitationState, ParticipantState,
    TranscriptEntry,
};

use crate::app::App;
use crate::runtime;
use crate::ui::dialer;
use crate::ui::widgets;

const MAX_LINES: usize = 300;

/// How one transcript entry reads: an optional speaker label and the text. Events (joins,
/// declines, …) have no speaker and render as a muted pixel line.
pub fn describe_entry(entry: &TranscriptEntry, me: &str) -> Option<(Option<String>, String)> {
    let actor = entry
        .actor
        .as_ref()
        .map(|a| {
            if a.uuid == me {
                "You".to_string()
            } else {
                a.label().to_string()
            }
        })
        .unwrap_or_default();
    let text = entry.text.clone().unwrap_or_default();
    let data_str = |key: &str| {
        entry
            .data
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    Some(match entry.kind {
        EntryKind::Speech => (Some(actor), text),
        EntryKind::CallStarted => (None, "Call started".into()),
        EntryKind::Answered => match entry.data.get("after_ms").and_then(|v| v.as_u64()) {
            Some(ms) => (
                None,
                format!("Answered after {}", widgets::format_duration(ms)),
            ),
            None => (None, "Answered".into()),
        },
        EntryKind::Joined => (None, format!("{actor} joined")),
        EntryKind::Left => (None, format!("{actor} left")),
        EntryKind::Invited => {
            let invitee = entry
                .data
                .get("invitee")
                .and_then(|v| serde_json::from_value::<Account>(v.clone()).ok());
            match invitee {
                Some(invitee) => (None, format!("{actor} invited {}", invitee.label())),
                None => (None, format!("{actor} invited someone")),
            }
        }
        EntryKind::Declined => match data_str("reason").filter(|r| !r.is_empty()) {
            Some(reason) => (None, format!("{actor} declined · {reason}")),
            None => (None, format!("{actor} declined")),
        },
        EntryKind::Missed => (None, format!("{actor} didn't answer")),
        EntryKind::Busy => (None, format!("{actor} was busy")),
        EntryKind::Voicemail => (None, "Voicemail left".into()),
        EntryKind::CallEnded => (None, "Call ended".into()),
        EntryKind::Note => (None, text),
        EntryKind::Delegation | EntryKind::Thinking | EntryKind::Commentary => return None,
    })
}

pub fn entry_widget(speaker: Option<&str>, text: &str, partial: bool) -> (gtk::Box, gtk::Label) {
    let line = gtk::Box::new(gtk::Orientation::Vertical, 2);
    line.add_css_class("transcript-line");
    let body = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(false)
        .build();
    match speaker {
        Some(speaker) => {
            let name = gtk::Label::builder()
                .label(speaker.to_uppercase())
                .xalign(0.0)
                .build();
            name.add_css_class("speaker");
            line.append(&name);
            body.add_css_class("text");
        }
        None => {
            line.add_css_class("event");
            body.set_label(&text.to_uppercase());
        }
    }
    if partial {
        line.add_css_class("partial");
    }
    line.append(&body);
    (line, body)
}

struct Tile {
    frame: gtk::Box,
    muted: gtk::Image,
}

/// An interim speech line: one speech-to-text segment until its final entry replaces it.
struct Partial {
    segment_id: String,
    speaker: String,
    revealer: gtk::Revealer,
    label: gtk::Label,
}

#[derive(Default)]
struct ViewState {
    call: Option<Call>,
    me: String,
    local_identity: Option<String>,
    tiles: HashMap<String, Tile>,
    lines: BTreeMap<u64, gtk::Revealer>,
    /// Interim lines, oldest first.
    partials: Vec<Partial>,
    /// Segments whose final entry has landed, so a late interim cannot bring them back.
    finished_segments: HashSet<String>,
    remote_muted: HashMap<String, bool>,
    ended: bool,
}

struct Inner {
    root: adw::ToolbarView,
    status: gtk::Label,
    timer: gtk::Label,
    media_label: gtk::Label,
    people: gtk::FlowBox,
    voicemail_note: gtk::Box,
    transcript: gtk::Box,
    scroller: gtk::ScrolledWindow,
    controls: gtk::Box,
    mute: gtk::ToggleButton,
    mute_box: gtk::Box,
    level: gtk::LevelBar,
    devices_box: gtk::Box,
    add_box: gtk::Box,
    hangup_box: gtk::Box,
    done_box: gtk::Box,
    state: RefCell<ViewState>,
    timer_source: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct CallView(Rc<Inner>);

fn control(button: &impl IsA<gtk::Widget>, label: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_halign(gtk::Align::Center);
    button.set_halign(gtk::Align::Center);
    column.append(button);
    let caption = gtk::Label::new(Some(label));
    caption.add_css_class("control-label");
    column.append(&caption);
    column
}

fn round_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("call-control");
    button.set_tooltip_text(Some(tooltip));
    button
}

impl CallView {
    pub fn new() -> CallView {
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_tooltip_text(Some("Back to Recents (the call continues)"));
        back.connect_clicked(|_| App::get().show_home());
        let header = adw::HeaderBar::builder().show_title(false).build();
        header.pack_start(&back);

        let status = gtk::Label::new(None);
        status.add_css_class("pixel");
        status.add_css_class("large");
        let timer = gtk::Label::new(None);
        timer.add_css_class("pixel");
        timer.add_css_class("timer");
        let status_line = gtk::Box::new(gtk::Orientation::Vertical, 4);
        status_line.set_halign(gtk::Align::Center);
        status_line.append(&status);
        status_line.append(&timer);
        let media_label = gtk::Label::new(None);
        media_label.add_css_class("pixel");
        media_label.add_css_class("tiny");
        media_label.add_css_class("muted");
        status_line.append(&media_label);

        let people = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .halign(gtk::Align::Center)
            .homogeneous(true)
            .max_children_per_line(4)
            .min_children_per_line(1)
            .column_spacing(24)
            .row_spacing(16)
            .build();

        let voicemail_note = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        voicemail_note.set_halign(gtk::Align::Center);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("recording-dot");
        dot.set_valign(gtk::Align::Center);
        voicemail_note.append(&dot);
        voicemail_note.append(&widgets::pixel("Leave a message after the beep"));
        voicemail_note.set_visible(false);

        let transcript = gtk::Box::new(gtk::Orientation::Vertical, 14);
        transcript.set_valign(gtk::Align::End);
        transcript.set_margin_top(72);
        transcript.set_margin_bottom(12);
        let transcript_clamp = adw::Clamp::builder()
            .maximum_size(640)
            .child(&transcript)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&transcript_clamp)
            .build();
        let fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fade.add_css_class("transcript-fade");
        fade.set_valign(gtk::Align::Start);
        fade.set_height_request(88);
        fade.set_can_target(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&scroller));
        overlay.add_overlay(&fade);
        overlay.set_vexpand(true);

        // Controls
        let mute = gtk::ToggleButton::builder()
            .icon_name("audio-input-microphone-symbolic")
            .tooltip_text("Mute")
            .build();
        mute.add_css_class("call-control");
        mute.connect_toggled(|button| {
            button.set_icon_name(if button.is_active() {
                "microphone-disabled-symbolic"
            } else {
                "audio-input-microphone-symbolic"
            });
            App::get().set_muted(button.is_active());
        });
        let level = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .width_request(40)
            .build();
        level.set_halign(gtk::Align::Center);
        let mute_box = control(&mute, "Mute");
        mute_box.append(&level);

        let devices = gtk::MenuButton::builder()
            .icon_name("audio-speakers-symbolic")
            .tooltip_text("Audio devices")
            .build();
        devices.add_css_class("call-control");
        devices.set_direction(gtk::ArrowType::Up);
        let popover = gtk::Popover::new();
        popover.connect_show(rebuild_device_menu);
        devices.set_popover(Some(&popover));
        let devices_box = control(&devices, "Audio");

        let add = round_button("contact-new-symbolic", "Add someone to this call");
        add.connect_clicked(|_| dialer::present(dialer::Mode::Invite));
        let add_box = control(&add, "Add");

        let hangup = round_button("call-stop-symbolic", "Hang up");
        hangup.add_css_class("hangup");
        hangup.add_css_class("big");
        hangup.connect_clicked(|_| App::get().hang_up());
        let hangup_box = control(&hangup, "End");

        let done = round_button("object-select-symbolic", "Finish and send the message");
        done.add_css_class("suggested-action");
        done.add_css_class("big");
        done.connect_clicked(|_| App::get().hang_up());
        let done_box = control(&done, "Done");

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 28);
        controls.set_halign(gtk::Align::Center);
        controls.set_margin_top(8);
        controls.set_margin_bottom(28);
        for part in [&mute_box, &devices_box, &add_box, &hangup_box, &done_box] {
            controls.append(part);
        }

        let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
        column.set_margin_start(16);
        column.set_margin_end(16);
        column.append(&status_line);
        column.append(&people);
        column.append(&voicemail_note);
        column.append(&overlay);
        column.append(&controls);

        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&column));

        CallView(Rc::new(Inner {
            root,
            status,
            timer,
            media_label,
            people,
            voicemail_note,
            transcript,
            scroller,
            controls,
            mute,
            mute_box,
            level,
            devices_box,
            add_box,
            hangup_box,
            done_box,
            state: RefCell::default(),
            timer_source: RefCell::default(),
        }))
    }

    pub fn root(&self) -> &adw::ToolbarView {
        &self.0.root
    }

    fn clear(&self) {
        if let Some(source) = self.0.timer_source.take() {
            source.remove();
        }
        while let Some(child) = self.0.transcript.first_child() {
            self.0.transcript.remove(&child);
        }
        self.0.people.remove_all();
        self.0.timer.set_label("");
        self.0.media_label.set_label("");
        self.0.level.set_value(0.0);
        self.0.mute.set_active(false);
        let me = std::mem::take(&mut self.0.state.borrow_mut().me);
        *self.0.state.borrow_mut() = ViewState {
            me,
            ..Default::default()
        };
    }

    /// The moment "Call" is pressed, before the server answers.
    pub fn show_dialing(&self, to: &str) {
        self.clear();
        let kind = if to.starts_with("si:") {
            AccountKind::Silicon
        } else {
            AccountKind::Carbon
        };
        let placeholder = Account {
            uuid: String::new(),
            id: to.to_string(),
            kind,
            display_name: None,
            pfp_url: None,
        };
        self.add_tile(&placeholder, true, true);
        self.0.status.set_label("CALLING…");
        self.0.voicemail_note.set_visible(false);
        self.show_controls(false, false, false, true, false);
    }

    pub fn load(&self, call: &Call, me: &str, identity: Option<&str>) {
        self.clear();
        {
            let mut state = self.0.state.borrow_mut();
            state.me = me.to_string();
            state.local_identity = identity.map(str::to_string);
        }
        self.update(call);
    }

    pub fn set_local_identity(&self, identity: &str) {
        self.0.state.borrow_mut().local_identity = Some(identity.to_string());
    }

    fn show_controls(&self, mute: bool, devices: bool, add: bool, hangup: bool, done: bool) {
        self.0.mute_box.set_visible(mute);
        self.0.devices_box.set_visible(devices);
        self.0.add_box.set_visible(add);
        self.0.hangup_box.set_visible(hangup);
        self.0.done_box.set_visible(done);
        self.0
            .controls
            .set_visible(mute || devices || add || hangup || done);
    }

    fn add_tile(&self, account: &Account, large: bool, ringing: bool) {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let (frame, _) = widgets::ringed_avatar(account, if large { 120 } else { 88 });
        if ringing {
            frame.add_css_class("ringing");
        }
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&frame));
        let muted = gtk::Image::from_icon_name("microphone-disabled-symbolic");
        muted.add_css_class("osd");
        muted.add_css_class("circular");
        muted.set_halign(gtk::Align::End);
        muted.set_valign(gtk::Align::End);
        muted.set_visible(false);
        overlay.add_overlay(&muted);
        overlay.set_halign(gtk::Align::Center);
        column.append(&overlay);
        column.append(&widgets::identity_block(
            account,
            if large { "hero" } else { "medium" },
        ));
        self.0.people.append(&column);
        // A homogeneous FlowBox is as wide as its column count, so it only centres the tiles
        // when that count matches them.
        let mut count = 0u32;
        let mut child = self.0.people.first_child();
        while let Some(widget) = child {
            count += 1;
            child = widget.next_sibling();
        }
        self.0.people.set_max_children_per_line(count.clamp(1, 4));
        self.0
            .state
            .borrow_mut()
            .tiles
            .insert(account.uuid.clone(), Tile { frame, muted });
    }

    pub fn update(&self, call: &Call) {
        let me = self.0.state.borrow().me.clone();
        let outgoing = call.created_by.uuid == me;
        self.0.state.borrow_mut().call = Some(call.clone());
        self.0.state.borrow_mut().ended = false;

        // People: everyone but me who has joined, then whoever is still being rung.
        self.0.people.remove_all();
        self.0.state.borrow_mut().tiles.clear();
        let joined: Vec<&Account> = call
            .participants
            .iter()
            .filter(|p| p.state == ParticipantState::Joined && p.account.uuid != me)
            .map(|p| &p.account)
            .collect();
        let ringing: Vec<&Account> = call
            .invitations
            .iter()
            .filter(|i| {
                i.state == InvitationState::Ringing
                    && i.to.uuid != me
                    && !joined.iter().any(|a| a.uuid == i.to.uuid)
            })
            .map(|i| &i.to)
            .collect();
        let large = joined.len() + ringing.len() <= 1;
        for account in &joined {
            self.add_tile(account, large, false);
        }
        for account in &ringing {
            self.add_tile(account, large, true);
        }
        if joined.is_empty() && ringing.is_empty() {
            // A voicemail or a callee who already declined: still show who was called.
            if let Some(other) = widgets::others(call, &me).first() {
                self.add_tile(other, true, false);
            }
        }
        let muted: Vec<(String, bool)> = self
            .0
            .state
            .borrow()
            .remote_muted
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (identity, is_muted) in muted {
            self.set_remote_muted(&identity, is_muted);
        }

        match call.state {
            CallState::Ringing => {
                let rung = call
                    .invitations
                    .iter()
                    .any(|i| i.state == InvitationState::Ringing);
                self.0.status.set_label(if outgoing && !rung {
                    "CALLING…"
                } else {
                    "RINGING"
                });
                self.0.voicemail_note.set_visible(false);
                self.show_controls(true, true, false, true, false);
            }
            CallState::Active => {
                self.0.status.set_label("ON CALL");
                self.0.voicemail_note.set_visible(false);
                self.show_controls(true, true, true, true, false);
                self.start_timer(call);
            }
            CallState::Voicemail => {
                let reason = call
                    .invitations
                    .iter()
                    .find(|i| i.from.uuid == me && i.to.uuid != me)
                    .map(|i| match (i.state, i.reason.as_deref()) {
                        (InvitationState::Busy, _) => "BUSY".to_string(),
                        (InvitationState::Declined, Some(r)) if !r.is_empty() => {
                            format!("DECLINED · {r}")
                        }
                        (InvitationState::Declined, _) => "DECLINED".to_string(),
                        _ => "NO ANSWER".to_string(),
                    })
                    .unwrap_or_else(|| "VOICEMAIL".to_string());
                self.0.status.set_label(&reason);
                self.0.voicemail_note.set_visible(outgoing);
                self.show_controls(false, true, false, !outgoing, outgoing);
            }
            CallState::Ended => self.show_ended(call, &me),
        }
    }

    fn start_timer(&self, call: &Call) {
        if self.0.timer_source.borrow().is_some() {
            return;
        }
        let Some(answered) = call.answered_at else {
            return;
        };
        let timer = self.0.timer.clone();
        let tick = move || {
            let elapsed = (ring_proto::Time::now().millis() - answered.millis()).max(0) as u64;
            timer.set_label(&widgets::format_duration(elapsed));
        };
        tick();
        let source = glib::timeout_add_seconds_local(1, move || {
            tick();
            glib::ControlFlow::Continue
        });
        self.0.timer_source.replace(Some(source));
    }

    /// The final state for a moment before returning home.
    pub fn show_ended(&self, call: &Call, me: &str) {
        if let Some(source) = self.0.timer_source.take() {
            source.remove();
        }
        self.0.state.borrow_mut().ended = true;
        let text = match (call.answered_at, call.ended_at) {
            (Some(start), Some(end)) => format!(
                "CALL ENDED · {}",
                widgets::format_duration((end.millis() - start.millis()).max(0) as u64)
            ),
            _ => {
                let invitation = call
                    .invitations
                    .iter()
                    .find(|i| i.from.uuid == me && i.to.uuid != me);
                match invitation.map(|i| (i.state, i.reason.clone())) {
                    Some((InvitationState::Declined, Some(reason))) if !reason.is_empty() => {
                        format!("DECLINED · {reason}")
                    }
                    Some((InvitationState::Declined, _)) => "DECLINED".into(),
                    Some((InvitationState::Busy, _)) => "BUSY".into(),
                    Some((InvitationState::Missed, _)) => "NO ANSWER".into(),
                    Some((InvitationState::Cancelled, _)) => "CANCELLED".into(),
                    _ => "CALL ENDED".into(),
                }
            }
        };
        self.0.status.set_label(&text);
        self.0.media_label.set_label("");
        self.0.voicemail_note.set_visible(false);
        for tile in self.0.state.borrow().tiles.values() {
            tile.frame.remove_css_class("ringing");
            tile.frame.remove_css_class("speaking");
        }
        self.show_controls(false, false, false, false, false);
    }

    /// The call carries on on another of this account's devices.
    pub fn show_moved(&self, device: &str) {
        self.0
            .status
            .set_label(&format!("MOVED TO {}", device.to_uppercase()));
    }

    pub fn set_media_state(&self, state: &MediaState) {
        if self.0.state.borrow().ended {
            return;
        }
        self.0.media_label.set_label(match state {
            MediaState::Connecting => "CONNECTING AUDIO…",
            MediaState::Connected => "",
            MediaState::Reconnecting => "RECONNECTING…",
            MediaState::Disconnected { .. } => "AUDIO DISCONNECTED",
        });
    }

    pub fn set_muted(&self, muted: bool) {
        if self.0.mute.is_active() != muted {
            self.0.mute.set_active(muted);
        }
    }

    pub fn set_level(&self, level: f32) {
        self.0.level.set_value(level.clamp(0.0, 1.0) as f64);
    }

    fn uuid_for_identity(&self, identity: &str) -> Option<String> {
        let state = self.0.state.borrow();
        if state.local_identity.as_deref() == Some(identity) {
            return Some(state.me.clone());
        }
        state
            .call
            .as_ref()?
            .participants
            .iter()
            .find(|p| p.media_identity.as_deref() == Some(identity))
            .map(|p| p.account.uuid.clone())
    }

    /// A soft ring pulse around whoever is speaking.
    pub fn set_speaking(&self, identities: &[String]) {
        let speaking: Vec<String> = identities
            .iter()
            .filter_map(|id| self.uuid_for_identity(id))
            .collect();
        for (uuid, tile) in self.0.state.borrow().tiles.iter() {
            if speaking.contains(uuid) {
                tile.frame.add_css_class("speaking");
            } else {
                tile.frame.remove_css_class("speaking");
            }
        }
    }

    pub fn set_remote_muted(&self, identity: &str, muted: bool) {
        self.0
            .state
            .borrow_mut()
            .remote_muted
            .insert(identity.to_string(), muted);
        if let Some(uuid) = self.uuid_for_identity(identity) {
            if let Some(tile) = self.0.state.borrow().tiles.get(&uuid) {
                tile.muted.set_visible(muted);
            }
        }
    }

    // ---- transcript -------------------------------------------------------------------------------

    fn near_bottom(&self) -> bool {
        let adj = self.0.scroller.vadjustment();
        adj.value() + adj.page_size() >= adj.upper() - 48.0
    }

    fn scroll_to_end(&self) {
        let scroller = self.0.scroller.clone();
        // After the slide-in so the final height is known.
        glib::timeout_add_local_once(Duration::from_millis(260), move || {
            let adj = scroller.vadjustment();
            adj.set_value(adj.upper() - adj.page_size());
        });
    }

    fn reveal(widget: &gtk::Box) -> gtk::Revealer {
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(220)
            .child(widget)
            .build();
        let r = revealer.clone();
        glib::idle_add_local_once(move || r.set_reveal_child(true));
        revealer
    }

    pub fn add_entry(&self, entry: &TranscriptEntry) {
        let me = self.0.state.borrow().me.clone();
        if self.0.state.borrow().lines.contains_key(&entry.seq) {
            return;
        }
        let Some((speaker, text)) = describe_entry(entry, &me) else {
            return;
        };
        if entry.kind == EntryKind::Speech && text.trim().is_empty() {
            return;
        }
        let stick = self.near_bottom();
        if entry.kind == EntryKind::Speech {
            self.finish_partial(entry);
        }
        let (line, _) = entry_widget(speaker.as_deref(), &text, false);
        let revealer = Self::reveal(&line);
        {
            let mut state = self.0.state.borrow_mut();
            let previous = state
                .lines
                .range(..entry.seq)
                .next_back()
                .map(|(_, w)| w.clone());
            let next_exists = state.lines.range(entry.seq..).next().is_some();
            match (previous, next_exists) {
                (Some(previous), true) => self
                    .0
                    .transcript
                    .insert_child_after(&revealer, Some(&previous)),
                (None, true) => self.0.transcript.prepend(&revealer),
                _ => {
                    self.0.transcript.append(&revealer);
                    // Live partial lines stay below the final ones, in the order they began.
                    for partial in &state.partials {
                        self.0.transcript.reorder_child_after(
                            &partial.revealer,
                            self.0.transcript.last_child().as_ref(),
                        );
                    }
                }
            }
            state.lines.insert(entry.seq, revealer);
            while state.lines.len() > MAX_LINES {
                if let Some((_, oldest)) = state.lines.pop_first() {
                    self.0.transcript.remove(&oldest);
                }
            }
        }
        if stick {
            self.scroll_to_end();
        }
    }

    /// Removes the interim line a final speech entry replaces: the one with its `segment_id`,
    /// or that speaker's newest when the entry carries none. That speaker's older interim lines
    /// go too, since their segments can no longer be finalised out of order.
    fn finish_partial(&self, entry: &TranscriptEntry) {
        let segment = entry
            .data
            .get("segment_id")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty());
        let mut state = self.0.state.borrow_mut();
        if let Some(id) = segment {
            if state.finished_segments.len() >= 1024 {
                state.finished_segments.clear();
            }
            state.finished_segments.insert(id.to_string());
        }
        let index = match (segment, entry.actor.as_ref()) {
            (Some(id), _) => state.partials.iter().position(|p| p.segment_id == id),
            (None, Some(actor)) => state.partials.iter().rposition(|p| p.speaker == actor.uuid),
            (None, None) => None,
        };
        let Some(index) = index else { return };
        let speaker = state.partials[index].speaker.clone();
        let mut position = 0;
        state.partials.retain(|p| {
            let keep = position > index || (position < index && p.speaker != speaker);
            position += 1;
            if !keep {
                self.0.transcript.remove(&p.revealer);
            }
            keep
        });
    }

    /// Interim speech, muted until the final entry for its segment replaces it.
    pub fn partial(&self, speaker: &Account, segment_id: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // Without a segment id, one interim line per speaker.
        let segment_id = if segment_id.is_empty() {
            format!("speaker:{}", speaker.uuid)
        } else {
            segment_id.to_string()
        };
        let me = self.0.state.borrow().me.clone();
        if self
            .0
            .state
            .borrow()
            .finished_segments
            .contains(&segment_id)
        {
            return;
        }
        let stick = self.near_bottom();
        let existing = self
            .0
            .state
            .borrow()
            .partials
            .iter()
            .find(|p| p.segment_id == segment_id)
            .map(|p| p.label.clone());
        match existing {
            Some(label) => label.set_label(text),
            None => {
                let name = if speaker.uuid == me {
                    "You".to_string()
                } else {
                    speaker.label().to_string()
                };
                let (line, label) = entry_widget(Some(&name), text, true);
                let revealer = Self::reveal(&line);
                self.0.transcript.append(&revealer);
                self.0.state.borrow_mut().partials.push(Partial {
                    segment_id,
                    speaker: speaker.uuid.clone(),
                    revealer,
                    label,
                });
            }
        }
        if stick {
            self.scroll_to_end();
        }
    }
}

/// Whether every device has its own id. WebRTC's PulseAudio and ALSA modules report empty ids,
/// and then the only device Ring can open is the system default.
pub fn selectable(devices: &[AudioDevice]) -> bool {
    let ids: HashSet<&str> = devices.iter().map(|d| d.id.as_str()).collect();
    !devices.is_empty() && ids.len() == devices.len() && !ids.contains("")
}

/// WebRTC lists the default first as "default: <name>"; the system default row stands for it.
fn is_default_alias(device: &AudioDevice) -> bool {
    device.name.to_ascii_lowercase().starts_with("default")
}

fn default_name(devices: &[AudioDevice]) -> String {
    devices
        .iter()
        .find(|d| d.is_default)
        .map(|d| match d.name.split_once(": ") {
            Some((prefix, name)) if prefix.eq_ignore_ascii_case("default") => name.to_string(),
            _ => d.name.clone(),
        })
        .unwrap_or_else(|| "System default".to_string())
}

fn device_section(
    title: &str,
    devices: Vec<AudioDevice>,
    selected: Option<String>,
    input: bool,
) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let heading = widgets::pixel(title);
    heading.add_css_class("muted");
    heading.add_css_class("tiny");
    section.append(&heading);
    if !selectable(&devices) {
        section.append(&widgets::label(&default_name(&devices), &[]));
        return section;
    }
    let choose = move |id: Option<String>| {
        if input {
            App::get().set_input_device(id)
        } else {
            App::get().set_output_device(id)
        }
    };
    let default =
        gtk::CheckButton::with_label(&format!("System default ({})", default_name(&devices)));
    let current = selected.filter(|id| devices.iter().any(|d| &d.id == id && !is_default_alias(d)));
    default.set_active(current.is_none());
    default.connect_toggled(move |b| {
        if b.is_active() {
            choose(None);
        }
    });
    section.append(&default);
    for device in devices.into_iter().filter(|d| !is_default_alias(d)) {
        let check = gtk::CheckButton::with_label(&device.name);
        check.set_group(Some(&default));
        check.set_active(current.as_deref() == Some(device.id.as_str()));
        check.connect_toggled(move |b| {
            if b.is_active() {
                choose(Some(device.id.clone()));
            }
        });
        section.append(&check);
    }
    section
}

fn rebuild_device_menu(popover: &gtk::Popover) {
    let config = App::get().config();
    let (inputs, outputs) = {
        let _guard = runtime::enter();
        (media::input_devices(), media::output_devices())
    };
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_margin_top(6);
    column.set_margin_bottom(6);
    column.set_margin_start(6);
    column.set_margin_end(6);
    let mut fixed = false;
    for (title, devices, selected, input) in [
        ("Microphone", inputs, config.input_device, true),
        ("Speaker", outputs, config.output_device, false),
    ] {
        match devices {
            Ok(devices) => {
                fixed |= !selectable(&devices);
                column.append(&device_section(title, devices, selected, input));
            }
            Err(err) => column.append(&widgets::label(&format!("{title}: {err}"), &["muted"])),
        }
    }
    if fixed {
        let hint = widgets::label(
            "Ring uses your system\u{2019}s default microphone and speaker. Change them in your sound settings.",
            &["muted"],
        );
        hint.set_max_width_chars(36);
        column.append(&hint);
    }
    popover.set_child(Some(&column));
}

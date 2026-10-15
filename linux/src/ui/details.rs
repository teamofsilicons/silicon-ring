//! Call details: participants, outcome, decline reasons, a timeline in the style of the
//! teamofsilicons.com writings timeline, the full transcript and the recording.

use adw::prelude::*;
use gtk::glib;
use ring_proto::{Call, EntryKind, InvitationState, RecordingStatus, TranscriptEntry};

use crate::app::App;
use crate::runtime;
use crate::ui::call::{describe_entry, entry_widget};
use crate::ui::player::Player;
use crate::ui::widgets;

fn heading(text: &str) -> gtk::Label {
    let label = widgets::pixel(text);
    label.add_css_class("muted");
    label.add_css_class("tiny");
    label.set_margin_top(10);
    label
}

/// One timeline row: a dot on a hairline rail, the event, and its time.
fn timeline_row(text: &str, when: &str, danger: bool, last: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rail.set_size_request(9, -1);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("timeline-dot");
    if danger {
        dot.add_css_class("danger");
    }
    dot.set_halign(gtk::Align::Center);
    dot.set_margin_top(5);
    rail.append(&dot);
    if !last {
        let line = gtk::Box::new(gtk::Orientation::Vertical, 0);
        line.add_css_class("timeline-rail");
        line.set_halign(gtk::Align::Center);
        line.set_vexpand(true);
        line.set_margin_top(4);
        rail.append(&line);
    }
    row.append(&rail);
    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_box.set_margin_bottom(14);
    text_box.set_hexpand(true);
    text_box.append(&widgets::label(text, &[]));
    let time = widgets::pixel(when);
    time.add_css_class("muted");
    time.add_css_class("tiny");
    text_box.append(&time);
    row.append(&text_box);
    row
}

fn offset_text(entry: &TranscriptEntry) -> String {
    match entry.offset_ms {
        Some(ms) if ms >= 0 => format!("+{}", widgets::format_duration(ms as u64)),
        _ => widgets::local(&entry.at)
            .and_then(|d| d.format("%H:%M:%S").ok())
            .map(|s| s.to_string())
            .unwrap_or_default(),
    }
}

pub fn present(call: &Call) {
    let app = App::get();
    let me = app.my_uuid();
    let people = widgets::others(call, &me);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(8);
    content.set_margin_bottom(24);
    content.set_margin_start(20);
    content.set_margin_end(20);

    // Who and how it went.
    let avatars = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    avatars.set_halign(gtk::Align::Center);
    for person in people.iter().take(4) {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.append(&widgets::avatar(
            person,
            if people.len() == 1 { 88 } else { 56 },
        ));
        column.append(&widgets::identity_block(
            person,
            if people.len() == 1 { "medium" } else { "small" },
        ));
        avatars.append(&column);
    }
    content.append(&avatars);
    let summary = widgets::pixel(&format!(
        "{} · {}",
        widgets::outcome(call, &me),
        widgets::format_full(&call.created_at)
    ));
    summary.add_css_class(if widgets::is_missed(call, &me) {
        "danger-text"
    } else {
        "muted"
    });
    summary.set_halign(gtk::Align::Center);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&summary);

    // Decline reasons are kept in the call log.
    let declines: Vec<String> = call
        .invitations
        .iter()
        .filter(|i| i.state == InvitationState::Declined)
        .map(|i| match i.reason.as_deref().filter(|r| !r.is_empty()) {
            Some(reason) => format!("{} declined: \u{201c}{reason}\u{201d}", i.to.label()),
            None => format!("{} declined without a reason", i.to.label()),
        })
        .collect();
    if !declines.is_empty() {
        content.append(&heading("Declined"));
        for line in declines {
            content.append(&widgets::label(&line, &[]));
        }
    }

    // Recording
    content.append(&heading("Recording"));
    let player = Player::new();
    content.append(player.widget());
    match call.recording.status {
        RecordingStatus::None => player.set_status("Not recorded"),
        RecordingStatus::Recording => player.set_status("Recording…"),
        RecordingStatus::Processing => player.set_status("Processing…"),
        RecordingStatus::Failed => player.set_status("The recording failed"),
        RecordingStatus::Ready => {
            player.set_status("Loading…");
            let client = app.client();
            let ringid = call.ringid.clone();
            let p = player.clone();
            glib::spawn_future_local(async move {
                let id = ringid.clone();
                match runtime::run(async move { client.recording(&id).await }).await {
                    Ok(recording) => match recording.url {
                        Some(url) => p.load(
                            &format!("recording-{ringid}"),
                            url,
                            recording.mime_type.as_deref(),
                            false,
                        ),
                        None => p.set_status("Processing…"),
                    },
                    Err(err) => p.set_status(&widgets::error_text(&err)),
                }
            });
        }
    }

    // Timeline and transcript, filled once loaded.
    let timeline_heading = heading("Timeline");
    let timeline = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let transcript_heading = heading("Transcript");
    let transcript = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let loading = gtk::Spinner::builder()
        .spinning(true)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    content.append(&timeline_heading);
    content.append(&timeline);
    content.append(&transcript_heading);
    content.append(&transcript);
    content.append(&loading);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some(first) = people.first().cloned() {
        let call_back = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("call-start-symbolic")
                    .label("Call")
                    .build(),
            )
            .tooltip_text(format!("Call {}", first.label()))
            .build();
        call_back.add_css_class("suggested-action");
        actions.append(&call_back);
        call_back.connect_clicked(move |button| {
            if let Some(dialog) = button
                .ancestor(adw::Dialog::static_type())
                .and_then(|w| w.downcast::<adw::Dialog>().ok())
            {
                dialog.close();
            }
            App::get().start_call(first.id.clone());
        });
    }
    let header = adw::HeaderBar::new();
    header.pack_end(&actions);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&content)
        .build();
    view.set_content(Some(&scroller));
    let dialog = adw::Dialog::builder()
        .title("Call details")
        .content_width(520)
        .content_height(680)
        .child(&view)
        .build();
    let p = player.clone();
    dialog.connect_closed(move |_| p.stop());
    dialog.present(Some(&app.window().window));

    let client = app.client();
    let ringid = call.ringid.clone();
    glib::spawn_future_local(async move {
        let mut entries: Vec<TranscriptEntry> = Vec::new();
        let mut after = 0;
        // The whole transcript, a page at a time.
        loop {
            let c = client.clone();
            let id = ringid.clone();
            match runtime::run(async move { c.transcript(&id, after, Some(200)).await }).await {
                Ok(page) => {
                    let done = page.items.is_empty() || page.next_after_seq <= after;
                    let seen = after;
                    after = page.next_after_seq;
                    entries.extend(page.items.into_iter().filter(|e| e.seq > seen));
                    if done || entries.len() >= 5000 {
                        break;
                    }
                }
                Err(err) => {
                    transcript.append(&widgets::label(&widgets::error_text(&err), &["muted"]));
                    break;
                }
            }
        }
        loading.set_visible(false);
        let events: Vec<&TranscriptEntry> = entries
            .iter()
            .filter(|e| e.kind != EntryKind::Speech && !e.kind.is_private())
            .collect();
        for (index, entry) in events.iter().enumerate() {
            if let Some((_, text)) = describe_entry(entry, &me) {
                let danger = matches!(
                    entry.kind,
                    EntryKind::Declined | EntryKind::Missed | EntryKind::Busy
                );
                timeline.append(&timeline_row(
                    &text,
                    &offset_text(entry),
                    danger,
                    index + 1 == events.len(),
                ));
            }
        }
        timeline_heading.set_visible(!events.is_empty());
        let mut spoken = 0;
        for entry in entries.iter().filter(|e| e.kind == EntryKind::Speech) {
            if let Some((speaker, text)) = describe_entry(entry, &me) {
                let (line, body) = entry_widget(speaker.as_deref(), &text, false);
                body.set_selectable(true);
                transcript.append(&line);
                spoken += 1;
            }
        }
        if spoken == 0 {
            transcript.append(&widgets::label(
                "Nothing was said on this call.",
                &["muted"],
            ));
        }
    });
}

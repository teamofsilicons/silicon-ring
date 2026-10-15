//! Desktop notifications.
//!
//! Incoming calls go through the XDG Notification portal (v2 understands the `call.incoming`
//! category, urgent priority and call button purposes), falling back to `gio::Notification`
//! when the portal is missing or refuses the request (older portals, unregistered host apps).
//! Both paths use the same exported application actions (`app.answer-call`, …), so a click
//! lands in the same place either way.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::OnceLock;

use ashpd::desktop::Icon;
use ashpd::desktop::notification::{
    Button, ButtonPurpose, Category, Notification, NotificationProxy, Priority,
};
use gtk::gio;
use gtk::prelude::*;

use crate::{APP_ID, runtime};

/// What to show. Every notification carries a target string passed to its actions.
#[derive(Clone, Debug)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: NoteKind,
    /// Passed as the parameter of every action (invitation id, voicemail id or ringid).
    pub target: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    IncomingCall,
    MissedCall,
    Voicemail,
    Info,
}

static PORTAL: OnceLock<tokio::sync::OnceCell<Option<NotificationProxy>>> = OnceLock::new();

async fn portal() -> Option<&'static NotificationProxy> {
    PORTAL
        .get_or_init(tokio::sync::OnceCell::new)
        .get_or_init(|| async {
            // Host (non-Flatpak) apps must tell the portal who they are before using it.
            if let Ok(app_id) = ashpd::AppID::try_from(APP_ID) {
                if let Err(err) = ashpd::register_host_app(app_id).await {
                    tracing::debug!("portal host registration unavailable: {err}");
                }
            }
            match NotificationProxy::new().await {
                Ok(proxy) => {
                    tracing::debug!("notification portal version {}", proxy.version());
                    Some(proxy)
                }
                Err(err) => {
                    tracing::info!(
                        "notification portal unavailable ({err}); using GIO notifications"
                    );
                    None
                }
            }
        })
        .await
        .as_ref()
}

/// Connects to the portal early, so the first incoming call is not delayed by it.
pub fn warm_up() {
    runtime::spawn(async {
        portal().await;
    });
}

fn portal_notification(note: &Note) -> Notification {
    let mut notification = Notification::new(&note.title)
        .body(note.body.as_str())
        .icon(Icon::with_names([APP_ID]))
        .default_action("app.show-target")
        .default_action_target::<&str>(Some(
            format!("{}:{}", kind_key(note.kind), note.target).as_str(),
        ));
    match note.kind {
        NoteKind::IncomingCall => {
            notification = notification
                .category(Category::IncomingCall)
                .priority(Priority::Urgent)
                .button(
                    Button::new("Decline", "app.decline-call")
                        .target::<&str>(Some(note.target.as_str()))
                        .purpose(ButtonPurpose::CallDecline),
                )
                .button(
                    Button::new("Answer", "app.answer-call")
                        .target::<&str>(Some(note.target.as_str()))
                        .purpose(ButtonPurpose::CallAccept),
                );
        }
        NoteKind::MissedCall => {
            notification = notification
                .category(Category::MissedCall)
                .priority(Priority::Normal)
                .button(
                    Button::new("Call back", "app.call-back")
                        .target::<&str>(Some(note.target.as_str())),
                );
        }
        NoteKind::Voicemail => {
            notification = notification.priority(Priority::Normal);
        }
        NoteKind::Info => {
            notification = notification.priority(Priority::Low);
        }
    }
    notification
}

fn gio_notification(note: &Note) -> gio::Notification {
    let notification = gio::Notification::new(&note.title);
    notification.set_body(Some(&note.body));
    notification.set_icon(&gio::ThemedIcon::new(APP_ID));
    let target = note.target.to_variant();
    notification.set_default_action_and_target_value(
        "app.show-target",
        Some(&format!("{}:{}", kind_key(note.kind), note.target).to_variant()),
    );
    match note.kind {
        NoteKind::IncomingCall => {
            notification.set_priority(gio::NotificationPriority::Urgent);
            notification.set_category(Some("call.incoming"));
            notification.add_button_with_target_value("Decline", "app.decline-call", Some(&target));
            notification.add_button_with_target_value("Answer", "app.answer-call", Some(&target));
        }
        NoteKind::MissedCall => {
            notification.set_category(Some("call.missed"));
            notification.add_button_with_target_value("Call back", "app.call-back", Some(&target));
        }
        NoteKind::Voicemail => {}
        NoteKind::Info => notification.set_priority(gio::NotificationPriority::Low),
    }
    notification
}

/// `show-target` parameters are `<kind>:<target>` so one action can route every notification.
fn kind_key(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::IncomingCall => "call",
        NoteKind::MissedCall => "missed",
        NoteKind::Voicemail => "voicemail",
        NoteKind::Info => "info",
    }
}

#[derive(Default)]
pub struct Notifier {
    /// Ids shown through GIO, which must be withdrawn through GIO as well.
    via_gio: RefCell<HashSet<String>>,
}

impl Notifier {
    pub fn show(self: &Rc<Self>, app: &adw::Application, note: Note) {
        let this = self.clone();
        let app = app.clone();
        let notification = portal_notification(&note);
        let id = note.id.clone();
        glib::spawn_future_local(async move {
            let sent = runtime::run(async move {
                match portal().await {
                    Some(proxy) => proxy
                        .add_notification(&id, notification)
                        .await
                        .map_err(|err| err.to_string()),
                    None => Err("no portal".to_string()),
                }
            })
            .await;
            if let Err(err) = sent {
                tracing::debug!("portal notification failed ({err}); using GIO");
                app.send_notification(Some(&note.id), &gio_notification(&note));
                this.via_gio.borrow_mut().insert(note.id.clone());
            }
        });
    }

    pub fn withdraw(&self, app: &adw::Application, id: &str) {
        if self.via_gio.borrow_mut().remove(id) {
            app.withdraw_notification(id);
            return;
        }
        let id = id.to_string();
        runtime::spawn(async move {
            if let Some(proxy) = portal().await {
                let _ = proxy.remove_notification(&id).await;
            }
        });
    }
}

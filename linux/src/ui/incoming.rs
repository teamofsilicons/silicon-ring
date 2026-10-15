//! The ringing window: an urgent, always-presented "moment" screen in the Ring mark's colours.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use ring_proto::{Account, Call, Invitation, limits};

use crate::app::App;
use crate::ui::widgets;

const REASONS: [&str; 4] = [
    "Can't talk right now",
    "In a meeting",
    "Call you back soon",
    "Text me instead",
];

struct Inner {
    window: adw::Window,
    ringid: String,
    invitation_id: String,
    caller: Account,
    me: String,
    with_label: gtk::Label,
    reasons: gtk::Revealer,
    decline_armed: Cell<bool>,
    decline_caption: gtk::Label,
}

#[derive(Clone)]
pub struct IncomingWindow(Rc<Inner>);

fn with_text(call: &Call, caller: &str, me: &str) -> String {
    let others: Vec<String> = call
        .joined()
        .filter(|p| p.account.uuid != caller && p.account.uuid != me)
        .map(|p| p.account.label().to_uppercase())
        .collect();
    if others.is_empty() {
        String::new()
    } else {
        format!("WITH {}", others.join(", "))
    }
}

impl IncomingWindow {
    pub fn new(
        app: &adw::Application,
        call: &Call,
        invitation: &Invitation,
        me: &str,
        in_call: bool,
    ) -> IncomingWindow {
        let caller = invitation.from.clone();
        let window = adw::Window::builder()
            .application(app)
            .title(format!("{} is calling", caller.label()))
            .default_width(400)
            .default_height(640)
            .resizable(false)
            .build();
        window.add_css_class("moment");

        let column = gtk::Box::new(gtk::Orientation::Vertical, 14);
        column.set_valign(gtk::Align::Center);
        column.set_margin_start(28);
        column.set_margin_end(28);
        column.set_margin_bottom(28);

        let (frame, _) = widgets::ringed_avatar(&caller, 88);
        frame.add_css_class("ringing");
        frame.set_margin_bottom(18);
        column.append(&frame);
        column.append(&widgets::identity_block(&caller, "hero"));

        let calling = widgets::pixel("Is calling");
        calling.add_css_class("muted");
        calling.set_halign(gtk::Align::Center);
        calling.set_margin_top(6);
        column.append(&calling);
        let with_label = widgets::pixel(&with_text(call, &caller.uuid, me));
        with_label.add_css_class("muted");
        with_label.set_halign(gtk::Align::Center);
        with_label.set_wrap(true);
        with_label.set_justify(gtk::Justification::Center);
        column.append(&with_label);

        // Decline reasons, revealed by the first press of Decline.
        let reasons_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        reasons_box.set_margin_top(10);
        let chips = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(2)
            .column_spacing(8)
            .row_spacing(8)
            .homogeneous(false)
            .build();
        reasons_box.append(&chips);
        let entry = gtk::Entry::builder()
            .placeholder_text("Something else…")
            .max_length(limits::DECLINE_REASON as i32)
            .build();
        entry.set_secondary_icon_name(Some("mail-send-symbolic"));
        entry.set_secondary_icon_tooltip_text(Some("Decline with this reason"));
        reasons_box.append(&entry);
        let hint = widgets::label(
            "Press Decline again to decline without a reason.",
            &["muted"],
        );
        hint.set_xalign(0.5);
        hint.set_justify(gtk::Justification::Center);
        reasons_box.append(&hint);
        let reasons = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&reasons_box)
            .build();
        column.append(&reasons);

        // Decline · Accept
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 56);
        buttons.set_halign(gtk::Align::Center);
        buttons.set_margin_top(18);
        let decline = gtk::Button::from_icon_name("call-stop-symbolic");
        decline.add_css_class("call-control");
        decline.add_css_class("big");
        decline.add_css_class("decline");
        decline.set_tooltip_text(Some("Decline"));
        let decline_caption = gtk::Label::new(Some("Decline"));
        decline_caption.add_css_class("control-label");
        let decline_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        decline_box.append(&decline);
        decline_box.append(&decline_caption);
        let accept = gtk::Button::from_icon_name("call-start-symbolic");
        accept.add_css_class("call-control");
        accept.add_css_class("big");
        accept.add_css_class("accept");
        let accept_text = if in_call { "End & accept" } else { "Accept" };
        accept.set_tooltip_text(Some(accept_text));
        let accept_caption = gtk::Label::new(Some(accept_text));
        accept_caption.add_css_class("control-label");
        let accept_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        accept_box.append(&accept);
        accept_box.append(&accept_caption);
        buttons.append(&decline_box);
        buttons.append(&accept_box);
        column.append(&buttons);

        let silence = gtk::Button::with_label("Silence");
        silence.add_css_class("flat");
        silence.set_halign(gtk::Align::Center);
        silence.set_tooltip_text(Some("Stop ringing on all your devices without declining"));
        column.append(&silence);

        let header = adw::HeaderBar::builder().show_title(false).build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&column));
        window.set_content(Some(&view));

        let this = IncomingWindow(Rc::new(Inner {
            window: window.clone(),
            ringid: call.ringid.clone(),
            invitation_id: invitation.invitation_id.clone(),
            caller,
            me: me.to_string(),
            with_label,
            reasons: reasons.clone(),
            decline_armed: Cell::new(false),
            decline_caption,
        }));

        for reason in REASONS {
            let chip = gtk::Button::with_label(reason);
            chip.add_css_class("chip");
            let id = this.0.invitation_id.clone();
            chip.connect_clicked(move |_| App::get().decline(&id, Some(reason.to_string())));
            chips.insert(&chip, -1);
        }
        let id = this.0.invitation_id.clone();
        let submit = move |entry: &gtk::Entry| {
            let text = entry.text().trim().to_string();
            if !text.is_empty() {
                App::get().decline(&id, Some(text));
            }
        };
        let submit_icon = submit.clone();
        entry.connect_activate(submit);
        entry.connect_icon_release(move |entry, _| submit_icon(entry));

        let weak = Rc::downgrade(&this.0);
        decline.connect_clicked(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            if inner.decline_armed.get() {
                App::get().decline(&inner.invitation_id, None);
            } else {
                inner.decline_armed.set(true);
                inner.reasons.set_reveal_child(true);
                inner.decline_caption.set_label("Decline now");
            }
        });
        let id = this.0.invitation_id.clone();
        accept.connect_clicked(move |_| App::get().accept(&id));
        let id = this.0.invitation_id.clone();
        silence.connect_clicked(move |_| App::get().silence(&id));
        let id = this.0.invitation_id.clone();
        window.connect_close_request(move |_| {
            // Closing only stops the ringing here; the caller keeps hearing it ring.
            App::get().dismiss_incoming(&id);
            glib::Propagation::Stop
        });
        let shortcuts = gtk::EventControllerKey::new();
        let id = this.0.invitation_id.clone();
        shortcuts.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                App::get().dismiss_incoming(&id);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(shortcuts);
        accept.grab_focus();
        this
    }

    pub fn ringid(&self) -> String {
        self.0.ringid.clone()
    }

    pub fn caller(&self) -> Account {
        self.0.caller.clone()
    }

    pub fn update(&self, call: &Call) {
        self.0
            .with_label
            .set_label(&with_text(call, &self.0.caller.uuid, &self.0.me));
    }

    /// Shows the window and asks the window manager for attention.
    pub fn present(&self) {
        self.0.window.present();
        if let Some(surface) = self.0.window.surface() {
            if let Ok(x11) = surface.downcast::<gdk_x11::X11Surface>() {
                x11.set_urgency_hint(true);
            }
        }
    }

    pub fn close(&self) {
        self.0.window.destroy();
    }
}

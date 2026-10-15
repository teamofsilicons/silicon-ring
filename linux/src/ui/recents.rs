//! Recents: the call history, newest first.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use ring_proto::{Call, Direction};

use crate::APP_ID;
use crate::app::App;
use crate::ui::widgets;

#[derive(Clone)]
pub struct RecentsPage {
    root: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListBox,
    error: adw::StatusPage,
    calls: Rc<RefCell<HashMap<String, Call>>>,
}

/// A page with a serif title above scrolling content, as every home section has.
pub fn section_frame(title: &str, content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 16);
    column.set_margin_top(20);
    column.set_margin_bottom(24);
    column.set_margin_start(16);
    column.set_margin_end(16);
    let heading = widgets::display(title, "large");
    heading.set_xalign(0.0);
    heading.set_justify(gtk::Justification::Left);
    column.append(&heading);
    column.append(content);
    let clamp = adw::Clamp::builder()
        .maximum_size(760)
        .tightening_threshold(560)
        .child(&column)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&scroller);
    root
}

/// The faint Ring mark above a short message.
pub fn empty_state(title: &str, description: &str) -> adw::StatusPage {
    let page = adw::StatusPage::builder()
        .icon_name(APP_ID)
        .title(title)
        .description(description)
        .build();
    page.add_css_class("compact");
    page.add_css_class("ring-empty");
    page
}

impl RecentsPage {
    pub fn new() -> RecentsPage {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_valign(gtk::Align::Start);

        let loading = gtk::Spinner::builder()
            .spinning(true)
            .halign(gtk::Align::Center)
            .margin_top(48)
            .build();
        let empty = empty_state("No calls yet", "Calls you make and receive appear here.");
        let error = adw::StatusPage::builder()
            .icon_name("network-offline-symbolic")
            .title("Couldn't load your calls")
            .build();
        error.add_css_class("compact");
        let retry = gtk::Button::with_label("Try again");
        retry.add_css_class("pill");
        retry.set_halign(gtk::Align::Center);
        retry.connect_clicked(|_| App::get().refresh_recents());
        error.set_child(Some(&retry));

        let stack = gtk::Stack::new();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&error, Some("error"));
        stack.add_named(&list, Some("list"));
        stack.set_visible_child_name("loading");

        let root = section_frame("Recents", &stack);
        let calls: Rc<RefCell<HashMap<String, Call>>> = Rc::default();
        let lookup = calls.clone();
        list.connect_row_activated(move |_, row| {
            let call = lookup.borrow().get(row.widget_name().as_str()).cloned();
            if let Some(call) = call {
                crate::ui::details::present(&call);
            }
        });
        RecentsPage {
            root,
            stack,
            list,
            error,
            calls,
        }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_error(&self, message: &str) {
        if self.list.first_child().is_none() {
            self.error
                .set_description(Some(&gtk::glib::markup_escape_text(message)));
            self.stack.set_visible_child_name("error");
        }
    }

    pub fn set_calls(&self, calls: Vec<Call>, me: &str) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut index = self.calls.borrow_mut();
        index.clear();
        for call in &calls {
            self.list.append(&row(call, me));
            index.insert(call.ringid.clone(), call.clone());
        }
        self.stack
            .set_visible_child_name(if calls.is_empty() { "empty" } else { "list" });
    }
}

fn row(call: &Call, me: &str) -> gtk::ListBoxRow {
    let people = widgets::others(call, me);
    let missed = widgets::is_missed(call, me);
    let first = people
        .first()
        .cloned()
        .unwrap_or_else(|| call.created_by.clone());

    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.set_margin_top(10);
    line.set_margin_bottom(10);
    line.set_margin_start(12);
    line.set_margin_end(8);
    line.append(&widgets::avatar(&first, 40));

    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let names: Vec<&str> = people.iter().map(|a| a.label()).collect();
    let name = gtk::Label::builder()
        .label(if names.is_empty() {
            first.label().to_string()
        } else {
            names.join(", ")
        })
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name.add_css_class("heading");
    if missed {
        name.add_css_class("danger-text");
    }
    title_line.append(&name);
    if people.len() <= 1 {
        if let Some(badge) = widgets::kind_badge(&first) {
            title_line.append(&badge);
        }
    }
    text.append(&title_line);

    let meta_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = match (call.direction, missed) {
        (_, true) => "call-missed-symbolic",
        (Direction::Incoming, false) => "call-incoming-symbolic",
        (Direction::Outgoing, false) => "call-outgoing-symbolic",
    };
    let direction = gtk::Image::from_icon_name(icon);
    direction.set_pixel_size(12);
    direction.add_css_class(if missed { "danger-text" } else { "muted" });
    meta_line.append(&direction);
    let meta = widgets::pixel(&format!(
        "{} · {}",
        widgets::format_when(&call.created_at),
        widgets::outcome(call, me)
    ));
    meta.add_css_class("tiny");
    meta.add_css_class(if missed { "danger-text" } else { "muted" });
    meta_line.append(&meta);
    text.append(&meta_line);
    line.append(&text);

    let redial = gtk::Button::from_icon_name("call-start-symbolic");
    redial.add_css_class("flat");
    redial.add_css_class("circular");
    redial.set_valign(gtk::Align::Center);
    redial.set_tooltip_text(Some(&format!("Call {}", first.label())));
    let to = first.id.clone();
    redial.connect_clicked(move |_| App::get().start_call(to.clone()));
    line.append(&redial);

    gtk::ListBoxRow::builder()
        .child(&line)
        .activatable(true)
        .name(call.ringid.as_str())
        .build()
}

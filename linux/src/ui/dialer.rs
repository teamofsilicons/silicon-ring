//! The dialer: type a Silicon Accounts id and see who it is before calling, or pick someone
//! recent. The same dialog adds people to the current call.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use ring_proto::{Account, AccountLookup, RecentPerson, normalize_actor_ref};

use crate::app::App;
use crate::runtime;
use crate::ui::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Call,
    Invite,
}

struct Dialer {
    mode: Mode,
    dialog: glib::WeakRef<adw::Dialog>,
    results: gtk::Stack,
    card: gtk::Box,
    not_found: gtk::Label,
    recents: gtk::ListBox,
    recents_heading: gtk::Label,
    generation: Cell<u64>,
    found: RefCell<Option<Account>>,
}

fn action_button(mode: Mode, account: &Account) -> gtk::Button {
    let (icon, tip) = match mode {
        Mode::Call => ("call-start-symbolic", format!("Call {}", account.label())),
        Mode::Invite => (
            "contact-new-symbolic",
            format!("Add {} to the call", account.label()),
        ),
    };
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(&tip));
    button.set_valign(gtk::Align::Center);
    button
}

impl Dialer {
    fn choose(&self, account: &Account) {
        if let Some(dialog) = self.dialog.upgrade() {
            dialog.close();
        }
        match self.mode {
            Mode::Call => App::get().start_call(account.id.clone()),
            Mode::Invite => App::get().invite(account.id.clone()),
        }
    }

    fn show_card(self: &Rc<Self>, lookup: &AccountLookup) {
        while let Some(child) = self.card.first_child() {
            self.card.remove(&child);
        }
        let account = &lookup.account;
        self.card.append(&widgets::avatar(account, 48));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let name = widgets::display(account.label(), "small");
        name.set_xalign(0.0);
        name.set_justify(gtk::Justification::Left);
        text.append(&name);
        let id = widgets::pixel_id(&account.id);
        id.add_css_class("muted");
        text.append(&id);
        let badges = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if let Some(badge) = widgets::kind_badge(account) {
            badges.append(&badge);
        }
        badges.append(&if lookup.on_ring {
            widgets::badge("On Ring", "on-ring")
        } else {
            widgets::badge("Not on Ring yet", "")
        });
        text.append(&badges);
        self.card.append(&text);
        let button = action_button(self.mode, account);
        button.add_css_class("suggested-action");
        button.add_css_class("circular");
        let this = Rc::downgrade(self);
        let chosen = account.clone();
        button.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.choose(&chosen);
            }
        });
        self.card.append(&button);
        self.found.replace(Some(account.clone()));
        self.results.set_visible_child_name("card");
    }

    fn show_recents(self: &Rc<Self>, people: Vec<RecentPerson>) {
        while let Some(child) = self.recents.first_child() {
            self.recents.remove(&child);
        }
        let me = App::get().my_uuid();
        // When adding to a call, leave out whoever is already on it.
        let in_call: Vec<String> = match self.mode {
            Mode::Invite => App::get()
                .active_call()
                .map(|c| c.joined().map(|p| p.account.uuid.clone()).collect())
                .unwrap_or_default(),
            Mode::Call => Vec::new(),
        };
        let mut shown = 0;
        for person in people
            .into_iter()
            .filter(|p| p.account.uuid != me && !in_call.contains(&p.account.uuid))
        {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(person.account.label()))
                .subtitle(&person.account.id)
                .activatable(true)
                .build();
            row.add_prefix(&widgets::avatar(&person.account, 36));
            if let Some(badge) = widgets::kind_badge(&person.account) {
                row.add_suffix(&badge);
            }
            let button = action_button(self.mode, &person.account);
            button.add_css_class("flat");
            button.add_css_class("circular");
            let this = Rc::downgrade(self);
            let account = person.account.clone();
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.choose(&account);
                }
            });
            row.add_suffix(&button);
            let this = Rc::downgrade(self);
            let account = person.account.clone();
            row.connect_activated(move |_| {
                if let Some(this) = this.upgrade() {
                    this.choose(&account);
                }
            });
            self.recents.append(&row);
            shown += 1;
        }
        self.recents.set_visible(shown > 0);
        self.recents_heading.set_visible(shown > 0);
    }

    fn search(self: &Rc<Self>, text: &str) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.found.replace(None);
        let query = text.trim().to_string();
        let client = App::get().client();

        // Recent people, filtered by what was typed.
        let this = self.clone();
        let c = client.clone();
        let q = query.clone();
        glib::spawn_future_local(async move {
            let filter = (!q.is_empty()).then_some(q);
            let result =
                runtime::run(async move { c.recent_people(filter.as_deref(), Some(20)).await })
                    .await;
            if this.generation.get() == generation {
                if let Ok(people) = result {
                    this.show_recents(people);
                }
            }
        });

        if query.is_empty() {
            self.results.set_visible_child_name("hint");
            return;
        }
        let Some(reference) = normalize_actor_ref(&query) else {
            self.not_found
                .set_label("Enter a full id such as c:alice or si:tos.");
            self.results.set_visible_child_name(if query.contains(':') {
                "notfound"
            } else {
                "hint"
            });
            return;
        };
        self.results.set_visible_child_name("loading");
        let this = self.clone();
        glib::spawn_future_local(async move {
            let r = reference.clone();
            let result = runtime::run(async move { client.account(&r).await }).await;
            if this.generation.get() != generation {
                return;
            }
            match result {
                Ok(lookup) => this.show_card(&lookup),
                Err(err) if err.status() == Some(404) => {
                    this.not_found
                        .set_label(&format!("No one is called {reference}."));
                    this.results.set_visible_child_name("notfound");
                }
                Err(err) => {
                    this.not_found.set_label(&widgets::error_text(&err));
                    this.results.set_visible_child_name("notfound");
                }
            }
        });
    }
}

pub fn present(mode: Mode) {
    let app = App::get();
    if mode == Mode::Invite && !app.has_active_call() {
        return;
    }
    let title = match mode {
        Mode::Call => "New call",
        Mode::Invite => "Add to call",
    };

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("c:handle or si:handle")
        .search_delay(300)
        .hexpand(true)
        .build();

    let hint = widgets::label(
        "Type a Silicon Accounts id, like c:alice or si:tos.",
        &["muted"],
    );
    let loading = gtk::Spinner::builder()
        .spinning(true)
        .halign(gtk::Align::Center)
        .build();
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    card.add_css_class("card");
    card.add_css_class("lookup-card");
    let not_found = widgets::label("", &["muted"]);
    let results = gtk::Stack::builder()
        .vhomogeneous(false)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    results.add_named(&hint, Some("hint"));
    results.add_named(&loading, Some("loading"));
    results.add_named(&card, Some("card"));
    results.add_named(&not_found, Some("notfound"));

    let recents_heading = widgets::pixel("Recent");
    recents_heading.add_css_class("muted");
    recents_heading.add_css_class("tiny");
    recents_heading.set_margin_top(8);
    let recents = gtk::ListBox::new();
    recents.add_css_class("boxed-list");
    recents.set_selection_mode(gtk::SelectionMode::None);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_margin_top(6);
    column.set_margin_bottom(24);
    column.set_margin_start(18);
    column.set_margin_end(18);
    column.append(&entry);
    column.append(&results);
    column.append(&recents_heading);
    column.append(&recents);

    let header = adw::HeaderBar::new();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&column)
        .build();
    view.set_content(Some(&scroller));
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(460)
        .content_height(560)
        .child(&view)
        .build();

    let dialer = Rc::new(Dialer {
        mode,
        dialog: dialog.downgrade(),
        results,
        card,
        not_found,
        recents,
        recents_heading,
        generation: Cell::new(0),
        found: RefCell::new(None),
    });
    let this = Rc::downgrade(&dialer);
    entry.connect_search_changed(move |entry| {
        if let Some(this) = this.upgrade() {
            this.search(&entry.text());
        }
    });
    let this = Rc::downgrade(&dialer);
    entry.connect_activate(move |_| {
        let Some(this) = this.upgrade() else { return };
        let found = this.found.borrow().clone();
        if let Some(account) = found {
            this.choose(&account);
        }
    });
    let weak_dialog = dialog.downgrade();
    entry.connect_stop_search(move |_| {
        if let Some(dialog) = weak_dialog.upgrade() {
            dialog.close();
        }
    });
    // Handlers hold the dialer weakly; the dialog keeps it alive until it closes.
    let holder = RefCell::new(Some(dialer.clone()));
    dialog.connect_closed(move |_| {
        holder.take();
    });

    dialer.search("");
    dialog.present(Some(&app.window().window));
    entry.grab_focus();
}

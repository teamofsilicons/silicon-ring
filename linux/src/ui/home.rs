//! Home: a sidebar (Recents, Voicemail, Settings) beside the selected section, with banners for
//! connection state and calls on other devices.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use ring_proto::{Call, DeviceRef};

use crate::app::App;
use crate::ui::recents::RecentsPage;
use crate::ui::settings::SettingsPage;
use crate::ui::voicemail::VoicemailPage;
use crate::ui::widgets;
use crate::ui::window::primary_menu_button;

const SECTIONS: [(&str, &str, &str); 3] = [
    ("recents", "Recents", "document-open-recent-symbolic"),
    ("voicemail", "Voicemail", "media-tape-symbolic"),
    ("settings", "Settings", "preferences-system-symbolic"),
];

#[derive(Clone)]
pub struct HomePage {
    pub root: adw::NavigationSplitView,
    sidebar: gtk::ListBox,
    content_page: adw::NavigationPage,
    sections: gtk::Stack,
    pub recents: RecentsPage,
    pub voicemail: VoicemailPage,
    pub settings: SettingsPage,
    offline: adw::Banner,
    elsewhere: adw::Banner,
    return_call: adw::Banner,
    unread: gtk::Label,
    elsewhere_ringid: Rc<RefCell<Option<String>>>,
}

impl HomePage {
    pub fn new() -> HomePage {
        // Sidebar
        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("navigation-sidebar");
        sidebar.add_css_class("ring-sidebar");
        let unread = gtk::Label::new(None);
        unread.add_css_class("count-badge");
        unread.set_visible(false);
        for (name, title, icon) in SECTIONS {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row_box.set_margin_top(6);
            row_box.set_margin_bottom(6);
            row_box.set_margin_start(6);
            row_box.set_margin_end(6);
            row_box.append(&gtk::Image::from_icon_name(icon));
            let label = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .build();
            row_box.append(&label);
            if name == "voicemail" {
                row_box.append(&unread);
            }
            let row = gtk::ListBoxRow::builder()
                .child(&row_box)
                .name(name)
                .build();
            sidebar.append(&row);
        }

        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        // The symbolic mark follows the text colour, so it stays legible on ink in dark mode.
        let mark = gtk::Image::from_icon_name(&format!("{}-symbolic", crate::APP_ID));
        mark.set_pixel_size(20);
        brand.append(&mark);
        let wordmark = gtk::Label::new(Some("RING"));
        wordmark.add_css_class("wordmark");
        brand.append(&wordmark);
        let sidebar_header = adw::HeaderBar::builder().title_widget(&brand).build();
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_header);
        let sidebar_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&sidebar)
            .build();
        sidebar_view.set_content(Some(&sidebar_scroll));
        let sidebar_page = adw::NavigationPage::builder()
            .title("Ring")
            .tag("sidebar")
            .child(&sidebar_view)
            .build();

        // Content
        let recents = RecentsPage::new();
        let voicemail = VoicemailPage::new();
        let settings = SettingsPage::new();
        let sections = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(150)
            .build();
        sections.add_named(recents.root(), Some("recents"));
        sections.add_named(voicemail.root(), Some("voicemail"));
        sections.add_named(settings.root(), Some("settings"));

        let new_call = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("call-start-symbolic")
                    .label("New call")
                    .build(),
            )
            .action_name("app.new-call")
            .tooltip_text("New call (Ctrl+N)")
            .build();
        new_call.add_css_class("suggested-action");
        let content_header = adw::HeaderBar::builder().show_title(false).build();
        content_header.pack_end(&primary_menu_button());
        content_header.pack_end(&new_call);

        let offline = adw::Banner::new("Reconnecting to Ring…");
        let elsewhere = adw::Banner::new("");
        // Titles carry other people's names, so they are plain text, never markup.
        elsewhere.set_use_markup(false);
        elsewhere.set_button_label(Some("Switch to this computer"));
        let return_call = adw::Banner::new("");
        return_call.set_use_markup(false);
        return_call.set_button_label(Some("Return to call"));
        return_call.connect_button_clicked(|_| App::get().show_call());

        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_header);
        content_view.add_top_bar(&return_call);
        content_view.add_top_bar(&elsewhere);
        content_view.add_top_bar(&offline);
        content_view.set_content(Some(&sections));
        let content_page = adw::NavigationPage::builder()
            .title("Recents")
            .tag("content")
            .child(&content_view)
            .build();

        let root = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content_page)
            .min_sidebar_width(200.0)
            .max_sidebar_width(260.0)
            .show_content(true)
            .build();

        let elsewhere_ringid: Rc<RefCell<Option<String>>> = Rc::default();
        let target = elsewhere_ringid.clone();
        elsewhere.connect_button_clicked(move |_| {
            if let Some(ringid) = target.borrow().clone() {
                App::get().switch_here(ringid);
            }
        });

        let page = HomePage {
            root,
            sidebar: sidebar.clone(),
            content_page,
            sections,
            recents,
            voicemail,
            settings,
            offline,
            elsewhere,
            return_call,
            unread,
            elsewhere_ringid,
        };
        let this = page.clone();
        sidebar.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                this.show_section(&row.widget_name());
            }
        });
        let this = page.clone();
        sidebar.connect_row_activated(move |_, _| this.root.set_show_content(true));
        page.select_section("recents");
        page
    }

    /// Collapses the sidebar into navigation on narrow windows.
    pub fn add_breakpoints(&self, window: &adw::ApplicationWindow) {
        let condition =
            adw::BreakpointCondition::parse("max-width: 640sp").expect("valid breakpoint");
        let breakpoint = adw::Breakpoint::new(condition);
        breakpoint.add_setter(&self.root, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(breakpoint);
    }

    fn show_section(&self, name: &str) {
        let Some((_, title, _)) = SECTIONS.iter().find(|(n, _, _)| *n == name) else {
            return;
        };
        self.sections.set_visible_child_name(name);
        self.content_page.set_title(title);
        if name == "voicemail" {
            self.voicemail.on_shown();
        }
    }

    pub fn select_section(&self, name: &str) {
        let mut index = 0;
        while let Some(row) = self.sidebar.row_at_index(index) {
            if row.widget_name() == name {
                self.sidebar.select_row(Some(&row));
                break;
            }
            index += 1;
        }
        self.root.set_show_content(true);
    }

    pub fn set_unread(&self, count: usize) {
        self.unread.set_visible(count > 0);
        self.unread.set_label(&count.to_string());
    }

    pub fn set_offline(&self, offline: bool) {
        self.offline.set_revealed(offline);
    }

    /// "On a call with Alice on iPhone · Switch to this computer".
    pub fn set_call_elsewhere(&self, call: Option<(&Call, &DeviceRef, &str)>) {
        match call {
            Some((call, device, me)) => {
                let names: Vec<String> = widgets::others(call, me)
                    .iter()
                    .map(|a| a.label().to_string())
                    .collect();
                let with = if names.is_empty() {
                    "On a call".to_string()
                } else {
                    format!("On a call with {}", names.join(", "))
                };
                self.elsewhere
                    .set_title(&format!("{with} on {}", widgets::device_label(device)));
                self.elsewhere_ringid.replace(Some(call.ringid.clone()));
                self.elsewhere.set_revealed(true);
            }
            None => {
                self.elsewhere_ringid.replace(None);
                self.elsewhere.set_revealed(false);
            }
        }
    }

    /// Shown while this device is on a call but the user is looking at home.
    pub fn set_return_to_call(&self, call: Option<&Call>, me: &str) {
        match call {
            Some(call) => {
                let names: Vec<String> = widgets::others(call, me)
                    .iter()
                    .map(|a| a.label().to_string())
                    .collect();
                self.return_call
                    .set_title(&format!("On a call with {}", names.join(", ")));
                self.return_call.set_revealed(true);
            }
            None => self.return_call.set_revealed(false),
        }
    }

    /// Forgets everything shown for the previous account.
    pub fn reset(&self) {
        self.recents.set_calls(Vec::new(), "");
        self.voicemail.set_voicemails(Vec::new());
        self.settings.reset();
        self.set_unread(0);
        self.set_call_elsewhere(None);
        self.set_return_to_call(None, "");
        self.set_offline(false);
        self.select_section("recents");
    }
}

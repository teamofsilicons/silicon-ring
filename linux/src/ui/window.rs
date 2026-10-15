//! The main window: sign in, connecting, home and the call screen, under one toast overlay.

use adw::prelude::*;
use gtk::glib;

use crate::app::App;
use crate::ui::call::CallView;
use crate::ui::home::HomePage;
use crate::ui::widgets;
use crate::{APP_ID, VERSION};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Loading,
    SignIn,
    Home,
    Call,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Loading => "loading",
            Page::SignIn => "signin",
            Page::Home => "home",
            Page::Call => "call",
        }
    }
}

#[derive(Clone)]
pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    pub signin: SignInPage,
    pub loading: LoadingPage,
    pub home: HomePage,
    pub call: CallView,
}

impl MainWindow {
    pub fn new(app: &adw::Application) -> MainWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Ring")
            .icon_name(APP_ID)
            .default_width(1000)
            .default_height(700)
            .width_request(360)
            .height_request(520)
            .build();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(180)
            .build();
        let loading = LoadingPage::new();
        let signin = SignInPage::new();
        let home = HomePage::new();
        let call = CallView::new();
        stack.add_named(&loading.root, Some(Page::Loading.name()));
        stack.add_named(&signin.root, Some(Page::SignIn.name()));
        stack.add_named(&home.root, Some(Page::Home.name()));
        stack.add_named(call.root(), Some(Page::Call.name()));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        window.set_content(Some(&toasts));
        home.add_breakpoints(&window);

        window.connect_close_request(|window| {
            if App::get().close_to_background() {
                window.set_visible(false);
                glib::Propagation::Stop
            } else {
                App::get().quit();
                glib::Propagation::Proceed
            }
        });

        MainWindow {
            window,
            toasts,
            stack,
            signin,
            loading,
            home,
            call,
        }
    }

    pub fn show(&self, page: Page) {
        self.stack.set_visible_child_name(page.name());
    }

    pub fn current_page(&self) -> Page {
        match self.stack.visible_child_name().as_deref() {
            Some("signin") => Page::SignIn,
            Some("home") => Page::Home,
            Some("call") => Page::Call,
            _ => Page::Loading,
        }
    }

    pub fn toast(&self, text: &str) {
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(text))
            .timeout(4)
            .build();
        self.toasts.add_toast(toast);
    }
}

/// The Ring mark, from the hicolor icon theme.
pub fn ring_mark(size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(APP_ID);
    image.set_pixel_size(size);
    image
}

#[derive(Clone)]
pub struct SignInPage {
    pub root: adw::ToolbarView,
    error: gtk::Label,
    button: gtk::Button,
    waiting: gtk::Box,
    manual: gtk::LinkButton,
}

impl SignInPage {
    fn new() -> SignInPage {
        let header = adw::HeaderBar::builder().show_title(false).build();
        let menu = primary_menu_button();
        header.pack_end(&menu);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.set_valign(gtk::Align::Center);
        column.set_halign(gtk::Align::Center);
        column.set_margin_start(24);
        column.set_margin_end(24);
        column.set_margin_bottom(32);

        let mark = ring_mark(88);
        mark.set_margin_bottom(16);
        column.append(&mark);

        let wordmark = gtk::Label::new(Some("RING"));
        wordmark.add_css_class("wordmark");
        column.append(&wordmark);

        let tagline = widgets::display("Calls between carbons and silicons.", "large");
        tagline.set_margin_top(20);
        tagline.set_margin_bottom(28);
        tagline.set_max_width_chars(40);
        column.append(&tagline);

        let error = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .max_width_chars(36)
            .visible(false)
            .build();
        error.add_css_class("danger-text");
        error.set_margin_bottom(12);
        column.append(&error);

        let button = gtk::Button::with_label("Continue with Silicon Accounts");
        button.add_css_class("suggested-action");
        button.add_css_class("pill");
        button.set_halign(gtk::Align::Center);
        button.connect_clicked(|_| App::get().sign_in());
        column.append(&button);

        let waiting = gtk::Box::new(gtk::Orientation::Vertical, 10);
        waiting.set_visible(false);
        waiting.set_halign(gtk::Align::Center);
        let spinner = gtk::Spinner::new();
        spinner.start();
        waiting.append(&spinner);
        let hint = widgets::label("Finish signing in in your browser.", &["secondary"]);
        hint.set_justify(gtk::Justification::Center);
        hint.set_xalign(0.5);
        waiting.append(&hint);
        let manual = gtk::LinkButton::with_label(
            "https://accounts.teamofsilicons.com",
            "Open the sign-in page",
        );
        manual.set_visible(false);
        waiting.append(&manual);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("flat");
        cancel.set_halign(gtk::Align::Center);
        cancel.connect_clicked(|_| App::get().cancel_sign_in());
        waiting.append(&cancel);
        column.append(&waiting);

        let footer = widgets::pixel("Silicons use the ring CLI · Ring sends no telemetry");
        footer.add_css_class("muted");
        footer.add_css_class("tiny");
        footer.set_halign(gtk::Align::Center);
        footer.set_margin_bottom(20);
        // One line: it fits the narrowest window (360 px), and GTK sizes wrapping labels from an
        // average glyph width that underestimates this pixel face.
        footer.set_xalign(0.5);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&column)
            .build();
        body.append(&scroller);
        body.append(&footer);

        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&body));
        SignInPage {
            root,
            error,
            button,
            waiting,
            manual,
        }
    }

    pub fn set_waiting(&self, waiting: bool) {
        self.button.set_visible(!waiting);
        self.waiting.set_visible(waiting);
        if !waiting {
            self.manual.set_visible(false);
        }
    }

    pub fn set_error(&self, error: Option<&str>) {
        self.error.set_visible(error.is_some());
        self.error.set_label(error.unwrap_or_default());
    }

    /// Shown when no browser could be launched, so the user can open the page themselves.
    pub fn show_manual_link(&self, url: &str) {
        self.manual.set_uri(url);
        self.manual.set_visible(true);
    }
}

#[derive(Clone)]
pub struct LoadingPage {
    pub root: adw::ToolbarView,
    status: adw::StatusPage,
}

impl LoadingPage {
    fn new() -> LoadingPage {
        let header = adw::HeaderBar::builder().show_title(false).build();
        header.pack_end(&primary_menu_button());

        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(24)
            .height_request(24)
            .build();
        let sign_out = gtk::Button::with_label("Sign out");
        sign_out.add_css_class("flat");
        sign_out.set_halign(gtk::Align::Center);
        sign_out.connect_clicked(|_| App::get().sign_out());
        let child = gtk::Box::new(gtk::Orientation::Vertical, 18);
        child.append(&spinner);
        child.append(&sign_out);

        let status = adw::StatusPage::builder()
            .title("Connecting to Ring…")
            .icon_name(APP_ID)
            .child(&child)
            .build();
        let root = adw::ToolbarView::new();
        root.add_top_bar(&header);
        root.set_content(Some(&status));
        LoadingPage { root, status }
    }

    pub fn set_status(&self, title: &str, description: Option<&str>) {
        self.status.set_title(title);
        // The description is markup, and error text can carry URLs with `&`.
        self.status
            .set_description(description.map(glib::markup_escape_text).as_deref());
    }
}

/// The "⋮" menu: About and Quit.
pub fn primary_menu_button() -> gtk::MenuButton {
    let menu = gtk::gio::Menu::new();
    menu.append(Some("New Call"), Some("app.new-call"));
    menu.append(Some("About Ring"), Some("app.about"));
    menu.append(Some("Quit"), Some("app.quit"));
    gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main Menu")
        .primary(true)
        .build()
}

pub fn show_about() {
    let app = App::get();
    let about = adw::AboutDialog::builder()
        .application_name("Ring")
        .application_icon(APP_ID)
        .developer_name("Team of Silicons")
        .version(VERSION)
        .website("https://ring.teamofsilicons.com")
        .issue_url("https://github.com/teamofsilicons/silicon-ring/issues")
        .license_type(gtk::License::MitX11)
        .comments("Voice calls between carbons and silicons.\n\nRing sends no telemetry.")
        .build();
    about.add_link("Source", "https://github.com/teamofsilicons/silicon-ring");
    about.add_legal_section(
        "Fonts",
        None,
        gtk::License::Custom,
        Some("BDO Grotesk, Hedvig Letters Serif and Geist Pixel are licensed under the SIL Open Font License 1.1."),
    );
    about.present(Some(&app.window().window));
}

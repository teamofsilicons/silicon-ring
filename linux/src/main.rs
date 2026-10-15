//! Silicon Ring for Linux: voice calls between carbons and silicons.
//!
//! A GTK4/libadwaita app on top of `ring-core`. It keeps running in the background while signed
//! in so the realtime connection can ring this computer, and it sends no telemetry.

mod app;
mod autostart;
mod config;
mod fonts;
mod notify;
mod runtime;
mod secret;
mod sound;
mod style;
mod ui;

use std::cell::Cell;
use std::ops::ControlFlow;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use tracing_subscriber::EnvFilter;

pub const APP_ID: &str = "com.teamofsilicons.Ring";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,ring_desktop=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    fonts::load();
    runtime::init();
    glib::set_application_name("Ring");

    let gtk_app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::empty())
        .build();
    gtk_app.add_main_option(
        "background",
        glib::Char::from(b'b'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start without opening a window (used at login)",
        None,
    );
    gtk_app.add_main_option(
        "api-url",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        "Ring API origin (default: $RING_API_URL or the production API)",
        Some("URL"),
    );
    gtk_app.add_main_option(
        "new-call",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Open the dialer",
        None,
    );
    gtk_app.add_main_option(
        "audio-devices",
        glib::Char::from(0),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "List the microphones and speakers Ring can use, then exit",
        None,
    );
    gtk_app.add_main_option(
        "version",
        glib::Char::from(b'V'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Print the version and exit",
        None,
    );

    let background = Rc::new(Cell::new(false));
    let api_url = Rc::new(std::cell::RefCell::new(None::<String>));
    {
        let background = background.clone();
        let api_url = api_url.clone();
        gtk_app.connect_handle_local_options(move |app, options| {
            if options.contains("version") {
                println!("ring-desktop {VERSION}");
                return ControlFlow::Break(glib::ExitCode::SUCCESS);
            }
            if options.contains("audio-devices") {
                return ControlFlow::Break(print_audio_devices());
            }
            // Registering below runs `startup`, which reads these, so they are set first.
            let wants_background = options.contains("background");
            background.set(wants_background);
            if let Ok(Some(url)) = options.lookup::<String>("api-url") {
                api_url.replace(Some(url));
            }
            if options.contains("new-call") && app.register(None::<&gio::Cancellable>).is_ok() {
                app.activate();
                app.activate_action("new-call", None);
                if app.is_remote() {
                    return ControlFlow::Break(glib::ExitCode::SUCCESS);
                }
            }
            // A second launch with --background (e.g. a duplicate autostart) has nothing to do.
            if wants_background
                && app.register(None::<&gio::Cancellable>).is_ok()
                && app.is_remote()
            {
                return ControlFlow::Break(glib::ExitCode::SUCCESS);
            }
            ControlFlow::Continue(())
        });
    }

    {
        let background = background.clone();
        let api_url = api_url.clone();
        gtk_app.connect_startup(move |gtk_app| {
            style::install();
            fonts::check();
            app::install_actions(gtk_app);
            let api_url = api_url
                .borrow()
                .clone()
                .or_else(|| std::env::var("RING_API_URL").ok().filter(|u| !u.is_empty()))
                .unwrap_or_else(|| ring_core::DEFAULT_API_URL.to_string());
            tracing::info!("Ring {VERSION} using {api_url}");
            match app::App::new(
                gtk_app,
                app::Options {
                    api_url: api_url.clone(),
                    background: background.get(),
                },
            ) {
                Ok(app) => app.start(),
                Err(err) => {
                    eprintln!("ring-desktop: invalid API URL {api_url}: {err}");
                    gtk_app.quit();
                }
            }
        });
    }

    let first_activation = Rc::new(Cell::new(true));
    gtk_app.connect_activate(move |_| {
        let Some(app) = app::App::try_get() else {
            return;
        };
        let first = first_activation.replace(false);
        if first && app.background_start() {
            return;
        }
        app.present();
    });

    gtk_app.run()
}

/// `--audio-devices`: what libwebrtc's device module sees (PulseAudio, or ALSA without it).
fn print_audio_devices() -> glib::ExitCode {
    let _guard = runtime::enter();
    let mut ok = true;
    for (title, devices) in [
        ("Microphones", ring_core::media::input_devices()),
        ("Speakers", ring_core::media::output_devices()),
    ] {
        println!("{title}:");
        match devices {
            Ok(devices) if devices.is_empty() => println!("  none"),
            Ok(devices) => {
                for device in devices {
                    let default = if device.is_default { " (default)" } else { "" };
                    // PulseAudio and ALSA devices have no id; only the system default is usable.
                    match device.id.as_str() {
                        "" => println!("  {}{default}", device.name),
                        id => println!("  {}{default}  [{id}]", device.name),
                    }
                }
            }
            Err(err) => {
                ok = false;
                println!("  unavailable: {err}");
            }
        }
    }
    if ok {
        glib::ExitCode::SUCCESS
    } else {
        glib::ExitCode::FAILURE
    }
}

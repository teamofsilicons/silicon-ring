mod args;
mod audio;
mod daemon;
mod platform;
mod store;
mod update;
use args::*;
use base64::Engine;
use clap::{CommandFactory, Parser};
use ring_client::{actor_id, checked_text, duration, Result, RingError};
use serde_json::{json, Value};
use std::{fs, path::Path};
use store::{invalid, io_error, Store};

#[tokio::main]
async fn main() {
    let mut argv = std::env::args().collect::<Vec<_>>();
    if argv.last().is_some_and(|s| {
        matches!(
            s.as_str(),
            "profile"
                | "voice"
                | "device"
                | "audio"
                | "config"
                | "context"
                | "call"
                | "delegation"
                | "voicemail"
                | "greeting"
                | "notifications"
                | "daemon"
                | "update"
                | "bug"
        )
    }) {
        argv.push("--help".into());
    }
    let mut cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                error.exit();
            }
            let mut message = error.to_string();
            if let Some(pos) = argv.iter().position(|s| s == "login") {
                if let Some(token) = argv
                    .get(pos + 1)
                    .filter(|s| s.as_str() != "status" && !s.starts_with('-'))
                {
                    message = message.replace(token, "[REDACTED]");
                }
            }
            let failure = invalid(message, "parse arguments");
            if argv.iter().any(|s| s == "--json") {
                eprintln!("{}", json!({"ok":false,"error":failure}));
            } else {
                eprintln!("{failure}");
            }
            std::process::exit(2);
        }
    };
    if !cli.json {
        if let Some(home) = std::env::var_os("SILICON_HOME") {
            if let Ok(local) =
                store::read_json(&std::path::PathBuf::from(home).join(".ring/config.json"))
            {
                cli.json = local["output"] == "json";
            }
        }
    }
    if cli.command.is_none() {
        let _ = Cli::command().print_long_help();
        println!();
        return;
    }
    let output_json = cli.json;
    match execute(cli).await {
        Ok(Some(mut value)) => {
            store::redact(&mut value);
            if output_json {
                println!("{value}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            }
        }
        Ok(None) => {}
        Err(mut e) => {
            if let Some(details) = e.details.as_mut() {
                store::redact(details);
            }
            if output_json {
                eprintln!("{}", json!({"ok":false,"error":e}));
            } else {
                eprintln!("{e}");
                if let Some(v) = e.details.as_ref() {
                    eprintln!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
                }
            }
            std::process::exit(e.exit_code());
        }
    }
}
struct App {
    store: Store,
    id: String,
    json: bool,
}
impl App {
    async fn req(&self, method: &str, params: Value) -> Result<Value> {
        self.step("operation", method, params).await
    }
    async fn step(&self, step: &str, method: &str, params: Value) -> Result<Value> {
        daemon::ensure(&self.store).await?;
        let id = format!("{}:{step}", self.id);
        let mut result = daemon::ipc(&self.store, &id, method, params).await;
        if let Err(e) = &mut result {
            if e.request_id.is_none() {
                e.request_id = Some(self.id.clone());
            }
            if e.step == "server" {
                e.step = method.into();
            }
        }
        result
    }
    fn actor(&self, value: &str) -> Result<String> {
        actor_id(value, self.store.org.as_deref())
    }
    async fn upload(&self, path: &str, purpose: &str) -> Result<Value> {
        let bytes = fs::read(path).map_err(|e| io_error(e, "upload file"))?;
        if bytes.len() > 50 * 1024 * 1024 {
            return Err(invalid("Upload exceeds 50 MiB", "upload"));
        }
        let mime = match Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "wav" => "audio/wav",
            "mp3" => "audio/mpeg",
            _ => {
                return Err(invalid(
                    "Use PNG/JPEG/WebP photos or WAV/MP3 greeting audio",
                    "upload",
                ))
            }
        };
        self.step("upload","local.upload",json!({"purpose":purpose,"mime_type":mime,"data_base64":base64::engine::general_purpose::STANDARD.encode(bytes)})).await
    }
    async fn playback(&self, mut value: Value, options: &Playback) -> Result<Value> {
        if !options.play && options.audio_out.is_none() {
            return Ok(value);
        }
        let asset = value["audio_asset_id"]
            .as_str()
            .or_else(|| value["asset_id"].as_str())
            .or_else(|| value["audio"]["asset_id"].as_str())
            .ok_or_else(|| {
                RingError::new(
                    "AUDIO_NOT_READY",
                    "No playable audio asset is ready",
                    "playback",
                    "Inspect the synthesis/recording status and retry when ready.",
                )
            })?;
        let audio = self
            .step("download", "local.download", json!({"asset_id":asset}))
            .await?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(audio["data_base64"].as_str().unwrap_or(""))
            .map_err(|e| io_error(e, "decode audio"))?;
        let path = options
            .audio_out
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                self.store
                    .path(&format!("play-{}.wav", uuid::Uuid::new_v4()))
            });
        store::write_output(&path, &bytes, options.overwrite)?;
        if options.play {
            play_file(&path)?;
            let _ = fs::remove_file(&path);
        } else {
            value["saved_to"] = json!(path);
        }
        Ok(value)
    }
    async fn start(
        &self,
        action: &str,
        target: &str,
        invitation: &Option<String>,
        options: &CallStart,
    ) -> Result<Value> {
        if let Some(text) = &options.context.context {
            checked_text(text, 400, "context")?;
        }
        if let Some(text) = &options.start {
            checked_text(text, 100, "start")?;
        }
        let status = self.step("identity", "auth.status", json!({})).await?;
        let actor = status["actor"]
            .as_str()
            .or_else(|| status["actor_id"].as_str())
            .unwrap_or("");
        let silicon = actor.starts_with("si:");
        let mut params = if action == "init" {
            json!({"target":self.actor(target)?})
        } else {
            json!({"ringid":target})
        };
        put(&mut params, "invitation_id", invitation);
        let mut preparation = None;
        if silicon {
            let mut p = params.clone();
            p["action"] = json!(action);
            context_fields(&mut p, &options.context);
            let preview = self.step("prepare", "calls.prepare", p).await?;
            params["preparation_id"] = preview["preparation_id"].clone();
            preparation = Some(preview);
        } else {
            if options.context.context.is_some()
                || options.context.context_mode.is_some()
                || options.start.is_some()
            {
                return Err(invalid(
                    "Context and starting words are silicon-only",
                    "call",
                ));
            }
            self.step("audio-prepare", "local.audio.prepare", json!({}))
                .await?;
        }
        put(&mut params, "start", &options.start);
        put(&mut params, "device_id", &options.device);
        let result = self
            .step("execute", &format!("calls.{action}"), params)
            .await;
        let result = match result {
            Ok(v) => v,
            Err(mut e) => {
                if !silicon {
                    let _ = self
                        .step("audio-cancel", "local.audio.cancel", json!({}))
                        .await;
                }
                if e.code == "CONTEXT_APPROVAL_REQUIRED" {
                    e.details = preparation;
                    e.next_action="Inspect the displayed default and effective context; run ring context approve PREPARATION_ID --for 1h, then repeat the original command.".into();
                }
                return Err(e);
            }
        };
        if !silicon {
            self.step(
                "audio-bind",
                "local.audio.bind",
                json!({"ringid":result["ringid"]}),
            )
            .await?;
        }
        if options.live {
            println!("{result}");
            let ringid = result["ringid"].as_str().unwrap_or(target);
            daemon::watch(
                &self.store,
                &format!("{}:watch", self.id),
                json!({"ringid":ringid}),
                self.json,
            )
            .await?;
        }
        Ok(result)
    }
}
fn put(value: &mut Value, key: &str, text: &Option<String>) {
    if let Some(text) = text {
        value[key] = json!(text);
    }
}
fn page(value: &mut Value, p: &Pagination) {
    value["limit"] = json!(p.limit);
    put(value, "cursor", &p.cursor);
}
fn context_fields(value: &mut Value, c: &ContextArgs) {
    put(value, "context", &c.context);
    put(value, "context_mode", &c.context_mode);
}
fn filters(app: &App, filters: &Filters) -> Result<Value> {
    let mut v = json!({});
    if let Some(a) = &filters.actor {
        v["actor"] = json!(app.actor(a)?);
    }
    for (key, value) in [("since", &filters.since), ("until", &filters.until)] {
        if let Some(value) = value {
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| invalid(format!("{key} must be an RFC 3339 timestamp"), key))?;
            v[key] = json!(value);
        }
    }
    if let (Some(since), Some(until)) = (&filters.since, &filters.until) {
        if chrono::DateTime::parse_from_rfc3339(since).unwrap()
            >= chrono::DateTime::parse_from_rfc3339(until).unwrap()
        {
            return Err(invalid("since must be earlier than until", "time filter"));
        }
    }
    page(&mut v, &filters.page);
    Ok(v)
}

async fn execute(cli: Cli) -> Result<Option<Value>> {
    let command = cli.command.unwrap();
    if matches!(command, Command::Version) {
        return Ok(Some(
            json!({"cli":env!("CARGO_PKG_VERSION"),"daemon":env!("CARGO_PKG_VERSION"),"protocol_major":1,"platform":std::env::consts::OS,"arch":std::env::consts::ARCH}),
        ));
    }
    if let Command::Login(Login {
        token: None,
        token_stdin: false,
        command: None,
    }) = &command
    {
        let mut root = Cli::command();
        root.find_subcommand_mut("login")
            .unwrap()
            .print_long_help()
            .map_err(|e| io_error(e, "help"))?;
        println!();
        return Ok(None);
    }
    let app = App {
        store: Store::new(cli.org, cli.test)?,
        id: cli
            .request_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        json: cli.json,
    };
    let mut result = match command {
        Command::Version => unreachable!(),
        Command::Iam => app.req("app.info", json!({})).await?,
        Command::Login(login) => match login.command {
            Some(LoginSub::Status) => {
                let session = app.store.read("session.json")?;
                if session["session_token"].as_str().is_none() {
                    json!({"authenticated":false,"connected":false})
                } else {
                    match app.req("auth.status", json!({})).await {
                        Ok(mut v) => {
                            v["connected"] = json!(true);
                            v
                        }
                        Err(e) if e.exit_code() == 5 => {
                            json!({"authenticated":false,"verified_online":false,"connected":false,"cached_identity":{"actor":session["actor"],"org_id":session["org_id"],"expires_at":session["expires_at"]},"error":e})
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            None => {
                let token = if login.token_stdin {
                    store::read_text(&None, &Some("-".into()))?
                        .trim()
                        .to_owned()
                } else {
                    login.token.unwrap_or_default()
                };
                if token.is_empty() {
                    return Err(invalid("IAM token is empty", "login"));
                }
                app.req("auth.login", json!({"token":token})).await?
            }
        },
        Command::Logout { all_devices } => {
            app.req("auth.logout", json!({"all_devices":all_devices}))
                .await?
        }
        Command::Profile(profile) => match profile {
            Profile::Show { actor } => {
                let mut p = json!({});
                if let Some(actor) = actor {
                    p["actor"] = json!(app.actor(&actor)?);
                }
                app.req("profile.get", p).await?
            }
            Profile::Set {
                display_name,
                photo,
                clear_photo,
                voice,
            } => {
                let mut p = json!({});
                put(&mut p, "display_name", &display_name);
                put(&mut p, "voice_id", &voice);
                if let Some(photo) = photo {
                    p["photo_asset_id"] =
                        app.upload(&photo, "profile_photo").await?["asset_id"].clone();
                }
                if clear_photo {
                    p["photo_asset_id"] = Value::Null;
                }
                if p.as_object().unwrap().is_empty() {
                    return Err(invalid("Choose at least one profile field", "profile set"));
                }
                app.req("profile.update", p).await?
            }
        },
        Command::Voice(Voice::Ls) => app.req("voices.list", json!({})).await?,
        Command::Device(device) => match device {
            Device::Ls => app.req("devices.list", json!({})).await?,
            Device::Revoke { id } => app.req("devices.revoke", json!({"device_id":id})).await?,
            Device::Set {
                id,
                name,
                ring_enabled,
            } => {
                let mut p = json!({"device_id":id});
                put(&mut p, "name", &name);
                if let Some(ring) = ring_enabled {
                    p["ring_enabled"] = json!(ring);
                }
                app.req("devices.update", p).await?
            }
        },
        Command::Audio(Audio::Devices) => audio::devices()?,
        Command::Config(config) => config_command(&app, config).await?,
        Command::Context(context) => match context {
            Context::Show {
                init,
                accept,
                invitation,
                context,
            } => {
                if let Some(text) = &context.context {
                    checked_text(text, 400, "context")?;
                }
                let mut p = if let Some(actor) = init {
                    json!({"action":"init","target":app.actor(&actor)?})
                } else {
                    json!({"action":"accept","ringid":accept})
                };
                put(&mut p, "invitation_id", &invitation);
                context_fields(&mut p, &context);
                app.req("calls.prepare", p).await?
            }
            Context::Approve {
                preparation_id,
                duration: input,
            } => {
                let seconds = duration(&input)?;
                if seconds > 86400 {
                    return Err(invalid(
                        "Approval duration cannot exceed 24 hours",
                        "context approve",
                    ));
                }
                app.req(
                    "context.approve",
                    json!({"preparation_id":preparation_id,"valid_for_seconds":seconds}),
                )
                .await?
            }
            Context::Ls(p) => {
                let mut v = json!({});
                page(&mut v, &p);
                app.req("context.approvals.list", v).await?
            }
            Context::Revoke { approval_id } => {
                app.req(
                    "context.approvals.revoke",
                    json!({"approval_id":approval_id}),
                )
                .await?
            }
        },
        Command::Call(call) => match call {
            Call::Init { actor, options } => app.start("init", &actor, &None, &options).await?,
            Call::Accept {
                ringid,
                invitation,
                options,
            } => app.start("accept", &ringid, &invitation, &options).await?,
            Call::Decline {
                ringid,
                invitation,
                reason,
                give_no_reason,
            } => {
                let mut p = json!({"ringid":ringid,"give_no_reason":give_no_reason});
                put(&mut p, "invitation_id", &invitation);
                put(&mut p, "reason", &reason);
                app.req("calls.decline", p).await?
            }
            Call::Cut { ringid } => app.req("calls.cut", json!({"ringid":ringid})).await?,
            Call::Invite { actor, ringid } => {
                app.req(
                    "calls.invite",
                    json!({"target":app.actor(&actor)?,"ringid":ringid}),
                )
                .await?
            }
            Call::Silence { ringid, invitation } => {
                let mut p = json!({"ringid":ringid});
                put(&mut p, "invitation_id", &invitation);
                app.req("calls.silence", p).await?
            }
            Call::Show { ringid } => app.req("calls.get", json!({"ringid":ringid})).await?,
            Call::History {
                state,
                direction,
                filters: f,
            } => {
                let mut p = filters(&app, &f)?;
                put(&mut p, "state", &state);
                put(&mut p, "direction", &direction);
                app.req("calls.list", p).await?
            }
            Call::Transcript {
                ringid,
                kind,
                after_seq,
                page: p,
            } => {
                let mut v = json!({"ringid":ringid});
                page(&mut v, &p);
                put(&mut v, "kind", &kind);
                if let Some(seq) = after_seq {
                    v["after_seq"] = json!(seq);
                }
                app.req("transcript.list", v).await?
            }
            Call::Recording { ringid, playback } => {
                app.playback(
                    app.req("recordings.get", json!({"ringid":ringid})).await?,
                    &playback,
                )
                .await?
            }
            Call::Watch { ringid, after_seq } => {
                daemon::ensure(&app.store).await?;
                let mut p = json!({"ringid":ringid});
                if let Some(seq) = after_seq {
                    p["after_seq"] = json!(seq);
                }
                daemon::watch(&app.store, &app.id, p, app.json).await?;
                return Ok(None);
            }
            Call::Handoff { ringid, to_device } => {
                let identity = app.step("identity", "auth.status", json!({})).await?;
                let local = identity["device_id"] == to_device;
                if local {
                    app.step("audio-prepare", "local.audio.prepare", json!({}))
                        .await?;
                    if let Err(e) = app
                        .step(
                            "audio-standby",
                            "local.audio.attach",
                            json!({"ringid":ringid}),
                        )
                        .await
                    {
                        let _ = app
                            .step("audio-cancel", "local.audio.cancel", json!({}))
                            .await;
                        return Err(e);
                    }
                }
                let result = app
                    .req(
                        "calls.handoff",
                        json!({"ringid":ringid,"to_device_id":to_device}),
                    )
                    .await;
                if local && result.is_err() {
                    let _ = app
                        .step("audio-cancel", "local.audio.cancel", json!({}))
                        .await;
                }
                result?
            }
            Call::Mute { ringid } => {
                app.req("local.audio.mute", json!({"ringid":ringid,"muted":true}))
                    .await?
            }
            Call::Unmute { ringid } => {
                app.req("local.audio.mute", json!({"ringid":ringid,"muted":false}))
                    .await?
            }
        },
        Command::Send(send) => {
            let text = store::read_text(&send.text, &send.text_file)?;
            checked_text(&text, 160, "text")?;
            app.req("representative.send",json!({"ringid":send.ringid,"kind":send.kind,"text":text,"delegation_id":send.delegation_id})).await?
        }
        Command::Delegation(delegation) => match delegation {
            Delegation::Show { id } => {
                app.req("delegations.get", json!({"delegation_id":id}))
                    .await?
            }
            Delegation::Ls {
                ringid,
                status,
                page: p,
            } => {
                let mut v = json!({"ringid":ringid});
                page(&mut v, &p);
                put(&mut v, "status", &status);
                app.req("delegations.list", v).await?
            }
        },
        Command::Voicemail(voicemail) => voicemail_command(&app, voicemail).await?,
        Command::Notifications(notifications) => match notifications {
            Notifications::Authorize {
                authorization_id,
                code,
            } => {
                let mut p = json!({});
                put(&mut p, "authorization_id", &authorization_id);
                put(&mut p, "code", &code);
                app.req("notifications.authorize", p).await?
            }
            Notifications::Status => app.req("notifications.status", json!({})).await?,
            Notifications::Retry { id } => {
                let mut p = json!({});
                put(&mut p, "notification_id", &id);
                app.req("notifications.retry", p).await?
            }
        },
        Command::Daemon(command) => match command {
            Daemon::Run => {
                daemon::run(app.store.clone()).await?;
                return Ok(None);
            }
            Daemon::Start => {
                daemon::ensure(&app.store).await?;
                daemon::ipc(&app.store, &app.id, "local.status", json!({})).await?
            }
            Daemon::Status => daemon::ipc(&app.store, &app.id, "local.status", json!({}))
                .await
                .unwrap_or(json!({"running":false})),
            Daemon::Stop => daemon::ipc(&app.store, &app.id, "local.stop", json!({})).await?,
            Daemon::Logs {
                since,
                limit,
                follow,
            } => {
                logs(&app, &since, limit, follow).await?;
                return Ok(None);
            }
        },
        Command::Update(command) => {
            let release=app.req("release.info",json!({"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"current_version":env!("CARGO_PKG_VERSION"),"channel":app.store.local()?["updates.channel"].as_str().unwrap_or("stable")})).await?;
            match command {
                Update::Check => release,
                Update::Apply => {
                    let status = daemon::ipc(
                        &app.store,
                        &format!("{}:media", app.id),
                        "local.status",
                        json!({}),
                    )
                    .await?;
                    update::apply(&release, !status["active_media"].is_null()).await?
                }
            }
        }
        Command::Doctor { output, overwrite } => {
            let mut checks = json!({"local_storage":"ready","protocol_major":1,"version":env!("CARGO_PKG_VERSION"),"realm":if app.store.test{"test"}else{"production"}});
            for (key, method) in [
                ("app", "app.info"),
                ("auth", "auth.status"),
                ("notifications", "notifications.status"),
            ] {
                checks[key] = match app.step(key, method, json!({})).await {
                    Ok(v) => v,
                    Err(e) => json!({"error":e}),
                };
            }
            store::redact(&mut checks);
            if let Some(path) = output {
                store::write_output(
                    Path::new(&path),
                    &serde_json::to_vec_pretty(&checks).unwrap(),
                    overwrite,
                )?;
                checks["saved_to"] = json!(path);
            }
            checks
        }
        Command::Bug(Bug::Report {
            title,
            description,
            description_file,
            reproduction,
            expected,
            actual,
            pr,
        }) => {
            let description = store::read_text(&description, &description_file)?;
            if title.trim().is_empty() || description.trim().is_empty() {
                return Err(invalid(
                    "Title and description cannot be empty",
                    "bug report",
                ));
            }
            let mut p = json!({"title":title,"description":description});
            put(&mut p, "reproduction", &reproduction);
            put(&mut p, "expected", &expected);
            put(&mut p, "actual", &actual);
            put(&mut p, "pr_url", &pr);
            app.req("bugs.submit", p).await?
        }
    };
    if let Some(obj) = result.as_object_mut() {
        obj.insert("request_id".into(), json!(app.id));
    }
    Ok(Some(result))
}
async fn config_command(app: &App, command: Config) -> Result<Value> {
    match command {
        Config::Show { scope } => {
            if scope == "local" {
                Ok(
                    json!({"scope":"local","values":app.store.local()?,"defaults":{"server_url":"ws://127.0.0.1:8765/ws","output":"text","audio.input":null,"audio.output":null,"updates.channel":"stable","telemetry.enabled":true},"schema":{"server_url":"WebSocket URL; remote requires TLS","output":["text","json"],"audio.input":"OS device ID/name or null","audio.output":"OS device ID/name or null","updates.channel":["stable","beta"],"telemetry.enabled":"boolean"}}),
                )
            } else {
                app.req("config.get", json!({"scope":scope})).await
            }
        }
        Config::Set {
            values,
            file,
            scope,
        } => {
            let text = store::read_text(&values, &file)?;
            let values: Value = serde_json::from_str(&text).map_err(|e| {
                invalid(
                    format!("Configuration is not valid JSON: {e}"),
                    "config set",
                )
            })?;
            let object = values
                .as_object()
                .ok_or_else(|| invalid("Configuration must be a JSON object", "config set"))?;
            if scope == "local" {
                store::validate_local(&values)?;
                let mut current = app.store.local()?;
                for (k, v) in object {
                    current[k] = v.clone();
                }
                app.store.save_local(&current)?;
                Ok(
                    json!({"scope":scope,"values":current,"next_action":"Restart the daemon to apply server/audio settings."}),
                )
            } else {
                app.req("config.set", json!({"scope":scope,"values":values}))
                    .await
            }
        }
        Config::Reset { keys, all, scope } => {
            if scope == "local" {
                let mut current = app.store.local()?;
                if all {
                    current = json!({});
                } else {
                    for key in keys {
                        current.as_object_mut().unwrap().remove(&key);
                    }
                }
                app.store.save_local(&current)?;
                Ok(json!({"scope":scope,"values":current}))
            } else {
                let keys = if all {
                    let current = app
                        .step("config-before-reset", "config.get", json!({"scope":scope}))
                        .await?;
                    current["values"]
                        .as_object()
                        .or_else(|| current["saved"].as_object())
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                } else {
                    keys
                };
                app.req("config.reset", json!({"scope":scope,"keys":keys}))
                    .await
            }
        }
    }
}
async fn voicemail_command(app: &App, command: Voicemail) -> Result<Value> {
    match command {
        Voicemail::Ls { all, filters: f } => {
            let mut p = filters(app, &f)?;
            p["unread"] = json!(!all);
            app.req("voicemail.list", p).await
        }
        Voicemail::Show { id, playback } => {
            app.playback(
                app.req("voicemail.get", json!({"voicemail_id":id})).await?,
                &playback,
            )
            .await
        }
        Voicemail::Read { id } => {
            app.req("voicemail.mark", json!({"voicemail_id":id,"read":true}))
                .await
        }
        Voicemail::Unread { id } => {
            app.req("voicemail.mark", json!({"voicemail_id":id,"read":false}))
                .await
        }
        Voicemail::Delete { id } => {
            app.req("voicemail.delete", json!({"voicemail_id":id}))
                .await
        }
        Voicemail::Begin {
            ringid,
            invitation,
            format,
        } => {
            let mut p = json!({"ringid":ringid,"format":format});
            put(&mut p, "invitation_id", &invitation);
            app.req("voicemail.begin", p).await
        }
        Voicemail::Send { id, message } => {
            let text = store::read_text(&message.text, &message.text_file)?;
            app.req("voicemail.send", json!({"voicemail_id":id,"text":text}))
                .await
        }
        Voicemail::Commit { id } => {
            app.req("voicemail.commit", json!({"voicemail_id":id}))
                .await
        }
        Voicemail::Abort { id } => app.req("voicemail.abort", json!({"voicemail_id":id})).await,
        Voicemail::Leave {
            ringid,
            invitation,
            content,
        } => {
            let audio = content.record || content.audio_file.is_some();
            let text = if !audio {
                Some(store::read_text(
                    &content.message.text,
                    &content.message.text_file,
                )?)
            } else {
                None
            };
            let mut p = json!({"ringid":ringid,"format":if audio{"audio"}else{"text"}});
            put(&mut p, "invitation_id", &invitation);
            let draft = app.step("begin", "voicemail.begin", p).await?;
            let id = draft["voicemail_id"]
                .as_str()
                .ok_or_else(|| invalid("Draft response has no voicemail ID", "voicemail begin"))?;
            let result = if let Some(text) = text {
                app.step(
                    "send",
                    "voicemail.send",
                    json!({"voicemail_id":id,"text":text}),
                )
                .await?;
                let mut ready = false;
                for n in 0..120 {
                    let state = app
                        .step(
                            &format!("synthesis-{n}"),
                            "voicemail.get",
                            json!({"voicemail_id":id}),
                        )
                        .await?;
                    let status = state["synthesis_status"]
                        .as_str()
                        .or_else(|| state["status"].as_str())
                        .unwrap_or("");
                    if matches!(status, "ready" | "synthesized" | "complete")
                        || state["ready"] == true
                    {
                        ready = true;
                        break;
                    }
                    if matches!(status, "failed" | "unavailable") {
                        return Err(RingError::new("SYNTHESIS_FAILED","Voicemail speech generation failed","voicemail synthesis","Inspect ring voicemail show ID and retry only after fixing the provider."));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                if !ready {
                    return Err(RingError::new(
                        "SYNTHESIS_PENDING",
                        "Speech is still processing; draft has not been committed",
                        "voicemail synthesis",
                        "Inspect ring voicemail show ID, then ring voicemail commit ID when ready.",
                    ));
                }
                app.step("commit", "voicemail.commit", json!({"voicemail_id":id}))
                    .await
            } else {
                carbon_voicemail(app, &ringid, id, &draft, &content).await
            };
            if let Err(e) = &result {
                if !e.retryable {
                    let _ = app
                        .step("abort", "voicemail.abort", json!({"voicemail_id":id}))
                        .await;
                }
            }
            result
        }
        Voicemail::Greeting(greeting) => match greeting {
            Greeting::Show { when } => {
                let config = app.req("config.get", json!({"scope":"actor"})).await?;
                if let Some(when) = when {
                    let key = format!("voicemail.greetings.{when}");
                    Ok(
                        json!({"when":when,"greeting":config["effective"][&key],"saved":config["values"][&key]}),
                    )
                } else {
                    Ok(config)
                }
            }
            Greeting::Reset { when } => {
                app.req(
                    "config.reset",
                    json!({"scope":"actor","keys":[format!("voicemail.greetings.{when}")]}),
                )
                .await
            }
            Greeting::Set { when, content } => {
                let greeting = if content.record {
                    let pcm = record_pcm(app, content.duration.as_deref()).await?;
                    let path = app.store.path("greeting.wav");
                    store::atomic_write(&path, &wav(&pcm))?;
                    let result = app
                        .upload(path.to_str().unwrap(), "voicemail_greeting")
                        .await;
                    let _ = fs::remove_file(path);
                    json!({"asset_id":result?["asset_id"]})
                } else if let Some(path) = content.audio_file {
                    json!({"asset_id":app.upload(&path,"voicemail_greeting").await?["asset_id"]})
                } else {
                    json!({"text":store::read_text(&content.message.text,&content.message.text_file)?})
                };
                let mut values = json!({});
                values[format!("voicemail.greetings.{when}")] = greeting;
                app.req("config.set", json!({"scope":"actor","values":values}))
                    .await
            }
        },
    }
}
async fn carbon_voicemail(
    app: &App,
    ringid: &str,
    id: &str,
    draft: &Value,
    content: &VoicemailContent,
) -> Result<Value> {
    let status = app
        .step("media-before-private", "local.status", json!({}))
        .await?;
    let current = status["active_media"]["ringid"].as_str().map(str::to_owned);
    let previous = status["active_media"]["muted"].as_bool().unwrap_or(false);
    if let Some(current) = &current {
        app.step(
            "private-mute",
            "local.audio.mute",
            json!({"ringid":current,"muted":true}),
        )
        .await?;
    }
    let result=async {
        if let Some(greeting)=draft["greeting"]["asset_id"].as_str().or_else(||draft["greeting_asset_id"].as_str()){app.playback(json!({"audio_asset_id":greeting}),&Playback{play:true,audio_out:None,overwrite:false}).await?;}
        eprint!("\x07");
        let samples=if let Some(path)=&content.audio_file {read_wav(&fs::read(path).map_err(|e|io_error(e,"voicemail audio"))?)?}else{record_pcm(app,content.duration.as_deref()).await?};
        let pcm=samples.iter().flat_map(|s|s.to_le_bytes()).collect::<Vec<_>>();
        app.step("private-recording","local.voicemail.audio",json!({"ringid":ringid,"voicemail_id":id,"data_base64":base64::engine::general_purpose::STANDARD.encode(pcm)})).await?;
        app.step("commit","voicemail.commit",json!({"voicemail_id":id})).await
    }.await;
    if let Some(current) = current {
        let restored = app
            .step(
                "private-restore",
                "local.audio.mute",
                json!({"ringid":current,"muted":previous}),
            )
            .await;
        if result.is_ok() {
            restored?;
        }
    }
    result
}
async fn record_pcm(app: &App, duration_text: Option<&str>) -> Result<Vec<i16>> {
    let seconds = duration_text.map(duration).transpose()?.unwrap_or(180);
    if seconds > 180 {
        return Err(invalid("A recording cannot exceed three minutes", "record"));
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _audio = audio::Audio::start(&app.store.local()?, tx)?;
    let end = tokio::time::sleep(std::time::Duration::from_secs(seconds));
    tokio::pin!(end);
    let mut pcm = Vec::new();
    let (enter_tx, mut enter_rx) = tokio::sync::mpsc::unbounded_channel();
    if duration_text.is_none() {
        eprintln!("Recording private voicemail. Press Enter to finish; Ctrl-C aborts.");
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            let _ = enter_tx.send(());
        });
    }
    loop {
        tokio::select! {_=tokio::signal::ctrl_c()=>return Err(RingError::new("INTERRUPTED","Recording aborted","record","The draft was not committed.")),_= &mut end=>break,Some(_)=enter_rx.recv(),if duration_text.is_none()=>break,Some(chunk)=rx.recv()=>pcm.extend(chunk)}
    }
    Ok(pcm)
}
fn wav(samples: &[i16]) -> Vec<u8> {
    let size = (samples.len() * 2) as u32;
    let mut data = Vec::new();
    data.extend(b"RIFF");
    data.extend((size + 36).to_le_bytes());
    data.extend(b"WAVEfmt ");
    data.extend(16u32.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(24000u32.to_le_bytes());
    data.extend(48000u32.to_le_bytes());
    data.extend(2u16.to_le_bytes());
    data.extend(16u16.to_le_bytes());
    data.extend(b"data");
    data.extend(size.to_le_bytes());
    for s in samples {
        data.extend(s.to_le_bytes());
    }
    data
}
fn read_wav(bytes: &[u8]) -> Result<Vec<i16>> {
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid("Audio file must be a PCM WAV file", "audio file"));
    }
    let mut offset = 12usize;
    let mut valid = false;
    let mut output = None;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("Truncated WAV chunk", "audio file"))?;
        if &bytes[offset..offset + 4] == b"fmt " && size >= 16 {
            valid = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap()) == 1
                && u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap()) == 1
                && u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()) == 24000
                && u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap()) == 16;
        }
        if &bytes[offset..offset + 4] == b"data" {
            if size % 2 != 0 {
                return Err(invalid("PCM data has an incomplete sample", "audio file"));
            }
            output = Some(
                bytes[start..end]
                    .chunks_exact(2)
                    .map(|p| i16::from_le_bytes([p[0], p[1]]))
                    .collect(),
            );
        }
        offset = end + (size % 2);
    }
    if !valid {
        return Err(invalid(
            "Use signed 16-bit PCM mono WAV at 24 kHz",
            "audio file",
        ));
    }
    output.ok_or_else(|| invalid("WAV contains no audio data", "audio file"))
}
fn play_file(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile","-NonInteractive","-Command","$player=New-Object System.Media.SoundPlayer;$player.SoundLocation=$env:RING_LOCAL_PATH;$player.Load();$player.PlaySync()"]).env("RING_LOCAL_PATH",path);
        cmd
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut cmd = std::process::Command::new(if cfg!(target_os = "macos") {
            "afplay"
        } else {
            "aplay"
        });
        cmd.arg(path);
        cmd
    };
    let status = command.status().map_err(|_| {
        RingError::new(
            "AUDIO_UNAVAILABLE",
            "Could not launch the native audio player",
            "playback",
            "Install the platform audio player or use --audio-out PATH.",
        )
    })?;
    if !status.success() {
        return Err(RingError::new(
            "AUDIO_UNAVAILABLE",
            "Native audio playback failed",
            "playback",
            "Check audio permissions/format or save with --audio-out.",
        ));
    }
    Ok(())
}

async fn logs(app: &App, since: &Option<String>, limit: usize, follow: bool) -> Result<()> {
    let since = since
        .as_ref()
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .map_err(|_| invalid("since must be RFC 3339", "logs"))
        })
        .transpose()?;
    let mut seen = 0usize;
    let mut first = true;
    loop {
        let content = fs::read_to_string(app.store.path("events.jsonl")).unwrap_or_default();
        let lines = content.lines().collect::<Vec<_>>();
        let start = if first {
            lines.len().saturating_sub(limit)
        } else {
            seen
        };
        for line in lines.iter().skip(start) {
            if let Ok(mut v) = serde_json::from_str::<Value>(line) {
                if since.is_some_and(|s| {
                    v["time"]
                        .as_str()
                        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                        .is_some_and(|t| t < s)
                }) {
                    continue;
                }
                store::redact(&mut v);
                println!("{v}");
            }
        }
        seen = lines.len();
        first = false;
        if !follow {
            return Ok(());
        }
        tokio::select! {_=tokio::signal::ctrl_c()=>return Ok(()),_=tokio::time::sleep(std::time::Duration::from_millis(500))=>{}}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_contract() {
        Cli::command().debug_assert();
        assert!(Cli::try_parse_from([
            "ring",
            "send",
            "ring_1",
            "commentary",
            "x",
            "--text-file",
            "a"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "ring",
            "call",
            "transcript",
            "r",
            "--after-seq",
            "4",
            "--cursor",
            "abc"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["ring", "login", "status", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["ring", "context", "approve", "p", "--for", "1h"]).is_ok());
    }
    #[test]
    fn pcm_wav_roundtrip() {
        let samples = vec![0, 1, -1, i16::MAX, i16::MIN];
        assert_eq!(read_wav(&wav(&samples)).unwrap(), samples);
        assert!(read_wav(&[0; 44]).is_err());
    }
}

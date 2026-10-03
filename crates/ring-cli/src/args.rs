use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "ring",
    version,
    about = "Calls, conferences and voicemail for carbons and silicons",
    long_about = "Silicon Ring connects verified IAM actors within an organization. A successful init means ringing, not answered. The daemon keeps carbon audio alive after the CLI exits. Context approval is always explicit.",
    after_help = "Start: ring login --token-stdin\nThen: ring call init @c:alex\nExplore: ring call --help; ring context --help; ring doctor\nDocumentation: bundled udd/cli.md and udd/api.md. Published links: ring iam --json"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Print nonsecret machine-readable JSON; watches use JSON Lines"
    )]
    pub json: bool,
    #[arg(
        long,
        global = true,
        env = "SILICON_ORG",
        help = "Select verified IAM organization"
    )]
    pub org: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Use the isolated test realm and configured test app secret"
    )]
    pub test: bool,
    #[arg(
        long,
        global = true,
        help = "Stable ID for exact retries; changed input requires a new ID"
    )]
    pub request_id: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Discover app identity, compatibility and published links. Example: ring iam --json
    Iam,
    /// Authenticate using an IAM short-lived app token. Next: ring login status
    Login(Login),
    /// Revoke this session. Example: ring logout --all-devices
    Logout {
        #[arg(long)]
        all_devices: bool,
    },
    /// Show local build and protocol versions; works offline without SILICON_HOME
    Version,
    /// Read or update public profiles. Example: ring profile show si:alex
    #[command(subcommand)]
    Profile(Profile),
    /// Discover supported Natural Voices. Next: ring profile set --voice ID
    #[command(subcommand)]
    Voice(Voice),
    /// Inspect and manage your Ring devices. Example: ring device ls
    #[command(subcommand)]
    Device(Device),
    /// Inspect native OS audio devices and defaults. Example: ring audio devices
    #[command(subcommand)]
    Audio(Audio),
    /// Inspect or atomically update configuration. Example: ring config show --scope local
    #[command(subcommand)]
    Config(Config),
    /// Preview and explicitly approve representative defaults. Example: ring context show --init c:alex
    #[command(subcommand)]
    Context(Context),
    /// Place, answer and manage calls. Example: ring call init c:alex; ring call history
    #[command(subcommand)]
    Call(Call),
    /// Send context to your active silicon representative. Example: ring send RINGID commentary "Ready."
    Send(SendArgs),
    /// Inspect your representative's delegation requests. Example: ring delegation ls RINGID
    #[command(subcommand)]
    Delegation(Delegation),
    /// Read, leave and manage voicemail. Example: ring voicemail ls
    #[command(subcommand)]
    Voicemail(Voicemail),
    /// Inspect Ring-to-Ting delivery health. Example: ring notifications status
    #[command(subcommand)]
    Notifications(Notifications),
    /// Manage the persistent local runtime. Example: ring daemon status
    #[command(subcommand)]
    Daemon(Daemon),
    /// Check or install cryptographically verified compatible releases
    #[command(subcommand)]
    Update(Update),
    /// Collect a redacted diagnostic report, without submitting it
    Doctor {
        #[arg(long)]
        output: Option<String>,
        #[arg(long, requires = "output")]
        overwrite: bool,
    },
    /// Submit an explicit bug report; no private logs are attached
    #[command(subcommand)]
    Bug(Bug),
}
#[derive(Args, Debug)]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_precedence_over_arg = true
)]
pub struct Login {
    #[arg(
        value_name = "TOKEN",
        conflicts_with = "token_stdin",
        help = "IAM app token; prefer --token-stdin to avoid shell history"
    )]
    pub token: Option<String>,
    #[arg(long)]
    pub token_stdin: bool,
    #[command(subcommand)]
    pub command: Option<LoginSub>,
}
#[derive(Subcommand, Debug)]
pub enum LoginSub {
    /// Show verified session status, or authenticated:false when logged out
    Status,
}
#[derive(Subcommand, Debug)]
pub enum Profile {
    /// Read self or an authorized public actor profile. Example: ring profile show c:alex
    Show { actor: Option<String> },
    /// Update your profile. Voice is silicon-only. Example: ring profile set --display-name Alex
    Set {
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long, conflicts_with = "clear_photo")]
        photo: Option<String>,
        #[arg(long)]
        clear_photo: bool,
        #[arg(long)]
        voice: Option<String>,
    },
}
#[derive(Subcommand, Debug)]
pub enum Voice {
    /// List available Natural Voices and stable voice IDs
    Ls,
}
#[derive(Subcommand, Debug)]
pub enum Device {
    /// List your devices, ringing preferences and active media locations
    Ls,
    /// Change an owned device. Example: ring device set ID --ring-enabled false
    Set {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        ring_enabled: Option<bool>,
    },
    /// Revoke an owned device and its sessions/media
    Revoke { id: String },
}
#[derive(Subcommand, Debug)]
pub enum Audio {
    /// List native inputs/outputs. Configure with ring config set --scope local
    Devices,
}
#[derive(Subcommand, Debug)]
pub enum Config {
    /// Show values, defaults and schema. Example: ring config show --scope actor
    Show {
        #[arg(long,default_value="actor",value_parser=["local","actor","org"])]
        scope: String,
    },
    /// Atomic JSON partial update. Example: ring config set '{"telemetry.enabled":false}'
    Set {
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        values: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long,default_value="actor",value_parser=["local","actor","org"])]
        scope: String,
    },
    /// Remove overrides. Example: ring config reset representative.default_context
    Reset {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        keys: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long,default_value="actor",value_parser=["local","actor","org"])]
        scope: String,
    },
}
#[derive(Args, Debug, Clone, Default)]
pub struct ContextArgs {
    #[arg(
        long,
        help = "Literal per-call context; at most 400 Unicode code points"
    )]
    pub context: Option<String>,
    #[arg(long,value_parser=["append","prepend","overwrite"])]
    pub context_mode: Option<String>,
}
#[derive(Subcommand, Debug)]
pub enum Context {
    /// Prepare a preview without dialing or answering. Next: ring context approve PREPARATION_ID
    Show {
        #[arg(long, required_unless_present = "accept", conflicts_with = "accept")]
        init: Option<String>,
        #[arg(long)]
        accept: Option<String>,
        #[arg(long, requires = "accept")]
        invitation: Option<String>,
        #[command(flatten)]
        context: ContextArgs,
    },
    /// Explicitly approve a displayed default (1h by default, maximum 24h). Repeat init/accept afterward
    Approve {
        preparation_id: String,
        #[arg(long = "for", default_value = "1h")]
        duration: String,
    },
    /// List your context approvals
    Ls(Pagination),
    /// Revoke an approval; running calls retain their original context
    Revoke { approval_id: String },
}
#[derive(Args, Debug, Clone, Default)]
pub struct Pagination {
    #[arg(long,default_value_t=50,value_parser=clap::value_parser!(u16).range(1..=200))]
    pub limit: u16,
    #[arg(long)]
    pub cursor: Option<String>,
}
#[derive(Args, Debug, Default)]
pub struct Filters {
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long, help = "Inclusive RFC 3339 timestamp")]
    pub since: Option<String>,
    #[arg(long, help = "Exclusive RFC 3339 timestamp")]
    pub until: Option<String>,
    #[command(flatten)]
    pub page: Pagination,
}
#[derive(Args, Debug)]
pub struct CallStart {
    #[command(flatten)]
    pub context: ContextArgs,
    #[arg(
        long,
        help = "Silicon's first utterance after connection; at most 100 code points"
    )]
    pub start: Option<String>,
    #[arg(long, help = "Carbon's owned device ID")]
    pub device: Option<String>,
    #[arg(
        long,
        help = "Watch events; Ctrl-C detaches the view without hanging up"
    )]
    pub live: bool,
}
#[derive(Args, Debug)]
pub struct Playback {
    #[arg(long, conflicts_with = "audio_out")]
    pub play: bool,
    #[arg(long)]
    pub audio_out: Option<String>,
    #[arg(long)]
    pub overwrite: bool,
}
#[derive(Subcommand, Debug)]
pub enum Call {
    /// Start ringing an actor. Example: ring call init c:alex --context "Discuss launch"
    Init {
        actor: String,
        #[command(flatten)]
        options: CallStart,
    },
    /// Atomically accept an offer. Example: ring call accept RINGID --start "Hello"
    Accept {
        ringid: String,
        #[arg(long)]
        invitation: Option<String>,
        #[command(flatten)]
        options: CallStart,
    },
    /// Decline; silicon must give a reason or --give-no-reason
    Decline {
        ringid: String,
        #[arg(long)]
        invitation: Option<String>,
        #[arg(long, conflicts_with = "give_no_reason")]
        reason: Option<String>,
        #[arg(long)]
        give_no_reason: bool,
    },
    /// Leave as yourself or cancel an unanswered outgoing call
    Cut { ringid: String },
    /// Invite an actor to your existing conference. Example: ring call invite si:alex RINGID
    Invite { actor: String, ringid: String },
    /// Silence recipient ringing on all devices without declining
    Silence {
        ringid: String,
        #[arg(long)]
        invitation: Option<String>,
    },
    /// Show roster, offers, timing, media location and recording status
    Show { ringid: String },
    /// List active and historical calls
    History {
        #[arg(long,value_parser=["ringing","connecting","active","voicemail","ended"])]
        state: Option<String>,
        #[arg(long,value_parser=["incoming","outgoing","all"])]
        direction: Option<String>,
        #[command(flatten)]
        filters: Filters,
    },
    /// Read authorized transcript entries. Event and transcript cursors are separate
    Transcript {
        ringid: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, conflicts_with = "cursor")]
        after_seq: Option<u64>,
        #[command(flatten)]
        page: Pagination,
    },
    /// Inspect or download your authorized recording intervals
    Recording {
        ringid: String,
        #[command(flatten)]
        playback: Playback,
    },
    /// Watch authorized events; Ctrl-C unsubscribes without cutting the call
    Watch {
        ringid: String,
        #[arg(long)]
        after_seq: Option<u64>,
    },
    /// Move carbon audio to an owned ready device
    Handoff {
        ringid: String,
        #[arg(long)]
        to_device: String,
    },
    /// Mute the owned carbon microphone; incoming audio continues
    Mute { ringid: String },
    /// Unmute the owned carbon microphone
    Unmute { ringid: String },
}
#[derive(Args, Debug)]
pub struct SendArgs {
    pub ringid: String,
    #[arg(value_parser=["thinking","commentary"])]
    pub kind: String,
    #[arg(required_unless_present = "text_file", conflicts_with = "text_file")]
    pub text: Option<String>,
    #[arg(
        long,
        help = "UTF-8 file or - for stdin; the 160-code-point limit still applies"
    )]
    pub text_file: Option<String>,
    #[arg(long = "delegationid", alias = "delegation-id")]
    pub delegation_id: Option<String>,
}
#[derive(Subcommand, Debug)]
pub enum Delegation {
    /// List your representative's requests for a call
    Ls {
        ringid: String,
        #[arg(long,value_parser=["open","closed"])]
        status: Option<String>,
        #[command(flatten)]
        page: Pagination,
    },
    /// Show a request, context and linked replies
    Show { id: String },
}
#[derive(Args, Debug)]
pub struct MessageText {
    #[arg(long, conflicts_with = "text_file")]
    pub text: Option<String>,
    #[arg(long)]
    pub text_file: Option<String>,
}
#[derive(Args, Debug)]
pub struct VoicemailContent {
    #[command(flatten)]
    pub message: MessageText,
    #[arg(long,conflicts_with_all=["text","text_file","audio_file"])]
    pub record: bool,
    #[arg(long,conflicts_with_all=["text","text_file"])]
    pub audio_file: Option<String>,
    #[arg(long, requires = "record")]
    pub duration: Option<String>,
}
#[derive(Subcommand, Debug)]
pub enum Voicemail {
    /// List unread messages; --all includes read messages without marking anything
    Ls {
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        filters: Filters,
    },
    /// Show metadata/transcript and optionally play audio; does not mark read
    Show {
        id: String,
        #[command(flatten)]
        playback: Playback,
    },
    /// Explicitly mark a message read
    Read { id: String },
    /// Explicitly mark a message unread
    Unread { id: String },
    /// Delete your inbox message without deleting its call history
    Delete { id: String },
    /// Leave a private message on an eligible unanswered offer
    Leave {
        ringid: String,
        #[arg(long)]
        invitation: Option<String>,
        #[command(flatten)]
        content: VoicemailContent,
    },
    /// Start a low-level draft; next: ring voicemail send, commit or abort
    Begin {
        ringid: String,
        #[arg(long)]
        invitation: Option<String>,
        #[arg(long,value_parser=["text","audio"])]
        format: String,
    },
    /// Fill a text draft once; new text requires a new draft
    Send {
        id: String,
        #[command(flatten)]
        message: MessageText,
    },
    /// Commit a complete recording or synthesized message
    Commit { id: String },
    /// Discard an uncommitted draft
    Abort { id: String },
    /// Inspect or change reason-specific greetings
    #[command(subcommand)]
    Greeting(Greeting),
}
#[derive(Subcommand, Debug)]
pub enum Greeting {
    /// Show effective greetings; optionally choose a reason
    Show {
        #[arg(long,value_parser=["busy","declined","timeout"])]
        when: Option<String>,
    },
    /// Set a text or carbon audio greeting; previous setting survives failed uploads
    Set {
        #[arg(long,value_parser=["busy","declined","timeout"])]
        when: String,
        #[command(flatten)]
        content: VoicemailContent,
    },
    /// Restore the built-in greeting for a reason
    Reset {
        #[arg(long,value_parser=["busy","declined","timeout"])]
        when: String,
    },
}
#[derive(Subcommand, Debug)]
pub enum Notifications {
    /// Start Ting consent, then finish with the returned authorization ID and code
    Authorize {
        #[arg(long, requires = "code")]
        authorization_id: Option<String>,
        #[arg(long, requires = "authorization_id")]
        code: Option<String>,
    },
    /// Show Ring-to-Ting publication health and failures
    Status,
    /// Retry publications with original IDs; Ting owns downstream delivery
    Retry { id: Option<String> },
}
#[derive(Subcommand, Debug)]
pub enum Daemon {
    /// Start the local daemon if it is not already running
    Start,
    /// Show runtime process, connection and active stream information
    Status,
    /// Stop local media/runtime without cutting silicon calls
    Stop,
    /// Inspect redacted operational logs
    Logs {
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        follow: bool,
    },
    #[command(hide = true)]
    Run,
}
#[derive(Subcommand, Debug)]
pub enum Update {
    /// Ask the server for compatible signed release metadata
    Check,
    /// Verify and atomically install a compatible release; defers during active media
    Apply,
}
#[derive(Subcommand, Debug)]
pub enum Bug {
    /// Submit a report. Example: ring bug report --title "Issue" --description "Steps..."
    Report {
        #[arg(long)]
        title: String,
        #[arg(
            long,
            required_unless_present = "description_file",
            conflicts_with = "description_file"
        )]
        description: Option<String>,
        #[arg(long)]
        description_file: Option<String>,
        #[arg(long)]
        reproduction: Option<String>,
        #[arg(long)]
        expected: Option<String>,
        #[arg(long)]
        actual: Option<String>,
        #[arg(long)]
        pr: Option<String>,
    },
}

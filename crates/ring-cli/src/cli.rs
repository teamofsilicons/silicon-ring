//! Command-line grammar. Every command and group carries what it is for, examples and the
//! commands usually run next.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use ring_proto::{CallState, ContextMode, DelegationStatus, MessageKind, Time, VoicemailReason};

const LOGIN_ABOUT: &str = "Exchanges a short-lived token (SLT) from Silicon Accounts for a Ring session.\n\n\
The token is used once. After an uncertain failure, mint a new one. With no token argument, ring runs \
`silicon-accounts login --app ring -q` itself when silicon-accounts is installed. Signing in also turns \
on Ting notifications for Ring. Only silicons can sign in.";

const SEND_ABOUT: &str = "Feeds your live representative on RINGID.\n\n\
thinking is working context for the representative; commentary is something for it to say. Each \
message is at most 160 characters. Answer a delegation by passing its id; leave it out to send \
delegation_id null. The representative must be live (exit 5 otherwise).";

#[derive(Parser, Debug)]
#[command(
    name = "ring",
    version,
    about = "Make and take Silicon Ring voice calls as a silicon.",
    long_about = "ring is how a silicon makes and takes voice calls on Silicon Ring.\n\n\
On a call your voice is a representative that runs on the Ring server, so this CLI never touches \
audio. Every command does one job and exits. Ting brings you incoming calls, transcript batches, \
delegations and voicemail.\n\nCarbons cannot sign in here; they use the Ring apps or \
https://ring.teamofsilicons.com. Ring sends no telemetry.",
    after_help = "\
Start here:
  silicon-accounts login --app ring -q | ring login --slt-stdin
  ring login status --json
  ring call init c:alice --context \"You represent TOS. Ask me before promising dates.\" --start \"Hi Alice, it's TOS.\"
  ring call accept RINGID --start \"Hello, TOS here.\"      # after a ring.call.incoming ting
  ring send RINGID commentary \"It is sunny in Mumbai.\" --delegation-id DLGID
  ring call cut RINGID

Every command and group has --help. Documentation offline: ring docs",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Machine-readable output: JSON on stdout; errors as {"error":{...}} on stderr.
    #[arg(long, global = true)]
    pub json: bool,

    /// API origin [default: https://backend.ring.teamofsilicons.com]. A session is bound to the
    /// origin that issued it.
    #[arg(
        long,
        global = true,
        value_name = "URL",
        env = "RING_API_URL",
        hide_env_values = true
    )]
    pub api_url: Option<String>,

    /// Retry a mutation (call init, call invite, send, voicemail leave) safely with the same key.
    #[arg(long, global = true, value_name = "KEY")]
    pub idempotency_key: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show Ring's Silicon Accounts and Apps integration (offline).
    #[command(
        long_about = "Prints Ring's app id, the Accounts and Apps URLs, the API origin, the docs and source \
links, the Rust client package and this version. Works offline.",
        after_help = "Examples:\n  ring accounts --json\n\nNext:\n  ring login"
    )]
    Accounts,

    /// Sign in with a short-lived token from Silicon Accounts.
    #[command(long_about = LOGIN_ABOUT)]
    Login(LoginArgs),

    /// Revoke this session and forget it.
    #[command(
        long_about = "Signs this CLI out: revokes the session on the server (and its Accounts sign-in) and \
deletes session.json. If the server cannot be reached the local session is kept so you can retry.",
        after_help = "Examples:\n  ring logout\n\nNext:\n  ring login"
    )]
    Logout,

    /// Look up accounts: yours or anyone's id, kind, name and photo.
    #[command(
        after_help = "Examples:\n  ring profile show\n  ring profile show c:alice\n\nNext:\n  ring call init c:alice"
    )]
    Profile(Group<ProfileCommand>),

    /// Make, answer, end and inspect calls.
    #[command(
        long_about = "Make, answer, decline, end and inspect calls.\n\n\
A successful `call init` means the call is ringing, not answered. Watch it with `ring call watch RINGID`, \
or add --live to init or accept to stream the transcript until the call ends.",
        after_help = "Examples:\n  ring call init c:alice --start \"Hi Alice, it's TOS.\" --live\n  \
ring call accept RINGID --start \"Hello, TOS here.\"\n  ring call decline RINGID --reason \"In a meeting\"\n  \
ring call history --state active,ringing\n\nNext:\n  ring call watch RINGID\n  ring send RINGID commentary \"…\""
    )]
    Call(Group<CallCommand>),

    /// Feed your live representative a thinking or commentary message.
    #[command(long_about = SEND_ABOUT)]
    Send(SendArgs),

    /// Read what your representative asked you for.
    #[command(
        long_about = "Delegations are requests your representative makes when it needs you, for example \
\"check the weather in Mumbai\". Each arrives by Ting (ring.delegation.created) with the last 30 s of \
transcript. Answer with `ring send`.",
        after_help = "Examples:\n  ring delegation ls RINGID --status open\n  ring delegation show DLGID\n\n\
Next:\n  ring send RINGID commentary \"It is sunny in Mumbai.\" --delegation-id DLGID"
    )]
    Delegation(Group<DelegationCommand>),

    /// Preview, approve and revoke use of your stored default context.
    #[command(
        long_about = "Your representative can be seeded with up to 400 characters of context. A stored \
default context is never used until you approve it for a while; Ring shows you the default, the merge \
mode and the effective text first.\n\nWhen `call init` or `call accept` needs approval it exits 5 and \
prints the exact `ring context approve` command. Approve, then repeat the original command.",
        after_help = "Examples:\n  ring context show --init c:alice --context \"Discuss the launch.\"\n  \
ring context approve PREPID --for 1h\n  ring context ls\n  ring context revoke APRID\n\nNext:\n  \
ring config set '{\"representative.default_context\":\"…\"}'"
    )]
    Context(Group<ContextCommand>),

    /// Read voicemail, leave a message, and set your greetings.
    #[command(
        after_help = "Examples:\n  ring voicemail ls\n  ring voicemail show VMID\n  \
ring voicemail leave RINGID --text \"Hi Alice, call me back about the launch.\"\n  \
ring voicemail greeting set --when busy --text \"I'm on another call. Leave a message.\"\n\nNext:\n  \
ring voicemail read VMID"
    )]
    Voicemail(Group<VoicemailCommand>),

    /// Show and change your Ring settings.
    #[command(
        after_help = "Examples:\n  ring config show\n  ring config set '{\"voice\":\"vesper\"}'\n  \
ring config reset representative.default_context\n\nNext:\n  ring voice ls"
    )]
    Config(Group<ConfigCommand>),

    /// List the Natural Voices your representative can speak with.
    #[command(
        after_help = "Examples:\n  ring voice ls\n\nNext:\n  ring config set '{\"voice\":\"vesper\"}'"
    )]
    Voice(Group<VoiceCommand>),

    /// List and revoke your signed-in devices (each session is one device).
    #[command(
        after_help = "Examples:\n  ring device ls\n  ring device revoke DEVID\n\nNext:\n  ring login status"
    )]
    Device(Group<DeviceCommand>),

    /// Ting delivery of Ring events to your webhook.
    #[command(
        long_about = "Ring tells you about incoming calls, transcript batches, delegations and voicemail \
through Ting. Signing in turns this on. Configure where Ting delivers with `ting webhook URL`.",
        after_help = "Examples:\n  ring notifications status\n  ring notifications enable\n\nNext:\n  ting webhook URL"
    )]
    Notifications(Group<NotificationsCommand>),

    /// Read the bundled documentation (offline).
    #[command(
        long_about = "Prints Ring's documentation, bundled into this binary. Topics: usage (this CLI), \
understanding (the product), api (the HTTP contract), accounts (sign-in), architecture, design, and \
tree (every command).",
        after_help = "Examples:\n  ring docs\n  ring docs api\n  ring docs tree --json\n\nNext:\n  ring login"
    )]
    Docs(DocsArgs),

    /// Open a GitHub issue on teamofsilicons/silicon-ring through gh.
    #[command(
        long_about = "Opens an issue on github.com/teamofsilicons/silicon-ring with `gh issue create`, using \
your gh sign-in. Only your message (and the PR link, if given) is sent; nothing is attached \
automatically.",
        after_help = "Examples:\n  ring report \"call watch stops after a reconnect\"\n  \
ring report \"typo in docs\" --pr https://github.com/teamofsilicons/silicon-ring/pull/12\n\nNext:\n  gh issue list --repo teamofsilicons/silicon-ring --author @me"
    )]
    Report(ReportArgs),
}

/// A command group. Run without a subcommand it prints its help.
#[derive(Args, Debug)]
pub struct Group<T: Subcommand> {
    #[command(subcommand)]
    pub command: Option<T>,
}

// ---- login --------------------------------------------------------------------------------

#[derive(Args, Debug)]
#[command(
    args_conflicts_with_subcommands = true,
    after_help = "Examples:\n  silicon-accounts login --app ring -q | ring login --slt-stdin\n  ring login\n  \
ring login status --json\n\nNext:\n  ring call init c:alice\n  ring notifications status"
)]
pub struct LoginArgs {
    #[command(subcommand)]
    pub command: Option<LoginCommand>,

    /// The short-lived token.
    #[arg(value_name = "TOKEN", conflicts_with_all = ["slt", "slt_stdin", "token_stdin"])]
    pub token: Option<String>,

    /// The short-lived token, as an option.
    #[arg(long, value_name = "TOKEN", conflicts_with_all = ["slt_stdin", "token_stdin"])]
    pub slt: Option<String>,

    /// Read the token from stdin.
    #[arg(long, conflicts_with = "token_stdin")]
    pub slt_stdin: bool,

    /// Read the token from stdin (same as --slt-stdin).
    #[arg(long)]
    pub token_stdin: bool,
}

#[derive(Subcommand, Debug)]
pub enum LoginCommand {
    /// Show who is signed in. Signed out, it answers offline and exits 0.
    #[command(
        long_about = "Shows the signed-in silicon. Signed out it prints {\"authenticated\":false,\"id\":null} \
without contacting the server and exits 0. Signed in it checks the session with the server.",
        after_help = "Examples:\n  ring login status --json\n\nNext:\n  ring call history\n  ring notifications status"
    )]
    Status,
}

// ---- profile ------------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ProfileCommand {
    /// An account's id, kind, display name and photo URL (yours without ACTOR).
    #[command(
        after_help = "Examples:\n  ring profile show\n  ring profile show si:scout --json\n\nNext:\n  ring call init ACTOR"
    )]
    Show {
        /// c:handle, si:handle, @c:handle or an account uuid.
        actor: Option<String>,
    },
}

// ---- calls --------------------------------------------------------------------------------

#[derive(Args, Debug, Clone, Default)]
pub struct ContextArgs {
    /// Context for your representative: what you do, your tools, when to ask you, who you are
    /// talking to (at most 400 characters).
    #[arg(long, value_name = "TEXT")]
    pub context: Option<String>,

    /// How --context combines with your stored default context.
    #[arg(long, value_name = "MODE")]
    pub context_mode: Option<ContextModeArg>,
}

#[derive(Subcommand, Debug)]
pub enum CallCommand {
    /// Call ACTOR. Success means ringing, not answered.
    #[command(
        long_about = "Calls ACTOR (a carbon or a silicon). The call rings for 60 seconds; if it is declined, \
missed or ACTOR is busy, it goes to voicemail and Ting offers you `ring voicemail leave`.\n\n\
Your representative speaks --start first once the call is answered. If a stored default context would be \
used without approval, this exits 5 and prints the exact approve command.",
        after_help = "Examples:\n  ring call init c:alice --context \"You represent TOS. Ask me before promising dates.\" \
--start \"Hi Alice, it's TOS.\"\n  ring call init si:scout --live --json\n\nNext:\n  ring call watch RINGID\n  \
ring call cut RINGID"
    )]
    Init {
        /// Who to call: c:handle, si:handle, @c:handle or an account uuid.
        actor: String,
        #[command(flatten)]
        context: ContextArgs,
        /// What your representative says first (at most 100 characters).
        #[arg(long, value_name = "TEXT")]
        start: Option<String>,
        /// Stream the transcript and call events until the call ends or Ctrl-C (which never hangs up).
        #[arg(long)]
        live: bool,
    },

    /// Answer a ringing call.
    #[command(
        long_about = "Answers RINGID. Your representative joins the call and speaks --start first. \
--invitation is needed only when you have more than one ringing invitation on the call.",
        after_help = "Examples:\n  ring call accept RINGID --start \"Hello, TOS here.\"\n  \
ring call accept RINGID --context \"Alice runs launch ops.\" --live\n\nNext:\n  ring call watch RINGID\n  \
ring delegation ls RINGID"
    )]
    Accept {
        /// The call id (ring_…).
        ringid: String,
        /// The invitation to answer.
        #[arg(long, value_name = "ID")]
        invitation: Option<String>,
        #[command(flatten)]
        context: ContextArgs,
        /// What your representative says first (at most 100 characters).
        #[arg(long, value_name = "TEXT")]
        start: Option<String>,
        /// Stream the transcript and call events until the call ends or Ctrl-C (which never hangs up).
        #[arg(long)]
        live: bool,
    },

    /// Decline a ringing call. Silicons must give a reason or --give-no-reason.
    #[command(
        long_about = "Declines RINGID. The reason is shown to the caller and kept in the call log. On a \
direct call the caller is then offered voicemail.",
        after_help = "Examples:\n  ring call decline RINGID --reason \"In a meeting until 3pm\"\n  \
ring call decline RINGID --give-no-reason\n\nNext:\n  ring call history --direction incoming\n  ring voicemail ls"
    )]
    #[command(group = clap::ArgGroup::new("why").required(true).args(["reason", "give_no_reason"]))]
    Decline {
        /// The call id (ring_…).
        ringid: String,
        /// The invitation to decline.
        #[arg(long, value_name = "ID")]
        invitation: Option<String>,
        /// Why you are declining (at most 200 characters).
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Decline without a reason.
        #[arg(long)]
        give_no_reason: bool,
    },

    /// Leave a call, or cancel your call that nobody answered yet.
    #[command(
        after_help = "Examples:\n  ring call cut RINGID\n\nNext:\n  ring call show RINGID\n  ring call transcript RINGID"
    )]
    Cut {
        /// The call id (ring_…).
        ringid: String,
    },

    /// Invite ACTOR into a call you are on.
    #[command(
        long_about = "Offers ACTOR a place in RINGID. They see who is already on the call before they answer. \
Any joined participant may invite.",
        after_help = "Examples:\n  ring call invite c:bob RINGID\n\nNext:\n  ring call watch RINGID"
    )]
    Invite {
        /// Who to invite.
        actor: String,
        /// The call id (ring_…).
        ringid: String,
    },

    /// Stop the ringing without declining.
    #[command(
        long_about = "Stops the ringing on your side without declining. The caller keeps hearing it ring \
until the invitation expires or you answer.",
        after_help = "Examples:\n  ring call silence RINGID\n\nNext:\n  ring call accept RINGID\n  ring call decline RINGID --reason \"…\""
    )]
    Silence {
        /// The call id (ring_…).
        ringid: String,
        /// The invitation to silence.
        #[arg(long, value_name = "ID")]
        invitation: Option<String>,
    },

    /// Participants, invitations, timings and recording status of a call.
    #[command(
        after_help = "Examples:\n  ring call show RINGID --json\n\nNext:\n  ring call transcript RINGID"
    )]
    Show {
        /// The call id (ring_…).
        ringid: String,
    },

    /// Your calls, newest first.
    #[command(
        after_help = "TIME is RFC 3339 (2026-10-15T09:30:00Z), a date (2026-10-15), or an age such as 30m, 6h or 7d.\n\n\
Examples:\n  ring call history\n  ring call history --state active,ringing --json\n  \
ring call history --with c:alice --since 7d\n\nNext:\n  ring call show RINGID"
    )]
    History {
        /// Only these states (comma-separated).
        #[arg(long, value_name = "STATES", value_delimiter = ',')]
        state: Vec<CallStateArg>,
        #[arg(long, value_name = "DIRECTION")]
        direction: Option<DirectionArg>,
        /// Only calls with this actor.
        #[arg(long, value_name = "ACTOR")]
        with: Option<String>,
        #[arg(long, value_name = "TIME", value_parser = parse_time)]
        since: Option<Time>,
        #[arg(long, value_name = "TIME", value_parser = parse_time)]
        until: Option<Time>,
        /// Page size, 1–200 [default: 50].
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: Option<u32>,
        /// Continue from a previous page's next_cursor.
        #[arg(long, value_name = "CURSOR")]
        cursor: Option<String>,
    },

    /// A call's transcript, oldest first.
    #[command(
        after_help = "Examples:\n  ring call transcript RINGID\n  ring call transcript RINGID --after-seq 57 --json\n\nNext:\n  \
ring call recording RINGID"
    )]
    Transcript {
        /// The call id (ring_…).
        ringid: String,
        /// Only entries after this sequence number.
        #[arg(long, value_name = "N", default_value_t = 0)]
        after_seq: u64,
        /// Page size, 1–200 [default: 200].
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: Option<u32>,
    },

    /// A call's recording status; downloads the WAV when it is ready.
    #[command(
        after_help = "Examples:\n  ring call recording RINGID\n  ring call recording RINGID --audio-out call.wav\n\nNext:\n  ring call transcript RINGID"
    )]
    Recording {
        /// The call id (ring_…).
        ringid: String,
        /// Save the recording (WAV) here.
        #[arg(long, value_name = "PATH")]
        audio_out: Option<PathBuf>,
        /// Replace PATH if it exists.
        #[arg(long, requires = "audio_out")]
        overwrite: bool,
    },

    /// Stream a call's transcript and events until it ends or Ctrl-C.
    #[command(
        long_about = "Prints the transcript so far, then streams new entries, delegations and state changes \
until the call ends or you press Ctrl-C. Ctrl-C stops watching; it never hangs up. With --json every line \
is one JSON object {\"type\",\"data\"} using the event types of `ring docs api`, after a first \
call.snapshot line holding the call (call.created or call.accepted with --live on init or accept). The \
stream exits 0 after call.ended, and 130 on Ctrl-C.",
        after_help = "Examples:\n  ring call watch RINGID\n  ring call watch RINGID --json\n\nNext:\n  \
ring send RINGID commentary \"…\" --delegation-id DLGID\n  ring call cut RINGID"
    )]
    Watch {
        /// The call id (ring_…).
        ringid: String,
    },
}

// ---- send ---------------------------------------------------------------------------------

#[derive(Args, Debug)]
#[command(
    after_help = "Examples:\n  ring send RINGID thinking \"Looking up the forecast now.\" --delegation-id DLGID\n  \
ring send RINGID commentary \"It is sunny in Mumbai.\" --delegation-id DLGID\n  \
echo \"Tell them I'll call back at 5.\" | ring send RINGID commentary --text-file -\n\nNext:\n  \
ring delegation ls RINGID --status open"
)]
pub struct SendArgs {
    /// The call id (ring_…).
    pub ringid: String,
    /// thinking (context) or commentary (something to say).
    pub kind: MessageKindArg,
    /// The message (at most 160 characters).
    #[arg(required_unless_present = "text_file", conflicts_with = "text_file")]
    pub text: Option<String>,
    /// The delegation this answers.
    #[arg(long, value_name = "ID", visible_alias = "delegationid")]
    pub delegation_id: Option<String>,
    /// Read the message from a file, or - for stdin.
    #[arg(long, value_name = "PATH|-")]
    pub text_file: Option<String>,
}

// ---- delegations --------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum DelegationCommand {
    /// Delegations on a call.
    #[command(
        after_help = "Examples:\n  ring delegation ls RINGID --status open\n\nNext:\n  ring delegation show DLGID"
    )]
    Ls {
        /// The call id (ring_…).
        ringid: String,
        #[arg(long, value_name = "STATUS")]
        status: Option<DelegationStatusArg>,
    },
    /// One delegation, with its 30 s of context and your replies.
    #[command(
        after_help = "Examples:\n  ring delegation show DLGID --json\n\nNext:\n  ring send RINGID commentary \"…\" --delegation-id DLGID"
    )]
    Show {
        /// The delegation id (dlg_…).
        id: String,
    },
}

// ---- context ------------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ContextCommand {
    /// Show the default, supplied and effective context for a call you are about to make or take.
    #[command(after_help = "Examples:\n  ring context show --init c:alice\n  \
ring context show --accept RINGID --context \"Alice runs launch ops.\" --context-mode prepend\n\nNext:\n  \
ring context approve PREPID --for 1h")]
    #[command(group = clap::ArgGroup::new("action").required(true).args(["init", "accept"]))]
    Show {
        /// Preview for calling ACTOR.
        #[arg(long, value_name = "ACTOR")]
        init: Option<String>,
        /// Preview for answering RINGID.
        #[arg(long, value_name = "RINGID")]
        accept: Option<String>,
        #[command(flatten)]
        context: ContextArgs,
    },
    /// Approve the shown default context for a while (default 1h, at most 24h).
    #[command(
        after_help = "DURATION is like 30m, 1h, 2h30m or 24h.\n\nExamples:\n  ring context approve PREPID --for 1h\n\n\
Next:\n  repeat the command that asked for approval"
    )]
    Approve {
        /// The preparation id (prep_…) from `ring context show` or the approval prompt.
        preparation_id: String,
        #[arg(long = "for", value_name = "DURATION", default_value = "1h", value_parser = parse_approval_window)]
        valid_for: u64,
    },
    /// Your current approvals.
    #[command(after_help = "Examples:\n  ring context ls\n\nNext:\n  ring context revoke APRID")]
    Ls,
    /// Withdraw an approval.
    #[command(after_help = "Examples:\n  ring context revoke APRID\n\nNext:\n  ring context ls")]
    Revoke {
        /// The approval id (apr_…).
        approval_id: String,
    },
}

// ---- voicemail ----------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum VoicemailCommand {
    /// Voicemail you received; unread only unless --all.
    #[command(
        after_help = "Examples:\n  ring voicemail ls\n  ring voicemail ls --all --from c:alice --since 7d\n\nNext:\n  \
ring voicemail show VMID"
    )]
    Ls {
        /// Include messages you have read.
        #[arg(long)]
        all: bool,
        #[arg(long, value_name = "ACTOR")]
        from: Option<String>,
        #[arg(long, value_name = "TIME", value_parser = parse_time)]
        since: Option<Time>,
        #[arg(long, value_name = "TIME", value_parser = parse_time)]
        until: Option<Time>,
        /// Page size, 1–200 [default: 50].
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: Option<u32>,
        /// Continue from a previous page's next_cursor.
        #[arg(long, value_name = "CURSOR")]
        cursor: Option<String>,
    },
    /// One message with its transcript; optionally save the audio.
    #[command(
        after_help = "Examples:\n  ring voicemail show VMID\n  ring voicemail show VMID --audio-out message.wav\n\nNext:\n  \
ring voicemail read VMID\n  ring call init ACTOR"
    )]
    Show {
        /// The voicemail id (vm_…).
        id: String,
        /// Save the audio here when it is ready.
        #[arg(long, value_name = "PATH")]
        audio_out: Option<PathBuf>,
        /// Replace PATH if it exists.
        #[arg(long, requires = "audio_out")]
        overwrite: bool,
    },
    /// Mark a message read.
    #[command(after_help = "Examples:\n  ring voicemail read VMID\n\nNext:\n  ring voicemail ls")]
    Read {
        /// The voicemail id (vm_…).
        id: String,
    },
    /// Mark a message unread.
    #[command(after_help = "Examples:\n  ring voicemail unread VMID\n\nNext:\n  ring voicemail ls")]
    Unread {
        /// The voicemail id (vm_…).
        id: String,
    },
    /// Delete a message.
    #[command(
        after_help = "Examples:\n  ring voicemail delete VMID\n\nNext:\n  ring voicemail ls --all"
    )]
    Delete {
        /// The voicemail id (vm_…).
        id: String,
    },
    /// Leave a message after a call you made went to voicemail; it is spoken in your voice.
    #[command(
        long_about = "Leaves a text message (at most 600 characters) on RINGID, a call you made that is in \
voicemail state (Ting sends ring.voicemail.offered). It is spoken in your representative's voice and \
transcribed for the recipient.",
        after_help = "Examples:\n  ring voicemail leave RINGID --text \"Hi Alice, call me back about the launch.\"\n  \
ring voicemail leave RINGID --text-file message.txt\n\nNext:\n  ring call show RINGID"
    )]
    #[command(group = clap::ArgGroup::new("message").required(true).args(["text", "text_file"]))]
    Leave {
        /// The call id (ring_…).
        ringid: String,
        #[arg(long, value_name = "TEXT")]
        text: Option<String>,
        /// Read the message from a file, or - for stdin.
        #[arg(long, value_name = "PATH|-")]
        text_file: Option<String>,
    },
    /// What callers hear before the beep.
    #[command(
        long_about = "Your greetings for busy, declined and missed calls. Callers hear the greeting, then a \
beep, and can leave a message. A greeting is at most 300 characters and is spoken in your voice.",
        after_help = "Examples:\n  ring voicemail greeting show\n  \
ring voicemail greeting set --when busy --text \"I'm on another call. Leave a message.\"\n  \
ring voicemail greeting reset --when busy\n\nNext:\n  ring voice ls"
    )]
    Greeting(Group<GreetingCommand>),
}

#[derive(Subcommand, Debug)]
pub enum GreetingCommand {
    /// Your greetings for busy, declined and missed calls.
    #[command(
        after_help = "Examples:\n  ring voicemail greeting show\n\nNext:\n  ring voicemail greeting set --when busy --text \"…\""
    )]
    Show,
    /// Set the greeting for one case (at most 300 characters), spoken in your voice.
    #[command(
        after_help = "Examples:\n  ring voicemail greeting set --when busy --text \"I'm on another call. Leave a message.\"\n\nNext:\n  ring voicemail greeting show"
    )]
    Set {
        #[arg(long, value_name = "CASE")]
        when: GreetingCase,
        #[arg(long, value_name = "TEXT")]
        text: String,
    },
    /// Go back to the built-in greeting for one case.
    #[command(
        after_help = "Examples:\n  ring voicemail greeting reset --when declined\n\nNext:\n  ring voicemail greeting show"
    )]
    Reset {
        #[arg(long, value_name = "CASE")]
        when: GreetingCase,
    },
}

// ---- settings -----------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Your settings.
    #[command(
        after_help = "Examples:\n  ring config show --json\n\nNext:\n  ring config set '{\"voice\":\"vesper\"}'"
    )]
    Show,
    /// Change settings from a JSON object of dotted keys.
    #[command(
        long_about = "Changes settings. JSON is an object of these keys (dotted or nested):\n  \
voice                              a voice id from `ring voice ls`\n  \
representative.default_context     up to 400 characters (\"\" clears it)\n  \
representative.context_mode        append | prepend | overwrite\n  \
representative.context_required    true | false\n  \
voicemail.enabled                  true | false\n  \
voicemail.greetings.busy|declined|missed   up to 300 characters, or null for the default\n\n\
Changing the default context or its mode withdraws your context approvals.",
        after_help = "Examples:\n  ring config set '{\"voice\":\"vesper\"}'\n  \
ring config set '{\"representative.default_context\":\"I am TOS. Ask me before promising dates.\",\"representative.context_mode\":\"append\"}'\n\n\
Next:\n  ring config show\n  ring context show --init ACTOR"
    )]
    Set {
        /// A JSON object such as {"voice":"vesper"}.
        #[arg(value_name = "JSON")]
        settings: String,
    },
    /// Put settings back to their defaults.
    #[command(
        long_about = "Resets each KEY to its default. KEY is one of the keys of `ring config set`, or a \
group: representative, voicemail, voicemail.greetings.",
        after_help = "Examples:\n  ring config reset voice\n  ring config reset representative\n\nNext:\n  ring config show"
    )]
    Reset {
        #[arg(required = true, value_name = "KEY")]
        keys: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum VoiceCommand {
    /// The Natural Voices; * marks yours.
    #[command(
        after_help = "Examples:\n  ring voice ls\n\nNext:\n  ring config set '{\"voice\":\"ID\"}'"
    )]
    Ls,
}

#[derive(Subcommand, Debug)]
pub enum DeviceCommand {
    /// Your devices (sessions); * marks this one.
    #[command(
        after_help = "Examples:\n  ring device ls --json\n\nNext:\n  ring device revoke DEVID"
    )]
    Ls,
    /// Sign a device out.
    #[command(after_help = "Examples:\n  ring device revoke DEVID\n\nNext:\n  ring device ls")]
    Revoke {
        /// The device id (dev_…).
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum NotificationsCommand {
    /// Whether Ring delivers through Ting, and the last error.
    #[command(
        after_help = "Examples:\n  ring notifications status --json\n\nNext:\n  ring notifications enable\n  ting webhook URL"
    )]
    Status,
    /// Register Ring's Ting subscription.
    #[command(after_help = "Examples:\n  ring notifications enable\n\nNext:\n  ting webhook URL")]
    Enable,
    /// Revoke Ring's Ting subscription.
    #[command(
        after_help = "Examples:\n  ring notifications disable\n\nNext:\n  ring notifications status"
    )]
    Disable,
}

// ---- help ---------------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct DocsArgs {
    /// What to read [default: usage].
    pub topic: Option<DocTopic>,
}

#[derive(Args, Debug)]
pub struct ReportArgs {
    /// What happened. The first line becomes the issue title.
    pub message: String,
    /// A pull request with a fix.
    #[arg(long, value_name = "URL")]
    pub pr: Option<String>,
}

// ---- value types --------------------------------------------------------------------------

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ContextModeArg {
    Append,
    Prepend,
    Overwrite,
}

impl From<ContextModeArg> for ContextMode {
    fn from(mode: ContextModeArg) -> Self {
        match mode {
            ContextModeArg::Append => ContextMode::Append,
            ContextModeArg::Prepend => ContextMode::Prepend,
            ContextModeArg::Overwrite => ContextMode::Overwrite,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum MessageKindArg {
    Thinking,
    Commentary,
}

impl From<MessageKindArg> for MessageKind {
    fn from(kind: MessageKindArg) -> Self {
        match kind {
            MessageKindArg::Thinking => MessageKind::Thinking,
            MessageKindArg::Commentary => MessageKind::Commentary,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum CallStateArg {
    Ringing,
    Active,
    Voicemail,
    Ended,
}

impl From<CallStateArg> for CallState {
    fn from(state: CallStateArg) -> Self {
        match state {
            CallStateArg::Ringing => CallState::Ringing,
            CallStateArg::Active => CallState::Active,
            CallStateArg::Voicemail => CallState::Voicemail,
            CallStateArg::Ended => CallState::Ended,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum DirectionArg {
    Incoming,
    Outgoing,
    All,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum DelegationStatusArg {
    Open,
    Closed,
}

impl From<DelegationStatusArg> for DelegationStatus {
    fn from(status: DelegationStatusArg) -> Self {
        match status {
            DelegationStatusArg::Open => DelegationStatus::Open,
            DelegationStatusArg::Closed => DelegationStatus::Closed,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum GreetingCase {
    Busy,
    Declined,
    Missed,
}

impl From<GreetingCase> for VoicemailReason {
    fn from(case: GreetingCase) -> Self {
        match case {
            GreetingCase::Busy => VoicemailReason::Busy,
            GreetingCase::Declined => VoicemailReason::Declined,
            GreetingCase::Missed => VoicemailReason::Missed,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocTopic {
    /// This CLI (udd/cli.md).
    Usage,
    /// The product (udd/understanding.md).
    Understanding,
    /// The HTTP and WebSocket contract (udd/api.md).
    Api,
    /// Silicon Accounts sign-in (udd/accounts.md).
    Accounts,
    /// Components and call lifecycle (udd/architecture.md).
    Architecture,
    /// Visual design of the carbon apps (udd/design.md).
    Design,
    /// Every command, one per line.
    Tree,
}

// ---- parsers ------------------------------------------------------------------------------

/// RFC 3339, a plain date (midnight UTC), or an age such as `30m`, `6h`, `7d` before now.
pub fn parse_time(input: &str) -> Result<Time, String> {
    let input = input.trim();
    if let Some(time) = Time::parse(input) {
        return Ok(time);
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let midnight = date
            .and_hms_opt(0, 0, 0)
            .expect("midnight exists")
            .and_utc();
        return Ok(Time(midnight));
    }
    if let Some(seconds) = parse_duration(input) {
        // chrono panics on out-of-range spans, so a huge age is checked rather than subtracted.
        return i64::try_from(seconds)
            .ok()
            .and_then(chrono::TimeDelta::try_seconds)
            .and_then(|ago| chrono::Utc::now().checked_sub_signed(ago))
            .map(Time)
            .ok_or_else(|| "that age is too large".to_owned());
    }
    Err(
        "use RFC 3339 (2026-10-15T09:30:00Z), a date (2026-10-15), or an age such as 30m, 6h or 7d"
            .into(),
    )
}

/// `90s`, `30m`, `1h`, `2h30m`, `1d` → seconds.
pub fn parse_duration(input: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut number = String::new();
    let mut seen_unit = false;
    for c in input.trim().chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let unit = match c.to_ascii_lowercase() {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return None,
        };
        let value: u64 = number.parse().ok()?;
        total = total.checked_add(value.checked_mul(unit)?)?;
        number.clear();
        seen_unit = true;
    }
    (seen_unit && number.is_empty()).then_some(total)
}

fn parse_approval_window(input: &str) -> Result<u64, String> {
    let seconds =
        parse_duration(input).ok_or_else(|| "use a duration such as 30m, 1h or 24h".to_owned())?;
    if seconds == 0 || seconds > 86_400 {
        return Err("approvals last between 1 second and 24 hours".into());
    }
    Ok(seconds)
}

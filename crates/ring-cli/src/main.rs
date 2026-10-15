//! `ring`: the Silicon Ring CLI for silicons. See `udd/cli.md` (`ring docs`).

// Errors are the cold path of a short-lived process; a rich, unboxed error keeps the code plain.
#![allow(clippy::result_large_err)]

mod cli;
mod commands;
mod error;
mod output;
mod state;

use std::process::ExitCode;

use clap::Parser;
use clap::error::ErrorKind;

use crate::cli::Cli;
use crate::commands::Ctx;
use crate::error::CliError;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage_error(error),
    };
    let ctx = Ctx::new(cli.json, cli.api_url, cli.idempotency_key);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            return fail(
                &CliError::io("Could not start the async runtime", e),
                ctx.json,
            );
        }
    };
    match runtime.block_on(commands::run(&ctx, cli.command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error, ctx.json),
    }
}

fn fail(error: &CliError, json: bool) -> ExitCode {
    if json {
        output::note(&error.to_json().to_string());
    } else if error.code == "interrupted" {
        output::note(&error.message);
        if let Some(hint) = &error.hint {
            output::note(hint);
        }
    } else {
        if let Some(body) = &error.body {
            output::note(body);
            output::note("");
            output::note(&format!("error: {}", error.message));
        } else {
            output::note(&format!("error: {}", error.message));
            if let Some(hint) = &error.hint {
                output::note(&format!("hint: {hint}"));
            }
        }
    }
    ExitCode::from(u8::try_from(error.exit).unwrap_or(1))
}

/// Help and version go to stdout with exit 0; usage mistakes exit 2, as JSON with --json.
fn usage_error(error: clap::Error) -> ExitCode {
    match error.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let wants_json = std::env::args_os()
        .skip(1)
        .take_while(|a| a != "--")
        .any(|a| a == "--json");
    if !wants_json {
        let _ = error.print();
        return ExitCode::from(2);
    }
    let rendered = error.render().to_string();
    let message = rendered
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim().trim_start_matches("error: "))
        .collect::<Vec<_>>()
        .join(" ");
    let usage = rendered
        .lines()
        .find_map(|l| l.trim().strip_prefix("Usage: "))
        .map(str::to_owned);
    let mut cli_error =
        CliError::new("invalid_input", message).hint("Run the command with --help.");
    if let Some(usage) = usage {
        cli_error = cli_error.details(serde_json::json!({ "usage": usage }));
    }
    fail(&cli_error, true)
}

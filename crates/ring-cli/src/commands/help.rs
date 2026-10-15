//! Offline commands: `accounts`, `docs`, group help, and `report` through gh.

use std::process::Stdio;

use clap::CommandFactory;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::Ctx;
use crate::cli::{Cli, DocTopic, DocsArgs, ReportArgs};
use crate::error::{CliError, CliResult};
use crate::output;

const REPOSITORY: &str = "teamofsilicons/silicon-ring";

pub fn accounts(ctx: &Ctx) -> CliResult<()> {
    let info = json!({
        "app_id": "ring",
        "accounts_url": "https://accounts.teamofsilicons.com",
        "apps_url": "https://apps.teamofsilicons.com/apps/ring",
        "api_url": ctx.origin()?,
        "api_version": ring_proto::API_VERSION,
        "docs": "https://ring.teamofsilicons.com/docs",
        "source": format!("https://github.com/{REPOSITORY}"),
        "rust_package": "silicon-ring-core",
        "version": env!("CARGO_PKG_VERSION"),
    });
    output::emit(ctx.json, &info, || {
        format!(
            "app_id        ring\naccounts_url  {}\napps_url      {}\napi_url       {}\napi_version   {}\ndocs          {}\n\
             source        {}\nrust_package  {}\nversion       {}",
            info["accounts_url"].as_str().unwrap_or_default(),
            info["apps_url"].as_str().unwrap_or_default(),
            info["api_url"].as_str().unwrap_or_default(),
            info["api_version"].as_str().unwrap_or_default(),
            info["docs"].as_str().unwrap_or_default(),
            info["source"].as_str().unwrap_or_default(),
            info["rust_package"].as_str().unwrap_or_default(),
            info["version"].as_str().unwrap_or_default(),
        )
    });
    Ok(())
}

/// Prints the long help of the command at `path` (the root when empty) to stdout.
pub fn print_help(path: &[&str]) -> CliResult<()> {
    let mut command = Cli::command();
    command.build();
    let mut current = &mut command;
    for name in path {
        current = current
            .find_subcommand_mut(name)
            .expect("help paths name real commands");
    }
    output::line(&current.render_long_help().to_string());
    Ok(())
}

pub fn docs(ctx: &Ctx, args: DocsArgs) -> CliResult<()> {
    let topic = args.topic.unwrap_or(DocTopic::Usage);
    if topic == DocTopic::Tree {
        let rows = tree();
        output::emit(
            ctx.json,
            &json!({ "topic": "tree", "commands": rows }),
            || {
                rows.iter()
                    .map(|r| format!("{}\n    {}", r.usage, r.about))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        );
        return Ok(());
    }
    let (name, content) = match topic {
        DocTopic::Usage => ("usage", include_str!("../../../../udd/cli.md")),
        DocTopic::Understanding => (
            "understanding",
            include_str!("../../../../udd/understanding.md"),
        ),
        DocTopic::Api => ("api", include_str!("../../../../udd/api.md")),
        DocTopic::Accounts => ("accounts", include_str!("../../../../udd/accounts.md")),
        DocTopic::Architecture => (
            "architecture",
            include_str!("../../../../udd/architecture.md"),
        ),
        DocTopic::Design => ("design", include_str!("../../../../udd/design.md")),
        DocTopic::Tree => unreachable!("handled above"),
    };
    output::emit(
        ctx.json,
        &json!({ "topic": name, "content": content }),
        || content.to_owned(),
    );
    Ok(())
}

#[derive(serde::Serialize)]
struct TreeRow {
    command: String,
    usage: String,
    about: String,
}

/// Every runnable command with its arguments, generated from the grammar so it cannot drift.
fn tree() -> Vec<TreeRow> {
    let mut command = Cli::command();
    command.build();
    let mut rows = Vec::new();
    walk(&command, "ring", &mut rows);
    rows
}

fn walk(command: &clap::Command, path: &str, rows: &mut Vec<TreeRow>) {
    let runnable = command.get_subcommands().next().is_none()
        || command
            .get_arguments()
            .any(|a| a.is_positional() && !a.is_global_set());
    if path != "ring" && runnable {
        rows.push(TreeRow {
            command: path.to_owned(),
            usage: usage(command, path),
            about: command
                .get_about()
                .map(|a| a.to_string())
                .unwrap_or_default(),
        });
    }
    for sub in command.get_subcommands() {
        walk(sub, &format!("{path} {}", sub.get_name()), rows);
    }
}

fn usage(command: &clap::Command, path: &str) -> String {
    let mut positionals = Vec::new();
    let mut options = Vec::new();
    // A required choice between options, such as `(--reason TEXT | --give-no-reason)`.
    let mut grouped = Vec::new();
    for group in command.get_groups().filter(|g| g.is_required_set()) {
        let members: Vec<&clap::Arg> = group
            .get_args()
            .filter_map(|id| command.get_arguments().find(|a| a.get_id() == id))
            .collect();
        grouped.extend(members.iter().map(|a| a.get_id().clone()));
        options.push(format!(
            "({})",
            members
                .iter()
                .filter_map(|a| render_arg(a))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.is_global_set()
            || arg.is_hide_set()
            || id == "help"
            || id == "version"
            || grouped.contains(arg.get_id())
        {
            continue;
        }
        let Some(rendered) = render_arg(arg) else {
            continue;
        };
        let rendered = if arg.is_required_set() {
            rendered
        } else {
            format!("[{rendered}]")
        };
        if arg.is_positional() {
            positionals.push(rendered)
        } else {
            options.push(rendered)
        }
    }
    std::iter::once(path.to_owned())
        .chain(positionals)
        .chain(options)
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_arg(arg: &clap::Arg) -> Option<String> {
    let value = arg
        .get_value_names()
        .and_then(|names| names.first())
        .map(|n| n.to_string())
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());
    if arg.is_positional() {
        let choices = arg.get_possible_values();
        let many = arg.get_num_args().is_some_and(|n| n.max_values() > 1);
        return Some(if !choices.is_empty() {
            choices
                .iter()
                .map(|v| v.get_name())
                .collect::<Vec<_>>()
                .join("|")
        } else if many {
            format!("{value}...")
        } else {
            value
        });
    }
    let long = arg.get_long()?;
    let takes_value = arg.get_num_args().is_some_and(|n| n.takes_values());
    let choices = arg.get_possible_values();
    Some(if !takes_value {
        format!("--{long}")
    } else if !choices.is_empty() && choices.len() <= 4 && arg.get_value_delimiter().is_none() {
        format!(
            "--{long} {}",
            choices
                .iter()
                .map(|v| v.get_name())
                .collect::<Vec<_>>()
                .join("|")
        )
    } else {
        format!("--{long} {value}")
    })
}

pub async fn report(ctx: &Ctx, args: ReportArgs) -> CliResult<()> {
    let message = args.message.trim();
    if message.is_empty() {
        return Err(CliError::input("The report message is empty."));
    }
    if let Some(pr) = &args.pr {
        let valid = url::Url::parse(pr).ok().is_some_and(|u| {
            u.scheme() == "https"
                && u.host_str() == Some("github.com")
                && u.path().contains("/pull/")
        });
        if !valid {
            return Err(CliError::input(
                "--pr must be a GitHub pull request URL (https://github.com/OWNER/REPO/pull/N).",
            ));
        }
    }
    let first_line = message.lines().next().unwrap_or(message).trim();
    let title: String = if first_line.chars().count() > 80 {
        format!("{}…", first_line.chars().take(79).collect::<String>())
    } else {
        first_line.to_owned()
    };
    let mut body = message.to_owned();
    if let Some(pr) = &args.pr {
        body.push_str(&format!("\n\nProposed fix: {pr}"));
    }

    let child = tokio::process::Command::new("gh")
        .args([
            "issue",
            "create",
            "--repo",
            REPOSITORY,
            "--title",
            &title,
            "--body-file",
            "-",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CliError::new("gh_not_found", "The GitHub CLI (gh) is not installed.").hint(format!(
                "Install gh and run `gh auth login`, or open an issue at https://github.com/{REPOSITORY}/issues/new"
            )));
        }
        Err(e) => return Err(CliError::io("Could not start gh", e)),
    };
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| CliError::io("Could not write to gh", e))?;
    }
    let finished = child
        .wait_with_output()
        .await
        .map_err(|e| CliError::io("gh failed", e))?;
    if !finished.status.success() {
        let stderr = String::from_utf8_lossy(&finished.stderr).trim().to_owned();
        return Err(
            CliError::new("gh_failed", format!("gh issue create failed: {stderr}"))
                .hint("Check `gh auth status`, then retry."),
        );
    }
    let url = String::from_utf8_lossy(&finished.stdout).trim().to_owned();
    output::emit(
        ctx.json,
        &json!({ "url": url, "repository": REPOSITORY }),
        || format!("Opened {url}"),
    );
    Ok(())
}

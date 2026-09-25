use std::collections::BTreeMap;
use std::io::{IsTerminal, stderr, stdin};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use console::style;

use crate::client::Client;
use crate::config::{Config, config_path, default_socket_path, format_lifetime, parse_lifetime};
use crate::prompt::{confirm, input, intro, note, outro, outro_cancel, remark, select, warning};
use crate::protocol::{Request, Response};

pub fn run(current: Config) -> Result<()> {
    if !(stdin().is_terminal() && stderr().is_terminal()) {
        print_current(&current);
        return Ok(());
    }

    intro(style(" op-cache ").on_cyan().black())?;
    remark(
        "Secrets are held in memory by a background daemon.\nThese settings decide how long they stay there.",
    )?;

    let ttl = ask_lifetime(
        "How long should a cached secret live?",
        current.ttl,
        (
            "Until the daemon exits",
            "the default; nothing ever goes stale on its own",
        ),
        (
            "For a fixed duration",
            "re-read from 1Password once it lapses",
        ),
        "1h",
    )?;
    let overrides = ask_overrides(current.overrides.clone(), cached_references(&current))?;
    let idle_timeout = ask_lifetime(
        "Should the daemon shut down after sitting idle?",
        current.idle_timeout,
        (
            "No, keep it running",
            "until op-cache stop, or the machine reboots",
        ),
        (
            "Yes, after a quiet period",
            "drops every secret from memory when it exits",
        ),
        "8h",
    )?;
    let op: String = input("Which op binary should it call?")
        .default_input(&current.op)
        .interact()?;
    let socket = ask_socket(current.socket.clone())?;

    let next = Config {
        ttl,
        idle_timeout,
        op,
        socket,
        overrides,
    };
    let mut rows = vec![(
        "Secrets live",
        format_lifetime(next.ttl, "until the daemon exits"),
    )];
    for (i, (reference, ttl)) in next.overrides.iter().enumerate() {
        let label = if i == 0 { "Except" } else { "" };
        rows.push((
            label,
            format!(
                "{reference}  {}",
                format_lifetime(*ttl, "until the daemon exits")
            ),
        ));
    }
    rows.extend([
        (
            "Daemon idles",
            format_lifetime(next.idle_timeout, "forever"),
        ),
        ("op binary", next.op.clone()),
        ("Socket", next.socket_path().display().to_string()),
    ]);
    let review: Vec<String> = rows
        .iter()
        .map(|(label, value)| format!("{} {value}", style(format!("{label:<13}")).dim()))
        .collect();
    note("One last look", review.join("\n"))?;

    let path = config_path();
    if !confirm(format!("Write {}?", display_path(&path)))
        .initial_value(true)
        .interact()?
    {
        outro_cancel("Nothing was written.")?;
        return Ok(());
    }
    next.save()?;

    let daemon_affected =
        next.idle_timeout != current.idle_timeout || next.socket != current.socket;
    if daemon_affected && Client::connect(&current.socket_path()).is_some() {
        warning(
            "A daemon is already running with the old settings. Run `op-cache stop` to restart it.",
        )?;
    }
    outro(format!("Saved to {}", display_path(&path)))?;
    Ok(())
}

fn ask_lifetime(
    question: &str,
    current: Option<Duration>,
    never: (&str, &str),
    fixed: (&str, &str),
    example: &str,
) -> Result<Option<Duration>> {
    let choice = select(question)
        .item(false, never.0, never.1)
        .item(true, fixed.0, fixed.1)
        .initial_value(current.is_some())
        .interact()?;
    if !choice {
        return Ok(None);
    }
    let default = current.map(|d| humantime::format_duration(d).to_string());
    let answer: String = input("For how long?")
        .placeholder(&format!("e.g. {example}, 30m, 1d"))
        .default_input(default.as_deref().unwrap_or(example))
        .validate(|s: &String| match parse_lifetime(s) {
            Ok(Some(_)) => Ok(()),
            _ => Err("use a duration like 30m, 2h or 1d"),
        })
        .interact()?;
    Ok(parse_lifetime(&answer).unwrap_or(None))
}

#[derive(Clone, PartialEq, Eq)]
enum Pick {
    Edit(String),
    Add,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Change,
    Remove,
    Keep,
}

fn ask_overrides(
    mut overrides: BTreeMap<String, Option<Duration>>,
    cached: Vec<String>,
) -> Result<BTreeMap<String, Option<Duration>>> {
    loop {
        let question = if overrides.is_empty() {
            "Should any references live for a different length?"
        } else {
            "These references have their own lifetime. Anything to change?"
        };
        let mut menu = select(question);
        for (reference, ttl) in &overrides {
            menu = menu.item(
                Pick::Edit(reference.clone()),
                reference.as_str(),
                format_lifetime(*ttl, "until the daemon exits"),
            );
        }
        let pick = menu
            .item(
                Pick::Add,
                "Add a reference",
                "or a whole vault, with a trailing /",
            )
            .item(Pick::Done, "No, move on", "")
            .initial_value(Pick::Done)
            .interact()?;
        match pick {
            Pick::Done => return Ok(overrides),
            Pick::Add => {
                let candidates: Vec<&String> = cached
                    .iter()
                    .filter(|r| !overrides.contains_key(*r))
                    .collect();
                let reference = ask_reference(&candidates)?;
                let ttl = ask_override_lifetime(None)?;
                overrides.insert(reference, ttl);
            }
            Pick::Edit(reference) => {
                let action = select(&reference)
                    .item(Action::Change, "Change how long it lives", "")
                    .item(
                        Action::Remove,
                        "Remove the override",
                        "back to the global setting",
                    )
                    .item(Action::Keep, "Leave it", "")
                    .initial_value(Action::Keep)
                    .interact()?;
                match action {
                    Action::Change => {
                        let ttl = ask_override_lifetime(overrides[&reference])?;
                        overrides.insert(reference, ttl);
                    }
                    Action::Remove => {
                        overrides.remove(&reference);
                    }
                    Action::Keep => {}
                }
            }
        }
    }
}

/// Offers what the daemon is holding right now, since those are the references
/// the person has actually been using, with typing one in as the way out.
fn ask_reference(cached: &[&String]) -> Result<String> {
    if !cached.is_empty() {
        let mut menu = select("Which reference?");
        for reference in cached {
            menu = menu.item(
                Some((*reference).clone()),
                reference.as_str(),
                "in the cache now",
            );
        }
        if let Some(reference) = menu
            .item(
                None,
                "Type one in",
                "a reference, or a vault with a trailing /",
            )
            .interact()?
        {
            return Ok(reference);
        }
    }
    Ok(input("Which reference?")
        .placeholder("op://vault/item/field, or op://vault/ for everything in it")
        .validate(|s: &String| {
            if s.starts_with("op://") {
                Ok(())
            } else {
                Err("that should start with op://")
            }
        })
        .interact()?)
}

/// The op:// references the running daemon holds, if there is one. Cache keys
/// are whole `read` argument lists, so the reference is picked out of each.
fn cached_references(config: &Config) -> Vec<String> {
    let Some(client) = Client::connect(&config.socket_path()) else {
        return Vec::new();
    };
    let Ok(Response::Entries { entries }) = client.call(&Request::Inspect) else {
        return Vec::new();
    };
    let mut refs: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            e.key
                .split('\u{1f}')
                .find(|a| a.starts_with("op://"))
                .map(String::from)
        })
        .collect();
    refs.sort();
    refs.dedup();
    refs
}

fn ask_override_lifetime(current: Option<Duration>) -> Result<Option<Duration>> {
    ask_lifetime(
        "How long should it live?",
        current,
        ("Until the daemon exits", ""),
        ("For a fixed duration", ""),
        "10m",
    )
}

fn ask_socket(current: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let default = default_socket_path();
    let custom = select("Where should the daemon's socket live?")
        .item(false, "The default location", display_path(&default))
        .item(true, "A path of my choosing", "")
        .initial_value(current.is_some())
        .interact()?;
    if !custom {
        return Ok(None);
    }
    let answer: String = input("Socket path?")
        .default_input(&current.unwrap_or(default).display().to_string())
        .validate(|s: &String| {
            if s.starts_with('/') {
                Ok(())
            } else {
                Err("give an absolute path")
            }
        })
        .interact()?;
    Ok(Some(PathBuf::from(answer)))
}

fn print_current(config: &Config) {
    println!("# {}", config_path().display());
    println!("# `op-cache config` on a terminal walks through these interactively.");
    print!("{}", toml::to_string(config).unwrap_or_default());
}

fn display_path(path: &std::path::Path) -> String {
    let shown = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && shown.starts_with(&home) => {
            format!("~{}", &shown[home.len()..])
        }
        _ => shown,
    }
}

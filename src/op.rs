use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Output, Stdio};

use anyhow::{Context, Result};

/// Runs `op read <args>` with the terminal attached, so any sign-in prompt
/// reaches the user, and captures only stdout.
pub fn read<S: AsRef<OsStr>>(op: &str, args: &[S]) -> Result<Output> {
    Command::new(op)
        .arg("read")
        .args(args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {op}"))
}

/// Runs `op run --no-masking -- <child>` with each reference in the child's
/// environment as `<prefix><i>`, and every other `op://` variable hidden so op
/// resolves these alone. One op process is one sign-in prompt, however many
/// references it carries. Masking is off because the child's stdout is the
/// values themselves; the terminal is attached for the prompt, and only stdout
/// is captured.
pub fn run_batch(
    op: &str,
    references: &[&str],
    prefix: &str,
    child: &[OsString],
) -> Result<Output> {
    let mut command = Command::new(op);
    command.args(["run", "--no-masking", "--"]).args(child);
    for (name, value) in env::vars_os() {
        if value.as_encoded_bytes().starts_with(b"op://") {
            command.env_remove(name);
        }
    }
    for (i, reference) in references.iter().enumerate() {
        command.env(format!("{prefix}{i}"), reference);
    }
    command
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {op} run"))
}

/// Replaces this process with `op <args>`. Only returns if exec itself failed.
pub fn exec<S: AsRef<OsStr>>(op: &str, args: &[S]) -> anyhow::Error {
    let err = Command::new(op).args(args).exec();
    anyhow::Error::from(err).context(format!("running {op}"))
}

pub fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

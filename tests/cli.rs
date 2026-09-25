use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::Duration;

use tempfile::TempDir;

const STUB: &str = r#"#!/bin/sh
echo "$*" >> "$OP_STUB_LOG"
case "$1" in
  read)
    case "$*" in
      *fail*) echo "stub: no such item" >&2; exit 3 ;;
      *" -o "*) echo "passthrough: $*" ;;
      *) printf 'secret-for-%s\n' "$2" ;;
    esac ;;
  run)
    [ "$2" = "--no-masking" ] || { echo "passthrough: $*"; exit 0; }
    shift 3
    for name in $(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=op:\/\/.*/\1/p'); do
      eval "ref=\$$name"
      case "$ref" in *fail*) echo "stub: no such item" >&2; exit 3 ;; esac
      echo "resolve $ref" >> "$OP_STUB_LOG"
      export "$name=secret-for-$ref"
    done
    exec "$@" ;;
  *) echo "passthrough: $*" ;;
esac
"#;

/// A sandbox with its own config, socket and a fake `op` that logs every call.
struct Harness {
    dir: TempDir,
}

impl Harness {
    fn new(config: &str) -> Self {
        let dir = tempfile::Builder::new().prefix("opc").tempdir().unwrap();
        let op = dir.path().join("op");
        fs::write(&op, STUB).unwrap();
        fs::set_permissions(&op, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            dir.path().join("config.toml"),
            format!("op = \"{}\"\n{config}", op.display()),
        )
        .unwrap();
        fs::write(dir.path().join("op.log"), "").unwrap();
        Self { dir }
    }

    fn op_cache(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_op-cache"));
        cmd.args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.dir.path())
            .env("OP_CACHE_CONFIG", self.dir.path().join("config.toml"))
            .env("OP_CACHE_SOCKET", self.socket())
            .env("OP_STUB_LOG", self.dir.path().join("op.log"));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.op_cache(args).output().unwrap()
    }

    fn stdout(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("d.sock")
    }

    fn op_calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("op.log"))
            .unwrap()
            .lines()
            .map(|l| l.replace(env!("CARGO_BIN_EXE_op-cache"), "op-cache"))
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.run(&["stop"]);
    }
}

#[test]
fn read_hits_op_once_until_cleared() {
    let h = Harness::new("");
    assert_eq!(
        h.stdout(&["read", "op://v/item/field"]),
        "secret-for-op://v/item/field\n"
    );
    assert_eq!(
        h.stdout(&["read", "op://v/item/field"]),
        "secret-for-op://v/item/field\n"
    );
    assert_eq!(h.op_calls(), ["read op://v/item/field"]);

    assert!(h.stdout(&["status"]).contains("cached   1"));
    let inspect = h.stdout(&["inspect"]);
    assert!(
        inspect.contains("op://v/item/field  sec••••••eld  when the daemon exits"),
        "{inspect}"
    );
    assert!(!inspect.contains("secret-for"), "{inspect}");

    assert_eq!(h.stdout(&["clear"]), "op-cache: cleared\n");
    h.stdout(&["read", "op://v/item/field"]);
    assert_eq!(h.op_calls().len(), 2);

    assert_eq!(h.stdout(&["stop"]), "op-cache: stopped\n");
    sleep(Duration::from_millis(100));
    assert!(!h.socket().exists());
    assert!(h.stdout(&["status"]).contains("daemon   not running"));
    assert_eq!(h.stdout(&["inspect"]), "op-cache: not running\n");
    assert_eq!(h.stdout(&["stop"]), "op-cache: not running\n");
}

#[test]
fn different_read_flags_are_different_entries() {
    let h = Harness::new("");
    h.stdout(&["read", "op://v/i/f"]);
    h.stdout(&["read", "--account", "work", "op://v/i/f"]);
    h.stdout(&["read", "--account", "work", "op://v/i/f"]);
    assert_eq!(
        h.op_calls(),
        ["read op://v/i/f", "read --account work op://v/i/f"]
    );
}

#[test]
fn run_resolves_op_references_in_the_environment() {
    let h = Harness::new("");
    let mut cmd = h.op_cache(&[
        "run",
        "--",
        "sh",
        "-c",
        "printf '%s|%s' \"$TOKEN\" \"$PLAIN\"",
    ]);
    cmd.env("TOKEN", "op://v/tok/credential")
        .env("PLAIN", "op-less");
    let out = cmd.output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "secret-for-op://v/tok/credential|op-less"
    );

    assert_eq!(
        h.stdout(&["read", "op://v/tok/credential"]),
        "secret-for-op://v/tok/credential\n"
    );
    assert_eq!(
        h.op_calls(),
        [
            "run --no-masking -- op-cache __emit 1",
            "resolve op://v/tok/credential"
        ]
    );
}

#[test]
fn run_fetches_every_cold_reference_behind_one_op_run() {
    let h = Harness::new("");
    let mut cmd = h.op_cache(&[
        "run",
        "--",
        "sh",
        "-c",
        "printf '%s|%s|%s|%s' \"$A\" \"$B\" \"$C\" \"$PLAIN\"",
    ]);
    cmd.env("A", "op://v/a/f")
        .env("B", "op://v/b/f")
        .env("C", "op://v/c/f")
        .env("PLAIN", "op-less");
    let out = cmd.output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "secret-for-op://v/a/f|secret-for-op://v/b/f|secret-for-op://v/c/f|op-less"
    );

    let mut calls = h.op_calls();
    calls.sort();
    assert_eq!(
        calls,
        [
            "resolve op://v/a/f",
            "resolve op://v/b/f",
            "resolve op://v/c/f",
            "run --no-masking -- op-cache __emit 3",
        ]
    );

    for reference in ["op://v/a/f", "op://v/b/f", "op://v/c/f"] {
        assert_eq!(
            h.stdout(&["read", reference]),
            format!("secret-for-{reference}\n")
        );
    }
    assert_eq!(h.op_calls().len(), 4);
}

#[test]
fn run_only_fetches_what_the_cache_is_missing() {
    let h = Harness::new("");
    h.stdout(&["read", "op://v/a/f"]);
    let mut cmd = h.op_cache(&["run", "--", "sh", "-c", "printf '%s|%s' \"$A\" \"$B\""]);
    cmd.env("A", "op://v/a/f").env("B", "op://v/b/f");
    let out = cmd.output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "secret-for-op://v/a/f|secret-for-op://v/b/f"
    );
    assert_eq!(
        h.op_calls(),
        [
            "read op://v/a/f",
            "run --no-masking -- op-cache __emit 1",
            "resolve op://v/b/f"
        ]
    );
}

#[test]
fn run_fetches_a_reference_shared_by_two_variables_once() {
    let h = Harness::new("");
    let mut cmd = h.op_cache(&["run", "--", "sh", "-c", "printf '%s|%s' \"$A\" \"$B\""]);
    cmd.env("A", "op://v/x/f").env("B", "op://v/x/f");
    let out = cmd.output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "secret-for-op://v/x/f|secret-for-op://v/x/f"
    );
    assert_eq!(
        h.op_calls(),
        [
            "run --no-masking -- op-cache __emit 1",
            "resolve op://v/x/f"
        ]
    );
}

#[test]
fn a_failed_batch_caches_nothing_and_keeps_its_exit_code() {
    let h = Harness::new("");
    let mut cmd = h.op_cache(&["run", "--", "sh", "-c", "echo ran"]);
    cmd.env("A", "op://v/ok/f").env("B", "op://v/fail/f");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no such item"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("ran"));
    assert_eq!(h.stdout(&["inspect"]), "op-cache: nothing cached\n");
}

#[test]
fn entries_expire_after_their_ttl_with_overrides_taking_precedence() {
    let h = Harness::new("ttl = \"1s\"\n[overrides]\n\"op://v/keep/\" = \"until-exit\"\n");
    h.stdout(&["read", "op://v/i/f"]);
    h.stdout(&["read", "op://v/keep/f"]);
    h.stdout(&["read", "op://v/i/f"]);
    let inspect = h.stdout(&["inspect"]);
    assert!(
        inspect.contains("op://v/i/f     sec••••••i/f  in "),
        "{inspect}"
    );
    assert!(
        inspect.contains("op://v/keep/f  sec••••••p/f  when the daemon exits"),
        "{inspect}"
    );

    sleep(Duration::from_millis(1100));
    h.stdout(&["read", "op://v/i/f"]);
    h.stdout(&["read", "op://v/keep/f"]);
    assert_eq!(
        h.op_calls(),
        ["read op://v/i/f", "read op://v/keep/f", "read op://v/i/f"]
    );
}

#[test]
fn failed_reads_are_not_cached_and_keep_their_exit_code() {
    let h = Harness::new("");
    let out = h.run(&["read", "op://v/fail/f"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no such item"));
    h.run(&["read", "op://v/fail/f"]);
    assert_eq!(h.op_calls().len(), 2);
    assert_eq!(h.stdout(&["inspect"]), "op-cache: nothing cached\n");
}

#[test]
fn an_absurd_lifetime_is_capped_at_a_day_and_leaves_the_daemon_stoppable() {
    let h = Harness::new("");
    h.stdout(&["read", "op://v/i/f"]);

    // Far too long to add to an Instant. It once panicked the daemon while it
    // held its lock, after which every request, `stop` and the idle timeout
    // among them, panicked in turn. Now it is capped at a day.
    let mut stream = UnixStream::connect(h.socket()).unwrap();
    stream
        .write_all(
            b"{\"op\":\"put\",\"key\":\"k\",\"value\":[1],\"ttl_secs\":18446744073709551615}\n",
        )
        .unwrap();
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).unwrap();
    assert_eq!(reply, "{\"kind\":\"done\"}\n");

    assert!(h.stdout(&["status"]).contains("cached   2"));
    let inspect = h.stdout(&["inspect"]);
    let capped = inspect.lines().find(|l| l.starts_with("k ")).unwrap();
    assert!(
        capped.ends_with("in 1day") || capped.contains("in 23h 59m"),
        "{inspect}"
    );
    assert_eq!(h.stdout(&["read", "op://v/i/f"]), "secret-for-op://v/i/f\n");
    assert_eq!(h.op_calls(), ["read op://v/i/f"]);
    assert_eq!(h.stdout(&["stop"]), "op-cache: stopped\n");
    sleep(Duration::from_millis(100));
    assert!(!h.socket().exists());
}

#[test]
fn everything_else_goes_to_op() {
    let h = Harness::new("");
    assert_eq!(h.stdout(&["item", "get", "x"]), "passthrough: item get x\n");
    assert_eq!(
        h.stdout(&["--account", "a", "read", "x"]),
        "passthrough: --account a read x\n"
    );
    assert_eq!(
        h.stdout(&["run", "--env-file", ".env", "--", "true"]),
        "passthrough: run --env-file .env -- true\n"
    );
    assert_eq!(
        h.stdout(&["read", "op://v/i/f", "-o", "out"]),
        "passthrough: read op://v/i/f -o out\n"
    );
    assert!(h.stdout(&["status"]).contains("daemon   not running"));
}

#[test]
fn help_version_and_misuse_never_reach_op() {
    let h = Harness::new("");
    for args in [&[][..], &["--help"], &["help"]] {
        assert!(h.stdout(args).contains("Usage: op-cache [COMMAND]"));
    }
    for args in [["help", "status"], ["status", "--help"]] {
        assert!(h.stdout(&args).contains("Usage: op-cache status\n"));
    }
    assert!(
        h.stdout(&["help", "read"])
            .contains("Usage: op-cache read <ARGS>...")
    );
    assert_eq!(
        h.stdout(&["--version"]),
        concat!("op-cache ", env!("CARGO_PKG_VERSION"), "\n")
    );
    assert!(
        h.stdout(&["daemon", "--help"])
            .contains("Usage: op-cache [COMMAND]")
    );
    for args in [
        &["read"][..],
        &["status", "extra"],
        &["daemon", "extra"],
        &["help", "bogus"],
        &["help", "status", "extra"],
    ] {
        assert_eq!(h.run(args).status.code(), Some(2), "{args:?}");
    }
    assert!(h.op_calls().is_empty());
}

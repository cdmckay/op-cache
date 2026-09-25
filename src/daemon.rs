use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::cache::Cache;
use crate::config::Config;
use crate::protocol::{Entry, Request, Response, Status};

struct State {
    cache: Cache,
    last_activity: Instant,
}

/// Serves the cache on the configured socket until told to stop, the idle
/// timeout passes, or another daemon already holds the socket's lock.
pub fn run(config: &Config) -> Result<()> {
    let socket = config.socket_path();
    let lock_path = socket.with_extension("lock");
    let lock =
        File::create(&lock_path).with_context(|| format!("creating {}", lock_path.display()))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }

    let _ = fs::remove_file(&socket);
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;

    let started = Instant::now();
    let state = Arc::new(Mutex::new(State {
        cache: Cache::default(),
        last_activity: started,
    }));

    if let Some(idle) = config.idle_timeout {
        let state = Arc::clone(&state);
        let socket = socket.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(1).min(idle));
                match lock_state(&state) {
                    Some(state) if state.last_activity.elapsed() < idle => {}
                    _ => shutdown(&socket),
                }
            }
        });
    }

    let idle_timeout_secs = config.idle_timeout.map(|d| d.as_secs());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let state = Arc::clone(&state);
        let socket = socket.clone();
        thread::spawn(move || {
            let _ = serve(stream, &state, &socket, idle_timeout_secs, started);
        });
    }
    Ok(())
}

fn serve(
    mut stream: UnixStream,
    state: &Mutex<State>,
    socket: &Path,
    idle_timeout_secs: Option<u64>,
    started: Instant,
) -> Result<()> {
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let request: Request = serde_json::from_str(&line)?;
    let now = Instant::now();
    let Some(mut state) = lock_state(state) else {
        shutdown(socket)
    };
    state.last_activity = now;

    let response = match request {
        Request::Get { key } => match state.cache.get(&key, now) {
            Some(value) => Response::Hit {
                value: value.to_vec(),
            },
            None => Response::Miss,
        },
        Request::Put {
            key,
            value,
            ttl_secs,
        } => {
            state
                .cache
                .put(key, value, ttl_secs.map(Duration::from_secs), now);
            Response::Done
        }
        Request::Clear => {
            state.cache.clear();
            Response::Done
        }
        Request::Status => Response::Status(Status {
            pid: std::process::id(),
            uptime_secs: started.elapsed().as_secs(),
            idle_timeout_secs,
            cached: state.cache.entries(now).len(),
        }),
        Request::Inspect => Response::Entries {
            entries: state
                .cache
                .entries(now)
                .into_iter()
                .map(|(key, value, left)| Entry {
                    key: key.to_string(),
                    preview: mask(value),
                    expires_in_secs: left.map(|d| d.as_secs()),
                })
                .collect(),
        },
        Request::Stop => {
            reply(&mut stream, &Response::Done)?;
            shutdown(socket);
        }
    };
    reply(&mut stream, &response)
}

fn reply(stream: &mut UnixStream, response: &Response) -> Result<()> {
    serde_json::to_writer(&mut *stream, response)?;
    stream.write_all(b"\n")?;
    Ok(())
}

/// Keeps the ends of a long value, enough to tell secrets apart, and hides a
/// short one completely.
fn mask(value: &[u8]) -> String {
    let text = String::from_utf8_lossy(value);
    let text = text.trim_end_matches(['\n', '\r']);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 12 {
        return "••••••••".to_string();
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 3..].iter().collect();
    format!("{head}••••••{tail}")
}

/// The state, or `None` if a thread panicked while holding it. Nothing behind a
/// poisoned lock can be trusted, and every later request, `stop` and the idle
/// timeout among them, would panic trying to take it, leaving a daemon that
/// holds its secrets until it is killed. So callers shut down instead; the next
/// client starts a fresh daemon.
fn lock_state(state: &Mutex<State>) -> Option<MutexGuard<'_, State>> {
    state.lock().ok()
}

fn shutdown(socket: &Path) -> ! {
    let _ = fs::remove_file(socket);
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_keeps_only_the_ends_of_long_values() {
        assert_eq!(mask(b"ghp_abcdefghijklmnop\n"), "ghp••••••nop");
        assert_eq!(mask(b"short\n"), "••••••••");
        assert_eq!(mask(b"exactly12chr"), "exa••••••chr");
    }

    #[test]
    fn a_poisoned_lock_is_refused_rather_than_unwrapped() {
        let state = Mutex::new(State {
            cache: Cache::default(),
            last_activity: Instant::now(),
        });
        let _ = std::panic::catch_unwind(|| {
            let _held = state.lock().unwrap();
            panic!("poisoning the lock on purpose");
        });
        assert!(state.is_poisoned());
        assert!(lock_state(&state).is_none());
    }
}

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The longest a cached secret lives, whatever lifetime it was given. A day
/// covers any real schedule, and the bound keeps an absurd lifetime from
/// overflowing the clock, which once panicked the daemon while it held its lock.
/// `None`, until the daemon exits, is not a lifetime and is left alone.
pub const MAX_TTL: Duration = Duration::from_secs(24 * 60 * 60);

struct Entry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
}

impl Cache {
    pub fn get(&mut self, key: &str, now: Instant) -> Option<&[u8]> {
        if self
            .entries
            .get(key)
            .is_some_and(|e| e.expires_at.is_some_and(|t| t <= now))
        {
            self.entries.remove(key);
        }
        self.entries.get(key).map(|e| e.value.as_slice())
    }

    pub fn put(&mut self, key: String, value: Vec<u8>, ttl: Option<Duration>, now: Instant) {
        let expires_at = ttl.map(|t| now + t.min(MAX_TTL));
        self.entries.insert(key, Entry { value, expires_at });
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Live entries as (key, value, time left), sorted by key.
    pub fn entries(&self, now: Instant) -> Vec<(&str, &[u8], Option<Duration>)> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at.is_none_or(|t| t > now))
            .map(|(k, e)| {
                (
                    k.as_str(),
                    e.value.as_slice(),
                    e.expires_at.map(|t| t - now),
                )
            })
            .collect();
        entries.sort();
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_expire_only_when_given_a_ttl() {
        let mut cache = Cache::default();
        let t0 = Instant::now();
        cache.put("forever".into(), b"a".to_vec(), None, t0);
        cache.put(
            "brief".into(),
            b"b".to_vec(),
            Some(Duration::from_secs(10)),
            t0,
        );

        let later = t0 + Duration::from_secs(5);
        assert_eq!(cache.get("forever", later), Some(&b"a"[..]));
        assert_eq!(cache.get("brief", later), Some(&b"b"[..]));
        assert_eq!(
            cache.entries(later),
            vec![
                ("brief", &b"b"[..], Some(Duration::from_secs(5))),
                ("forever", &b"a"[..], None)
            ]
        );

        let expired = t0 + Duration::from_secs(10);
        assert_eq!(cache.entries(expired), vec![("forever", &b"a"[..], None)]);
        assert_eq!(cache.get("brief", expired), None);
        assert_eq!(cache.get("forever", expired), Some(&b"a"[..]));

        cache.clear();
        assert!(cache.entries(expired).is_empty());
    }

    #[test]
    fn lifetimes_are_capped_at_a_day() {
        let mut cache = Cache::default();
        let t0 = Instant::now();
        cache.put("huge".into(), b"h".to_vec(), Some(Duration::MAX), t0);
        cache.put("week".into(), b"w".to_vec(), Some(MAX_TTL * 7), t0);
        cache.put(
            "hour".into(),
            b"o".to_vec(),
            Some(Duration::from_secs(3600)),
            t0,
        );
        cache.put("exit".into(), b"e".to_vec(), None, t0);
        assert_eq!(
            cache.entries(t0),
            vec![
                ("exit", &b"e"[..], None),
                ("hour", &b"o"[..], Some(Duration::from_secs(3600))),
                ("huge", &b"h"[..], Some(MAX_TTL)),
                ("week", &b"w"[..], Some(MAX_TTL)),
            ]
        );

        let a_day_on = t0 + MAX_TTL;
        assert_eq!(cache.get("huge", a_day_on), None);
        assert_eq!(cache.get("week", a_day_on), None);
        assert_eq!(cache.get("exit", a_day_on), Some(&b"e"[..]));
    }
}

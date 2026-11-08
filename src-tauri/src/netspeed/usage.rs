//! Data used, per day: what the computer's own connections (Wi-Fi, Ethernet,
//! mobile — never a VPN or virtual adapter on top of them, which would count
//! the same bytes twice) received and sent, kept across restarts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Days kept: a little over a year.
const KEEP_DAYS: usize = 400;

/// Two system start times within this many seconds are the same start.
const SAME_BOOT_SECS: i64 = 120;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Ledger {
    /// When the system started (seconds since 1970), as last seen.
    pub boot: i64,
    /// When this was last written.
    pub saved_at: i64,
    /// Each physical adapter's counters (received, sent) as last seen.
    pub counters: HashMap<String, [u64; 2]>,
    /// "2026-11-06" → (received, sent).
    pub days: BTreeMap<String, [u64; 2]>,
}

impl Ledger {
    pub fn add(&mut self, day: &str, rx: u64, tx: u64) {
        if rx == 0 && tx == 0 {
            return;
        }
        let e = self.days.entry(day.to_string()).or_insert([0, 0]);
        e[0] = e[0].saturating_add(rx);
        e[1] = e[1].saturating_add(tx);
        while self.days.len() > KEEP_DAYS {
            let first = self.days.keys().next().cloned();
            if let Some(k) = first {
                self.days.remove(&k);
            }
        }
    }

    pub fn day(&self, day: &str) -> [u64; 2] {
        self.days.get(day).copied().unwrap_or([0, 0])
    }

    /// "2026-11" → the month's total.
    pub fn month(&self, month: &str) -> [u64; 2] {
        let prefix = format!("{}-", month);
        self.days.iter().filter(|(d, _)| d.starts_with(&prefix)).fold([0, 0], |a, (_, v)| [a[0] + v[0], a[1] + v[1]])
    }

    /// GitSwitch has just started: count what moved while it was closed.
    /// The same system start: what each adapter's counters gained since they
    /// were last seen. A restart since: everything since the system started
    /// (counters begin at zero then). The very first time: nothing — it
    /// counts from now. Either way it goes to `day`.
    pub fn catch_up(&mut self, boot: i64, now: &HashMap<String, [u64; 2]>, day: &str) {
        let (mut rx, mut tx) = (0u64, 0u64);
        if !self.counters.is_empty() {
            if (boot - self.boot).abs() <= SAME_BOOT_SECS {
                for (id, c) in now {
                    if let Some(p) = self.counters.get(id) {
                        rx += c[0].saturating_sub(p[0]);
                        tx += c[1].saturating_sub(p[1]);
                    }
                }
            } else if boot > self.saved_at {
                for c in now.values() {
                    rx += c[0];
                    tx += c[1];
                }
            }
        }
        self.add(day, rx, tx);
        self.boot = boot;
        self.counters = now.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(v: &[(&str, u64, u64)]) -> HashMap<String, [u64; 2]> {
        v.iter().map(|(k, r, t)| (k.to_string(), [*r, *t])).collect()
    }

    #[test]
    fn days_add_up_to_months_and_old_days_go() {
        let mut l = Ledger::default();
        l.add("2026-10-31", 5, 1);
        l.add("2026-11-01", 10, 2);
        l.add("2026-11-06", 100, 20);
        l.add("2026-11-06", 1, 1);
        l.add("2026-11-07", 0, 0);
        assert_eq!(l.day("2026-11-06"), [101, 21]);
        assert_eq!(l.month("2026-11"), [111, 23]);
        assert_eq!(l.month("2026-10"), [5, 1]);
        assert!(!l.days.contains_key("2026-11-07"), "an empty day is not written");
        for i in 0..500 {
            l.add(&format!("2027-{:03}", i), 1, 1);
        }
        assert_eq!(l.days.len(), KEEP_DAYS);
        assert!(!l.days.contains_key("2026-10-31"), "the oldest days go first");
    }

    #[test]
    fn what_moved_while_gitswitch_was_closed_is_counted_once() {
        let mut l = Ledger::default();
        // First time: counting starts now.
        l.catch_up(1000, &counters(&[("wifi", 500, 50)]), "d1");
        assert_eq!(l.day("d1"), [0, 0]);
        l.saved_at = 2000;
        // Same system start (within two minutes): what the counters gained.
        l.catch_up(1060, &counters(&[("wifi", 800, 90), ("eth", 10, 1)]), "d2");
        assert_eq!(l.day("d2"), [300, 40], "a new adapter starts from now");
        // A counter that went backwards (adapter reset) adds nothing.
        l.catch_up(1060, &counters(&[("wifi", 100, 10), ("eth", 10, 1)]), "d3");
        assert_eq!(l.day("d3"), [0, 0]);
        // Restarted since it was last written: everything since that start.
        l.saved_at = 3000;
        l.catch_up(5000, &counters(&[("wifi", 70, 7), ("eth", 30, 3)]), "d4");
        assert_eq!(l.day("d4"), [100, 10]);
        // A start before the last write (the clock moved, say): nothing guessed.
        l.saved_at = 9000;
        l.catch_up(4000, &counters(&[("wifi", 90, 9)]), "d5");
        assert_eq!(l.day("d5"), [0, 0]);
    }
}

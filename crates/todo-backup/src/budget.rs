//! A local, monthly count of the S3 operations this tool performs, with hard caps.
//!
//! R2 bills by operation class. Class A covers writes and listings (PUT, LIST, the multipart calls); Class B
//! covers reads (GET, HEAD). The free tier is 1M Class A and 10M Class B a month. The default caps here are
//! far below that, and the counter is charged *before* each request is sent, so a failed request still counts
//! and the tool errs on the side of stopping early.

use std::fmt;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::state::write_json_atomic;
use crate::{Error, Result};

/// R2's monthly free tier. Caps above it are rejected outright.
pub const FREE_TIER_CLASS_A: u64 = 1_000_000;
pub const FREE_TIER_CLASS_B: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    A,
    B,
}

impl fmt::Display for OpClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OpClass::A => "A",
            OpClass::B => "B",
        })
    }
}

/// Monthly caps, from the `[budget]` table of the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BudgetCaps {
    pub class_a_monthly: u64,
    pub class_b_monthly: u64,
}

impl Default for BudgetCaps {
    fn default() -> Self {
        Self {
            class_a_monthly: 20_000,
            class_b_monthly: 200_000,
        }
    }
}

impl BudgetCaps {
    pub fn validate(&self) -> Result<()> {
        if self.class_a_monthly > FREE_TIER_CLASS_A || self.class_b_monthly > FREE_TIER_CLASS_B {
            return Err(Error::Config(format!(
                "budget caps must stay inside the R2 free tier ({FREE_TIER_CLASS_A} Class A, \
                 {FREE_TIER_CLASS_B} Class B a month)"
            )));
        }
        Ok(())
    }

    fn cap(&self, class: OpClass) -> u64 {
        match class {
            OpClass::A => self.class_a_monthly,
            OpClass::B => self.class_b_monthly,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// `YYYY-MM`, in UTC.
    pub month: String,
    pub class_a: u64,
    pub class_b: u64,
}

impl Usage {
    fn used(&self, class: OpClass) -> u64 {
        match class {
            OpClass::A => self.class_a,
            OpClass::B => self.class_b,
        }
    }

    fn used_mut(&mut self, class: OpClass) -> &mut u64 {
        match class {
            OpClass::A => &mut self.class_a,
            OpClass::B => &mut self.class_b,
        }
    }
}

/// The counter, persisted as `budget.json` in the state directory after every charge.
#[derive(Debug)]
pub struct Budget {
    path: PathBuf,
    caps: BudgetCaps,
    usage: Usage,
}

impl Budget {
    pub const FILE: &'static str = "budget.json";

    /// Loads the counter for the month of `now`, starting from zero in a new month.
    pub fn open(state_dir: &Path, caps: BudgetCaps, now: Timestamp) -> Result<Self> {
        let path = state_dir.join(Self::FILE);
        let month = month_of(now);
        let usage = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let usage: Usage = serde_json::from_str(&text).map_err(|e| {
                    // A corrupt counter must not silently reset to zero: that would defeat the cap.
                    Error::Config(format!(
                        "budget file {} is corrupt ({e}); fix or delete it deliberately",
                        path.display()
                    ))
                })?;
                if usage.month == month {
                    usage
                } else {
                    Usage {
                        month,
                        ..Usage::default()
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Usage {
                month,
                ..Usage::default()
            },
            Err(e) => return Err(Error::io(format!("reading {}", path.display()))(e)),
        };
        Ok(Self { path, caps, usage })
    }

    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    pub fn caps(&self) -> BudgetCaps {
        self.caps
    }

    /// Fails if `n` more operations of `class` would exceed the cap. Charges nothing.
    pub fn check(&self, class: OpClass, n: u64) -> Result<()> {
        let used = self.usage.used(class);
        let cap = self.caps.cap(class);
        if used.saturating_add(n) > cap {
            return Err(Error::BudgetExceeded {
                class,
                month: self.usage.month.clone(),
                used,
                wanted: n,
                cap,
            });
        }
        Ok(())
    }

    /// Records `n` operations of `class` and persists the counter, or refuses if that would exceed the cap.
    pub fn charge(&mut self, class: OpClass, n: u64) -> Result<()> {
        self.check(class, n)?;
        *self.usage.used_mut(class) += n;
        write_json_atomic(&self.path, &self.usage)
    }
}

pub fn month_of(t: Timestamp) -> String {
    t.strftime("%Y-%m").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn charges_persist_and_refuse_at_the_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let caps = BudgetCaps {
            class_a_monthly: 3,
            class_b_monthly: 1,
        };
        let now = ts("2026-11-21T10:00:00Z");
        let mut b = Budget::open(dir.path(), caps, now).expect("open");
        b.charge(OpClass::A, 2).expect("2 of 3");
        b.charge(OpClass::B, 1).expect("1 of 1");
        assert!(matches!(
            b.charge(OpClass::B, 1),
            Err(Error::BudgetExceeded {
                class: OpClass::B,
                ..
            })
        ));

        // Reloading keeps the counts; the refused charge was not recorded.
        let mut b = Budget::open(dir.path(), caps, now).expect("reopen");
        assert_eq!((b.usage().class_a, b.usage().class_b), (2, 1));
        b.charge(OpClass::A, 1).expect("3 of 3");
        let err = b.charge(OpClass::A, 1).expect_err("over");
        assert!(err.to_string().contains("Class A"), "{err}");
    }

    #[test]
    fn new_month_starts_from_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let caps = BudgetCaps {
            class_a_monthly: 1,
            class_b_monthly: 1,
        };
        let mut b = Budget::open(dir.path(), caps, ts("2026-11-30T23:59:59Z")).expect("open");
        b.charge(OpClass::A, 1).expect("charge");
        let b = Budget::open(dir.path(), caps, ts("2026-11-30T23:59:59Z")).expect("same month");
        assert!(b.check(OpClass::A, 1).is_err());
        let b = Budget::open(dir.path(), caps, ts("2026-12-01T00:00:00Z")).expect("next month");
        assert_eq!(b.usage().month, "2026-12");
        b.check(OpClass::A, 1).expect("fresh month");
    }

    #[test]
    fn corrupt_counter_is_an_error_not_a_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(Budget::FILE), "{not json").expect("write");
        assert!(
            Budget::open(
                dir.path(),
                BudgetCaps::default(),
                ts("2026-11-21T00:00:00Z")
            )
            .is_err()
        );
    }

    #[test]
    fn caps_above_free_tier_are_rejected() {
        let caps = BudgetCaps {
            class_a_monthly: FREE_TIER_CLASS_A + 1,
            class_b_monthly: 1,
        };
        assert!(caps.validate().is_err());
        BudgetCaps::default().validate().expect("defaults are fine");
    }
}

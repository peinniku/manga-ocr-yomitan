//! Spaced repetition with FSRS-7 (official `fsrs` crate).
//!
//! A card's schedule is never stored as the source of truth: it is derived by
//! replaying its review log in time order. Logs are append-only and merge as a
//! set union across devices, so two devices that reviewed offline converge to
//! the same schedule after syncing.

use fsrs::{DEFAULT_PARAMETERS, FSRS, MemoryState};
use serde::{Deserialize, Serialize};

pub const DAY_MS: f64 = 86_400_000.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewLog {
    pub id: String,
    pub card_id: String,
    /// 1 again, 2 hard, 3 good, 4 easy.
    pub rating: u8,
    /// Unix milliseconds.
    pub reviewed_at: i64,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    New,
    /// Seen, but the next review is within the same day.
    Learning,
    Review,
    /// Forgotten (rated again) after having graduated.
    Relearning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schedule {
    pub phase: Phase,
    /// Unix ms; for new cards this is 0 (always available).
    pub due: i64,
    pub stability: Option<f32>,
    /// FSRS-7 short-term stability.
    #[serde(default)]
    pub stability_fast: Option<f32>,
    pub difficulty: Option<f32>,
    pub last_review: Option<i64>,
    pub reps: u32,
    pub lapses: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self { phase: Phase::New, due: 0, stability: None, stability_fast: None, difficulty: None, last_review: None, reps: 0, lapses: 0 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SrsConfig {
    pub desired_retention: f32,
    pub max_interval_days: f32,
}

impl Default for SrsConfig {
    fn default() -> Self {
        Self { desired_retention: 0.9, max_interval_days: 3650.0 }
    }
}

pub struct Scheduler {
    fsrs: FSRS,
    pub config: SrsConfig,
}

/// Shortest wait after each rating, so "again" does not come back instantly.
const MIN_WAIT_MS: [f64; 4] = [60_000.0, 5.0 * 60_000.0, 10.0 * 60_000.0, DAY_MS];

impl Scheduler {
    pub fn new(config: SrsConfig) -> Self {
        Self { fsrs: FSRS::new(&DEFAULT_PARAMETERS).expect("default FSRS parameters"), config }
    }

    fn memory(s: &Schedule) -> Option<MemoryState> {
        match (s.stability, s.difficulty) {
            (Some(stability), Some(difficulty)) => {
                Some(MemoryState { stability, difficulty, stability_fast: s.stability_fast.unwrap_or(stability) })
            }
            _ => None,
        }
    }

    /// Apply one rating at time `at` (unix ms) and return the new schedule.
    pub fn apply(&self, s: &Schedule, rating: u8, at: i64) -> Schedule {
        let rating = rating.clamp(1, 4);
        let elapsed_days = s.last_review.map(|l| ((at - l) as f64 / DAY_MS).max(0.0) as f32).unwrap_or(0.0);
        let mem = Self::memory(s);
        let Ok(next) = self.fsrs.next_states_with_elapsed_days(mem, self.config.desired_retention, elapsed_days) else {
            return s.clone();
        };
        let item = match rating {
            1 => next.again,
            2 => next.hard,
            3 => next.good,
            _ => next.easy,
        };
        let interval_ms =
            (item.interval.min(self.config.max_interval_days) as f64 * DAY_MS).max(MIN_WAIT_MS[(rating - 1) as usize]);
        let graduated = matches!(s.phase, Phase::Review);
        let phase = if interval_ms < DAY_MS {
            if rating == 1 && graduated || s.phase == Phase::Relearning { Phase::Relearning } else { Phase::Learning }
        } else {
            Phase::Review
        };
        Schedule {
            phase,
            due: at + interval_ms.round() as i64,
            stability: Some(item.memory.stability),
            stability_fast: Some(item.memory.stability_fast),
            difficulty: Some(item.memory.difficulty),
            last_review: Some(at),
            reps: s.reps + 1,
            lapses: s.lapses + u32::from(rating == 1 && graduated),
        }
    }

    /// Replay a card's logs (any order; sorted here by time, then id for determinism).
    pub fn replay(&self, logs: &[ReviewLog]) -> Schedule {
        let mut sorted: Vec<&ReviewLog> = logs.iter().collect();
        sorted.sort_by(|a, b| a.reviewed_at.cmp(&b.reviewed_at).then(a.id.cmp(&b.id)));
        sorted.into_iter().fold(Schedule::default(), |s, l| self.apply(&s, l.rating, l.reviewed_at))
    }

    /// Due time for each of the four ratings if answered at `now` (for button labels).
    pub fn preview(&self, s: &Schedule, now: i64) -> [i64; 4] {
        [1, 2, 3, 4].map(|r| self.apply(s, r, now).due)
    }
}

/// Short human label for a wait, e.g. "10分钟", "3天", "2.5个月".
pub fn format_interval(ms: i64) -> String {
    let m = ms as f64 / 60_000.0;
    if m < 60.0 {
        format!("{}分钟", m.round().max(1.0))
    } else if m < 60.0 * 24.0 {
        format!("{}小时", (m / 60.0).round())
    } else {
        let d = m / 1440.0;
        if d < 30.0 {
            format!("{}天", d.round())
        } else if d < 365.0 {
            format!("{:.1}个月", d / 30.0)
        } else {
            format!("{:.1}年", d / 365.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(id: &str, rating: u8, at: i64) -> ReviewLog {
        ReviewLog { id: id.into(), card_id: "c".into(), rating, reviewed_at: at, device_id: String::new(), duration_ms: None }
    }

    #[test]
    fn good_answers_grow_intervals() {
        let s = Scheduler::new(SrsConfig::default());
        let t0 = 1_700_000_000_000;
        let a = s.apply(&Schedule::default(), 3, t0);
        assert_eq!(a.phase, Phase::Review);
        let b = s.apply(&a, 3, a.due);
        assert!(b.due - a.due > a.due - t0, "second interval should be longer");
        let again = s.apply(&b, 1, b.due);
        assert_eq!(again.phase, Phase::Relearning);
        assert_eq!(again.lapses, 1);
        assert!(again.due - b.due < DAY_MS as i64);
    }

    #[test]
    fn replay_is_order_independent() {
        let s = Scheduler::new(SrsConfig::default());
        let t0 = 1_700_000_000_000;
        let logs = vec![log("a", 3, t0), log("b", 1, t0 + 5 * DAY_MS as i64), log("c", 3, t0 + 5 * DAY_MS as i64 + 600_000)];
        let mut shuffled = logs.clone();
        shuffled.reverse();
        assert_eq!(s.replay(&logs), s.replay(&shuffled));
        assert_eq!(s.replay(&logs).reps, 3);
    }

    #[test]
    fn interval_labels() {
        assert_eq!(format_interval(60_000), "1分钟");
        assert_eq!(format_interval(3 * DAY_MS as i64), "3天");
    }
}

/// WebAssembly entry points for the phone web app. JSON in, JSON out, through
/// linear memory: the host calls `alloc`, writes UTF-8 JSON, calls a function,
/// and reads the result from (`result_ptr`, `result_len`).
#[cfg(target_arch = "wasm32")]
pub mod wasm {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    #[derive(Deserialize)]
    struct ReplayIn {
        #[serde(default)]
        config: Option<SrsConfig>,
        logs: Vec<ReviewLog>,
    }

    #[derive(Deserialize)]
    struct PreviewIn {
        #[serde(default)]
        config: Option<SrsConfig>,
        schedule: Schedule,
        now: i64,
    }

    #[derive(Serialize)]
    struct PreviewOut {
        due: [i64; 4],
        labels: [String; 4],
    }

    fn input(ptr: *mut u8, len: usize) -> Vec<u8> {
        unsafe { Vec::from_raw_parts(ptr, len, len) }
    }

    fn output(v: impl Serialize) -> i32 {
        let bytes = serde_json::to_vec(&v).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}").into_bytes());
        OUT.with(|o| *o.borrow_mut() = bytes);
        0
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn alloc(len: usize) -> *mut u8 {
        let mut v = Vec::<u8>::with_capacity(len);
        let p = v.as_mut_ptr();
        std::mem::forget(v);
        p
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn result_ptr() -> *const u8 {
        OUT.with(|o| o.borrow().as_ptr())
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn result_len() -> usize {
        OUT.with(|o| o.borrow().len())
    }

    /// `{config?, logs}` → `Schedule`
    #[unsafe(no_mangle)]
    pub extern "C" fn replay(ptr: *mut u8, len: usize) -> i32 {
        match serde_json::from_slice::<ReplayIn>(&input(ptr, len)) {
            Ok(r) => output(Scheduler::new(r.config.unwrap_or_default()).replay(&r.logs)),
            Err(e) => output(serde_json::json!({ "error": e.to_string() })),
        }
    }

    /// `{config?, schedule, now}` → `{due: [4], labels: [4]}`
    #[unsafe(no_mangle)]
    pub extern "C" fn preview(ptr: *mut u8, len: usize) -> i32 {
        match serde_json::from_slice::<PreviewIn>(&input(ptr, len)) {
            Ok(p) => {
                let due = Scheduler::new(p.config.unwrap_or_default()).preview(&p.schedule, p.now);
                output(PreviewOut { due, labels: due.map(|d| format_interval(d - p.now)) })
            }
            Err(e) => output(serde_json::json!({ "error": e.to_string() })),
        }
    }

    /// fsrs inference never draws random numbers, but `rand` still links getrandom.
    #[unsafe(no_mangle)]
    unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), getrandom::Error> {
        for i in 0..len {
            unsafe { *dest.add(i) = (i as u8).wrapping_mul(131).wrapping_add(7) };
        }
        Ok(())
    }
}

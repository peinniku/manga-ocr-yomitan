//! The phone (wasm) and the Mac (native) must derive the same schedule from the
//! same review log. `fixture.json` holds logs and the native result; the web
//! test (`web/scripts/srs.test.mjs`) checks the wasm build against the same file.

use picyomi_srs::{ReviewLog, Schedule, Scheduler, SrsConfig};

#[derive(serde::Deserialize, serde::Serialize)]
struct Fixture {
    logs: Vec<ReviewLog>,
    expected: Schedule,
}

fn sample_logs() -> Vec<ReviewLog> {
    let t0 = 1_763_000_000_000i64;
    let day = 86_400_000i64;
    [(3, 0), (3, 4 * day), (1, 15 * day), (3, 15 * day + 600_000), (4, 20 * day), (2, 60 * day)]
        .iter()
        .enumerate()
        .map(|(i, (r, t))| ReviewLog {
            id: format!("log-{i}"),
            card_id: "c1".into(),
            rating: *r,
            reviewed_at: t0 + t,
            device_id: if i % 2 == 0 { "mac".into() } else { "phone".into() },
            duration_ms: None,
        })
        .collect()
}

#[test]
fn native_matches_fixture() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixture.json");
    let sched = Scheduler::new(SrsConfig::default());
    let logs = sample_logs();
    let actual = sched.replay(&logs);
    if std::env::var_os("UPDATE_FIXTURE").is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&Fixture { logs, expected: actual.clone() }).unwrap()).unwrap();
    }
    let f: Fixture = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(sched.replay(&f.logs), f.expected);
}

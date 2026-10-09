use sushiai_core::hibernate::*;
use sushiai_protocol::AgentStatus;

const HOUR: u64 = 3_600_000;

fn sleepy() -> Candidate {
    Candidate {
        eligible: true,
        running: true,
        agent_status: Some(AgentStatus::Idle),
        open_asks: 0,
        pinned: false,
        focused: false,
        quiet_since_ms: 0,
    }
}

#[test]
fn a_decision_table() {
    let working = Some(AgentStatus::Working);
    let blocked = Some(AgentStatus::Blocked);
    let cases: Vec<(&str, Candidate, u64, Option<bool>, Decision)> = vec![
        (
            "quiet children",
            sleepy(),
            5 * HOUR,
            Some(false),
            Decision::Hibernate,
        ),
        (
            "busy children",
            sleepy(),
            5 * HOUR,
            Some(true),
            Decision::Sample,
        ),
        ("no sample yet", sleepy(), 5 * HOUR, None, Decision::Sample),
        (
            "window before the threshold",
            sleepy(),
            4 * HOUR - 60_000,
            None,
            Decision::Sample,
        ),
        ("idle only a little", sleepy(), HOUR, None, Decision::Keep),
        (
            "just under the threshold",
            sleepy(),
            4 * HOUR - 1,
            Some(false),
            Decision::Sample,
        ),
        (
            "pinned",
            Candidate {
                pinned: true,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "focused",
            Candidate {
                focused: true,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "open ask",
            Candidate {
                open_asks: 1,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "working",
            Candidate {
                agent_status: working,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "blocked",
            Candidate {
                agent_status: blocked,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "no agent status",
            Candidate {
                agent_status: None,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "not eligible",
            Candidate {
                eligible: false,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
        (
            "not running",
            Candidate {
                running: false,
                ..sleepy()
            },
            5 * HOUR,
            Some(false),
            Decision::Keep,
        ),
    ];
    for (name, candidate, now, busy, want) in cases {
        assert_eq!(decide(&candidate, now, 4 * HOUR, busy), want, "{name}");
    }
}

#[test]
fn a_short_threshold_samples_from_the_start() {
    let c = Candidate {
        quiet_since_ms: 1_000,
        ..sleepy()
    };
    assert_eq!(decide(&c, 1_000, 1_500, None), Decision::Sample);
    assert_eq!(decide(&c, 2_600, 1_500, None), Decision::Sample);
    assert_eq!(decide(&c, 2_600, 1_500, Some(false)), Decision::Hibernate);
}

#[test]
fn the_meter_needs_two_samples_and_counts_only_the_window() {
    let mut m = BusyMeter::default();
    assert_eq!(m.busy(0), None);
    m.observe(0, 500);
    assert_eq!(m.busy(0), None, "one sample is a baseline");
    m.observe(60_000, 900);
    assert_eq!(m.busy(60_000), Some(false), "400 ms of CPU");
    m.observe(120_000, 3_900);
    assert_eq!(m.busy(120_000), Some(true), "3.4 s in two minutes");
    let later = 120_000 + BUSY_WINDOW_MS + 1;
    m.observe(later, 3_900);
    assert_eq!(m.busy(later), Some(false), "old use left the window");
}

#[test]
fn a_total_that_drops_is_no_use() {
    let mut m = BusyMeter::default();
    m.observe(0, 10_000);
    m.observe(60_000, 100);
    assert_eq!(m.busy(60_000), Some(false));
}

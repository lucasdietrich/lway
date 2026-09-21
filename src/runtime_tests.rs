use crate::{
    restart::RestartDelay,
    support::log_buffer::{MemFdBuffer, MemMapBuffer},
};

use super::*;

fn make_params(restart: RestartPolicy) -> AppParams {
    AppParams {
        cwd: PathBuf::from("."),
        name: "test-app".to_string(),
        prog: "true".to_string(),
        args: vec!["true".to_string()],
        uid: 0,
        gid: 0,
        env: Vec::new(),
        oneshot: false,
        cgroup: AppCgroupConfig {
            cpu_weight: None,
            io_weight: None,
            memory_hard_limit: None,
            memory_soft_limit: None,
            memory_swap_limit: None,
        },
        autostart: false,
        restart,
    }
}

fn make_app(restart: RestartPolicy) -> App {
    App {
        name: "test-app".to_string(),
        params: make_params(restart),
        state: State::default(),
        runtime_stats: AppRuntimeStats::default(),
        stop_requested: false,
        log_buffer: Box::new(
            MemMapBuffer::new_from_memfd(
                MemFdBuffer::new_sealed("log", DEFAULT_LOG_BUFFER_SIZE)
                    .expect("Failed to create log buffer"),
            )
            .expect("Failed to create mmap log buffer"),
        ),
    }
}

fn pending_deadline(app: &App) -> Instant {
    match &app.state {
        State::Stopped(Some(LastExecutionInfo {
            restart_deadline: Some(deadline),
            ..
        })) => *deadline,
        other => panic!("expected Stopped with restart_deadline, got {:?}", other),
    }
}

#[test]
fn schedule_restart_uses_success_or_error_strategy() {
    let policy = RestartPolicy {
        on_success: RestartDelay::Constant { delay_ms: 100 },
        on_error: RestartDelay::Constant { delay_ms: 500 },
        reset_after_ms: 60_000,
        max_restart_attempts: None,
    };
    let mut app = make_app(policy);

    let before = Instant::now();
    app.schedule_restart(ReturnState::Completed { ret: 0 }, Duration::from_secs(5));
    let deadline = pending_deadline(&app);
    assert!(deadline >= before + Duration::from_millis(100));
    assert!(deadline < before + Duration::from_millis(500));
    assert_eq!(app.runtime_stats.consecutive_failures, 0);

    app.state = State::default();
    let before = Instant::now();
    app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::from_secs(5));
    let deadline = pending_deadline(&app);
    assert!(deadline >= before + Duration::from_millis(500));
    assert_eq!(app.runtime_stats.consecutive_failures, 1);
}

#[test]
fn restart_on_failure_only_stops_after_success() {
    let policy = RestartPolicy {
        on_success: RestartDelay::Never,
        on_error: RestartDelay::Constant { delay_ms: 100 },
        reset_after_ms: 60_000,
        max_restart_attempts: None,
    };
    let mut app = make_app(policy);

    // Fails: gets scheduled for restart.
    app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::from_secs(1));
    assert!(matches!(
        app.state,
        State::Stopped(Some(LastExecutionInfo {
            restart_deadline: Some(..),
            ..
        }))
    ));

    // Succeeds: stays put, no restart is scheduled.
    app.state = State::Stopped(Some(LastExecutionInfo {
        restart_deadline: None,
        cause: ReturnState::Completed { ret: 0 },
    }));
    app.schedule_restart(ReturnState::Completed { ret: 0 }, Duration::from_secs(1));
    assert!(matches!(
        app.state,
        State::Stopped(Some(LastExecutionInfo {
            restart_deadline: None,
            cause: ReturnState::Completed { ret: 0 }
        }))
    ));
}

#[test]
fn consecutive_failures_reset_on_success() {
    let policy = RestartPolicy {
        on_success: RestartDelay::default(),
        on_error: RestartDelay::ExponentialBackoff {
            initial_delay_ms: 10,
            max_delay_ms: 1000,
            multiplier: 2.0,
        },
        reset_after_ms: 60_000,
        max_restart_attempts: None,
    };
    let mut app = make_app(policy);

    app.schedule_restart(
        ReturnState::Abnormal { signal: 6 },
        Duration::from_millis(10),
    );
    assert_eq!(app.runtime_stats.consecutive_failures, 1);
    app.state = State::default();

    app.schedule_restart(
        ReturnState::Abnormal { signal: 6 },
        Duration::from_millis(10),
    );
    assert_eq!(app.runtime_stats.consecutive_failures, 2);
    app.state = State::default();

    // A clean exit resets the failure streak.
    app.schedule_restart(ReturnState::Completed { ret: 0 }, Duration::from_secs(1));
    assert_eq!(app.runtime_stats.consecutive_failures, 0);
}

#[test]
fn consecutive_failures_reset_after_long_enough_uptime() {
    let policy = RestartPolicy {
        on_success: RestartDelay::default(),
        on_error: RestartDelay::default(),
        reset_after_ms: 1_000,
        max_restart_attempts: None,
    };
    let mut app = make_app(policy);

    app.schedule_restart(
        ReturnState::Completed { ret: 1 },
        Duration::from_millis(500),
    );
    assert_eq!(app.runtime_stats.consecutive_failures, 1);
    app.state = State::default();

    // Ran longer than reset_after_ms before crashing again: streak resets, then this
    // failure brings it back to 1 rather than 2.
    app.schedule_restart(
        ReturnState::Completed { ret: 1 },
        Duration::from_millis(2_000),
    );
    assert_eq!(app.runtime_stats.consecutive_failures, 1);
}

#[test]
fn max_restart_attempts_stops_scheduling_further_restarts() {
    let policy = RestartPolicy {
        on_success: RestartDelay::default(),
        on_error: RestartDelay::Constant { delay_ms: 0 },
        reset_after_ms: 60_000,
        max_restart_attempts: Some(2),
    };
    let mut app = make_app(policy);

    app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::ZERO);
    assert!(matches!(
        app.state,
        State::Stopped(Some(LastExecutionInfo {
            restart_deadline: Some(..),
            ..
        }))
    ));
    app.state = State::default();

    app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::ZERO);
    assert!(matches!(
        app.state,
        State::Stopped(Some(LastExecutionInfo {
            restart_deadline: Some(..),
            ..
        }))
    ));
    app.state = State::default();

    // Third consecutive failure exceeds max_restart_attempts: give up, no more restarts.
    app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::ZERO);
    assert_eq!(app.runtime_stats.consecutive_failures, 3);
    assert!(matches!(app.state, State::Stopped(..)));
}

#[test]
fn maybe_restart_waits_for_its_deadline() {
    let policy = RestartPolicy {
        on_success: RestartDelay::default(),
        on_error: RestartDelay::Constant { delay_ms: 60_000 },
        reset_after_ms: 60_000,
        max_restart_attempts: None,
    };
    let mut app = make_app(policy);
    app.schedule_restart(ReturnState::Abnormal { signal: 9 }, Duration::ZERO);
    let deadline = app.pending_restart_deadline().expect("pending restart");

    let poll = mio::Poll::new().expect("create poll");
    let mut token_slab = MioTokenSlab::new(8, 0);

    // Deadline is 60s out: nothing should happen yet.
    app.maybe_restart(Instant::now(), &poll, &mut token_slab, true);
    assert_eq!(app.pending_restart_deadline(), Some(deadline));
}

#[test]
fn maybe_restart_cancelled_on_shutdown() {
    let policy = RestartPolicy {
        on_success: RestartDelay::default(),
        on_error: RestartDelay::Constant { delay_ms: 60_000 },
        reset_after_ms: 60_000,
        max_restart_attempts: None,
    };
    let mut app = make_app(policy);
    app.schedule_restart(ReturnState::Abnormal { signal: 9 }, Duration::ZERO);
    assert!(app.pending_restart_deadline().is_some());

    let poll = mio::Poll::new().expect("create poll");
    let mut token_slab = MioTokenSlab::new(8, 0);

    app.maybe_restart(Instant::now(), &poll, &mut token_slab, false);
    assert!(app.pending_restart_deadline().is_none());
    assert!(matches!(
        app.state,
        State::Stopped(Some(LastExecutionInfo {
            restart_deadline: None,
            cause: ReturnState::Abnormal { signal: 9 }
        }))
    ));
}

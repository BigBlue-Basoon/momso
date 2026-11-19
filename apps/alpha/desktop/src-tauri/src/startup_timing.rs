//! Content-free startup timings; other timing traces remain opt-in.
use std::{sync::OnceLock, time::Instant};

static ENABLED: OnceLock<bool> = OnceLock::new();
static ORIGIN: OnceLock<Instant> = OnceLock::new();

fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("MOMSO_STARTUP_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1"))
    })
}

pub(crate) fn begin() {
    ORIGIN.get_or_init(Instant::now);
}

pub(crate) struct Timer {
    stage: &'static str,
    started: Option<Instant>,
}

pub(crate) fn start(stage: &'static str) -> Timer {
    let started = if enabled() {
        begin();
        Some(Instant::now())
    } else {
        None
    };
    Timer { stage, started }
}

pub(crate) fn start_startup(stage: &'static str) -> Timer {
    begin();
    Timer {
        stage,
        started: Some(Instant::now()),
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let (Some(started), Some(origin)) = (self.started, ORIGIN.get()) {
            let ended = Instant::now();
            let line = timing_line(
                self.stage,
                started.duration_since(*origin).as_millis(),
                ended.duration_since(started).as_millis(),
                ended.duration_since(*origin).as_millis(),
            );
            log::info!(target: "momso_startup_timing", "{line}");
        }
    }
}

fn timing_line(stage: &'static str, start_ms: u128, elapsed_ms: u128, end_ms: u128) -> String {
    format!("stage={stage} start_ms={start_ms} elapsed_ms={elapsed_ms} end_ms={end_ms}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_stage_is_timed_without_opt_in() {
        let timer = super::start_startup("runtime_prepare");
        assert!(timer.started.is_some());
    }

    #[test]
    fn startup_timing_line_contains_only_stage_and_milliseconds() {
        assert_eq!(
            super::timing_line("codex_account", 5, 20, 25),
            "stage=codex_account start_ms=5 elapsed_ms=20 end_ms=25"
        );
    }
}

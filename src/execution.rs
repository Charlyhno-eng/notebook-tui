use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Previous,
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Default)]
pub struct Execution {
    pub state: State,
    pub started: Option<Instant>,
    pub elapsed: Option<Duration>,
    pub edited: bool,
    pub queued: bool,
}

impl Execution {
    pub fn start(&mut self) {
        *self = Self {
            state: State::Running,
            started: Some(Instant::now()),
            ..Self::default()
        };
    }

    pub fn finish(&mut self, state: State) {
        self.elapsed = self.started.take().map(|start| start.elapsed());
        self.state = state;
    }

    pub fn duration(&self) -> Option<Duration> {
        self.started.map(|start| start.elapsed()).or(self.elapsed)
    }

    pub fn label(&self) -> String {
        if self.queued {
            return "◷ Queued".into();
        }
        let label = match self.state {
            State::Idle => return String::new(),
            State::Previous => "Previous run".to_string(),
            State::Running => {
                let frame = self.duration().unwrap_or_default().as_millis() / 100 % 4;
                format!("{} Running", ['◐', '◓', '◑', '◒'][frame as usize])
            }
            State::Succeeded => "✓ Completed".to_string(),
            State::Failed => "× Error".to_string(),
            State::Interrupted => "■ Interrupted".to_string(),
        };
        let duration = self
            .duration()
            .map(|d| format!(" · {}", format_duration(d)))
            .unwrap_or_default();
        let edited = if self.edited { " · edited" } else { "" };
        format!("{label}{duration}{edited}")
    }
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}h {:02}m {:02}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_freezes_the_timer_and_rerunning_resets_it() {
        let mut execution = Execution::default();
        execution.start();
        execution.started = Some(Instant::now() - Duration::from_millis(1250));
        assert!(execution.label().contains("Running · 1.3s"));
        execution.finish(State::Succeeded);
        let elapsed = execution.duration();
        assert!(execution.started.is_none());
        assert!(execution.label().starts_with("✓ Completed"));
        assert_eq!(execution.duration(), elapsed);
        execution.edited = true;
        assert!(execution.label().ends_with("edited"));
        execution.start();
        assert!(!execution.edited);
        assert_eq!(execution.elapsed, None);
        assert_eq!(execution.state, State::Running);
    }

    #[test]
    fn durations_remain_readable_for_long_cells() {
        assert_eq!(format_duration(Duration::from_secs(65)), "1m 05s");
        assert_eq!(format_duration(Duration::from_secs(3665)), "1h 01m 05s");
    }
}

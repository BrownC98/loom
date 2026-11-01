use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

const WINDOW: usize = 120;
const IDLE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct State {
    begun: Option<Instant>,
    last_end: Option<Instant>,
    intervals: Vec<f32>,
    work: Vec<f32>,
}

#[derive(Clone, Default)]
pub struct Probe(Rc<RefCell<State>>);

fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q).round() as usize]
}

fn summary(label: &str, values: &mut [f32]) -> String {
    values.sort_by(f32::total_cmp);
    format!(
        "{label} p50 {:.2} ms p95 {:.2} ms max {:.2} ms",
        percentile(values, 0.5),
        percentile(values, 0.95),
        values[values.len() - 1]
    )
}

impl Probe {
    pub fn begin(&self) {
        self.0.borrow_mut().begun = Some(Instant::now());
    }

    pub fn end(&self) {
        let now = Instant::now();
        let mut state = self.0.borrow_mut();
        if let Some(begun) = state.begun.take() {
            state.work.push((now - begun).as_secs_f32() * 1000.0);
        }
        if let Some(last) = state.last_end.replace(now) {
            let gap = now - last;
            if gap < IDLE {
                state.intervals.push(gap.as_secs_f32() * 1000.0);
            }
        }
        if state.intervals.len() >= WINDOW {
            let line = format!(
                "frames {}: {}; {}",
                state.intervals.len(),
                summary("interval", &mut state.intervals),
                summary("render+paint", &mut state.work),
            );
            let _ = writeln!(std::io::stderr(), "{line}");
            state.intervals.clear();
            state.work.clear();
        }
    }
}

//! `MARGINAL_TRACE=1` prints any main-thread work slower than a frame budget, so stalls can be
//! measured instead of guessed at. `MARGINAL_TRACE=all` prints every timed span.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Work slower than this is reported.
const BUDGET: Duration = Duration::from_millis(8);

fn mode() -> Option<bool> {
    static MODE: OnceLock<Option<bool>> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("MARGINAL_TRACE").as_deref() {
        Ok("all") => Some(true),
        Ok(v) if !v.is_empty() && v != "0" => Some(false),
        _ => None,
    })
}

/// Times until dropped; reports if over budget (or always, under `MARGINAL_TRACE=all`).
pub struct Span {
    label: String,
    start: Instant,
    all: bool,
}

pub fn span(label: impl FnOnce() -> String) -> Option<Span> {
    let all = mode()?;
    Some(Span { label: label(), start: Instant::now(), all })
}

impl Drop for Span {
    fn drop(&mut self) {
        let took = self.start.elapsed();
        if self.all || took > BUDGET {
            eprintln!("[trace] {:>7.1} ms  {}", took.as_secs_f64() * 1000.0, self.label);
        }
    }
}

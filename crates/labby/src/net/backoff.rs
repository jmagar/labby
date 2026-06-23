use std::time::Duration;

#[must_use]
pub fn reprobe_backoff(attempt: u32) -> Duration {
    let seconds = match attempt {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        5 => 32,
        _ => 60,
    };
    Duration::from_secs(seconds)
}

pub fn jitter_window(delay: Duration) -> (Duration, Duration) {
    let millis = delay.as_millis() as u64;
    let spread = millis / 5;
    let min = millis.saturating_sub(spread);
    let max = millis.saturating_add(spread);
    (Duration::from_millis(min), Duration::from_millis(max))
}

pub fn jitter_delay(delay: Duration, seed: u64) -> Duration {
    let (min, max) = jitter_window(delay);
    let min_ms = min.as_millis() as u64;
    let max_ms = max.as_millis() as u64;
    if min_ms >= max_ms {
        return min;
    }
    let width = max_ms - min_ms;
    Duration::from_millis(min_ms + (seed % (width + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprobe_backoff_caps_and_jitters() {
        for attempt in 0..10_u32 {
            let backoff = reprobe_backoff(attempt);
            assert!(
                backoff.as_secs() <= 60,
                "backoff {attempt} exceeded 60s: {backoff:?}"
            );
            let jittered = jitter_delay(backoff, 17);
            assert!(
                jittered.as_millis() > 0,
                "jitter {attempt} collapsed to zero"
            );
        }
    }
}

//! The input-latency sample event.
//!
//! The game publishes one `__LATENCY__` event per followed input (see
//! `pulsar_game::latency`): its start is the OS event's time, its duration the
//! input-to-present total, and its metadata the stages in chain order as
//! `stage=milliseconds;stage=milliseconds;...`. The stage names belong to the
//! publisher; this module only reads them back in the order they were written.

use profiling::ProfileEvent;

/// Name of the event.
pub const EVENT_NAME: &str = "__LATENCY__";

/// The stages of one sample, in chain order, in milliseconds.
pub fn parse_stages(metadata: &str) -> Vec<(String, f64)> {
    metadata
        .split(';')
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.trim().to_owned(), value.trim().parse::<f64>().ok()?))
        })
        .collect()
}

/// Encode stages the way [`parse_stages`] reads them.
pub fn encode_stages(stages: &[(&str, f32)]) -> String {
    let mut out = String::new();
    for (i, (name, ms)) in stages.iter().enumerate() {
        if i > 0 {
            out.push(';');
        }
        out.push_str(&format!("{name}={ms:.3}"));
    }
    out
}

pub fn is_sample(e: &ProfileEvent) -> bool {
    e.name == EVENT_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_round_trip_in_order() {
        let text = encode_stages(&[("os_to_tick", 1.5), ("present", 0.25)]);
        assert_eq!(
            parse_stages(&text),
            vec![("os_to_tick".to_owned(), 1.5), ("present".to_owned(), 0.25)]
        );
        assert!(parse_stages("garbage").is_empty());
    }
}

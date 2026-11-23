//! One camera's motion event, from detector samples to event actions. The
//! daemon sends them as `event.start` / `event.update` / `event.end`.
use crate::detector::Sample;
use std::collections::BTreeSet;

/// How often an open event's peak score, area and zones are sent.
pub const UPDATE_INTERVAL_MS: i64 = 2000;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Start {
        score: f32,
    },
    Update {
        peak: f32,
        bbox: Option<[f32; 4]>,
        zones: Vec<String>,
    },
    End,
}

#[derive(Debug, Default)]
pub struct Activity {
    open: bool,
    updated_ms: i64,
    peak: f32,
    bbox: Option<[f32; 4]>,
    zones: BTreeSet<String>,
}

impl Activity {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn observe(&mut self, sample: &Sample, now_ms: i64) -> Vec<Action> {
        let mut actions = Vec::new();
        if sample.active && !self.open {
            actions.push(Action::Start {
                score: sample.score,
            });
            self.open = true;
            // The first update follows at once, with the starting picture's area.
            self.updated_ms = now_ms - UPDATE_INTERVAL_MS;
        }
        if self.open {
            self.peak = self.peak.max(sample.score);
            // The event's box is where it started: the largest moving area on
            // its first picture (near its thumbnail's moment). Widened over the
            // event it grew to cover everything that moved for its whole life.
            if self.bbox.is_none() {
                self.bbox = sample.boxes.first().copied().or(sample.bbox);
            }
            self.zones.extend(sample.zones.iter().cloned());
            if !sample.active {
                actions.extend(self.finish());
            } else if now_ms - self.updated_ms >= UPDATE_INTERVAL_MS {
                actions.push(self.update());
                self.updated_ms = now_ms;
            }
        }
        actions
    }

    /// End an open event: when the detector goes quiet, and on a gap in the
    /// stream, while the camera moves and on stop.
    pub fn finish(&mut self) -> Vec<Action> {
        if !self.open {
            return Vec::new();
        }
        let update = self.update();
        self.open = false;
        self.peak = 0.0;
        self.bbox = None;
        self.zones.clear();
        vec![update, Action::End]
    }

    fn update(&self) -> Action {
        Action::Update {
            peak: self.peak,
            bbox: self.bbox,
            zones: self.zones.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(active: bool, score: f32, bbox: Option<[f32; 4]>, zones: &[&str]) -> Sample {
        Sample {
            active,
            score,
            bbox,
            boxes: bbox.into_iter().collect(),
            zones: zones.iter().map(|z| z.to_string()).collect(),
            blocks: vec![],
        }
    }

    #[test]
    fn quiet_samples_produce_nothing() {
        let mut activity = Activity::default();
        assert!(activity
            .observe(&sample(false, 0.0, None, &[]), 0)
            .is_empty());
        assert!(activity.finish().is_empty());
    }

    #[test]
    fn an_event_starts_updates_at_once_and_then_every_interval() {
        let mut activity = Activity::default();
        let start = activity.observe(
            &sample(true, 0.2, Some([0.1, 0.1, 0.2, 0.2]), &["door"]),
            1000,
        );
        assert_eq!(
            start,
            vec![
                Action::Start { score: 0.2 },
                Action::Update {
                    peak: 0.2,
                    bbox: Some([0.1, 0.1, 0.2, 0.2]),
                    zones: vec!["door".into()]
                }
            ]
        );
        assert!(activity
            .observe(&sample(true, 0.5, None, &[]), 2999)
            .is_empty());
        let update = activity.observe(
            &sample(true, 0.3, Some([0.5, 0.5, 0.1, 0.1]), &["yard"]),
            3000,
        );
        let [Action::Update { peak, bbox, zones }] = update.as_slice() else {
            panic!("expected one update, got {update:?}");
        };
        assert_eq!(*peak, 0.5);
        // The event's box stays where it started (its thumbnail's moment); it
        // no longer widens to every later moving area (2026-11-23).
        assert_eq!(*bbox, Some([0.1, 0.1, 0.2, 0.2]));
        assert_eq!(zones, &["door", "yard"]);
    }

    #[test]
    fn going_quiet_or_finishing_ends_with_a_final_update_and_resets() {
        let mut activity = Activity::default();
        activity.observe(&sample(true, 0.4, None, &["door"]), 0);
        let end = activity.observe(&sample(false, 0.0, None, &[]), 500);
        assert_eq!(end.last(), Some(&Action::End));
        assert!(!activity.is_open());

        activity.observe(&sample(true, 0.1, None, &[]), 1000);
        let stopped = activity.finish();
        assert_eq!(
            stopped,
            vec![
                Action::Update {
                    peak: 0.1,
                    bbox: None,
                    zones: vec![]
                },
                Action::End
            ]
        );
    }
}

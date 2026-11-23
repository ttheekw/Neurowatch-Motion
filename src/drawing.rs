//! What the plugin draws over the camera's picture: the moving area while
//! there is motion (`detections`), and what the detector sees (`debug`):
//! the blocks that changed, the zones and the score.
use crate::detector::{Sample, COLUMNS, ROWS};
use crate::settings::{MotionSettings, ZoneKind};
use nw_analytics_protocol::messages::{Anchor, OverlayItem, Style};

/// A box per moving area while motion is reported; the largest carries the
/// score, the others say "Motion".
pub fn detections(sample: &Sample) -> Vec<OverlayItem> {
    if !sample.active {
        return vec![];
    }
    sample
        .boxes
        .iter()
        .enumerate()
        .map(|(at, rect)| OverlayItem::Box {
            rect: *rect,
            label: Some(if at == 0 {
                format!("Motion {:.0}%", sample.score * 100.0)
            } else {
                "Motion".to_owned()
            }),
            style: Style::Warning,
            fill: false,
        })
        .collect()
}

/// The changed blocks, the zones and a line of numbers.
pub fn debug(sample: &Sample, settings: &MotionSettings, fps: f32) -> Vec<OverlayItem> {
    let (width, height) = (1.0 / COLUMNS as f32, 1.0 / ROWS as f32);
    // One box per run of changed blocks side by side in a row: a full row is
    // one item, not COLUMNS (the browser's message is size-limited).
    let style = if sample.active {
        Style::Warning
    } else {
        Style::Muted
    };
    let mut blocks = sample.blocks.clone();
    blocks.sort_unstable();
    blocks.dedup();
    let mut items: Vec<OverlayItem> = Vec::new();
    let mut at = 0;
    while at < blocks.len() {
        let first = usize::from(blocks[at]);
        let (column, row) = (first % COLUMNS, first / COLUMNS);
        let mut length = 1;
        while at + length < blocks.len()
            && usize::from(blocks[at + length]) == first + length
            && column + length < COLUMNS
        {
            length += 1;
        }
        at += length;
        items.push(OverlayItem::Box {
            rect: [
                column as f32 * width,
                row as f32 * height,
                length as f32 * width,
                height,
            ],
            label: None,
            style,
            fill: true,
        });
    }
    items.extend(settings.zones.iter().filter(|z| z.is_valid()).map(|zone| {
        let watched = zone.kind == ZoneKind::Include;
        OverlayItem::Polygon {
            points: zone.points.clone(),
            label: Some(if zone.name.is_empty() {
                zone.id.clone()
            } else {
                zone.name.clone()
            }),
            style: if watched {
                Style::Accent
            } else {
                Style::Danger
            },
            fill: !watched,
        }
    }));
    let state = if sample.active { "motion" } else { "quiet" };
    items.push(OverlayItem::Text {
        anchor: Anchor::BottomLeft,
        lines: vec![
            format!(
                "Score {:.1}% of {:.1}% needed · {state}",
                sample.score * 100.0,
                settings.min_area() * 100.0
            ),
            format!("{} blocks changed · {fps:.1} fps", sample.blocks.len()),
        ],
        style: if sample.active {
            Style::Warning
        } else {
            Style::Muted
        },
    });
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Zone;

    fn sample(active: bool) -> Sample {
        Sample {
            active,
            score: 0.25,
            bbox: Some([0.1, 0.2, 0.3, 0.4]),
            boxes: vec![[0.1, 0.2, 0.3, 0.4]],
            zones: vec![],
            blocks: vec![0, (COLUMNS + 1) as u16],
        }
    }

    #[test]
    fn the_moving_area_is_drawn_only_while_motion_is_reported() {
        let drawn = detections(&sample(true));
        assert_eq!(drawn.len(), 1);
        assert!(
            matches!(&drawn[0], OverlayItem::Box { rect, label: Some(label), .. }
            if *rect == [0.1, 0.2, 0.3, 0.4] && label == "Motion 25%")
        );
        assert!(detections(&sample(false)).is_empty());
    }

    #[test]
    fn a_full_row_of_changed_blocks_is_one_box() {
        let mut active = sample(true);
        // Row 2 whole, plus a gap-separated pair in row 3 and one at its end.
        active.blocks = (0..COLUMNS).map(|c| (2 * COLUMNS + c) as u16).collect();
        active.blocks.extend([
            (3 * COLUMNS) as u16,
            (3 * COLUMNS + 1) as u16,
            (3 * COLUMNS + 3) as u16,
            (4 * COLUMNS - 1) as u16,
            (4 * COLUMNS) as u16, // next row's first: not joined to the end of row 3
        ]);
        let drawn = debug(&active, &MotionSettings::default(), 5.0);
        let (w, h) = (1.0 / COLUMNS as f32, 1.0 / ROWS as f32);
        let boxes: Vec<[f32; 4]> = drawn
            .iter()
            .filter_map(|item| match item {
                OverlayItem::Box {
                    rect, fill: true, ..
                } => Some(*rect),
                _ => None,
            })
            .collect();
        assert_eq!(boxes.len(), 5, "{boxes:?}");
        assert!((boxes[0][0]).abs() < 1e-6 && (boxes[0][2] - 1.0).abs() < 1e-4);
        assert!((boxes[0][1] - 2.0 * h).abs() < 1e-6);
        assert!((boxes[1][2] - 2.0 * w).abs() < 1e-6, "adjacent pair merged");
        assert!(matches!(drawn.last(), Some(OverlayItem::Text { .. })));
    }

    #[test]
    fn debug_drawings_show_blocks_zones_and_numbers() {
        let settings = MotionSettings {
            zones: vec![
                Zone {
                    id: "door".into(),
                    name: "Door".into(),
                    kind: ZoneKind::Include,
                    points: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5]],
                },
                Zone {
                    id: "road".into(),
                    name: String::new(),
                    kind: ZoneKind::Exclude,
                    points: vec![[0.5, 0.5], [1.0, 0.5], [1.0, 1.0]],
                },
            ],
            ..MotionSettings::default()
        };
        let drawn = debug(&sample(false), &settings, 5.0);
        assert_eq!(drawn.len(), 5);
        let (w, h) = (1.0 / COLUMNS as f32, 1.0 / ROWS as f32);
        assert!(
            matches!(&drawn[1], OverlayItem::Box { rect, fill: true, style: Style::Muted, .. }
            if (rect[0] - w).abs() < 1e-6 && (rect[1] - h).abs() < 1e-6)
        );
        assert!(
            matches!(&drawn[2], OverlayItem::Polygon { label: Some(l), style: Style::Accent, .. } if l == "Door")
        );
        assert!(
            matches!(&drawn[3], OverlayItem::Polygon { label: Some(l), style: Style::Danger, fill: true, .. } if l == "road")
        );
        assert!(matches!(&drawn[4], OverlayItem::Text { lines, .. }
            if lines[0] == "Score 25.0% of 6.0% needed · quiet" && lines[1] == "2 blocks changed · 5.0 fps"));
    }
}

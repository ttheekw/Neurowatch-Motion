//! Fixed-size, deterministic motion analysis. No media, clock, IPC or database I/O.
use crate::settings::{MotionSettings, Zone, ZoneKind};

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 180;
const W: usize = WIDTH / 2;
const H: usize = HEIGHT / 2;
const BLOCK: usize = 10;
pub const COLUMNS: usize = W / BLOCK;
pub const ROWS: usize = H / BLOCK;
const BLOCKS: usize = COLUMNS * ROWS;
/// A pixel that has differed from the background this long is background:
/// what stopped moving (or left the first picture) stops counting as motion.
const STALE_MS: i64 = 5_000;
/// After a (re)start, the background is the per-pixel median of this long of
/// pictures, and nothing is reported meanwhile.
const WARMUP_MS: i64 = 1_000;
/// At most this many warm-up pictures are kept (a stalled clock ends it too).
const WARMUP_PICTURES: usize = 25;

#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub active: bool,
    pub score: f32,
    /// Around everything that moved (events).
    pub bbox: Option<[f32; 4]>,
    /// One per moving area (drawings).
    pub boxes: Vec<[f32; 4]>,
    pub zones: Vec<String>,
    pub blocks: Vec<u16>,
}

pub struct Detector {
    background: Vec<u16>,
    /// Pictures since the (re)start, until the background is learned from them.
    warmup: Vec<Vec<u8>>,
    warmup_from: Option<i64>,
    /// Per pixel: since when it has differed from the background.
    changed_since: Vec<Option<i64>>,
    mask: Vec<bool>,
    zones: Vec<(String, Vec<bool>)>,
    threshold: u8,
    min_area: f32,
    duration_ms: i64,
    cooldown_ms: i64,
    previous_ms: Option<i64>,
    candidate_ms: Option<i64>,
    last_motion_ms: i64,
    active: bool,
}

impl Detector {
    pub fn new(settings: &MotionSettings) -> Self {
        let zones: Vec<_> = settings
            .zones
            .iter()
            .filter(|zone| zone.is_valid())
            .collect();
        let includes = zones.iter().any(|zone| zone.kind == ZoneKind::Include);
        let raster: Vec<_> = zones
            .iter()
            .map(|zone| {
                let pixels: Vec<_> = (0..W * H)
                    .map(|i| {
                        contains(
                            zone,
                            (i % W) as f32 / W as f32 + 0.5 / W as f32,
                            (i / W) as f32 / H as f32 + 0.5 / H as f32,
                        )
                    })
                    .collect();
                (*zone, pixels)
            })
            .collect();
        let mask = (0..W * H)
            .map(|i| {
                (!includes
                    || raster
                        .iter()
                        .any(|(zone, pixels)| zone.kind == ZoneKind::Include && pixels[i]))
                    && !raster
                        .iter()
                        .any(|(zone, pixels)| zone.kind == ZoneKind::Exclude && pixels[i])
            })
            .collect();
        Self {
            background: Vec::new(),
            warmup: Vec::new(),
            warmup_from: None,
            changed_since: vec![None; W * H],
            mask,
            zones: raster
                .into_iter()
                .filter(|(zone, _)| zone.kind == ZoneKind::Include)
                .map(|(zone, pixels)| (zone.id.clone(), pixels))
                .collect(),
            threshold: settings.pixel_threshold(),
            min_area: settings.min_area(),
            duration_ms: seconds_ms(settings.min_duration_sec),
            cooldown_ms: seconds_ms(settings.cooldown_sec),
            previous_ms: None,
            candidate_ms: None,
            last_motion_ms: 0,
            active: false,
        }
    }

    pub fn observe(&mut self, frame: &[u8], now_ms: i64) -> Result<Sample, &'static str> {
        if frame.len() != WIDTH * HEIGHT {
            return Err("expected 320 by 180 GRAY8 frame");
        }
        if self.previous_ms.is_some_and(|previous| now_ms < previous) {
            return Err("frame clock moved backwards");
        }
        self.previous_ms = Some(now_ms);
        // Box smoothing/downscale suppresses isolated sensor noise.
        let pixels: Vec<u8> = (0..W * H)
            .map(|i| {
                let at = (i / W * 2) * WIDTH + i % W * 2;
                ((u32::from(frame[at])
                    + u32::from(frame[at + 1])
                    + u32::from(frame[at + WIDTH])
                    + u32::from(frame[at + WIDTH + 1]))
                    / 4) as u8
            })
            .collect();
        if self.background.is_empty() {
            // The background is what each pixel shows most of a short while:
            // learned from one picture, whatever moved through it stayed behind
            // as a "change" (a box where nothing was) until it went stale.
            let from = *self.warmup_from.get_or_insert(now_ms);
            self.warmup.push(pixels);
            if now_ms.saturating_sub(from) >= WARMUP_MS || self.warmup.len() >= WARMUP_PICTURES {
                self.background = median(&self.warmup);
                self.warmup.clear();
                self.warmup_from = None;
            }
            return Ok(Sample::default());
        }
        let mut changed: Vec<bool> = pixels
            .iter()
            .zip(&self.background)
            .map(|(pixel, background)| pixel.abs_diff((background / 256) as u8) > self.threshold)
            .collect();
        // Detect lighting changes over the whole image, not just a tiny include
        // zone: otherwise a person filling that zone would always be suppressed.
        if changed.iter().filter(|v| **v).count() * 10 > pixels.len() * 6 {
            self.background.clear();
            self.warmup = vec![pixels];
            self.warmup_from = Some(now_ms);
            self.changed_since.fill(None);
            self.candidate_ms = None;
            self.active = false;
            return Ok(Sample::default());
        }
        // A changed pixel learns the background only slowly (below), so what
        // stopped moving, or left the first picture, would count as motion for
        // minutes and stretch every box to it. Past STALE_MS it is background.
        for i in 0..W * H {
            if !changed[i] {
                self.changed_since[i] = None;
                continue;
            }
            let since = *self.changed_since[i].get_or_insert(now_ms);
            if now_ms.saturating_sub(since) >= STALE_MS {
                self.background[i] = u16::from(pixels[i]) * 256;
                self.changed_since[i] = None;
                changed[i] = false;
            }
        }
        let mut area = [0u32; BLOCKS];
        let mut movement = [0u32; BLOCKS];
        for i in 0..W * H {
            if self.mask[i] {
                let block = (i / W / BLOCK) * COLUMNS + (i % W / BLOCK);
                area[block] += 1;
                movement[block] += u32::from(changed[i]);
            }
            let target = i32::from(pixels[i]) * 256;
            let current = i32::from(self.background[i]);
            self.background[i] =
                (current + (target - current) / if changed[i] { 256 } else { 32 }) as u16;
        }
        let watched = area.iter().filter(|a| **a > 0).count();
        let blocks: Vec<usize> = (0..BLOCKS)
            .filter(|i| area[*i] > 0 && movement[*i] >= 2 && movement[*i] * 5 >= area[*i])
            .collect();
        let mut active_blocks = [false; BLOCKS];
        for block in &blocks {
            active_blocks[*block] = true;
        }
        let score = if watched == 0 {
            0.0
        } else {
            blocks.len() as f32 / watched as f32
        };
        if score >= self.min_area && !blocks.is_empty() {
            let candidate = *self.candidate_ms.get_or_insert(now_ms);
            self.last_motion_ms = now_ms;
            if now_ms.saturating_sub(candidate) >= self.duration_ms {
                self.active = true;
            }
        } else {
            self.candidate_ms = None;
            if self.active && now_ms.saturating_sub(self.last_motion_ms) >= self.cooldown_ms {
                self.active = false;
            }
        }
        // One box per group of touching moving blocks, around the changed
        // pixels in it (not the blocks: snapped to the 16x9 grid a box sat up to
        // a block off its object). One box around everything spanned from the
        // ball to the camera's clock across the picture.
        let groups = groups(&active_blocks);
        let mut extents: Vec<Option<(usize, usize, usize, usize)>> = vec![None; groups.count];
        for i in 0..W * H {
            let block = (i / W / BLOCK) * COLUMNS + (i % W / BLOCK);
            let Some(group) = groups.of[block] else {
                continue;
            };
            if self.mask[i] && changed[i] {
                let (x, y) = (i % W, i / W);
                extents[group] = Some(match extents[group] {
                    None => (x, y, x + 1, y + 1),
                    Some((x1, y1, x2, y2)) => (x1.min(x), y1.min(y), x2.max(x + 1), y2.max(y + 1)),
                });
            }
        }
        let rect = |(x1, y1, x2, y2): (usize, usize, usize, usize)| {
            [
                x1 as f32 / W as f32,
                y1 as f32 / H as f32,
                (x2 - x1) as f32 / W as f32,
                (y2 - y1) as f32 / H as f32,
            ]
        };
        let all = extents
            .iter()
            .flatten()
            .copied()
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
        let bbox = all.map(rect);
        let mut boxes: Vec<[f32; 4]> = extents.into_iter().flatten().map(rect).collect();
        // The largest first: it carries the label.
        boxes.sort_by(|a, b| (b[2] * b[3]).total_cmp(&(a[2] * a[3])));
        let zones = self
            .zones
            .iter()
            .filter(|(_, zone)| {
                (0..W * H).any(|i| {
                    zone[i]
                        && self.mask[i]
                        && changed[i]
                        && active_blocks[(i / W / BLOCK) * COLUMNS + (i % W / BLOCK)]
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        Ok(Sample {
            active: self.active,
            score,
            bbox,
            boxes,
            zones,
            blocks: blocks.into_iter().map(|i| i as u16).collect(),
        })
    }
}

/// Per pixel, the middle value of the pictures, as a background.
fn median(pictures: &[Vec<u8>]) -> Vec<u16> {
    let mut values = Vec::with_capacity(pictures.len());
    (0..W * H)
        .map(|i| {
            values.clear();
            values.extend(pictures.iter().map(|picture| picture[i]));
            values.sort_unstable();
            u16::from(values[values.len() / 2]) * 256
        })
        .collect()
}

/// Touching moving blocks (sides or corners), numbered.
struct Groups {
    of: [Option<usize>; BLOCKS],
    count: usize,
}

fn groups(active: &[bool; BLOCKS]) -> Groups {
    let mut found = Groups {
        of: [None; BLOCKS],
        count: 0,
    };
    let mut stack = Vec::new();
    for start in 0..BLOCKS {
        if !active[start] || found.of[start].is_some() {
            continue;
        }
        found.of[start] = Some(found.count);
        stack.push(start);
        while let Some(block) = stack.pop() {
            let (column, row) = ((block % COLUMNS) as isize, (block / COLUMNS) as isize);
            for (dx, dy) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (x, y) = (column + dx, row + dy);
                if x < 0 || y < 0 || x >= COLUMNS as isize || y >= ROWS as isize {
                    continue;
                }
                let next = y as usize * COLUMNS + x as usize;
                if active[next] && found.of[next].is_none() {
                    found.of[next] = Some(found.count);
                    stack.push(next);
                }
            }
        }
        found.count += 1;
    }
    found
}

fn seconds_ms(seconds: f32) -> i64 {
    if seconds.is_finite() {
        (seconds.clamp(0.0, 3600.0) * 1000.0).round() as i64
    } else {
        0
    }
}

fn contains(zone: &Zone, x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut previous = zone.points.len() - 1;
    for (i, [ax, ay]) in zone.points.iter().copied().enumerate() {
        let [bx, by] = zone.points[previous];
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
        previous = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{MotionSettings, Zone, ZoneKind};

    /// A detector that has learned `scene` (still) through its warm-up, ending at 0 ms.
    fn warmed(settings: &MotionSettings, scene: &[u8]) -> Detector {
        let mut detector = Detector::new(settings);
        let mut at = -WARMUP_MS;
        while at <= 0 {
            assert!(detector.observe(scene, at).unwrap().blocks.is_empty());
            at += 200;
        }
        detector
    }

    fn config() -> MotionSettings {
        MotionSettings {
            min_duration_sec: 0.4,
            cooldown_sec: 1.0,
            ..Default::default()
        }
    }
    fn frame(moving: bool) -> Vec<u8> {
        let mut frame = vec![30; WIDTH * HEIGHT];
        if moving {
            for y in 40..100 {
                for x in 30..110 {
                    frame[y * WIDTH + x] = 180;
                }
            }
        }
        frame
    }
    fn zone(kind: ZoneKind, x: f32) -> Zone {
        Zone {
            id: "entrance".into(),
            name: "Entrance".into(),
            kind,
            points: vec![[0., 0.], [x, 0.], [x, 1.], [0., 1.]],
        }
    }
    #[test]
    fn still_scene_never_starts_and_lighting_change_reinitializes() {
        let mut detector = Detector::new(&config());
        for at in [0, 200, 400, 600] {
            assert!(!detector.observe(&frame(false), at).unwrap().active);
        }
        assert!(
            !detector
                .observe(&vec![180; WIDTH * HEIGHT], 800)
                .unwrap()
                .active
        );
        assert!(
            !detector
                .observe(&vec![180; WIDTH * HEIGHT], 1000)
                .unwrap()
                .active
        );
    }
    #[test]
    fn minimum_duration_and_cooldown_use_elapsed_time_not_frame_count() {
        let mut detector = warmed(&config(), &frame(false));
        assert!(!detector.observe(&frame(true), 200).unwrap().active);
        assert!(!detector.observe(&frame(true), 400).unwrap().active);
        let started = detector.observe(&frame(true), 600).unwrap();
        assert!(started.active);
        assert!(started.score > 0.02);
        assert!(started.bbox.is_some());
        assert!(detector.observe(&frame(false), 800).unwrap().active);
        assert!(detector.observe(&frame(false), 1400).unwrap().active);
        assert!(!detector.observe(&frame(false), 1600).unwrap().active);
    }
    #[test]
    fn exclusion_masks_and_include_zone_names_are_respected() {
        let mut settings = config();
        settings.zones = vec![zone(ZoneKind::Exclude, 0.5)];
        let mut detector = warmed(&settings, &frame(false));
        for at in [200, 600, 1000] {
            assert!(!detector.observe(&frame(true), at).unwrap().active);
        }
        settings.zones = vec![zone(ZoneKind::Include, 0.5)];
        let mut detector = warmed(&settings, &frame(false));
        detector.observe(&frame(true), 200).unwrap();
        let sample = detector.observe(&frame(true), 600).unwrap();
        assert!(sample.active);
        assert_eq!(sample.zones, vec!["entrance"]);
    }
    #[test]
    fn malformed_frames_and_backwards_times_are_rejected_without_state_changes() {
        let mut detector = Detector::new(&config());
        assert!(detector.observe(&[0; 3], 0).is_err());
        detector.observe(&frame(false), 100).unwrap();
        assert!(detector.observe(&frame(true), 99).is_err());
        assert!(!detector.observe(&frame(false), 200).unwrap().active);
    }

    /// A frame with a bright square at `x` (in 320-wide pixels).
    fn square_at(x: usize) -> Vec<u8> {
        let mut frame = vec![30; WIDTH * HEIGHT];
        for row in 40..100 {
            for column in x..x + 40 {
                frame[row * WIDTH + column] = 180;
            }
        }
        frame
    }

    /// Feeds `frame` at 5 fps from `from_ms` to `to_ms`; returns the last sample.
    fn feed(detector: &mut Detector, frame: &[u8], from_ms: i64, to_ms: i64) -> Sample {
        let mut sample = Sample::default();
        let mut at = from_ms;
        while at <= to_ms {
            sample = detector.observe(frame, at).unwrap();
            at += 200;
        }
        sample
    }

    #[test]
    fn what_was_there_at_the_start_stops_counting_once_it_has_gone() {
        // The square is in the first picture, so it is learned as background;
        // when it leaves, the place it left must not stay "changed" for long.
        // Still through the warm-up, so it is part of the scene; then it leaves.
        let mut detector = warmed(&config(), &square_at(20));
        feed(&mut detector, &frame(false), 200, STALE_MS + 400);
        let sample = feed(
            &mut detector,
            &frame(false),
            STALE_MS + 600,
            STALE_MS + 2_000,
        );
        assert!(sample.blocks.is_empty(), "ghost blocks {:?}", sample.blocks);
        assert!(!sample.active);
        assert_eq!(sample.bbox, None);
    }

    #[test]
    fn something_that_arrives_and_stays_still_stops_being_motion() {
        let mut detector = warmed(&config(), &frame(false));
        let arrived = feed(&mut detector, &frame(true), 200, 1_000);
        assert!(arrived.active);
        // Parked for longer than STALE_MS plus the cooldown: no motion, no box.
        let parked = feed(&mut detector, &frame(true), 1_200, 1_200 + STALE_MS + 2_000);
        assert!(
            parked.blocks.is_empty(),
            "parked blocks {:?}",
            parked.blocks
        );
        assert!(!parked.active);
        assert_eq!(parked.bbox, None);
    }

    #[test]
    fn a_stale_change_does_not_anchor_the_box_of_new_motion() {
        // A square learned at the left, gone, then new motion at the right:
        // the box must cover only the new motion, not stretch to the left.
        // Still through the warm-up, so it is part of the scene; then it leaves.
        let mut detector = warmed(&config(), &square_at(20));
        feed(&mut detector, &frame(false), 200, STALE_MS + 400);
        let mut sample = Sample::default();
        let mut at = STALE_MS + 600;
        for x in (200..260).step_by(10) {
            sample = detector.observe(&square_at(x), at).unwrap();
            at += 200;
        }
        let [x, _, _, _] = sample.bbox.expect("the new motion has a box");
        assert!(x > 0.5, "box starts at x = {x}, anchored at the old spot");
    }

    #[test]
    fn something_moving_steadily_stays_motion_past_the_stale_time() {
        // Each pixel changes only briefly as the square crosses: never stale.
        let mut detector = warmed(&config(), &frame(false));
        let mut at = 200;
        let mut sample = Sample::default();
        for step in 0..60 {
            sample = detector.observe(&square_at(10 + step * 4), at).unwrap();
            at += 200;
        }
        assert!(at > STALE_MS * 2);
        assert!(sample.active);
        assert!(sample.bbox.is_some());
    }

    #[test]
    fn the_box_follows_the_changed_pixels_not_the_block_grid() {
        // The moving area spans x 30..110 and y 40..100 of 320x180: its left
        // edge is mid-block. Snapped to the 16x9 grid the box began at 0.0625,
        // a quarter block off on every stream's tile (2026-11-23).
        let mut detector = warmed(&config(), &frame(false));
        let [x, y, w, h] = detector.observe(&frame(true), 200).unwrap().bbox.unwrap();
        let near = |a: f32, b: f32| (a - b).abs() <= 1.0 / W as f32 + 1e-4;
        assert!(near(x, 30.0 / 320.0), "x {x}");
        assert!(near(x + w, 110.0 / 320.0), "right {}", x + w);
        assert!(near(y, 40.0 / 180.0), "y {y}");
        assert!(near(y + h, 100.0 / 180.0), "bottom {}", y + h);
    }

    #[test]
    fn something_moving_through_the_first_pictures_leaves_no_ghost() {
        // Motion restarts its detector whenever its picture source changes (the
        // wall switching between a grid and one camera). It learned the first
        // picture as background, ball included, so the spot the ball had left
        // showed as changed for seconds: a box where nothing was (2026-11-23).
        let mut detector = Detector::new(&config());
        let mut at = 0;
        let mut x = 20;
        while at <= WARMUP_MS {
            detector.observe(&square_at(x), at).unwrap();
            x += 30;
            at += 200;
        }
        let sample = detector.observe(&square_at(250), at).unwrap();
        let first = 250 / 2 / BLOCK - 1;
        assert!(!sample.blocks.is_empty());
        assert!(
            sample
                .blocks
                .iter()
                .all(|b| usize::from(*b) % COLUMNS >= first),
            "blocks away from the square: {:?}",
            sample.blocks
        );
    }

    #[test]
    fn two_moving_areas_get_a_box_each() {
        // One box around everything that changed spanned from the ball to the
        // camera's clock across the picture (2026-11-23).
        let two = {
            let mut frame = vec![30; WIDTH * HEIGHT];
            for row in 40..100 {
                for column in (20..80).chain(240..300) {
                    frame[row * WIDTH + column] = 180;
                }
            }
            frame
        };
        let mut detector = warmed(&config(), &frame(false));
        let mut sample = Sample::default();
        for at in [200, 400, 600] {
            sample = detector.observe(&two, at).unwrap();
        }
        assert!(sample.active);
        assert_eq!(sample.boxes.len(), 2, "{:?}", sample.boxes);
        for [_, _, w, _] in &sample.boxes {
            assert!(*w < 0.25, "a box spans the gap: {:?}", sample.boxes);
        }
        // Events still get one box around everything that moved.
        let [x, _, w, _] = sample.bbox.unwrap();
        assert!(x < 0.1 && x + w > 0.9);
    }

    #[test]
    fn a_small_object_counts_with_a_custom_moving_area() {
        // The mock's ball changes 1-3 of the 144 blocks: never 5.2% (sensitivity
        // 6) of a large watched area, so it raised no events (2026-11-23). The
        // moving area is a setting now.
        let small = |x: usize| {
            let mut frame = vec![30; WIDTH * HEIGHT];
            for row in 60..80 {
                for column in x..x + 20 {
                    frame[row * WIDTH + column] = 200;
                }
            }
            frame
        };
        let active_after = |settings: &MotionSettings| {
            let mut detector = warmed(settings, &frame(false));
            let mut active = false;
            for step in 1..8 {
                active = detector
                    .observe(&small(20 * step), step as i64 * 200)
                    .unwrap()
                    .active;
            }
            active
        };
        let six = MotionSettings {
            sensitivity: 6,
            min_duration_sec: 0.0,
            ..Default::default()
        };
        assert!(!active_after(&six), "about 1 block of 144 is below 5.2%");
        let custom = MotionSettings {
            thresholds: crate::settings::Thresholds::Custom,
            thresholds_area_percent: 0.5,
            ..six
        };
        assert!(active_after(&custom), "0.5% counts one block of 144");
    }

    #[test]
    fn extreme_timestamps_do_not_overflow() {
        let mut detector = Detector::new(&config());
        detector.observe(&frame(false), i64::MIN).unwrap();
        detector.observe(&frame(true), i64::MIN + 1).unwrap();
        detector.observe(&frame(true), i64::MAX).unwrap();
        detector.observe(&frame(false), i64::MAX).unwrap();
    }
}

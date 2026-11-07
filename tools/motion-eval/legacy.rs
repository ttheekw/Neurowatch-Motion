//! Evaluation bridge only. Runs the production detector without reimplementing it.
#![allow(dead_code)]

// Only the configuration fields read by detector.rs are needed by this standalone
// rustc build. Serialization and the media/protocol dependencies are not involved.
mod settings {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum ZoneKind {
        Include,
        Exclude,
    }
    pub struct Zone {
        pub id: String,
        pub kind: ZoneKind,
        pub points: Vec<[f32; 2]>,
    }
    impl Zone {
        pub fn is_valid(&self) -> bool {
            !self.id.is_empty()
                && self.points.len() >= 3
                && self
                    .points
                    .iter()
                    .all(|[x, y]| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
        }
    }
    pub struct MotionSettings {
        pub sensitivity: u8,
        pub min_duration_sec: f32,
        pub cooldown_sec: f32,
        pub zones: Vec<Zone>,
    }
}

#[path = "../../src/detector.rs"]
mod detector;

use detector::{Detector, COLUMNS, HEIGHT, WIDTH};
use std::ffi::c_void;

#[no_mangle]
pub extern "C" fn legacy_create(sensitivity: u8) -> *mut c_void {
    Box::into_raw(Box::new(Detector::new(&settings::MotionSettings {
        sensitivity,
        min_duration_sec: 0.5,
        cooldown_sec: 10.0,
        zones: Vec::new(),
    })))
    .cast()
}

/// The caller supplies two non-overlapping WIDTH*HEIGHT byte buffers and a valid
/// handle. The output is a BLOCK mask, not a raw foreground segmentation mask.
#[no_mangle]
pub unsafe extern "C" fn legacy_observe(
    handle: *mut c_void,
    gray: *const u8,
    time_ms: i64,
    block_mask: *mut u8,
    score: *mut f32,
    active: *mut u8,
) -> i32 {
    if handle.is_null()
        || gray.is_null()
        || block_mask.is_null()
        || score.is_null()
        || active.is_null()
    {
        return -1;
    }
    let detector = &mut *handle.cast::<Detector>();
    let frame = std::slice::from_raw_parts(gray, WIDTH * HEIGHT);
    let Ok(sample) = detector.observe(frame, time_ms) else {
        return -2;
    };
    let out = std::slice::from_raw_parts_mut(block_mask, WIDTH * HEIGHT);
    out.fill(0);
    // Each 10x10 analysis block represents a 20x20 region of the 320x180 input.
    for block in sample.blocks {
        let x = usize::from(block) % COLUMNS * 20;
        let y = usize::from(block) / COLUMNS * 20;
        for row in y..y + 20 {
            out[row * WIDTH + x..row * WIDTH + x + 20].fill(255);
        }
    }
    *score = sample.score;
    *active = u8::from(sample.active);
    0
}

#[no_mangle]
pub unsafe extern "C" fn legacy_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<Detector>()));
    }
}

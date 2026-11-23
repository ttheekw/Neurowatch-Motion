//! What this plugin tells NeuroWatch about itself.
use nw_analytics_protocol::{
    messages::{
        Codec, EventKindInfo, Manifest, MediaRequest, OverlayLayer, StreamChoice, VideoRequest,
    },
    PROTOCOL_VERSION,
};
use serde_json::json;

pub const PLUGIN_ID: &str = "nw.motion";
/// The moving area while there is motion.
pub const DETECTIONS: &str = "detections";
/// What the detector sees: changed blocks, zones, the score.
pub const DEBUG: &str = "debug";

pub fn manifest() -> Manifest {
    Manifest {
        id: PLUGIN_ID.into(),
        name: "Motion detection".into(),
        vendor: "NeuroWatch".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: PROTOCOL_VERSION,
        ping: true,
        locality_proof: None,
        identity: None,
        web_ui: Some(nw_analytics_protocol::web::WebUi {
            entry: "/".into(),
            title: "Motion dashboard".into(),
        }),
        permissions: vec![
            "video".into(),
            "events".into(),
            "overlays".into(),
            "decoded_frames".into(),
            "web_ui".into(),
        ],
        media: MediaRequest {
            decoded: Some(nw_analytics_protocol::decoded::DecodedRequest {
                formats: vec![
                    nw_analytics_protocol::decoded::RawFormat::Gray8,
                    nw_analytics_protocol::decoded::RawFormat::Nv12,
                    nw_analytics_protocol::decoded::RawFormat::Rgb24,
                ],
                max_fps: 5,
                // No minimum size: only the bound stream's own pictures. With
                // one, NeuroWatch may hand over another of the camera's streams
                // a desktop shows (its main stream in a solo tile), and what is
                // detected would change with what someone looks at: on a main
                // stream the same object is smaller after scaling to 320×180,
                // the settings were made for the bound stream, and every switch
                // starts the detection again.
                min_width: 0,
                min_height: 0,
            }),
            video: VideoRequest {
                // 320×180 analysis: the sub stream is always enough.
                stream: StreamChoice::Sub,
                codecs: vec![Codec::H264, Codec::H265],
                keyframes_only: false,
                delivery: nw_analytics_protocol::messages::DeliveryMode::Latest,
                max_buffer_ms: 1000,
            },
        },
        config_schema: Some(config_schema()),
        event_kinds: vec![EventKindInfo {
            id: "motion".into(),
            label: "Motion".into(),
        }],
        overlay_layers: vec![
            OverlayLayer {
                id: DETECTIONS.into(),
                label: "Motion boxes".into(),
                default_on: true,
            },
            OverlayLayer {
                id: DEBUG.into(),
                label: "Analysis (changed blocks, zones, score)".into(),
                default_on: false,
            },
        ],
    }
}

/// JSON Schema of a binding's settings (`crate::settings::MotionSettings`).
fn config_schema() -> serde_json::Value {
    let point = json!({"type": "array", "items": {"type": "number", "minimum": 0, "maximum": 1},
                       "minItems": 2, "maxItems": 2});
    let decoder_values: Vec<&str> = crate::decoding::choices().iter().map(|c| c.0).collect();
    let decoder_labels: serde_json::Map<String, serde_json::Value> = crate::decoding::choices()
        .iter()
        .map(|(value, label)| ((*value).to_owned(), json!(label)))
        .collect();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "sensitivity": {"type": "integer", "minimum": 1, "maximum": 10, "default": 5,
                            "title": "Sensitivity",
                            "description": "Higher notices smaller changes, and gives more false alarms from rain, shadows or leaves. It sets the two thresholds below unless Thresholds is Custom."},
            // Keys sort right after "sensitivity" (the form follows key order).
            "thresholds": {
                "enum": ["sensitivity", "custom"], "default": "sensitivity",
                "x-nw-labels": {"sensitivity": "From sensitivity", "custom": "Custom"},
                "title": "Thresholds",
                "description": "Motion is reported when enough of the watched area changes, and a pixel counts as changed when its brightness differs enough from the scene Motion has learned. From sensitivity: the sensitivity sets both (at 5: 6% of the area, 18 brightness levels; at 10: 2% and 8). Custom: the two values below are used instead, and the sensitivity is not."
            },
            "thresholds_area_percent": {
                "type": "number", "minimum": 0.5, "maximum": 50, "default": 6,
                "title": "Moving area, % (Custom)",
                "description": "How much of the watched area must change before it counts as motion: your watched areas, or the whole picture without any. Motion divides the picture into a grid of 16 by 9 cells, and a cell counts as changed when a fifth of its pixels changed, so in a watched area of about 100 cells each cell is about 1%. A person far away, or a small animal, often changes only 1 or 2 cells: lower this to catch them, or watch a smaller area. Lower also means more false alarms. From sensitivity it is 10% minus 0.8% per sensitivity step (6% at 5, 2% at 10). The analysis layer shows the score next to what it needs."
            },
            "thresholds_pixel_change": {
                "type": "integer", "minimum": 2, "maximum": 100, "default": 18,
                "title": "Pixel change, brightness levels (Custom)",
                "description": "How many brightness levels (of 255) a pixel must differ from the learned scene to count as changed. Lower notices faint or dark movement, and also more camera noise, rain and shadows. From sensitivity it is 28 minus 2 per sensitivity step (18 at 5, 8 at 10)."
            },
            "cooldown_sec": {"type": "number", "minimum": 0, "maximum": 600, "default": 10,
                             "title": "Ends after quiet (seconds)",
                             "description": "An event ends once nothing has moved for this long. Shorter splits a long visit into several events."},
            "min_duration_sec": {"type": "number", "minimum": 0, "maximum": 60, "default": 0.5,
                                 "title": "Minimum duration (seconds)",
                                 "description": "Movement shorter than this is ignored, such as a flash of light."},
            "low_cpu": {"type": "boolean", "default": false, "title": "Keyframes only (low CPU)",
                        "description": "Looks at about one picture every one or two seconds. Uses far less processing, and can miss quick movement."},
            "decoder": {
                "enum": decoder_values, "default": "auto", "x-nw-labels": decoder_labels,
                "x-nw-widget": "nw:decoder",
                "title": "Video decoder",
                "description": "What decodes this camera's video. Automatic and hardware decoders leave more of the processor free; a choice that cannot be used here falls back to the CPU, and the status says so."
            },
            "zones": {
                "type": "array", "maxItems": 32, "x-nw-widget": "nw:zones",
                "title": "Watched areas",
                "description": "Areas to watch or to ignore. With no watched area, the whole picture is watched. Ignore a camera's on-screen clock or name: a clock changes every second and counts as motion.",
                "items": {
                    "type": "object", "required": ["id", "points"],
                    "properties": {
                        "id": {"type": "string", "minLength": 1},
                        "name": {"type": "string"},
                        "kind": {"enum": ["include", "exclude"], "default": "include"},
                        "points": {"type": "array", "items": point, "minItems": 3, "maxItems": 64}
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thresholds_are_settings_explained_beside_the_sensitivity() {
        // The user's choice (2026-11-23): the thresholds the sensitivity sets
        // are settings of their own, described in the settings form.
        let schema = config_schema();
        let properties = schema["properties"].as_object().unwrap();
        let keys: Vec<&String> = properties.keys().collect();
        let at = keys.iter().position(|k| *k == "sensitivity").unwrap();
        assert_eq!(
            &keys[at..at + 4],
            [
                "sensitivity",
                "thresholds",
                "thresholds_area_percent",
                "thresholds_pixel_change"
            ],
            "the form lists them together, in key order"
        );
        for key in [
            "thresholds",
            "thresholds_area_percent",
            "thresholds_pixel_change",
        ] {
            let description = properties[key]["description"].as_str().unwrap();
            assert!(
                description.len() > 120,
                "{key} explains itself: {description}"
            );
            assert!(
                description.contains("sensitivity"),
                "{key} says what the sensitivity sets"
            );
        }
        let defaults = crate::settings::MotionSettings::default();
        assert_eq!(properties["thresholds_area_percent"]["default"], 6);
        assert_eq!(f64::from(defaults.thresholds_area_percent), 6.0);
        assert_eq!(
            properties["thresholds_pixel_change"]["default"],
            defaults.thresholds_pixel_change
        );
        assert_eq!(defaults.pixel_threshold(), defaults.thresholds_pixel_change);
        assert!((defaults.min_area() * 100.0 - defaults.thresholds_area_percent).abs() < 1e-4);
    }

    #[test]
    fn the_analysis_stays_on_the_bound_stream_whatever_a_desktop_shows() {
        // Reported: boxes on the sub stream's tile, none once the camera was
        // shown on its own (main stream): NeuroWatch had moved this plugin to
        // the desktop's main-stream pictures, which a minimum size allows.
        let decoded = manifest()
            .media
            .decoded
            .expect("uses NeuroWatch's pictures");
        assert_eq!((decoded.min_width, decoded.min_height), (0, 0));
        assert_eq!(manifest().media.video.stream, StreamChoice::Sub);
    }
}

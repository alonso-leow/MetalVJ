//! O.10: Physical input and HTTP triggers.
//!
//! This module defines the trigger infrastructure for activating overlays,
//! sequences, and transport control from HTTP, MIDI, OSC, or the keyboard.
//!
//! **Trigger model (IV.4):** Triggers are named references to actions that
//! already exist — overlays, sequences, transport — not a second state machine.
//! Config is a read of the graph, never a separate write path.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The source of a trigger: HTTP, MIDI, OSC, or keyboard.
///
/// Each trigger is identified by a unique string `id` that is registered in
/// the active trigger list. When fired, the route validates that the trigger
/// exists and its target does, then executes the corresponding action.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "source")]
pub enum TriggerSource {
    /// HTTP POST /trigger/{id} fires this trigger.
    /// The `route` names what action to fire (e.g., overlay, sequence, transport).
    /// Example: `POST /trigger/start-overlay-1` fires the action named `start-overlay-1`.
    #[serde(rename = "http")]
    HttpPost { route: String },

    /// MIDI: exactly one of cc/note is Some. `port` is the ALSA client:port string.
    #[serde(rename = "midi")]
    Midi {
        port: String,
        cc: Option<u8>,
        note: Option<u8>,
    },

    /// OSC: exact address match, e.g. "/button/1".
    #[serde(rename = "osc")]
    Osc { addr: String },

    /// Keyboard: `key` is an egui Key name (e.g. "Space", "A"); `mods` is a
    /// sorted list of modifier names ("ctrl", "shift", "alt", "command").
    #[serde(rename = "key")]
    Key { key: String, mods: Vec<String> },
}

impl TriggerSource {
    /// The HTTP route for this trigger, if it is an HTTP trigger.
    pub fn route(&self) -> Option<&str> {
        match self {
            TriggerSource::HttpPost { route } => Some(route),
            _ => None,
        }
    }

    /// The kind tag for this source ("http"/"midi"/"osc"/"key").
    pub fn kind(&self) -> &'static str {
        match self {
            TriggerSource::HttpPost { .. } => "http",
            TriggerSource::Midi { .. } => "midi",
            TriggerSource::Osc { .. } => "osc",
            TriggerSource::Key { .. } => "key",
        }
    }

    /// Whether this source is an OSC source matching `addr`.
    pub fn matches_osc(&self, addr: &str) -> bool {
        matches!(self, TriggerSource::Osc { addr: a } if a == addr)
    }

    /// Whether this source is a key source matching `key` and `mods`.
    pub fn matches_key(&self, key: &str, mods: &[String]) -> bool {
        matches!(self, TriggerSource::Key { key: k, mods: m } if k == key && m.as_slice() == mods)
    }

    /// Whether this source is a MIDI source matching `port`, `cc`, and `note`.
    pub fn matches_midi(&self, port: &str, cc: Option<u8>, note: Option<u8>) -> bool {
        matches!(self, TriggerSource::Midi { port: p, cc: c, note: n } if p == port && *c == cc && *n == note)
    }
}

/// A named trigger that is active and can be fired.
///
/// Triggers live in a list on a Device. Only active triggers can fire.
/// A trigger names a target (overlay_id, sequence_id, or "transport") and
/// is resolved at fire time — if the target no longer exists, the route
/// returns an error rather than silently doing nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// Unique identifier within this installation (e.g., "pedal-1", "fader-start").
    pub id: String,

    /// The source of the trigger (HTTP, MIDI, OSC, or keyboard).
    pub source: TriggerSource,

    /// What this trigger fires: either an overlay, sequence, or "transport".
    /// Resolved at fire time by the route handler.
    pub target: String,

    /// Optional parameters specific to the target (e.g., `{"loop": true}` for overlay).
    #[serde(default)]
    pub params: Value,
}

impl Trigger {
    /// Create a new HTTP trigger.
    pub fn new_http(id: String, target: String) -> Self {
        Trigger {
            id: id.clone(),
            source: TriggerSource::HttpPost { route: id },
            target,
            params: Value::Object(serde_json::Map::new()),
        }
    }

    /// Create a new MIDI trigger. Exactly one of `cc`/`note` should be `Some`.
    pub fn new_midi(
        id: String,
        target: String,
        port: String,
        cc: Option<u8>,
        note: Option<u8>,
    ) -> Self {
        Trigger {
            id,
            source: TriggerSource::Midi { port, cc, note },
            target,
            params: Value::Object(serde_json::Map::new()),
        }
    }

    /// Create a new OSC trigger matched by exact address.
    pub fn new_osc(id: String, target: String, addr: String) -> Self {
        Trigger {
            id,
            source: TriggerSource::Osc { addr },
            target,
            params: Value::Object(serde_json::Map::new()),
        }
    }

    /// Create a new keyboard trigger.
    pub fn new_key(id: String, target: String, key: String, mods: Vec<String>) -> Self {
        Trigger {
            id,
            source: TriggerSource::Key { key, mods },
            target,
            params: Value::Object(serde_json::Map::new()),
        }
    }

    /// Parse trigger data from a JSON value (from config).
    pub fn from_value(v: &Value) -> Option<Self> {
        serde_json::from_value(v.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_source_route_returns_correct_value() {
        let trigger = TriggerSource::HttpPost {
            route: "pedal-1".to_string(),
        };
        assert_eq!(trigger.route(), Some("pedal-1"));
    }

    #[test]
    fn trigger_new_http_creates_matching_id_and_route() {
        let trigger = Trigger::new_http("fader-1".to_string(), "overlay-a".to_string());
        assert_eq!(trigger.id, "fader-1");
        assert_eq!(trigger.source.route(), Some("fader-1"));
        assert_eq!(trigger.target, "overlay-a");
    }

    #[test]
    fn trigger_serializes_and_deserializes() {
        let original = Trigger::new_http("test".to_string(), "overlay-1".to_string());
        let json = serde_json::to_value(&original).unwrap();
        let restored = Trigger::from_value(&json).unwrap();
        assert_eq!(original.id, restored.id);
        assert_eq!(original.target, restored.target);
    }

    #[test]
    fn kind_and_route_none_for_non_http() {
        assert_eq!(
            TriggerSource::HttpPost {
                route: "r".to_string()
            }
            .kind(),
            "http"
        );
        assert_eq!(
            TriggerSource::Midi {
                port: "0:1".to_string(),
                cc: Some(1),
                note: None
            }
            .kind(),
            "midi"
        );
        assert_eq!(
            TriggerSource::Osc {
                addr: "/x".to_string()
            }
            .kind(),
            "osc"
        );
        assert_eq!(
            TriggerSource::Key {
                key: "A".to_string(),
                mods: vec![]
            }
            .kind(),
            "key"
        );

        assert_eq!(
            TriggerSource::Midi {
                port: "0:1".to_string(),
                cc: Some(1),
                note: None
            }
            .route(),
            None
        );
        assert_eq!(
            TriggerSource::Osc {
                addr: "/x".to_string()
            }
            .route(),
            None
        );
        assert_eq!(
            TriggerSource::Key {
                key: "A".to_string(),
                mods: vec![]
            }
            .route(),
            None
        );
    }

    #[test]
    fn midi_trigger_round_trips() {
        let original = Trigger::new_midi(
            "pad-1".to_string(),
            "overlay-a".to_string(),
            "0:1".to_string(),
            Some(64),
            None,
        );
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(json["source"]["source"], "midi");
        assert_eq!(json["source"]["port"], "0:1");
        assert_eq!(json["source"]["cc"], 64);
        assert_eq!(json["source"]["note"], serde_json::Value::Null);

        let restored = Trigger::from_value(&json).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn osc_trigger_round_trips() {
        let original = Trigger::new_osc(
            "osc-1".to_string(),
            "overlay-a".to_string(),
            "/button/1".to_string(),
        );
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(json["source"]["source"], "osc");
        assert_eq!(json["source"]["addr"], "/button/1");

        let restored = Trigger::from_value(&json).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn key_trigger_round_trips() {
        let original = Trigger::new_key(
            "key-1".to_string(),
            "overlay-a".to_string(),
            "Space".to_string(),
            vec!["ctrl".to_string(), "shift".to_string()],
        );
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(json["source"]["source"], "key");
        assert_eq!(json["source"]["key"], "Space");
        assert_eq!(json["source"]["mods"][0], "ctrl");
        assert_eq!(json["source"]["mods"][1], "shift");

        let restored = Trigger::from_value(&json).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn legacy_http_json_still_deserializes() {
        let json = serde_json::json!({
            "id": "legacy-1",
            "source": { "source": "http", "route": "legacy-1" },
            "target": "overlay-a",
            "params": {}
        });
        let restored = Trigger::from_value(&json).unwrap();
        assert_eq!(restored.id, "legacy-1");
        assert_eq!(restored.target, "overlay-a");
        assert_eq!(restored.source.route(), Some("legacy-1"));
    }

    #[test]
    fn matches_osc_positive_and_negative() {
        let osc = TriggerSource::Osc {
            addr: "/button/1".to_string(),
        };
        assert!(osc.matches_osc("/button/1"));
        assert!(!osc.matches_osc("/button/2"));

        let http = TriggerSource::HttpPost {
            route: "r".to_string(),
        };
        assert!(!http.matches_osc("/button/1"));
    }

    #[test]
    fn matches_key_positive_and_negative() {
        let key = TriggerSource::Key {
            key: "A".to_string(),
            mods: vec!["ctrl".to_string(), "shift".to_string()],
        };
        assert!(key.matches_key(
            "A",
            &["ctrl".to_string(), "shift".to_string()]
        ));
        assert!(!key.matches_key("A", &["ctrl".to_string()]));
        assert!(!key.matches_key("B", &["ctrl".to_string(), "shift".to_string()]));

        let osc = TriggerSource::Osc {
            addr: "/x".to_string(),
        };
        assert!(!osc.matches_key("A", &[]));
    }

    #[test]
    fn matches_midi_positive_and_negative() {
        let cc = TriggerSource::Midi {
            port: "0:1".to_string(),
            cc: Some(64),
            note: None,
        };
        assert!(cc.matches_midi("0:1", Some(64), None));
        assert!(!cc.matches_midi("0:2", Some(64), None));
        assert!(!cc.matches_midi("0:1", Some(65), None));
        assert!(!cc.matches_midi("0:1", Some(64), Some(1)));

        let note = TriggerSource::Midi {
            port: "0:1".to_string(),
            cc: None,
            note: Some(60),
        };
        assert!(note.matches_midi("0:1", None, Some(60)));
        assert!(!note.matches_midi("0:1", Some(60), None));

        let http = TriggerSource::HttpPost {
            route: "r".to_string(),
        };
        assert!(!http.matches_midi("0:1", Some(64), None));
    }

    #[test]
    fn new_midi_sets_id_source_target() {
        let trigger = Trigger::new_midi(
            "pad-1".to_string(),
            "overlay-a".to_string(),
            "0:1".to_string(),
            None,
            Some(60),
        );
        assert_eq!(trigger.id, "pad-1");
        assert_eq!(trigger.target, "overlay-a");
        assert_eq!(
            trigger.source,
            TriggerSource::Midi {
                port: "0:1".to_string(),
                cc: None,
                note: Some(60)
            }
        );
    }

    #[test]
    fn new_osc_sets_id_source_target() {
        let trigger = Trigger::new_osc(
            "osc-1".to_string(),
            "overlay-a".to_string(),
            "/button/1".to_string(),
        );
        assert_eq!(trigger.id, "osc-1");
        assert_eq!(trigger.target, "overlay-a");
        assert_eq!(
            trigger.source,
            TriggerSource::Osc {
                addr: "/button/1".to_string()
            }
        );
    }

    #[test]
    fn new_key_sets_id_source_target() {
        let trigger = Trigger::new_key(
            "key-1".to_string(),
            "overlay-a".to_string(),
            "Space".to_string(),
            vec!["ctrl".to_string()],
        );
        assert_eq!(trigger.id, "key-1");
        assert_eq!(trigger.target, "overlay-a");
        assert_eq!(
            trigger.source,
            TriggerSource::Key {
                key: "Space".to_string(),
                mods: vec!["ctrl".to_string()]
            }
        );
    }
}

//! Bounded framing and event filtering for Linux's persistent audio streams.
use anyhow::{Result, bail, ensure};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct JsonArrays {
    buffer: Vec<u8>,
    depth: usize,
    quoted: bool,
    escaped: bool,
}
impl JsonArrays {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Vec<Value>>> {
        let mut records = Vec::new();
        for &byte in bytes {
            if self.buffer.is_empty() {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                ensure!(byte == b'[', "Invalid PipeWire event stream");
            }
            self.buffer.push(byte);
            ensure!(
                self.buffer.len() <= 16 * 1024 * 1024,
                "PipeWire event is too large"
            );
            if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.quoted = false;
                }
            } else {
                match byte {
                    b'"' => self.quoted = true,
                    b'[' | b'{' => {
                        self.depth += 1;
                        ensure!(self.depth <= 128, "PipeWire event is too deeply nested");
                    }
                    b']' | b'}' => {
                        ensure!(self.depth > 0, "Invalid PipeWire event");
                        self.depth -= 1;
                    }
                    _ => {}
                }
                if self.depth == 0 {
                    records.push(serde_json::from_slice(&self.buffer)?);
                    self.buffer.clear();
                }
            }
        }
        Ok(records)
    }
    pub fn finish(&self) -> Result<()> {
        ensure!(self.buffer.is_empty(), "Truncated PipeWire event");
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MuteFlags {
    muted: Option<bool>,
    soft_muted: Option<bool>,
}

#[derive(Default)]
pub(super) struct PipewireEvents {
    sinks: HashMap<u64, Vec<MuteFlags>>,
    routes: HashMap<u64, Vec<Value>>,
    default: Option<Value>,
}
impl PipewireEvents {
    pub fn changed(&mut self, records: &[Value]) -> bool {
        let mut changed = false;
        for record in records {
            let Some(id) = record.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if record.get("info").is_some_and(Value::is_null) {
                changed |= self.sinks.remove(&id).is_some() | self.routes.remove(&id).is_some();
                continue;
            }
            if record
                .pointer("/info/props/media.class")
                .and_then(Value::as_str)
                == Some("Audio/Sink")
            {
                let muted: Vec<_> = record
                    .pointer("/info/params/Props")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|p| MuteFlags {
                        muted: p.get("mute").and_then(Value::as_bool),
                        soft_muted: p.get("softMute").and_then(Value::as_bool),
                    })
                    .collect();
                changed |= self.sinks.insert(id, muted.clone()) != Some(muted);
            }
            if let Some(routes) = record
                .pointer("/info/params/Route")
                .and_then(Value::as_array)
            {
                let muted: Vec<_> = routes
                    .iter()
                    .filter(|route| {
                        route.get("direction").and_then(Value::as_str) == Some("Output")
                    })
                    .map(|route| {
                        serde_json::json!([
                            route.get("index"),
                            route.get("device"),
                            route.pointer("/props/mute")
                        ])
                    })
                    .collect();
                changed |= self.routes.insert(id, muted.clone()) != Some(muted);
            }
            if record
                .pointer("/props/metadata.name")
                .and_then(Value::as_str)
                == Some("default")
                && let Some(metadata) = record.get("metadata").and_then(Value::as_array)
            {
                for entry in metadata {
                    if entry.get("key").and_then(Value::as_str) == Some("default.audio.sink") {
                        let next = entry.get("value").cloned();
                        changed |= next != self.default;
                        self.default = next;
                    }
                }
                // pw-dump represents metadata deletion by an empty changed-entry list.
                changed |= metadata.is_empty();
            }
        }
        changed
    }
}

pub(super) fn pulse_changed(line: &str) -> bool {
    let Some((event, target)) = line.trim().split_once(" on ") else {
        return false;
    };
    if !["Event 'change'", "Event 'new'", "Event 'remove'"].contains(&event) {
        return false;
    }
    ["sink #", "server #"].iter().any(|prefix| {
        target
            .strip_prefix(prefix)
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
    })
}

pub(super) fn parse_mute(text: &str, pipewire: bool) -> Result<bool> {
    if pipewire {
        let words: Vec<_> = text.split_whitespace().collect();
        ensure!(
            words.len() == 2 || words.len() == 3,
            "Invalid audio response"
        );
        ensure!(
            words[0] == "Volume:"
                && words[1]
                    .parse::<f32>()
                    .is_ok_and(|v| v.is_finite() && v >= 0.0),
            "Invalid audio response"
        );
        if words.len() == 3 {
            ensure!(words[2] == "[MUTED]", "Invalid audio response");
        }
        Ok(words.len() == 3)
    } else {
        match text.trim() {
            "Mute: yes" => Ok(true),
            "Mute: no" => Ok(false),
            _ => bail!("Invalid audio response"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn streams_split_json_and_ignore_query_client_feedback_and_volume_changes() {
        let sink = json!({"id": 42, "type":"PipeWire:Interface:Node", "info":{"props":{"media.class":"Audio/Sink"},"params":{"Props":[{"mute":false,"volume":0.5}]}}});
        let text = format!(
            "{}\n{}",
            json!([sink]),
            json!([{"id":90,"info":{"props":{"media.class":"Stream/Output/Audio","name":"a]b\\\"c"}}}])
        );
        let mut framing = JsonArrays::default();
        let mut events = PipewireEvents::default();
        let mut changes = 0;
        for chunk in text.as_bytes().chunks(3) {
            for record in framing.feed(chunk).unwrap() {
                changes += usize::from(events.changed(&record));
            }
        }
        framing.finish().unwrap();
        assert_eq!(changes, 1);
        assert!(!events.changed(std::slice::from_ref(&sink)));
        let mut volume = sink.clone();
        volume["info"]["params"]["Props"][0]["volume"] = json!(0.7);
        assert!(!events.changed(&[volume]));
        let mut mute = sink;
        mute["info"]["params"]["Props"][0]["mute"] = json!(true);
        assert!(events.changed(&[mute]));
        assert!(events.changed(&[json!({"id":1,"props":{"metadata.name":"default"},"metadata":[{"key":"default.audio.sink","value":{"name":"new speakers"}}]})]));
        assert!(events.changed(&[json!({"id":42,"info":null})]));
    }
    #[test]
    fn device_hardware_route_mute_changes_are_observed() {
        let mut events = PipewireEvents::default();
        let mut device = json!({"id": 20, "info": {"params": {"Route": [{"direction": "Output", "index": 1, "device": 0, "props": {"mute": false}}]}}});
        assert!(events.changed(std::slice::from_ref(&device)));
        assert!(!events.changed(std::slice::from_ref(&device)));
        device["info"]["params"]["Route"][0]["props"]["mute"] = json!(true);
        assert!(events.changed(&[device]));
    }
    #[test]
    fn malformed_or_truncated_streams_are_rejected() {
        assert!(JsonArrays::default().feed(b"not json").is_err());
        let mut partial = JsonArrays::default();
        partial.feed(b"[{\"id\":").unwrap();
        assert!(partial.finish().is_err());
        assert!(JsonArrays::default().feed(b"[broken]").is_err());
    }
    #[test]
    fn pulse_ignores_clients_and_microphones_to_avoid_query_feedback() {
        for line in [
            "Event 'change' on sink #1",
            "Event 'change' on server #0",
            "Event 'remove' on sink #42",
        ] {
            assert!(pulse_changed(line));
        }
        for line in [
            "Event 'new' on client #2",
            "Event 'change' on source #3",
            "Event 'change' on sink-input #4",
            "garbage",
        ] {
            assert!(!pulse_changed(line));
        }
    }
    #[test]
    fn parses_only_known_mute_states() {
        assert!(parse_mute("Volume: 0.50 [MUTED]", true).unwrap());
        assert!(!parse_mute("Volume: 1.0", true).unwrap());
        assert!(parse_mute("Mute: yes", false).unwrap());
        assert!(!parse_mute("Mute: no", false).unwrap());
        for text in ["", "Volume: NaN", "Volume: 1.0 error", "Volume: -1"] {
            assert!(parse_mute(text, true).is_err());
        }
        assert!(parse_mute("Mute: unknown", false).is_err());
    }
}

//! WebSocket frame helpers for message serialization/deserialization.

use serde::{de::DeserializeOwned, Serialize};

/// Serialize a message to a JSON string for WebSocket text frames.
pub fn encode<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string(msg)
}

/// Deserialize a message from a JSON string (WebSocket text frame).
pub fn decode<T: DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(text)
}

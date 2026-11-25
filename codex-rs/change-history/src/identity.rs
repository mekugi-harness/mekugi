use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;

use crate::error::ChangeHistoryError;

/// Stable handle vocabulary shared with Mekugi session handles.
/// Stream letters `'a'..='z'` map to the first 26 words.
const STREAM_WORDS: [&str; 26] = [
    "amber", "apple", "arch", "ash", "atlas", "beach", "birch", "bird", "bloom", "blue", "brook",
    "cloud", "coral", "dawn", "delta", "dove", "elm", "fern", "field", "flame", "flint", "forest",
    "frost", "glass", "gold", "green",
];

/// Reserved stream name for unattributed user-shell (`!`) change IDs (`shell1`, `shell2`, ...).
pub const UNATTRIBUTED_STREAM: &str = "shell";

/// Convert a 0-based stream index to a bijective base-26 lowercase stream name
/// (`0 -> "a"`, `25 -> "z"`, `26 -> "aa"`).
pub fn stream_name_from_index(mut index: usize) -> String {
    let mut chars = Vec::new();
    loop {
        let rem = index % 26;
        chars.push((b'a' + rem as u8) as char);
        if index < 26 {
            break;
        }
        index = (index / 26) - 1;
    }
    chars.reverse();
    chars.into_iter().collect()
}

/// A validated stream-scoped monotonic Mekugi change ID (e.g. `amber1`, `apple2`).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ChangeId {
    stream: String,
    number: u64,
    raw: String,
}

impl ChangeId {
    /// Build a `ChangeId` from a lowercase stream letter sequence (`"a"`, `"b"`, ...)
    /// and a 1-based monotonic number.
    pub fn new(stream: &str, number: u64) -> Result<Self, ChangeHistoryError> {
        if stream == UNATTRIBUTED_STREAM {
            return Self::new_unattributed(number);
        }
        if stream.is_empty() || number == 0 || !stream.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(ChangeHistoryError::InvalidChangeId(format!(
                "{stream}{number}"
            )));
        }
        let mut raw = String::new();
        for b in stream.bytes() {
            raw.push_str(STREAM_WORDS[(b - b'a') as usize]);
        }
        raw.push_str(&number.to_string());
        Ok(Self {
            stream: stream.to_string(),
            number,
            raw,
        })
    }

    /// Build an unattributed shell-mode `ChangeId` (e.g. `shell1`, `shell2`).
    pub fn new_unattributed(number: u64) -> Result<Self, ChangeHistoryError> {
        if number == 0 {
            return Err(ChangeHistoryError::InvalidChangeId(format!(
                "{UNATTRIBUTED_STREAM}{number}"
            )));
        }
        Ok(Self {
            stream: UNATTRIBUTED_STREAM.to_string(),
            number,
            raw: format!("{UNATTRIBUTED_STREAM}{number}"),
        })
    }

    pub fn is_unattributed(&self) -> bool {
        self.stream == UNATTRIBUTED_STREAM
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn number(&self) -> u64 {
        self.number
    }
}

impl fmt::Display for ChangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl FromStr for ChangeId {
    type Err = ChangeHistoryError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = input.strip_prefix(UNATTRIBUTED_STREAM)
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
        {
            let number: u64 = rest
                .parse()
                .map_err(|_| ChangeHistoryError::InvalidChangeId(input.to_string()))?;
            if number == 0 || number.to_string() != rest {
                return Err(ChangeHistoryError::InvalidChangeId(input.to_string()));
            }
            return Self::new_unattributed(number);
        }
        let mut tail = input;
        let mut stream = String::new();
        loop {
            let mut matched = false;
            for (idx, word) in STREAM_WORDS.iter().enumerate() {
                if let Some(rest) = tail.strip_prefix(word) {
                    stream.push((b'a' + idx as u8) as char);
                    tail = rest;
                    matched = true;
                    break;
                }
            }
            if !matched {
                break;
            }
        }
        if stream.is_empty() || tail.is_empty() {
            return Err(ChangeHistoryError::InvalidChangeId(input.to_string()));
        }
        let number: u64 = tail
            .parse()
            .map_err(|_| ChangeHistoryError::InvalidChangeId(input.to_string()))?;
        if number == 0 || number.to_string() != tail {
            return Err(ChangeHistoryError::InvalidChangeId(input.to_string()));
        }
        Ok(Self {
            stream,
            number,
            raw: input.to_string(),
        })
    }
}

impl Serialize for ChangeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for ChangeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        ChangeId::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

/// Expand a single change ID (`amber1`) or an inclusive range (`amber1..amber3` or `amber1..3`).
pub fn expand_id_spec(spec: &str) -> Result<Vec<ChangeId>, ChangeHistoryError> {
    let Some((start_raw, end_raw)) = spec.split_once("..") else {
        return Ok(vec![ChangeId::from_str(spec)?]);
    };
    if end_raw.contains("..") {
        return Err(ChangeHistoryError::InvalidChangeRange(
            spec.to_string(),
            "multiple range separators".to_string(),
        ));
    }
    let start = ChangeId::from_str(start_raw)?;
    let end = if end_raw.bytes().all(|b| b.is_ascii_digit()) && !end_raw.is_empty() {
        let end_num: u64 = end_raw.parse().map_err(|_| {
            ChangeHistoryError::InvalidChangeRange(
                spec.to_string(),
                "invalid end number".to_string(),
            )
        })?;
        ChangeId::new(start.stream(), end_num)?
    } else {
        ChangeId::from_str(end_raw)?
    };
    if start.stream() != end.stream() {
        return Err(ChangeHistoryError::InvalidChangeRange(
            spec.to_string(),
            "range endpoints must belong to the same stream".to_string(),
        ));
    }
    if end.number() < start.number() {
        return Err(ChangeHistoryError::InvalidChangeRange(
            spec.to_string(),
            "range end is smaller than start".to_string(),
        ));
    }
    let count = end.number() - start.number() + 1;
    if count > 10_000 {
        return Err(ChangeHistoryError::InvalidChangeRange(
            spec.to_string(),
            "range exceeds 10,000 IDs".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(count as usize);
    for n in start.number()..=end.number() {
        out.push(ChangeId::new(start.stream(), n)?);
    }
    Ok(out)
}

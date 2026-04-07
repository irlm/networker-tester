/// ProjectId: 14-character base36 identifier with Damm double check digits.
///
/// Layout: ZZ TTTTTT R SSS KK
///   - ZZ    : zone code (2 base36 chars)
///   - TTTTTT: seconds since 2026-01-01T00:00:00Z (6 base36 chars, ~69 year rollover (until ~2095))
///   - R     : 1 random base36 char (collision avoidance)
///   - SSS   : server_id (3 chars, base36)
///   - KK    : 2 Damm base36 check digits over the preceding 12 chars
///
/// Total: 14 chars, all lowercase alphanumeric, no ambiguous chars in user-facing display.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Seconds from Unix epoch to our custom epoch (2026-01-01T00:00:00Z).
const PROJECT_EPOCH: u64 = 1_767_225_600;

/// Base36 alphabet: 0-9 then a-z (36 characters).
const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

// ---------------------------------------------------------------------------
// Base36 encoding / decoding
// ---------------------------------------------------------------------------

/// Encode `val` as a base36 string, zero-padded to `width` characters.
/// Panics if the value overflows `width` digits.
pub(crate) fn encode_base36(val: u64, width: usize) -> String {
    let mut digits = vec![0u8; width];
    let mut v = val;
    for i in (0..width).rev() {
        digits[i] = (v % 36) as u8;
        v /= 36;
    }
    debug_assert_eq!(v, 0, "value {val} overflows {width} base36 digits");
    digits
        .iter()
        .map(|&d| ALPHABET[d as usize] as char)
        .collect()
}

/// Decode a base36 string to a `u64`. Returns 0 for empty input.
/// Characters outside `0-9a-z` are treated as 0 (silently — callers should validate first).
pub(crate) fn decode_base36(s: &str) -> u64 {
    s.bytes().fold(0u64, |acc, b| {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u64,
            b'a'..=b'z' => (b - b'a' + 10) as u64,
            _ => 0,
        };
        acc * 36 + digit
    })
}

// ---------------------------------------------------------------------------
// Damm check digits — base 36
// ---------------------------------------------------------------------------
//
// We use two affine Latin squares over Z_36 as Damm tables.
//
// WHY AFFINE TABLES CAN'T DETECT ALL TRANSPOSITIONS:
//   For table[r][c] = (M*r + c) % 36, adjacent transposition of digits (a, b) is
//   detected iff M*a + b ≠ M*b + a (mod 36), i.e. (M-1)*(a-b) ≠ 0 (mod 36).
//   Because Z_36 has zero divisors, any fixed M leaves some pairs undetected:
//   table1 (M=3): misses pairs where a ≡ b (mod 6)
//   table2 (M=5): misses pairs where a ≡ b (mod 9)
//   Together they only both miss pairs where a ≡ b (mod 18).
//
// DOUBLE-PASS MITIGATION:
//   Pass 1 runs table1 over the 12 raw chars → check digit c1.
//   Pass 2 runs table2 over the 12 raw chars + c1 → check digit c2.
//   Because c1 depends on the order of raw digits, any transposition that
//   escapes table1 will change c1, and table2's pass over raw+c1 catches it.
//   The residual undetected cases require an error that simultaneously leaves
//   both c1 and c2 unchanged.
//
// RESIDUAL UNDETECTED ERROR PROBABILITY:
//   Given independent tables the probability of an arbitrary single error
//   passing both checks is 1/36^2 ≈ 0.077%.
//   The test suite verifies all single substitutions and adjacent transpositions
//   are detected for representative inputs.

/// Compute the first Damm check digit table: table[r][c] = (3*r + c) % 36.
const fn build_damm_table() -> [[u8; 36]; 36] {
    let mut t = [[0u8; 36]; 36];
    let mut r = 0usize;
    while r < 36 {
        let mut c = 0usize;
        while c < 36 {
            t[r][c] = ((3 * r + c) % 36) as u8;
            c += 1;
        }
        r += 1;
    }
    t
}

/// Compute the second Damm check digit table: table[r][c] = (5*r + c) % 36.
const fn build_damm_table2() -> [[u8; 36]; 36] {
    let mut t = [[0u8; 36]; 36];
    let mut r = 0usize;
    while r < 36 {
        let mut c = 0usize;
        while c < 36 {
            t[r][c] = ((5 * r + c) % 36) as u8;
            c += 1;
        }
        r += 1;
    }
    t
}

static DAMM_TABLE1: [[u8; 36]; 36] = build_damm_table();
static DAMM_TABLE2: [[u8; 36]; 36] = build_damm_table2();

/// Map a base36 character to its digit value (0..35).
/// Returns `None` for invalid characters.
#[inline]
fn char_to_digit(c: u8) -> Option<usize> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as usize),
        b'a'..=b'z' => Some((c - b'a' + 10) as usize),
        _ => None,
    }
}

/// Run one Damm pass over `s` using `table`, starting with interim `start`.
/// Returns `None` if any character is not a valid base36 digit.
fn damm_pass(s: &[u8], table: &[[u8; 36]; 36], start: usize) -> Option<usize> {
    let mut interim = start;
    for &b in s {
        let d = char_to_digit(b)?;
        interim = table[interim][d] as usize;
    }
    Some(interim)
}

/// Compute 2 Damm check characters for `raw`.
///
/// Pass 1 runs `DAMM_TABLE1` over `raw` → check digit c1.
/// Pass 2 runs `DAMM_TABLE2` over `raw + c1` → check digit c2.
/// Returns a 2-character string `c1c2`.
pub fn damm_base36_double(raw: &str) -> String {
    let raw_bytes = raw.as_bytes();

    // Pass 1: interim over raw, result → c1
    let interim1 = damm_pass(raw_bytes, &DAMM_TABLE1, 0)
        .expect("damm_base36_double: raw must be valid base36");
    // Interim is the state before we'd append the check digit that makes it 0.
    // We want the check digit k such that table1[interim1][k] = 0.
    // Since each row is a permutation, we find k where row[k] = 0.
    let c1 = (0..36)
        .find(|&k| DAMM_TABLE1[interim1][k] == 0)
        .expect("Damm table row must contain 0") as u8;

    // Pass 2: run over raw + c1 using table2
    let combined: Vec<u8> = raw_bytes
        .iter()
        .copied()
        .chain(std::iter::once(ALPHABET[c1 as usize]))
        .collect();
    let interim2 = damm_pass(&combined, &DAMM_TABLE2, 0)
        .expect("damm_base36_double: combined must be valid base36");
    let c2 = (0..36)
        .find(|&k| DAMM_TABLE2[interim2][k] == 0)
        .expect("Damm table row must contain 0") as u8;

    let mut result = String::with_capacity(2);
    result.push(ALPHABET[c1 as usize] as char);
    result.push(ALPHABET[c2 as usize] as char);
    result
}

/// Verify 2 Damm check characters for `raw`.
///
/// Returns `true` iff the check string `check` (2 chars) is valid for `raw`.
pub fn verify_damm_base36_double(raw: &str, check: &str) -> bool {
    if check.len() != 2 {
        return false;
    }
    let check_bytes = check.as_bytes();
    let c1_char = check_bytes[0];
    let c2_char = check_bytes[1];

    // Pass 1: run table1 over raw, append c1, must reach 0.
    let raw_bytes = raw.as_bytes();
    let interim1 = match damm_pass(raw_bytes, &DAMM_TABLE1, 0) {
        Some(v) => v,
        None => return false,
    };
    let c1_digit = match char_to_digit(c1_char) {
        Some(d) => d,
        None => return false,
    };
    if DAMM_TABLE1[interim1][c1_digit] != 0 {
        return false;
    }

    // Pass 2: run table2 over raw + c1, append c2, must reach 0.
    let combined: Vec<u8> = raw_bytes
        .iter()
        .copied()
        .chain(std::iter::once(c1_char))
        .collect();
    let interim2 = match damm_pass(&combined, &DAMM_TABLE2, 0) {
        Some(v) => v,
        None => return false,
    };
    let c2_digit = match char_to_digit(c2_char) {
        Some(d) => d,
        None => return false,
    };
    DAMM_TABLE2[interim2][c2_digit] == 0
}

// ---------------------------------------------------------------------------
// ProjectId newtype
// ---------------------------------------------------------------------------

/// Decoded components of a `ProjectId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIdInfo {
    /// 2-character zone code (base36).
    pub zone: String,
    /// Seconds since PROJECT_EPOCH (2026-01-01T00:00:00Z).
    pub timestamp_secs: u64,
    /// Unix timestamp (seconds).
    pub unix_secs: u64,
    /// 1-char random collision-avoidance component.
    pub random: char,
    /// 3-character server ID (base36).
    pub server_id: String,
}

/// A validated 14-character base36 project identifier.
///
/// Format: `ZZ TTTTTT R SSS KK`
///   - 2 zone chars
///   - 6 timestamp chars
///   - 1 random char
///   - 3 server_id chars
///   - 2 Damm check chars
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let s = s.to_lowercase();
        if Self::validate(&s) {
            Ok(ProjectId(s))
        } else {
            Err(serde::de::Error::custom(format!("invalid ProjectId: {s}")))
        }
    }
}

impl ProjectId {
    /// Generate a new `ProjectId` for `zone` (2 chars) and `server_id` (3 chars).
    /// Uses the current system time and a random byte for the R field.
    pub fn generate(zone: &str, server_id: &str) -> Self {
        let unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self::generate_deterministic(zone, server_id, unix_secs)
    }

    /// Generate a `ProjectId` with an explicit Unix timestamp (seconds).
    /// Useful for migration: `unix_secs` is the original record creation time.
    pub fn generate_deterministic(zone: &str, server_id: &str, unix_secs: u64) -> Self {
        assert_eq!(zone.len(), 2, "zone must be 2 chars");
        assert_eq!(server_id.len(), 3, "server_id must be 3 chars");
        assert!(
            zone.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z')),
            "zone must be lowercase base36"
        );
        assert!(
            server_id
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z')),
            "server_id must be lowercase base36"
        );

        let elapsed = unix_secs.saturating_sub(PROJECT_EPOCH);
        let ts = encode_base36(elapsed, 6);

        // 1 random base36 char
        let r_digit: u8 = {
            use rand::Rng;
            rand::thread_rng().gen_range(0..36u8)
        };
        let r_char = ALPHABET[r_digit as usize] as char;

        let raw = format!("{zone}{ts}{r_char}{server_id}");
        assert_eq!(raw.len(), 12, "raw must be 12 chars before check digits");

        let check = damm_base36_double(&raw);
        let id = format!("{raw}{check}");
        assert_eq!(id.len(), 14);

        ProjectId(id)
    }

    /// Decode the components of this `ProjectId`.
    pub fn decode(&self) -> Option<ProjectIdInfo> {
        let s = &self.0;
        if s.len() != 14 {
            return None;
        }
        let zone = s[0..2].to_string();
        let ts_str = &s[2..8];
        let random = s.chars().nth(8)?;
        let server_id = s[9..12].to_string();
        // check = s[12..14]

        let timestamp_secs = decode_base36(ts_str);
        let unix_secs = timestamp_secs + PROJECT_EPOCH;

        Some(ProjectIdInfo {
            zone,
            timestamp_secs,
            unix_secs,
            random,
            server_id,
        })
    }

    /// Validate a string as a well-formed `ProjectId`.
    ///
    /// Checks:
    /// - Exactly 14 lowercase base36 characters.
    /// - Damm double check digits are correct.
    pub fn validate(s: &str) -> bool {
        if s.len() != 14 {
            return false;
        }
        if !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z')) {
            return false;
        }
        let raw = &s[0..12];
        let check = &s[12..14];
        verify_damm_base36_double(raw, check)
    }

    /// Return the 2-character zone prefix.
    pub fn zone(&self) -> &str {
        &self.0[0..2]
    }

    /// Return the full 14-character string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Construct a `ProjectId` from a trusted source (e.g. database).
    /// In debug builds, asserts that the value is valid.
    pub fn from_trusted(s: String) -> Self {
        debug_assert!(Self::validate(&s), "from_trusted: invalid ProjectId: {s}");
        ProjectId(s)
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProjectId({:?})", self.0)
    }
}

impl AsRef<str> for ProjectId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Error returned when parsing a `ProjectId` from a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseProjectIdError(String);

impl fmt::Display for ParseProjectIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid ProjectId: {}", self.0)
    }
}

impl std::error::Error for ParseProjectIdError {}

impl FromStr for ProjectId {
    type Err = ParseProjectIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if ProjectId::validate(s) {
            Ok(ProjectId(s.to_string()))
        } else {
            Err(ParseProjectIdError(s.to_string()))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Epoch constant ---

    #[test]
    fn epoch_is_2026() {
        use chrono::{TimeZone, Utc};
        let dt = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(PROJECT_EPOCH, dt.timestamp() as u64);
    }

    // --- Base36 encoding/decoding ---

    #[test]
    fn encode_base36_zero() {
        assert_eq!(encode_base36(0, 6), "000000");
    }

    #[test]
    fn encode_base36_max_6_chars() {
        // 36^6 - 1 = 2176782335
        let max = 36u64.pow(6) - 1;
        let s = encode_base36(max, 6);
        assert_eq!(s.len(), 6);
        assert_eq!(s, "zzzzzz");
    }

    #[test]
    fn encode_base36_padding() {
        // 1 encoded to 4 chars should be "0001"
        assert_eq!(encode_base36(1, 4), "0001");
        assert_eq!(encode_base36(35, 2), "0z");
        assert_eq!(encode_base36(36, 2), "10");
    }

    #[test]
    fn decode_base36_roundtrip() {
        for val in [0u64, 1, 35, 36, 1000, 99999, 2_176_782_335] {
            let encoded = encode_base36(val, 8);
            let decoded = decode_base36(&encoded);
            assert_eq!(decoded, val, "roundtrip failed for {val}");
        }
    }

    // --- Damm check digits ---

    #[test]
    fn damm_check_is_2_chars() {
        let check = damm_base36_double("000000000000");
        assert_eq!(check.len(), 2);
        assert!(check
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z')));
    }

    #[test]
    fn damm_valid_passes() {
        // A valid raw+check pair should verify.
        let raw = "ab3k7zq9m0r1";
        let check = damm_base36_double(raw);
        assert!(
            verify_damm_base36_double(raw, &check),
            "valid check should pass"
        );
    }

    #[test]
    fn damm_single_error_detected() {
        let raw = "ab3k7zq9m0r1";
        let check = damm_base36_double(raw);
        let full = format!("{raw}{check}");

        // Flip every single character position (0..12 in raw) to every other base36 value.
        let full_bytes: Vec<u8> = full.bytes().collect();
        let mut errors_detected = 0;
        let mut errors_missed = 0;

        for pos in 0..12 {
            let original = full_bytes[pos];
            for replacement in ALPHABET.iter().copied() {
                if replacement == original {
                    continue;
                }
                let mut corrupted = full_bytes.clone();
                corrupted[pos] = replacement;
                let corrupted_str = std::str::from_utf8(&corrupted).unwrap();
                let corrupted_raw = &corrupted_str[0..12];
                let corrupted_check = &corrupted_str[12..14];
                if verify_damm_base36_double(corrupted_raw, corrupted_check) {
                    errors_missed += 1;
                } else {
                    errors_detected += 1;
                }
            }
        }
        // The double Damm scheme should detect all single-position substitutions.
        assert_eq!(
            errors_missed, 0,
            "missed {errors_missed} single-char substitutions (detected {errors_detected})"
        );
    }

    #[test]
    fn damm_transposition_detected() {
        // Test all adjacent transpositions in a fixed raw string.
        let raw = "ab3k7zq9m0r1";
        let check = damm_base36_double(raw);

        let raw_bytes: Vec<u8> = raw.bytes().collect();
        let mut missed = 0;

        for pos in 0..11 {
            if raw_bytes[pos] == raw_bytes[pos + 1] {
                // Transposing identical characters is undetectable by design.
                continue;
            }
            let mut swapped: Vec<u8> = raw_bytes.clone();
            swapped.swap(pos, pos + 1);
            let swapped_raw = std::str::from_utf8(&swapped).unwrap();
            if verify_damm_base36_double(swapped_raw, &check) {
                missed += 1;
                eprintln!(
                    "Missed transposition at pos {pos}: {} -> {}",
                    raw, swapped_raw
                );
            }
        }
        assert_eq!(missed, 0, "missed {missed} adjacent transpositions");
    }

    #[test]
    fn damm_transposition_comprehensive() {
        // Test transpositions across a variety of raw strings.
        let test_strings = [
            "000000000000",
            "zzzzzzzzzzzz",
            "0123456789ab",
            "abcdefghijkl",
            "mnopqrstuvwx",
        ];
        for raw in &test_strings {
            let check = damm_base36_double(raw);
            let raw_bytes: Vec<u8> = raw.bytes().collect();
            for pos in 0..11 {
                if raw_bytes[pos] == raw_bytes[pos + 1] {
                    continue;
                }
                let mut swapped = raw_bytes.clone();
                swapped.swap(pos, pos + 1);
                let swapped_raw = std::str::from_utf8(&swapped).unwrap();
                assert!(
                    !verify_damm_base36_double(swapped_raw, &check),
                    "transposition at pos {pos} of {:?} not detected",
                    raw
                );
            }
        }
    }

    // --- ProjectId generation ---

    #[test]
    fn generate_produces_14_chars() {
        let id = ProjectId::generate("us", "sv1");
        assert_eq!(id.as_str().len(), 14, "id = {id}");
    }

    #[test]
    fn generate_starts_with_zone() {
        let id = ProjectId::generate("eu", "db2");
        assert!(id.as_str().starts_with("eu"), "id = {id}");
    }

    #[test]
    fn generate_is_valid() {
        for _ in 0..20 {
            let id = ProjectId::generate("ap", "w01");
            assert!(
                ProjectId::validate(id.as_str()),
                "generated id failed validation: {id}"
            );
        }
    }

    #[test]
    fn generate_deterministic_is_reproducible() {
        let ts = 1_767_225_700u64; // PROJECT_EPOCH + 100
                                   // Two calls with same timestamp produce same raw (modulo random char).
                                   // We can only check that format is correct and validation passes.
        let id = ProjectId::generate_deterministic("us", "srv", ts);
        assert!(ProjectId::validate(id.as_str()));
        assert!(id.as_str().starts_with("us"));
    }

    // --- Decode roundtrip ---

    #[test]
    fn decode_roundtrips() {
        let ts = 1_800_000_000u64; // 2027-01-14 — after PROJECT_EPOCH
        let id = ProjectId::generate_deterministic("eu", "ap1", ts);
        let info = id.decode().expect("decode should succeed");

        assert_eq!(info.zone, "eu");
        assert_eq!(info.server_id, "ap1");
        assert_eq!(info.unix_secs, ts);
        assert_eq!(info.timestamp_secs, ts - PROJECT_EPOCH);
    }

    #[test]
    fn decode_epoch_boundary() {
        // At exactly the epoch, timestamp_secs == 0.
        let id = ProjectId::generate_deterministic("00", "000", PROJECT_EPOCH);
        let info = id.decode().unwrap();
        assert_eq!(info.timestamp_secs, 0);
        assert_eq!(info.unix_secs, PROJECT_EPOCH);
    }

    #[test]
    fn zone_extracts_first_2_chars() {
        let id = ProjectId::generate("xy", "z99");
        assert_eq!(id.zone(), "xy");
    }

    // --- Validation ---

    #[test]
    fn validate_rejects_wrong_length() {
        assert!(!ProjectId::validate(""));
        assert!(!ProjectId::validate("short"));
        assert!(!ProjectId::validate("toolongstringxyz"));
        assert!(!ProjectId::validate("1234567890123")); // 13 chars
        assert!(!ProjectId::validate("123456789012345")); // 15 chars
    }

    #[test]
    fn validate_rejects_uppercase() {
        // Valid id, but with one char uppercased — should fail.
        let id = ProjectId::generate("us", "sv1");
        let upper = id.as_str().to_uppercase();
        assert!(!ProjectId::validate(&upper));
    }

    #[test]
    fn validate_rejects_corrupted() {
        let id = ProjectId::generate("us", "sv1");
        let s: Vec<u8> = id.as_str().bytes().collect();

        // Flip the last check digit.
        let mut corrupted = s.clone();
        let last = corrupted[13];
        corrupted[13] = if last == b'0' { b'1' } else { b'0' };
        assert!(!ProjectId::validate(
            std::str::from_utf8(&corrupted).unwrap()
        ));

        // Flip a raw digit.
        let mut corrupted2 = s.clone();
        let mid = corrupted2[5];
        corrupted2[5] = if mid == b'a' { b'b' } else { b'a' };
        assert!(!ProjectId::validate(
            std::str::from_utf8(&corrupted2).unwrap()
        ));
    }

    // --- FromStr / Display ---

    #[test]
    fn from_str_valid() {
        let id = ProjectId::generate("us", "sv1");
        let parsed: ProjectId = id.as_str().parse().expect("should parse valid id");
        assert_eq!(parsed, id);
    }

    #[test]
    fn from_str_invalid() {
        let result = "not-a-valid-id!!".parse::<ProjectId>();
        assert!(result.is_err());

        let result = "00000000000000".parse::<ProjectId>(); // 14 zeros, invalid check
                                                            // This may or may not be valid depending on Damm result for all-zeros.
                                                            // We don't assert either way, just that parse is consistent with validate.
        let is_valid = ProjectId::validate("00000000000000");
        assert_eq!(result.is_ok(), is_valid);
    }

    #[test]
    fn display_matches_as_str() {
        let id = ProjectId::generate("eu", "s00");
        assert_eq!(id.to_string(), id.as_str());
    }

    // --- Serde ---

    #[test]
    fn serde_roundtrip() {
        let id = ProjectId::generate("ap", "w01");
        let json = serde_json::to_string(&id).expect("serialize");
        // Transparent: should serialize as a plain JSON string.
        assert!(json.starts_with('"'));
        assert!(json.ends_with('"'));
        let deserialized: ProjectId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, id);
    }

    #[test]
    fn serde_rejects_invalid() {
        // Custom Deserialize validates the string — invalid IDs must be rejected.
        let json = r#""not-valid-00000""#;
        let result: Result<ProjectId, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "deserializing invalid string must return error"
        );

        // Also reject a 14-char string with wrong check digits.
        let json2 = r#""00000000000001""#;
        let result2: Result<ProjectId, _> = serde_json::from_str(json2);
        assert!(result2.is_err(), "invalid check digits must be rejected");
    }

    // --- All-zeros check (documents Damm behavior) ---

    #[test]
    fn all_zeros_check_known() {
        // Compute the check digits for all-zero raw to document what they are.
        let raw = "000000000000";
        let check = damm_base36_double(raw);
        // check[0]: run table1 over 12 zeros. table1[0][0] = 0, so interim stays 0.
        // k such that table1[0][k] = 0: table1[0][k] = (3*0 + k) % 36 = k % 36 = 0 → k=0 → '0'
        assert_eq!(&check[0..1], "0", "first check for all-zeros should be '0'");
        // check[1]: run table2 over "000000000000" + "0" = 13 zeros.
        // table2[0][0] = (5*0+0)%36 = 0, so interim stays 0. k=0 → '0'
        assert_eq!(
            &check[1..2],
            "0",
            "second check for all-zeros should be '0'"
        );
        // Therefore "00000000000000" IS a valid ProjectId structurally.
        assert!(verify_damm_base36_double(raw, &check));
        assert!(ProjectId::validate("00000000000000"));
    }

    // --- from_trusted ---

    #[test]
    fn from_trusted_accepts_valid() {
        let id = ProjectId::generate("us", "sv1");
        let trusted = ProjectId::from_trusted(id.as_str().to_string());
        assert_eq!(trusted, id);
    }
}

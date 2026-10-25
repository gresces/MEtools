//! Timestamp, identifier, validation, and idempotency helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::JsonValue;
use crate::{FunctionFailure, Request};

pub const RECEIPT_LIMIT: usize = 128;
const SHANGHAI_OFFSET_SECONDS: i64 = 8 * 3600;

// ---- now_iso8601 ------------------------------------------------------------

/// Return the current timestamp formatted in `Asia/Shanghai` (`+08:00`) as `YYYY-MM-DDTHH:MM:SS+08:00`.
pub fn now_iso8601() -> String {
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_shanghai_timestamp(unix_secs)
}

fn format_shanghai_timestamp(unix_secs: i64) -> String {
    let local_secs = unix_secs + SHANGHAI_OFFSET_SECONDS;
    let days = local_secs.div_euclid(86_400);
    let rem = local_secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+08:00")
}

/// Convert days since 1970-01-01 into `(year, month, day)` in the proleptic Gregorian calendar.
pub fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

// ---- generate_id ------------------------------------------------------------

/// Generate a unique record ID with the given prefix using current time and request entropy.
pub fn generate_id(prefix: &str, request_id: &str, sequence: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!("{prefix}:{request_id}:{sequence}:{nanos}");
    let digest = sha256_hex(seed.as_bytes());
    format!("{prefix}-{}", &digest[..12])
}

// ---- date and time validation -----------------------------------------------

/// Validate that `value` is a real `YYYY-MM-DD` calendar date and return `(year, month, day)`.
pub fn validate_iso_date(value: &str, field: &str) -> Result<(u32, u32, u32), FunctionFailure> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be YYYY-MM-DD"
        )));
    }
    if !bytes[0..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be YYYY-MM-DD"
        )));
    }

    let year: u32 = value[0..4]
        .parse()
        .map_err(|_| FunctionFailure::invalid_input(format!("invalid year in {field}")))?;
    let month: u32 = value[5..7]
        .parse()
        .map_err(|_| FunctionFailure::invalid_input(format!("invalid month in {field}")))?;
    let day: u32 = value[8..10]
        .parse()
        .map_err(|_| FunctionFailure::invalid_input(format!("invalid day in {field}")))?;

    if year == 0 || !(1..=12).contains(&month) {
        return Err(FunctionFailure::invalid_input(format!(
            "invalid calendar date in {field}"
        )));
    }

    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => unreachable!(),
    };

    if day == 0 || day > max_day {
        return Err(FunctionFailure::invalid_input(format!(
            "invalid calendar date in {field}"
        )));
    }

    Ok((year, month, day))
}

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Validate a 24-hour clock string `HH:MM` (`00:00`..`23:59`).
pub fn validate_hh_mm(value: &str, field: &str) -> Result<(u32, u32), FunctionFailure> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || !bytes[0..2].iter().all(u8::is_ascii_digit)
        || !bytes[3..5].iter().all(u8::is_ascii_digit)
    {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be HH:MM (00:00..23:59)"
        )));
    }
    let hour: u32 = value[0..2].parse().unwrap_or(99);
    let minute: u32 = value[3..5].parse().unwrap_or(99);
    if hour > 23 || minute > 59 {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be HH:MM (00:00..23:59)"
        )));
    }
    Ok((hour, minute))
}

/// Validate an ISO-8601 timestamp with timezone offset (`YYYY-MM-DDTHH:MM[:SS][.ffffff](Z|±HH:MM)`).
pub fn validate_instant(value: &str, field: &str) -> Result<(), FunctionFailure> {
    parse_instant_epoch_micros(value, field).map(|_| ())
}

/// Parse an ISO-8601 timestamp with timezone offset into UTC microseconds since Unix epoch.
pub fn parse_instant_epoch_micros(value: &str, field: &str) -> Result<i64, FunctionFailure> {
    if value.len() < 16 {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be an ISO-8601 timestamp with timezone offset"
        )));
    }
    let (year, month, day) = validate_iso_date(&value[0..10], field)?;
    let bytes = value.as_bytes();
    if bytes[10] != b'T' {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be an ISO-8601 timestamp with timezone offset"
        )));
    }
    let (hour, minute) = validate_hh_mm(&value[11..16], field)?;

    let mut pos = 16;
    let mut second: u32 = 0;
    let mut micros: u32 = 0;
    if bytes.get(pos) == Some(&b':') {
        if pos + 3 > bytes.len() || !bytes[pos + 1..pos + 3].iter().all(u8::is_ascii_digit) {
            return Err(FunctionFailure::invalid_input(format!(
                "invalid seconds in {field}"
            )));
        }
        second = value[pos + 1..pos + 3].parse().unwrap_or(99);
        if second > 59 {
            return Err(FunctionFailure::invalid_input(format!(
                "invalid seconds in {field}"
            )));
        }
        pos += 3;
        if bytes.get(pos) == Some(&b'.') {
            pos += 1;
            let frac_start = pos;
            while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
                pos += 1;
            }
            let frac_len = pos - frac_start;
            if frac_len == 0 || frac_len > 6 {
                return Err(FunctionFailure::invalid_input(format!(
                    "invalid fractional seconds in {field}"
                )));
            }
            let mut padded = String::from(&value[frac_start..pos]);
            while padded.len() < 6 {
                padded.push('0');
            }
            micros = padded.parse().unwrap_or(0);
        }
    }

    let tz_part = &value[pos..];
    let offset_seconds: i64 = if tz_part == "Z" {
        0
    } else {
        let tz_bytes = tz_part.as_bytes();
        if tz_bytes.len() == 6 && (tz_bytes[0] == b'+' || tz_bytes[0] == b'-') {
            let (tz_h, tz_m) = validate_hh_mm(&tz_part[1..6], field)?;
            let total = (tz_h as i64) * 3600 + (tz_m as i64) * 60;
            if tz_bytes[0] == b'-' { -total } else { total }
        } else {
            return Err(FunctionFailure::invalid_input(format!(
                "{field} must include a valid timezone offset (Z or +/-HH:MM)"
            )));
        }
    };

    let days = days_from_civil(year as i64, month, day);
    let local_seconds =
        days * 86_400 + (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64);
    let utc_seconds = local_seconds - offset_seconds;
    Ok(utc_seconds * 1_000_000 + (micros as i64))
}

/// Convert a `YYYY-MM-DD` date at `00:00:00+08:00` (`Asia/Shanghai`) into UTC microseconds since Unix epoch.
pub fn parse_shanghai_date_start_micros(value: &str, field: &str) -> Result<i64, FunctionFailure> {
    let (year, month, day) = validate_iso_date(value, field)?;
    let days = days_from_civil(year as i64, month, day);
    let utc_seconds = days * 86_400 - SHANGHAI_OFFSET_SECONDS;
    Ok(utc_seconds * 1_000_000)
}

/// Convert `(year, month, day)` in the proleptic Gregorian calendar into days since 1970-01-01.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let d = day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

// ---- field validation helpers -----------------------------------------------

/// Verify that `payload` only contains keys from `allowed` and includes all keys in `required`.
pub fn check_fields(
    payload: &BTreeMap<String, JsonValue>,
    allowed: &[&str],
    required: &[&str],
) -> Result<(), FunctionFailure> {
    let allowed_set: BTreeSet<&str> = allowed.iter().copied().collect();
    let unsupported: Vec<&str> = payload
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed_set.contains(k))
        .collect();
    if !unsupported.is_empty() {
        return Err(FunctionFailure::invalid_input(format!(
            "unsupported fields: {}",
            unsupported.join(", ")
        )));
    }

    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|req| !payload.contains_key(*req))
        .collect();
    if !missing.is_empty() {
        return Err(FunctionFailure::invalid_input(format!(
            "missing required fields: {}",
            missing.join(", ")
        )));
    }

    Ok(())
}

// ---- request receipts (idempotency) -----------------------------------------

/// Compute the canonical SHA-256 digest for `(operation, expected_version, input)`.
pub fn request_digest(request: &Request, operation: &str) -> String {
    let payload = JsonValue::object([
        ("input", JsonValue::Object(request.input.clone())),
        ("operation", JsonValue::from(operation)),
        ("version", JsonValue::from(request.expected_version)),
    ]);
    sha256_hex(payload.to_json_string().as_bytes())
}

/// Check `recent_requests` in `document` for a matching `request_id`.
///
/// Returns `Ok(Some(saved_result))` on an exact retry match, or `Err(REQUEST_ID_CONFLICT)`
/// if the same `request_id` was used with a different operation, version, or input.
pub fn replay_request(
    document: &BTreeMap<String, JsonValue>,
    request: &Request,
    operation: &str,
) -> Result<Option<JsonValue>, FunctionFailure> {
    let Some(receipts_val) = document.get("recent_requests") else {
        return Ok(None);
    };
    let receipts = receipts_val
        .as_array()
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "recent_requests must be an array"))?;

    let expected_digest = request_digest(request, operation);
    for entry in receipts {
        let obj = entry.as_object().ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", "invalid receipt entry in recent_requests")
        })?;
        let req_id = obj
            .get("request_id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid receipt request_id"))?;
        let digest = obj
            .get("digest")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid receipt digest"))?;
        let result = obj
            .get("result")
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid receipt result"))?;

        if req_id == request.request_id {
            if digest != expected_digest {
                return Err(FunctionFailure::new(
                    "REQUEST_ID_CONFLICT",
                    "request_id was already used for a different request",
                ));
            }
            return Ok(Some(result.clone()));
        }
    }

    Ok(None)
}

/// Append a successful request receipt to `document["recent_requests"]`, keeping at most `RECEIPT_LIMIT` entries.
pub fn remember_request(
    document: &mut BTreeMap<String, JsonValue>,
    request: &Request,
    operation: &str,
    result: &JsonValue,
) {
    let mut receipts = document
        .get("recent_requests")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();

    receipts.push(JsonValue::object([
        ("request_id", JsonValue::from(request.request_id.clone())),
        (
            "digest",
            JsonValue::from(request_digest(request, operation)),
        ),
        ("result", result.clone()),
    ]));

    if receipts.len() > RECEIPT_LIMIT {
        let drain_count = receipts.len() - RECEIPT_LIMIT;
        receipts.drain(0..drain_count);
    }

    document.insert("recent_requests".to_string(), JsonValue::Array(receipts));
}

// ---- SHA-256 implementation -------------------------------------------------

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Compute lowercase hexadecimal SHA-256 digest of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word_bytes) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word_bytes);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K256[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut out = String::with_capacity(64);
    for word in state {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

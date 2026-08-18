//! Dev utilities: epoch/ISO conversion (hand-rolled civil-date math, UTC
//! only), RFC 3986 percent-encoding, UUID v4, and JWT decoding (no signature
//! verification).

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

// ---------------------------------------------------------------------------
// Epoch <-> UTC ISO. Civil-date math after Howard Hinnant's algorithms.
// ---------------------------------------------------------------------------

/// Parse an epoch integer; 12+ digit values are treated as milliseconds.
pub fn parse_epoch(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = trimmed.parse().ok()?;
    Some(if digits.len() >= 12 { n / 1000 } else { n })
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Unix seconds → "YYYY-MM-DDTHH:MM:SSZ".
pub fn utc_iso(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// "YYYY-MM-DD", optionally followed by "THH:MM[:SS]" and a trailing "Z" (or
/// a space instead of the "T") → unix seconds. UTC only.
pub fn iso_to_epoch(input: &str) -> Option<i64> {
    let s = input.trim().trim_end_matches('Z');
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };

    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }

    let mut secs = days_from_civil(y, m, d) * 86400;
    if let Some(time) = time {
        let mut t = time.split(':');
        let h: i64 = t.next()?.parse().ok()?;
        let min: i64 = t.next()?.parse().ok()?;
        let sec: i64 = match t.next() {
            Some(v) => v.parse().ok()?,
            None => 0,
        };
        if t.next().is_some() || h > 23 || min > 59 || sec > 60 {
            return None;
        }
        secs += h * 3600 + min * 60 + sec;
    }
    Some(secs)
}

/// "3 days ago" / "in 2 hours" / "just now", relative to `now` (unix secs).
pub fn relative(secs: i64, now: i64) -> String {
    let delta = now - secs;
    let (mag, future) = (delta.unsigned_abs(), delta < 0);
    let (n, unit) = if mag < 45 {
        return "just now".to_string();
    } else if mag < 60 * 60 {
        (mag / 60, "minute")
    } else if mag < 60 * 60 * 24 {
        (mag / 3600, "hour")
    } else if mag < 60 * 60 * 24 * 30 {
        (mag / 86400, "day")
    } else if mag < 60 * 60 * 24 * 365 {
        (mag / (86400 * 30), "month")
    } else {
        (mag / (86400 * 365), "year")
    };
    let n = n.max(1);
    let s = if n == 1 { "" } else { "s" };
    if future {
        format!("in {n} {unit}{s}")
    } else {
        format!("{n} {unit}{s} ago")
    }
}

// ---------------------------------------------------------------------------
// URL percent-encoding (RFC 3986 unreserved set).
// ---------------------------------------------------------------------------

pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn url_decode(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .context("invalid percent-escape")?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).context("decoded bytes are not valid UTF-8")
}

// ---------------------------------------------------------------------------
// UUID v4.
// ---------------------------------------------------------------------------

pub fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// JWT decode — header + payload only, signature NOT verified.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JwtParts {
    /// Pretty-printed JSON.
    pub header: String,
    pub payload: String,
}

pub fn jwt_decode(token: &str) -> Result<JwtParts> {
    let mut parts = token.trim().split('.');
    let (Some(header), Some(payload)) = (parts.next(), parts.next()) else {
        bail!("not a JWT (expected header.payload.signature)");
    };
    Ok(JwtParts {
        header: decode_b64url_json(header).context("invalid JWT header")?,
        payload: decode_b64url_json(payload).context("invalid JWT payload")?,
    })
}

fn decode_b64url_json(part: &str) -> Result<String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .context("invalid base64url")?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).context("not JSON")?;
    serde_json::to_string_pretty(&value).context("re-serializing")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_and_round_trips() {
        assert_eq!(utc_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        for secs in [
            0i64,
            1,
            86399,
            86400,
            951_782_400,
            1_709_164_800,
            4_102_444_800,
        ] {
            assert_eq!(iso_to_epoch(&utc_iso(secs)), Some(secs), "for {secs}");
        }
    }

    #[test]
    fn epoch_handles_leap_years() {
        // 2024-02-29 exists; 2000 was a leap year (div-400 rule).
        assert_eq!(utc_iso(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(iso_to_epoch("2024-02-29"), Some(1_709_164_800));
        assert_eq!(utc_iso(951_782_400), "2000-02-29T00:00:00Z");
        // Day after Feb 28 in a non-leap year is Mar 1.
        assert_eq!(
            utc_iso(iso_to_epoch("2023-02-28").unwrap() + 86400),
            "2023-03-01T00:00:00Z"
        );
    }

    #[test]
    fn epoch_parses_seconds_and_millis() {
        assert_eq!(parse_epoch("1712345678"), Some(1_712_345_678));
        assert_eq!(parse_epoch(" 1712345678123 "), Some(1_712_345_678));
        assert_eq!(parse_epoch("-86400"), Some(-86400));
        assert_eq!(parse_epoch("2024-01-01"), None);
        assert_eq!(parse_epoch(""), None);
    }

    #[test]
    fn relative_phrases() {
        let now = 1_700_000_000;
        assert_eq!(relative(now - 10, now), "just now");
        assert_eq!(relative(now - 120, now), "2 minutes ago");
        assert_eq!(relative(now - 3 * 86400, now), "3 days ago");
        assert_eq!(relative(now + 2 * 3600, now), "in 2 hours");
        assert_eq!(relative(now - 400 * 86400, now), "1 year ago");
    }

    #[test]
    fn url_round_trip() {
        let input = "hello world/año?q=a&b=ñ+ç";
        let encoded = url_encode(input);
        assert!(!encoded.contains(' '));
        assert_eq!(
            encoded,
            "hello%20world%2Fa%C3%B1o%3Fq%3Da%26b%3D%C3%B1%2B%C3%A7"
        );
        assert_eq!(url_decode(&encoded).unwrap(), input);
        assert!(url_decode("%zz").is_err());
        assert!(url_decode("%e0").is_err()); // lone continuation byte, invalid UTF-8
    }

    #[test]
    fn uuid_v4_shape() {
        let a = uuid_v4();
        let b = uuid_v4();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
    }

    #[test]
    fn jwt_decodes_header_and_payload() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"1234567890","name":"Ada"}"#);
        let token = format!("{header}.{payload}.fakesig");
        let parts = jwt_decode(&token).unwrap();
        assert!(parts.header.contains("\"alg\": \"HS256\""));
        assert!(parts.payload.contains("\"name\": \"Ada\""));
        assert!(jwt_decode("not-a-token").is_err());
    }
}

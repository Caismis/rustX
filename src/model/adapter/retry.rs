//! Provider retry evidence normalization; these helpers never retry a request.

use std::time::{Duration, SystemTime};

/// RFC 9110 Retry-After accepts delay seconds or an HTTP date. Keep the full
/// provider wait; the execution owner decides whether its finite policy admits it.
pub(crate) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    parse_retry_after(
        headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?,
        SystemTime::now(),
    )
}

fn parse_retry_after(value: &str, now: SystemTime) -> Option<u64> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(
            value
                .parse::<u64>()
                .unwrap_or(u64::MAX)
                .saturating_mul(1_000),
        );
    }
    let until = httpdate::parse_http_date(value).ok()?;
    let duration = until.duration_since(now).unwrap_or(Duration::ZERO);
    let millis = duration
        .as_millis()
        .saturating_add(u128::from(duration.subsec_nanos() % 1_000_000 != 0));
    Some(u64::try_from(millis).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_waits_accept_http_dates_and_are_never_clamped_to_local_policy() {
        let now = httpdate::parse_http_date("Sun, 06 Nov 1994 08:49:00 GMT").unwrap();
        for value in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(parse_retry_after(value, now), Some(37_000));
        }
        assert_eq!(parse_retry_after("61", now), Some(61_000));
        assert_eq!(
            parse_retry_after("184467440737095516160", now),
            Some(u64::MAX)
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:48:00 GMT", now),
            Some(0)
        );
        for invalid in ["", "-1", "+1", "0.5", "tomorrow"] {
            assert_eq!(parse_retry_after(invalid, now), None);
        }
    }
}

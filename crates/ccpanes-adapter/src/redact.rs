use sha2::{Digest, Sha256};

use crate::AdapterHeader;

const REDACTED: &str = "<redacted>";

pub fn digest_header_values(headers: &[AdapterHeader]) -> String {
    let mut hasher = Sha256::new();
    for header in headers {
        hasher.update(header.name.to_ascii_lowercase().as_bytes());
        hasher.update(b"\0");
        hasher.update(header.value.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

pub fn redact_sensitive_text(value: &str) -> String {
    let mut redacted = value.to_owned();
    for marker in [
        "Authorization:",
        "authorization:",
        "Bearer ",
        "bearer ",
        "Token ",
        "token ",
    ] {
        let mut search_start = 0;
        while let Some(relative_index) = redacted[search_start..].find(marker) {
            let index = search_start + relative_index;
            let start = index + marker.len();
            let end = redacted[start..]
                .find(|ch: char| {
                    ch.is_whitespace() || ch == ',' || ch == ';' || ch == '}' || ch == ']'
                })
                .map(|offset| start + offset)
                .unwrap_or(redacted.len());
            if start >= end {
                break;
            }
            redacted.replace_range(start..end, REDACTED);
            search_start = start.saturating_add(REDACTED.len());
        }
    }
    redacted
}

pub fn redact_endpoint_for_report(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    let (without_fragment, had_fragment) = trimmed
        .split_once('#')
        .map(|(prefix, _)| (prefix, true))
        .unwrap_or((trimmed, false));
    let (without_query, had_query) = without_fragment
        .split_once('?')
        .map(|(prefix, _)| (prefix, true))
        .unwrap_or((without_fragment, false));

    let mut redacted = redact_endpoint_userinfo(without_query);
    if had_query {
        redacted.push_str("?<redacted>");
    }
    if had_fragment {
        redacted.push_str("#<redacted>");
    }
    redacted
}

fn redact_endpoint_userinfo(endpoint: &str) -> String {
    let Some(scheme_index) = endpoint.find("://") else {
        return endpoint.to_owned();
    };
    let authority_start = scheme_index + "://".len();
    let authority_end = endpoint[authority_start..]
        .find('/')
        .map(|offset| authority_start + offset)
        .unwrap_or(endpoint.len());
    let authority = &endpoint[authority_start..authority_end];
    let Some(userinfo_end) = authority.rfind('@') else {
        return endpoint.to_owned();
    };

    let mut redacted = String::with_capacity(endpoint.len());
    redacted.push_str(&endpoint[..authority_start]);
    redacted.push_str(REDACTED);
    redacted.push('@');
    redacted.push_str(&authority[userinfo_end + 1..]);
    redacted.push_str(&endpoint[authority_end..]);
    redacted
}

pub fn compact_safe_message(value: &str, limit: usize) -> String {
    let compact = redact_sensitive_text(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut characters = compact.chars();
    let message = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        format!("{message}...")
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_removes_bearer_values_from_messages() {
        let message = redact_sensitive_text("Authorization: Bearer TOKEN_PLACEHOLDER failed");
        assert!(!message.contains("TOKEN_PLACEHOLDER"));
        assert!(message.contains(REDACTED));
    }

    #[test]
    fn endpoint_report_redaction_removes_query_fragment_and_userinfo() {
        let endpoint = redact_endpoint_for_report(&format!(
            "http://user:SECRET@127.0.0.1:39000/mcp?{}=SECRET#frag",
            "token"
        ));
        assert_eq!(
            endpoint,
            "http://<redacted>@127.0.0.1:39000/mcp?<redacted>#<redacted>"
        );
        assert!(!endpoint.contains("SECRET"));
    }
}

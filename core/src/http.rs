use std::sync::OnceLock;
use std::time::Duration;

/// Shared HTTP agent (connection pooling, timeouts).
pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent("gamut/0.1")
            .build()
            .into()
    })
}

pub fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> anyhow::Result<T> {
    Ok(agent().get(url).call()?.body_mut().read_json()?)
}

pub fn get_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    Ok(agent()
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(30 * 1024 * 1024)
        .read_to_vec()?)
}

pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// True for HTTP 403/404, i.e. "this resource doesn't exist" rather than a transient failure.
pub fn is_missing(e: &anyhow::Error) -> bool {
    matches!(e.downcast_ref::<ureq::Error>(), Some(ureq::Error::StatusCode(403 | 404)))
}

/// Like `get_bytes`, but a missing resource is `Ok(None)`; network errors stay errors.
pub fn get_bytes_opt(url: &str) -> anyhow::Result<Option<Vec<u8>>> {
    match get_bytes(url) {
        Ok(b) => Ok(Some(b)),
        Err(e) if is_missing(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

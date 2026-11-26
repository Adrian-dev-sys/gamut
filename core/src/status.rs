//! Live checks that the keys in Settings work; results are short user-facing lines.
use anyhow::Result;
use serde_json::Value;

fn code(e: &anyhow::Error) -> Option<u16> {
    match e.downcast_ref::<ureq::Error>() {
        Some(ureq::Error::StatusCode(c)) => Some(*c),
        _ => None,
    }
}

/// "Connected as <persona>", with a note when the profile isn't public.
pub fn steam(cfg: &crate::Config) -> Result<String> {
    let key = cfg.steam_api_key.trim();
    let sid = crate::steam::steam_id(cfg);
    anyhow::ensure!(!sid.is_empty(), "No SteamID found; enter your SteamID64");
    let url = format!(
        "https://api.steampowered.com/ISteamUser/GetPlayerSummaries/v2/?key={}&steamids={}",
        crate::http::encode(key),
        crate::http::encode(&sid)
    );
    let v: Value = crate::http::get_json(&url).map_err(|e| match code(&e) {
        Some(401 | 403) => anyhow::anyhow!("Steam rejected the API key"),
        Some(c) => anyhow::anyhow!("Steam answered HTTP {c}"),
        None => anyhow::anyhow!("Couldn't reach Steam"),
    })?;
    let p = v.pointer("/response/players/0").ok_or_else(|| anyhow::anyhow!("Steam doesn't know this SteamID"))?;
    let name = p.get("personaname").and_then(Value::as_str).unwrap_or("?");
    // 3 = public; anything else hides the game list and achievements from other keys.
    let public = p.get("communityvisibilitystate").and_then(Value::as_i64) == Some(3);
    Ok(if public { format!("Connected as {name}") } else { format!("Connected as {name} · profile isn't public") })
}

pub fn steamgriddb(key: &str) -> Result<String> {
    crate::scrape::sgdb_check(key.trim())?;
    Ok("Connected".into())
}

/// "Connected as <user> · N points".
pub fn retro(user: &str, key: &str) -> Result<String> {
    let url = format!(
        "https://retroachievements.org/API/API_GetUserProfile.php?y={}&u={}",
        crate::http::encode(key.trim()),
        crate::http::encode(user.trim())
    );
    let v: Value = crate::http::get_json(&url).map_err(|e| match code(&e) {
        Some(401 | 403) => anyhow::anyhow!("RetroAchievements rejected the API key"),
        Some(404 | 422) => anyhow::anyhow!("RetroAchievements doesn't know this username"),
        Some(c) => anyhow::anyhow!("RetroAchievements answered HTTP {c}"),
        None => anyhow::anyhow!("Couldn't reach RetroAchievements"),
    })?;
    let name = v.get("User").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("RetroAchievements doesn't know this username"))?;
    // TotalPoints is hardcore only; softcore play lands in TotalSoftcorePoints.
    let pts = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
    let (soft, hard) = (pts("TotalSoftcorePoints"), pts("TotalPoints"));
    Ok(match hard {
        0 => format!("Connected as {name} · {soft} softcore points"),
        _ => format!("Connected as {name} · {soft} softcore · {hard} hardcore points"),
    })
}

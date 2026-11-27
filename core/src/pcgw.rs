//! Save game folders from PCGamingWiki's "Game data/saves" entries.
use crate::http::{agent, encode, throttle};
use crate::model::Game;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const API: &str = "https://www.pcgamingwiki.com/w/api.php";

/// The wiki page title: by Steam appid (the site redirects to the page), else the game's name.
fn page(g: &Game) -> Result<String> {
    if let Some(a) = g.steam_appid().filter(|_| g.source == crate::Source::Steam) {
        let url = format!("https://www.pcgamingwiki.com/api/appid.php?appid={a}");
        throttle(&url);
        use ureq::ResponseExt;
        let r = agent().get(&url).call()?;
        let last = r.get_uri().path().rsplit('/').next().unwrap_or_default().to_string();
        if !last.is_empty() && !last.ends_with(".php") {
            return Ok(decode(&last).replace('_', " "));
        }
    }
    Ok(g.name.clone())
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Windows save paths as written on the wiki (with `{{p|…}}` placeholders), Windows entries before Steam Cloud.
pub fn save_templates(g: &Game) -> Result<Vec<String>> {
    let url = format!("{API}?action=parse&prop=wikitext&format=json&redirects=1&page={}", encode(&page(g)?));
    throttle(&url);
    let v: serde_json::Value = agent().get(&url).call()?.body_mut().read_json()?;
    let text = v["parse"]["wikitext"]["*"].as_str().context("page not on PCGamingWiki")?;
    Ok(templates(text))
}

fn templates(wikitext: &str) -> Vec<String> {
    let mut out: Vec<(u8, String)> = Vec::new();
    for line in wikitext.lines() {
        let Some(rest) = line.trim().strip_prefix("{{Game data/saves|") else { continue };
        let Some((platform, paths)) = rest.split_once('|') else { continue };
        let rank = match platform.trim() {
            "Windows" => 0,
            "Steam" => 1,
            _ => continue,
        };
        let paths = paths.strip_suffix("}}").unwrap_or(paths);
        // Paths are '|'-separated, but placeholders contain '|' too: split only outside {{ }}.
        let (mut depth, mut cur) = (0, String::new());
        let mut chars = paths.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    depth += 1;
                    cur.push_str("{{");
                    chars.next();
                }
                '}' if chars.peek() == Some(&'}') => {
                    depth -= 1;
                    cur.push_str("}}");
                    chars.next();
                }
                '|' if depth == 0 => out.push((rank, std::mem::take(&mut cur))),
                c => cur.push(c),
            }
        }
        out.push((rank, cur));
    }
    out.sort_by_key(|(r, _)| *r);
    out.into_iter().map(|(_, p)| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// A wiki path on this PC; `{{p|uid}}`-style unknowns become wildcards. None if it uses a place we can't map.
pub fn expand(t: &str, g: &Game) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let re = regex::Regex::new(r"(?i)\{\{p\|([^}]*)\}\}").ok()?;
    let mut bad = false;
    let s = re.replace_all(t, |c: &regex::Captures| {
        let v = match c[1].to_lowercase().as_str() {
            "game" => g.folder().map(|p| p.display().to_string()),
            "steam" => crate::steam::steam_path().map(|p| p.display().to_string()),
            "uid" | "steamid" | "userid" | "username" => Some("*".into()),
            "userprofile" => env("USERPROFILE"),
            "userprofile\\documents" => env("USERPROFILE").map(|u| documents(&u)),
            "userprofile\\appdata\\locallow" => env("USERPROFILE").map(|u| format!("{u}\\AppData\\LocalLow")),
            "appdata" => env("APPDATA"),
            "localappdata" => env("LOCALAPPDATA"),
            "programdata" => env("PROGRAMDATA"),
            "public" => env("PUBLIC"),
            _ => None,
        };
        v.unwrap_or_else(|| {
            bad = true;
            String::new()
        })
    });
    (!bad && !s.contains("{{")).then(|| s.trim_end_matches(['\\', '/']).to_string())
}

/// Documents, which OneDrive may have moved.
fn documents(profile: &str) -> String {
    let local = format!("{profile}\\Documents");
    let onedrive = format!("{profile}\\OneDrive\\Documents");
    if !Path::new(&local).exists() && Path::new(&onedrive).exists() { onedrive } else { local }
}

/// The folder an expanded path points at, if it exists: file patterns are dropped, `*` picks the newest match.
pub fn resolve(path: &str) -> Option<PathBuf> {
    let mut parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    // "Saves\*.celeste" or "settings.ini": the folder holding them.
    if parts.last().is_some_and(|l| l.contains('.') && (l.contains('*') || Path::new(l).extension().is_some())) {
        parts.pop();
    }
    let mut cur = PathBuf::from(format!("{}\\", parts.first()?));
    for p in &parts[1..] {
        if p.contains(['*', '?']) {
            let pat = p.to_lowercase();
            let newest = std::fs::read_dir(&cur)
                .ok()?
                .flatten()
                .filter(|e| e.path().is_dir() && crate::scanner::wildcard(&pat, &e.file_name().to_string_lossy().to_lowercase()))
                .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
            cur = newest.path();
        } else {
            cur.push(p);
        }
    }
    cur.is_dir().then_some(cur)
}

/// The game's save folder on this PC, from PCGamingWiki.
pub fn find_save_dir(g: &Game) -> Result<Option<PathBuf>> {
    Ok(save_templates(g)?.iter().filter_map(|t| expand(t, g)).find_map(|p| resolve(&p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_wiki_entries() {
        let w = "{{Game data/saves|Steam|{{p|steam}}\\userdata\\{{p|uid}}\\2246340\\remote}}\n\
                 {{Game data/saves|Windows|{{p|game}}\\Saves\\*.celeste|{{p|appdata}}\\Celeste\\}}\n\
                 {{Game data/saves|OS X|{{p|osxhome}}/Library}}";
        assert_eq!(
            templates(w),
            ["{{p|game}}\\Saves\\*.celeste", "{{p|appdata}}\\Celeste\\", "{{p|steam}}\\userdata\\{{p|uid}}\\2246340\\remote"]
        );
        let g = Game { install_dir: Some("D:\\G\\Celeste".into()), ..Game::new_custom("Celeste".into(), None) };
        assert_eq!(expand("{{p|game}}\\Saves\\*.celeste", &g).as_deref(), Some("D:\\G\\Celeste\\Saves\\*.celeste"));
        assert_eq!(expand("{{p|hkcu}}\\Software\\X", &g), None);
        assert_eq!(decode("Grand_Theft_Auto%3A_Vice_City"), "Grand_Theft_Auto:_Vice_City");
    }
}

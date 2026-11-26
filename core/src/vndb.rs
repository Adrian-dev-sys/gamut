//! VNDB (visual novel database) through its public Kana API: search, metadata and images.
use crate::http::{agent, throttle};
use crate::model::Metadata;
use crate::scrape::SgdbArt;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;

const API: &str = "https://api.vndb.org/kana";

#[derive(Deserialize)]
struct Resp<T> {
    #[serde(default = "Vec::new")]
    results: Vec<T>,
}

#[derive(Deserialize)]
struct Img {
    url: String,
    thumbnail: Option<String>,
    dims: Option<[u32; 2]>,
    /// Average flagging: 0 safe, 1 suggestive, 2 explicit.
    #[serde(default)]
    sexual: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Vn {
    /// e.g. "v20016".
    pub id: String,
    pub title: String,
    pub alttitle: Option<String>,
    pub released: Option<String>,
    /// Short codes like "win", "ps2".
    #[serde(default)]
    pub platforms: Vec<String>,
}

impl Vn {
    pub fn year(&self) -> Option<i32> {
        self.released.as_deref()?.get(..4)?.parse().ok()
    }
}

fn post<T: serde::de::DeserializeOwned>(endpoint: &str, body: serde_json::Value) -> Result<T> {
    let url = format!("{API}/{endpoint}");
    throttle(&url);
    Ok(agent().post(&url).send_json(&body)?.body_mut().read_json()?)
}

/// Visual novels matching a name, best match first.
pub fn search(name: &str) -> Result<Vec<Vn>> {
    let r: Resp<Vn> = post(
        "vn",
        json!({"filters": ["search", "=", name], "fields": "title,alttitle,released,platforms", "sort": "searchrank", "results": 8}),
    )?;
    Ok(r.results)
}

/// All images of a VN: main image, release covers, then screenshots. Explicit ones only with `nsfw`.
pub fn images(id: &str, nsfw: bool) -> Result<Vec<SgdbArt>> {
    #[derive(Deserialize)]
    struct VnImages {
        image: Option<Img>,
        #[serde(default)]
        screenshots: Vec<Img>,
    }
    #[derive(Deserialize)]
    struct Release {
        #[serde(default)]
        images: Vec<Img>,
    }
    let f = |prefix: &str| ["url", "thumbnail", "dims", "sexual"].map(|x| format!("{prefix}.{x}")).join(",");
    let vn: Resp<VnImages> = post(
        "vn",
        json!({"filters": ["id", "=", id], "fields": format!("{},{}", f("image"), f("screenshots"))}),
    )?;
    let rel: Resp<Release> = post(
        "release",
        json!({"filters": ["vn", "=", ["id", "=", id]], "fields": f("images"), "results": 100}),
    )?;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |img: Img, source: &'static str| {
        if (nsfw || img.sexual < 1.0) && seen.insert(img.url.clone()) {
            let [width, height] = img.dims.unwrap_or_default();
            out.push(SgdbArt { thumb: img.thumbnail.unwrap_or_else(|| img.url.clone()), url: img.url, width, height, source: Some(source) });
        }
    };
    let mut shots = Vec::new();
    for v in vn.results {
        if let Some(i) = v.image {
            add(i, "VNDB cover");
        }
        shots.extend(v.screenshots);
    }
    for r in rel.results {
        for i in r.images {
            add(i, "VNDB release image");
        }
    }
    for i in shots {
        add(i, "VNDB screenshot");
    }
    Ok(out)
}

/// A VN's details for the editor: metadata plus its main non-spoiler content tags.
#[derive(Debug, Clone, Default)]
pub struct VnDetails {
    pub title: String,
    pub meta: Metadata,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    rating: f32,
    spoiler: f32,
    #[serde(default)]
    lie: bool,
    category: Option<String>,
}

#[derive(Deserialize)]
struct VnFull {
    title: String,
    released: Option<String>,
    description: Option<String>,
    rating: Option<f32>,
    #[serde(default)]
    developers: Vec<Named>,
    #[serde(default)]
    tags: Vec<Tag>,
}

#[derive(Deserialize)]
struct Producer {
    name: String,
    #[serde(default)]
    publisher: bool,
}

#[derive(Deserialize)]
struct Rel {
    #[serde(default)]
    official: bool,
    #[serde(default)]
    producers: Vec<Producer>,
}

/// Metadata of one VN (two requests: the VN, then its releases for the publisher).
pub fn details(id: &str) -> Result<VnDetails> {
    let fields = "title,released,description,rating,developers.name,tags.name,tags.rating,tags.spoiler,tags.lie,tags.category";
    let r: Resp<VnFull> = post("vn", json!({"filters": ["id", "=", id], "fields": fields}))?;
    let vn = r.results.into_iter().next().ok_or_else(|| anyhow::anyhow!("VNDB has no {id}"))?;
    let rel: Resp<Rel> = post(
        "release",
        json!({"filters": ["vn", "=", ["id", "=", id]], "fields": "official,producers.name,producers.publisher", "sort": "released", "results": 25}),
    )?;
    Ok(build(vn, &rel.results))
}

fn build(vn: VnFull, releases: &[Rel]) -> VnDetails {
    let devs: Vec<&str> = vn.developers.iter().map(|d| d.name.as_str()).collect();
    // Most frequent publisher across official releases; earliest wins ties.
    let mut pubs: Vec<(&str, usize)> = Vec::new();
    for p in releases.iter().filter(|r| r.official).flat_map(|r| &r.producers).filter(|p| p.publisher) {
        match pubs.iter_mut().find(|(n, _)| *n == p.name) {
            Some(e) => e.1 += 1,
            None => pubs.push((&p.name, 1)),
        }
    }
    let publisher = pubs.iter().enumerate().max_by_key(|(i, (_, c))| (*c, std::cmp::Reverse(*i))).map(|(_, (n, _))| n.to_string());
    let mut tags: Vec<&Tag> =
        vn.tags.iter().filter(|t| t.category.as_deref() == Some("cont") && t.spoiler == 0.0 && !t.lie && t.rating >= 2.0).collect();
    tags.sort_by(|a, b| b.rating.total_cmp(&a.rating));
    let description = vn.description.as_deref().map(strip_bbcode).filter(|d| !d.is_empty());
    VnDetails {
        meta: Metadata {
            description,
            developer: (!devs.is_empty()).then(|| devs.join(", ")),
            publisher,
            // Only full dates; VNDB also has "2009", "2009-10" and "TBA".
            release_date: vn.released.filter(|d| d.len() == 10),
            genres: Some("Visual Novel".into()),
            community_score: vn.rating.map(|r| r.round() as i64),
            ..Default::default()
        },
        tags: tags.iter().take(10).map(|t| t.name.clone()).collect(),
        title: vn.title,
    }
}

/// Plain text from VNDB's BBCode: formatting tags dropped, spoilers removed, link text kept.
pub fn strip_bbcode(s: &str) -> String {
    const TAGS: [&str; 9] = ["b", "i", "u", "s", "url", "quote", "raw", "code", "spoiler"];
    let mut out = String::new();
    let mut rest = s;
    let mut spoiler = 0usize;
    while let Some(i) = rest.find('[') {
        let (before, after) = rest.split_at(i);
        if spoiler == 0 {
            out.push_str(before);
        }
        let tag = after[1..].find(']').map(|j| &after[1..1 + j]);
        let name = tag.map(|t| t.trim_start_matches('/').split('=').next().unwrap_or("").to_ascii_lowercase());
        match (tag, name) {
            (Some(t), Some(n)) if TAGS.contains(&n.as_str()) => {
                if n == "spoiler" {
                    spoiler = if t.starts_with('/') { spoiler.saturating_sub(1) } else { spoiler + 1 };
                }
                rest = &after[t.len() + 2..];
            }
            _ => {
                if spoiler == 0 {
                    out.push('[');
                }
                rest = &after[1..];
            }
        }
    }
    if spoiler == 0 {
        out.push_str(rest);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbcode() {
        let d = "[b]Bold[/b] text [spoiler]secret[/spoiler]end.\n\n[From [url=https://x.y]Steam[/url]]";
        assert_eq!(strip_bbcode(d), "Bold text end.\n\n[From Steam]");
    }

    #[test]
    fn builds_details() {
        let vn: VnFull = serde_json::from_value(serde_json::json!({
            "title": "Foo", "released": "2009-10-15", "description": "[i]Hi[/i]", "rating": 90.2,
            "developers": [{"name": "A"}, {"name": "B"}],
            "tags": [
                {"name": "Low", "rating": 1.5, "spoiler": 0, "lie": false, "category": "cont"},
                {"name": "Spoil", "rating": 3, "spoiler": 2, "lie": false, "category": "cont"},
                {"name": "Tech", "rating": 3, "spoiler": 0, "lie": false, "category": "tech"},
                {"name": "Good", "rating": 2.5, "spoiler": 0, "lie": false, "category": "cont"}
            ]
        }))
        .unwrap();
        let rel: Vec<Rel> = serde_json::from_value(serde_json::json!([
            {"official": true, "producers": [{"name": "P1", "publisher": true}]},
            {"official": true, "producers": [{"name": "P2", "publisher": true}]},
            {"official": true, "producers": [{"name": "P2", "publisher": true}]},
            {"official": false, "producers": [{"name": "Fan", "publisher": true}]}
        ]))
        .unwrap();
        let d = build(vn, &rel);
        assert_eq!(d.meta.developer.as_deref(), Some("A, B"));
        assert_eq!(d.meta.publisher.as_deref(), Some("P2"));
        assert_eq!(d.meta.community_score, Some(90));
        assert_eq!(d.meta.description.as_deref(), Some("Hi"));
        assert_eq!(d.tags, vec!["Good"]);
    }
}

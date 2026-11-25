//! VNDB (visual novel database) through its public Kana API: search and images (metadata later).
use crate::http::{agent, throttle};
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
        json!({"filters": ["search", "=", name], "fields": "title,alttitle,released", "sort": "searchrank", "results": 8}),
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

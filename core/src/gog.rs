//! GOG store data through its public catalog and product APIs (no account needed).
use crate::http::{encode, get_json};
use crate::model::Metadata;
use crate::scrape::strip_html;
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct GogGame {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    /// "game" or "pack" (editions/bundles).
    pub kind: String,
    /// Everything but the description, which needs `description`.
    pub meta: Metadata,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct Catalog {
    #[serde(default)]
    products: Vec<Product>,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Product {
    id: String,
    title: String,
    release_date: Option<String>,
    #[serde(default)]
    product_type: String,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    #[serde(default)]
    genres: Vec<Name>,
    #[serde(default)]
    tags: Vec<Name>,
    /// Stars ×10 (48 = 4.8/5).
    #[serde(default)]
    reviews_rating: i64,
    #[serde(default)]
    reviews_count: i64,
}

/// Store search, GOG's relevance order.
pub fn search(name: &str) -> Result<Vec<GogGame>> {
    let url = format!(
        "https://catalog.gog.com/v1/catalog?limit=15&query=like:{}&productType=in:game,pack&locale=en-US",
        encode(name)
    );
    let c: Catalog = get_json(&url)?;
    Ok(c.products.into_iter().map(parse).collect())
}

fn parse(p: Product) -> GogGame {
    let join = |v: Vec<String>| (!v.is_empty()).then(|| v.join(", "));
    // "2015.05.18" -> "2015-05-18".
    let date = p.release_date.map(|d| d.replace('.', "-")).filter(|d| d.len() == 10);
    GogGame {
        year: date.as_deref().and_then(|d| d.get(..4)?.parse().ok()),
        meta: Metadata {
            developer: join(p.developers),
            publisher: join(p.publishers),
            release_date: date,
            genres: join(p.genres.into_iter().map(|g| g.name).collect()),
            community_score: (p.reviews_count > 0 && p.reviews_rating > 0).then_some(p.reviews_rating * 2),
            ..Default::default()
        },
        tags: p.tags.into_iter().map(|t| t.name).filter(|t| t != "Free Game").take(10).collect(),
        id: p.id,
        title: p.title,
        kind: p.product_type,
    }
}

/// The store page description as text.
pub fn description(id: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Desc {
        full: Option<String>,
        lead: Option<String>,
    }
    #[derive(Deserialize)]
    struct Prod {
        description: Option<Desc>,
    }
    let p: Prod = get_json(&format!("https://api.gog.com/products/{id}?expand=description&locale=en-US"))?;
    let d = p.description.and_then(|d| d.full.filter(|f| !f.trim().is_empty()).or(d.lead));
    Ok(d.map(|h| strip_html(&h)).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_catalog() {
        let p = serde_json::from_value(serde_json::json!({
            "id": "1207664663", "title": "The Witcher 3", "releaseDate": "2015.05.18", "productType": "game",
            "developers": ["CD PROJEKT RED"], "publishers": ["CD PROJEKT RED"],
            "genres": [{"name": "Role-playing"}], "tags": [{"name": "Open World"}, {"name": "Free Game"}],
            "reviewsRating": 48, "reviewsCount": 100
        }))
        .unwrap();
        let g = super::parse(p);
        assert_eq!(g.year, Some(2015));
        assert_eq!(g.meta.release_date.as_deref(), Some("2015-05-18"));
        assert_eq!(g.meta.community_score, Some(96));
        assert_eq!(g.tags, vec!["Open World"]);
    }
}

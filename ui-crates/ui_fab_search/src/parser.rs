//! Serde structs for deserialising the Fab marketplace search API.
//!
//! Endpoint: GET https://www.fab.com/i/listings/search
//! Query params: `q`, `listing_types`, `sort_by`, `cursor`

use serde::{Deserialize, Serialize};

/// Top-level API response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabSearchResponse {
    pub results: Vec<FabListing>,
    pub next: Option<String>,
    pub cursors: Option<FabCursors>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabCursors {
    pub next: Option<String>,
    pub previous: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabListing {
    pub uid: String,
    pub title: String,
    pub listing_type: Option<String>,
    #[serde(default)]
    pub is_free: bool,
    #[serde(default)]
    pub is_discounted: bool,
    pub starting_price: Option<FabStartingPrice>,
    pub ratings: Option<FabRatings>,
    #[serde(default)]
    pub thumbnails: Vec<FabThumbnail>,
    pub user: FabUser,
    pub category: Option<FabCategory>,
    #[serde(default)]
    pub asset_formats: Vec<FabAssetFormat>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabStartingPrice {
    #[serde(default)]
    pub price: f64,
    pub currency_code: Option<String>,
    pub discounted_price: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabRatings {
    #[serde(default)]
    pub average_rating: f64,
    #[serde(default)]
    pub total: i32,
    pub rating5: Option<i32>,
    pub rating4: Option<i32>,
    pub rating3: Option<i32>,
    pub rating2: Option<i32>,
    pub rating1: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabThumbnail {
    #[serde(default)]
    pub images: Vec<FabThumbnailImage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabThumbnailImage {
    pub url: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

impl FabThumbnail {
    /// Returns the URL of the image closest to `target_width` pixels wide.
    pub fn best_image_url(&self, target_width: u32) -> Option<&str> {
        self.images
            .iter()
            .min_by_key(|img| img.width.abs_diff(target_width))
            .map(|img| img.url.as_str())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabUser {
    #[serde(default)]
    pub seller_name: String,
    pub profile_image_url: Option<String>,
    pub profile_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabCategory {
    #[serde(default)]
    pub name: String,
    pub path: Option<String>,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabAssetFormat {
    pub asset_format_type: FabAssetFormatType,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabAssetFormatType {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub code: String,
    pub icon: Option<String>,
    pub group_name: Option<String>,
}

// ── Item detail ─────────────────────────────────────────────────────────────

/// Full item detail from GET /i/listings/{uid}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabItemDetail {
    pub uid: String,
    pub title: String,
    pub description: Option<String>,
    pub listing_type: Option<String>,
    #[serde(default)]
    pub is_free: bool,
    pub starting_price: Option<FabStartingPrice>,
    pub ratings: Option<FabRatings>,
    #[serde(default)]
    pub thumbnails: Vec<FabThumbnail>,
    pub user: FabUser,
    pub category: Option<FabCategory>,
    #[serde(default)]
    pub asset_formats: Vec<FabAssetFormat>,
    #[serde(default)]
    pub tags: Vec<FabTag>,
    #[serde(default)]
    pub changelogs: Vec<FabChangelog>,
    #[serde(default)]
    pub licenses: Vec<FabLicense>,
    #[serde(default)]
    pub medias: Vec<FabMedia>,
    pub review_count: Option<i32>,
    pub published_at: Option<String>,
    #[serde(default)]
    pub is_ai_forbidden: bool,
    #[serde(default)]
    pub is_ai_generated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FabTag {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabChangelog {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub published_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabLicense {
    #[serde(default)]
    pub name: String,
    pub price_tier: Option<FabPriceTier>,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabPriceTier {
    #[serde(default)]
    pub currency_code: String,
    #[serde(default)]
    pub price: f64,
    pub discounted_price: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabMedia {
    #[serde(default)]
    pub media_url: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub media_type: String,
    pub position: Option<i32>,
    #[serde(default)]
    pub images: Vec<FabThumbnailImage>,
    pub preview_uid: Option<String>,
}

/// Strip HTML tags from a string, leaving plain text.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

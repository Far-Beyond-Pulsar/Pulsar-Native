//! Serde structs for the Sketchfab REST API v3.
//!
//! Endpoints used:
//!   GET https://api.sketchfab.com/v3/search?type=models   – model search
//!   GET https://api.sketchfab.com/v3/models/{uid}         – model detail
//!   GET https://api.sketchfab.com/v3/categories           – category list

use serde::{Deserialize, Serialize};

// ── Pagination ───────────────────────────────────────────────────────────────

/// Paginated search response returned by /v3/search?type=models.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabSearchResponse {
    #[serde(default)]
    pub results: Vec<SketchfabModel>,
    pub next: Option<String>,
    pub previous: Option<String>,
    pub cursors: Option<SketchfabCursors>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabCursors {
    pub next: Option<String>,
    pub previous: Option<String>,
}

// ── Model (search list entry) ────────────────────────────────────────────────

/// A model entry as returned by the search endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchfabModel {
    pub uid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub viewer_url: String,
    pub embed_url: Option<String>,
    #[serde(default)]
    pub is_downloadable: bool,
    #[serde(default)]
    pub animation_count: i32,
    #[serde(default)]
    pub like_count: i64,
    #[serde(default)]
    pub view_count: i64,
    pub published_at: Option<String>,
    pub staffpicked_at: Option<serde_json::Value>,
    pub thumbnails: Option<SketchfabThumbnails>,
    pub user: Option<SketchfabUser>,
    #[serde(default)]
    pub categories: Vec<SketchfabCategory>,
    #[serde(default)]
    pub tags: Vec<SketchfabTag>,
    /// Either a license slug string or an object with `label`/`slug` fields.
    pub license: Option<serde_json::Value>,
    pub archives: Option<SketchfabArchives>,
}

impl SketchfabModel {
    /// Best thumbnail URL targeting `target_width` pixels.
    pub fn thumb_url(&self, target_width: u32) -> Option<&str> {
        self.thumbnails.as_ref()?.best_image_url(target_width)
    }

    /// Primary category name.
    pub fn primary_category(&self) -> Option<&str> {
        self.categories.first().map(|c| c.name.as_str())
    }

    /// Display-friendly license label.
    pub fn license_label(&self) -> Option<String> {
        license_value_label(self.license.as_ref())
    }
}

// ── Model Detail ─────────────────────────────────────────────────────────────

/// Full model detail from GET /v3/models/{uid}.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchfabModelDetail {
    pub uid: String,
    #[serde(default)]
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub viewer_url: String,
    pub embed_url: Option<String>,
    pub editor_url: Option<String>,
    #[serde(default)]
    pub is_downloadable: bool,
    #[serde(default)]
    pub is_age_restricted: bool,
    #[serde(default)]
    pub animation_count: i32,
    #[serde(default)]
    pub sound_count: i32,
    #[serde(default)]
    pub like_count: i64,
    #[serde(default)]
    pub view_count: i64,
    #[serde(default)]
    pub download_count: i64,
    #[serde(default)]
    pub comment_count: i32,
    pub face_count: Option<i64>,
    pub vertex_count: Option<i64>,
    pub material_count: Option<i32>,
    pub texture_count: Option<i32>,
    pub pbr_type: Option<String>,
    pub published_at: Option<String>,
    pub staffpicked_at: Option<serde_json::Value>,
    pub thumbnails: Option<SketchfabThumbnails>,
    pub user: Option<SketchfabUser>,
    #[serde(default)]
    pub categories: Vec<SketchfabCategory>,
    #[serde(default)]
    pub tags: Vec<SketchfabTag>,
    pub license: Option<serde_json::Value>,
    pub archives: Option<SketchfabArchives>,
    pub status: Option<serde_json::Value>,
    pub source: Option<String>,
}

impl SketchfabModelDetail {
    /// Display-friendly license label.
    pub fn license_label(&self) -> Option<String> {
        license_value_label(self.license.as_ref())
    }

    /// Hero thumbnail URL (largest available).
    pub fn hero_thumbnail_url(&self) -> Option<&str> {
        self.thumbnails.as_ref()?.best_image_url(1024)
    }

    /// All unique thumbnail image URLs sorted by width descending.
    pub fn all_thumbnail_urls(&self) -> Vec<&str> {
        let mut images = match self.thumbnails.as_ref() {
            Some(t) => t.images.iter().collect::<Vec<_>>(),
            None => return Vec::new(),
        };
        images.sort_by_key(|i| std::cmp::Reverse(i.width.unwrap_or(0)));
        images.dedup_by_key(|i| i.width.unwrap_or(0));
        images.iter().map(|i| i.url.as_str()).collect()
    }
}

fn license_value_label(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(license_slug_display(s)),
        serde_json::Value::Object(o) => {
            let label = o.get("label")
                .or_else(|| o.get("fullName"))
                .or_else(|| o.get("slug"))
                .or_else(|| o.get("name"))
                .and_then(|v| v.as_str())?;
            Some(license_slug_display(label))
        }
        _ => None,
    }
}

fn license_slug_display(slug: &str) -> String {
    match slug {
        "cc0"       => "CC0 (Public Domain)",
        "by"        => "CC BY",
        "by-sa"     => "CC BY-SA",
        "by-nd"     => "CC BY-ND",
        "by-nc"     => "CC BY-NC",
        "by-nc-sa"  => "CC BY-NC-SA",
        "by-nc-nd"  => "CC BY-NC-ND",
        "ed"        => "Editorial",
        "st"        => "Standard",
        _           => slug,
    }.to_string()
}

// ── Thumbnails ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabThumbnails {
    #[serde(default)]
    pub images: Vec<SketchfabThumbnailImage>,
}

impl SketchfabThumbnails {
    /// URL of the image whose width is closest to `target_width`.
    pub fn best_image_url(&self, target_width: u32) -> Option<&str> {
        if self.images.is_empty() {
            return None;
        }
        self.images
            .iter()
            .min_by_key(|i| i.width.unwrap_or(0).abs_diff(target_width))
            .map(|i| i.url.as_str())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabThumbnailImage {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub uid: Option<String>,
    pub size: Option<i64>,
}

// ── User ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchfabUser {
    pub uid: Option<String>,
    #[serde(default)]
    pub username: String,
    pub display_name: Option<String>,
    pub profile_url: Option<String>,
    pub account: Option<String>,
    pub uri: Option<String>,
    pub avatar: Option<SketchfabAvatar>,
}

impl SketchfabUser {
    /// Returns `displayName` if set and non-empty, otherwise `username`.
    pub fn display(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.username)
    }

    /// Best avatar image URL closest to `target_width`.
    pub fn avatar_url(&self, target_width: u32) -> Option<&str> {
        self.avatar.as_ref()?.best_image_url(target_width)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabAvatar {
    pub uid: Option<String>,
    pub uri: Option<String>,
    #[serde(default)]
    pub images: Vec<SketchfabThumbnailImage>,
}

impl SketchfabAvatar {
    pub fn best_image_url(&self, target_width: u32) -> Option<&str> {
        self.images
            .iter()
            .min_by_key(|i| i.width.unwrap_or(0).abs_diff(target_width))
            .map(|i| i.url.as_str())
    }
}

// ── Categories & Tags ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabCategory {
    pub uid: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
    pub uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabTag {
    #[serde(default)]
    pub slug: String,
    pub uri: Option<String>,
}

/// Response from GET /v3/categories.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabCategoryResponse {
    #[serde(default)]
    pub results: Vec<SketchfabCategory>,
}

// ── Archives ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SketchfabArchives {
    pub source: Option<SketchfabArchiveNested>,
    pub glb: Option<SketchfabArchiveNested>,
    pub gltf: Option<SketchfabArchiveNested>,
    pub usdz: Option<SketchfabArchiveNested>,
}

impl SketchfabArchives {
    /// Returns list of (label, archive) for all present archive types.
    pub fn available(&self) -> Vec<(&'static str, &SketchfabArchiveNested)> {
        let mut out = Vec::new();
        if let Some(ref a) = self.glb    { out.push(("GLB",    a)); }
        if let Some(ref a) = self.gltf   { out.push(("glTF",   a)); }
        if let Some(ref a) = self.usdz   { out.push(("USDZ",   a)); }
        if let Some(ref a) = self.source { out.push(("Source", a)); }
        out
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchfabArchiveNested {
    pub face_count: Option<i64>,
    pub texture_count: Option<i64>,
    pub size: Option<i64>,
    pub vertex_count: Option<i64>,
    pub texture_max_resolution: Option<i64>,
}

impl SketchfabArchiveNested {
    /// Human-readable size string (e.g. "12.3 MB").
    pub fn size_label(&self) -> Option<String> {
        let bytes = self.size?;
        if bytes >= 1_000_000 {
            Some(format!("{:.1} MB", bytes as f64 / 1_000_000.0))
        } else if bytes >= 1_000 {
            Some(format!("{:.0} KB", bytes as f64 / 1_000.0))
        } else {
            Some(format!("{} B", bytes))
        }
    }
}

// ── Utility ──────────────────────────────────────────────────────────────────

/// Strip HTML tags and decode common HTML entities to plain text.
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
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format a large integer for display (e.g. 1234567 → "1.2M").
pub fn fmt_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

use std::cmp::Reverse;


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



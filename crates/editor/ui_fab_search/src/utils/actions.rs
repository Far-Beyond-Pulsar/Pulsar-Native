use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) enum DownloadState {
    InProgress {
        filename: String,
        bytes_received: u64,
        total_bytes: Option<u64>,
        speed_history: Vec<f64>,
        speed_bps: f64,
    },
    Done {
        filename: String,
        path: PathBuf,
        total_bytes: u64,
    },
    Error {
        filename: String,
        message: String,
    },
}

impl DownloadState {
    pub(crate) fn filename(&self) -> &str {
        match self {
            DownloadState::InProgress { filename, .. } => filename,
            DownloadState::Done { filename, .. } => filename,
            DownloadState::Error { filename, .. } => filename,
        }
    }
}

pub(crate) enum DownloadMsg {
    Progress {
        bytes_received: u64,
        total: Option<u64>,
        speed_bps: f64,
    },
    Done {
        path: PathBuf,
        total: u64,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SortBy {
    Relevance,
    MostViewed,
    MostLiked,
    Newest,
    Oldest,
}

impl SortBy {
    pub(crate) fn api_value(&self) -> Option<&'static str> {
        match self {
            SortBy::Relevance => None,
            SortBy::MostViewed => Some("-viewCount"),
            SortBy::MostLiked => Some("-likeCount"),
            SortBy::Newest => Some("-publishedAt"),
            SortBy::Oldest => Some("publishedAt"),
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            SortBy::Relevance => "Relevance",
            SortBy::MostViewed => "Most Viewed",
            SortBy::MostLiked => "Most Liked",
            SortBy::Newest => "Newest",
            SortBy::Oldest => "Oldest",
        }
    }
    pub(crate) fn all() -> [SortBy; 5] {
        [
            SortBy::Relevance,
            SortBy::MostViewed,
            SortBy::MostLiked,
            SortBy::Newest,
            SortBy::Oldest,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LicenseFilter {
    All,
    CC0,
    CcBy,
    CcBySa,
    CcByNd,
    CcByNc,
    CcByNcSa,
    CcByNcNd,
    Standard,
    Editorial,
}

impl LicenseFilter {
    pub(crate) fn api_value(&self) -> Option<&'static str> {
        match self {
            LicenseFilter::All => None,
            LicenseFilter::CC0 => Some("cc0"),
            LicenseFilter::CcBy => Some("by"),
            LicenseFilter::CcBySa => Some("by-sa"),
            LicenseFilter::CcByNd => Some("by-nd"),
            LicenseFilter::CcByNc => Some("by-nc"),
            LicenseFilter::CcByNcSa => Some("by-nc-sa"),
            LicenseFilter::CcByNcNd => Some("by-nc-nd"),
            LicenseFilter::Standard => Some("st"),
            LicenseFilter::Editorial => Some("ed"),
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            LicenseFilter::All => "All Licenses",
            LicenseFilter::CC0 => "CC0",
            LicenseFilter::CcBy => "CC BY",
            LicenseFilter::CcBySa => "CC BY-SA",
            LicenseFilter::CcByNd => "CC BY-ND",
            LicenseFilter::CcByNc => "CC BY-NC",
            LicenseFilter::CcByNcSa => "CC BY-NC-SA",
            LicenseFilter::CcByNcNd => "CC BY-NC-ND",
            LicenseFilter::Standard => "Standard",
            LicenseFilter::Editorial => "Editorial",
        }
    }
    pub(crate) fn all() -> Vec<LicenseFilter> {
        use LicenseFilter::*;
        vec![
            All, CC0, CcBy, CcBySa, CcByNd, CcByNc, CcByNcSa, CcByNcNd, Standard, Editorial,
        ]
    }
}

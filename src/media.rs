use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

/// A single media item in the catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub year: Option<u16>,
    #[serde(default)]
    pub rating: Option<f32>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub poster_url: Option<String>,
    #[serde(default)]
    pub stream_url: Option<String>,
    /// Display name of the provider that supplied this item, if any.
    #[serde(default)]
    pub provider: Option<String>,
}

/// Raw JSON shape returned by the catalog source (file or HTTP endpoint).
#[derive(Debug, Deserialize)]
struct CatalogResponse {
    items: Vec<MediaItem>,
}

/// In-memory media catalog with search/filter and async loading.
pub struct Catalog {
    pub items: Vec<MediaItem>,
    pub search_query: String,
    pub loading: bool,
}

impl Catalog {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            search_query: String::new(),
            loading: false,
        }
    }

    /// Returns items matching the current search query (all items when empty).
    pub fn filtered(&self) -> Vec<&MediaItem> {
        if self.search_query.is_empty() {
            return self.items.iter().collect();
        }

        let q = self.search_query.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                item.title.to_lowercase().contains(&q)
                    || item.genres.iter().any(|g| g.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn set_search(&mut self, query: String) {
        self.search_query = query;
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
    }

    /// Load catalog from a local JSON file (useful for demo / offline testing).
    pub async fn load_from_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.loading = true;
        let content = tokio::fs::read_to_string(path).await?;
        let response: CatalogResponse = serde_json::from_str(&content)?;
        self.items = response.items;
        self.loading = false;
        Ok(())
    }

    /// Load catalog from an HTTP JSON endpoint.
    pub async fn fetch_from_url(&mut self, url: &str) -> Result<()> {
        self.loading = true;
        let result: Result<()> = async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()?;
            let response = client.get(url).send().await?.error_for_status()?;
            let catalog: CatalogResponse = response.json().await?;
            self.items = catalog.items;
            Ok(())
        }
        .await;
        self.loading = false;
        result
    }
}

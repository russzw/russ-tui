use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single watch-history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub media_id: String,
    pub title: String,
    /// Last known playback position in seconds.
    pub position: f64,
    /// Total duration in seconds, if known.
    pub duration: Option<f64>,
    /// Unix timestamp (seconds) of the last watch.
    pub watched_at: u64,
    /// Whether the item was watched to (near) completion.
    #[serde(default)]
    pub completed: bool,
}

/// Where the catalog is loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    /// Bundled `data/movies.json` sample catalog.
    #[default]
    Bundled,
    /// HTTP(S) endpoint returning catalog JSON.
    RemoteUrl,
    /// Public-domain films from the Internet Archive.
    InternetArchive,
    /// User-configured providers (catalog JSON or Stremio addons).
    Providers,
}

impl CatalogSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bundled => "Bundled sample",
            Self::RemoteUrl => "Remote URL",
            Self::InternetArchive => "Internet Archive",
            Self::Providers => "My providers",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Bundled => Self::RemoteUrl,
            Self::RemoteUrl => Self::InternetArchive,
            Self::InternetArchive => Self::Providers,
            Self::Providers => Self::Bundled,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            Self::Bundled => Self::Providers,
            Self::RemoteUrl => Self::Bundled,
            Self::InternetArchive => Self::RemoteUrl,
            Self::Providers => Self::InternetArchive,
        }
    }
}

/// A user-configured media provider.
///
/// The app only consumes the URL — no providers are bundled. Supported endpoint
/// shapes are catalog JSON (`{"items": [...]}` or a bare array) and Stremio
/// addon manifests (`.../manifest.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Display name shown in the UI. Empty means "derive from the URL".
    #[serde(default)]
    pub name: String,
    /// Catalog JSON endpoint or Stremio addon base/manifest URL.
    pub url: String,
    /// Disabled providers are skipped when loading the catalog.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl ProviderConfig {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            enabled: true,
        }
    }
}

/// Everything persisted to disk between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub volume: i32,
    /// Bookmarked media IDs.
    pub bookmarks: Vec<String>,
    pub history: Vec<HistoryEntry>,
    /// Which catalog source to load at startup.
    #[serde(default)]
    pub catalog_source: CatalogSource,
    /// Optional HTTP endpoint serving the catalog JSON. Falls back to the bundled
    /// `data/movies.json` when unset or unreachable.
    #[serde(default)]
    pub catalog_url: Option<String>,
    /// Real-Debrid API key — enables magnet-link resolution into direct stream URLs.
    #[serde(default)]
    pub debrid_api_key: Option<String>,
    /// User-configured catalog providers.
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            volume: 70,
            bookmarks: Vec::new(),
            history: Vec::new(),
            catalog_source: CatalogSource::Bundled,
            catalog_url: None,
            debrid_api_key: None,
            providers: Vec::new(),
        }
    }
}

impl PersistedState {
    /// Path to the state file: `<config_dir>/russ-tui/state.json`.
    pub fn config_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("could not determine the platform config directory")?
            .join("russ-tui");
        Ok(dir.join("state.json"))
    }

    /// Load state from disk, falling back to defaults on any error.
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    fn try_load() -> Result<Self> {
        let path = Self::config_path()?;
        let content = std::fs::read_to_string(&path)?;
        let state = serde_json::from_str(&content)?;
        Ok(state)
    }

    /// Persist state to disk, creating the parent directory if needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    // ── Bookmarks ───────────────────────────────────────────────────────

    pub fn is_bookmarked(&self, id: &str) -> bool {
        self.bookmarks.iter().any(|b| b == id)
    }

    /// Add or remove a bookmark; returns `true` if it is now bookmarked.
    pub fn toggle_bookmark(&mut self, id: &str) -> bool {
        if let Some(pos) = self.bookmarks.iter().position(|b| b == id) {
            self.bookmarks.remove(pos);
            false
        } else {
            self.bookmarks.push(id.to_string());
            true
        }
    }

    // ── History ─────────────────────────────────────────────────────────

    /// Record or update a history entry, keeping most-recent first.
    pub fn record_history(
        &mut self,
        media_id: &str,
        title: &str,
        position: f64,
        duration: Option<f64>,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let completed = matches!(duration, Some(d) if d > 0.0 && position >= d - 5.0);

        if let Some(entry) = self.history.iter_mut().find(|h| h.media_id == media_id) {
            entry.position = position;
            entry.duration = duration;
            entry.watched_at = now;
            entry.completed = completed;
        } else {
            self.history.push(HistoryEntry {
                media_id: media_id.to_string(),
                title: title.to_string(),
                position,
                duration,
                watched_at: now,
                completed,
            });
        }

        self.history
            .sort_by_key(|entry| std::cmp::Reverse(entry.watched_at));
        self.history.truncate(100);
    }

    /// Resume position for an item, if it was partially watched (>5s, not completed).
    pub fn resume_position(&self, media_id: &str) -> Option<f64> {
        self.history
            .iter()
            .find(|h| h.media_id == media_id)
            .filter(|h| !h.completed && h.position > 5.0)
            .map(|h| h.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_source_cycles_both_ways() {
        assert_eq!(CatalogSource::Bundled.next(), CatalogSource::RemoteUrl);
        assert_eq!(
            CatalogSource::RemoteUrl.next(),
            CatalogSource::InternetArchive
        );
        assert_eq!(
            CatalogSource::InternetArchive.next(),
            CatalogSource::Providers
        );
        assert_eq!(CatalogSource::Providers.next(), CatalogSource::Bundled);

        assert_eq!(CatalogSource::Bundled.previous(), CatalogSource::Providers);
        assert_eq!(
            CatalogSource::Providers.previous(),
            CatalogSource::InternetArchive
        );
        assert_eq!(
            CatalogSource::InternetArchive.previous(),
            CatalogSource::RemoteUrl
        );
        assert_eq!(CatalogSource::RemoteUrl.previous(), CatalogSource::Bundled);
    }

    #[test]
    fn catalog_source_defaults_to_bundled_for_legacy_state() {
        let state: PersistedState =
            serde_json::from_str(r#"{"volume":70,"bookmarks":[],"history":[]}"#)
                .expect("legacy state should deserialize");
        assert_eq!(state.catalog_source, CatalogSource::Bundled);
        assert_eq!(state.catalog_url, None);
        assert_eq!(state.debrid_api_key, None);
        assert!(state.providers.is_empty());
    }

    #[test]
    fn providers_default_to_enabled_and_round_trip() {
        let state: PersistedState = serde_json::from_str(
            r#"{"volume":70,"bookmarks":[],"history":[],
                "providers":[{"name":"Test","url":"https://example.com/catalog.json"}]}"#,
        )
        .expect("state with provider should deserialize");

        assert_eq!(state.providers.len(), 1);
        assert_eq!(state.providers[0].name, "Test");
        assert!(state.providers[0].enabled);

        let json = serde_json::to_string(&state).expect("state should serialize");
        assert!(json.contains("\"enabled\":true"));
    }
}

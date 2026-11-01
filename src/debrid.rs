use anyhow::{Context, Result};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// A resolver that turns a torrent-ish source (magnet link or file host URL) into a
/// direct HTTP stream URL playable by mpv.
pub trait DebridResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;
}

/// Real-Debrid (https://real-debrid.com) REST API client.
///
/// Magnet flow: addMagnet -> selectFiles -> info -> unrestrict the first link.
/// HTTP host flow: unrestrict/link.
pub struct RealDebrid {
    client: reqwest::Client,
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct RdAddMagnet {
    id: String,
}

#[derive(Debug, Deserialize)]
struct RdTorrentInfo {
    links: Vec<String>,
    status: String,
}

#[derive(Debug, Deserialize)]
struct RdUnrestrict {
    download: String,
}

impl RealDebrid {
    pub fn new(api_key: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            api_key: api_key.to_string(),
        }
    }

    fn base(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.post(url).bearer_auth(&self.api_key)
    }

    /// Resolve a magnet URI into the first unrestrictable direct link.
    async fn resolve_magnet(&self, magnet: &str) -> Result<String> {
        // 1. Add the magnet.
        let added: RdAddMagnet = self
            .base("https://api.real-debrid.com/rest/1.0/torrents/addMagnet")
            .form(&[("magnet", magnet)])
            .send()
            .await
            .context("real-debrid: addMagnet failed")?
            .error_for_status()
            .context("real-debrid: addMagnet rejected")?
            .json()
            .await?;

        // 2. Select all files (a real client would let the user pick).
        let select_url = format!(
            "https://api.real-debrid.com/rest/1.0/torrents/selectFiles/{}/",
            added.id
        );
        self.base(&select_url)
            .form(&[("files", "all")])
            .send()
            .await
            .context("real-debrid: selectFiles failed")?
            .error_for_status()
            .context("real-debrid: selectFiles rejected")?;

        // 3. Poll until the torrent is downloaded (RD caches popular torrents).
        let info_url = format!(
            "https://api.real-debrid.com/rest/1.0/torrents/info/{}",
            added.id
        );
        let mut info: RdTorrentInfo = self
            .client
            .get(&info_url)
            .bearer_auth(&self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        for _ in 0..30 {
            if info.status == "downloaded" && !info.links.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            info = self
                .client
                .get(&info_url)
                .bearer_auth(&self.api_key)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
        }
        anyhow::ensure!(
            info.status == "downloaded" && !info.links.is_empty(),
            "real-debrid: torrent not ready (status: {})",
            info.status
        );

        // 4. Unrestrict the first link into a direct stream URL.
        let unrestricted: RdUnrestrict = self
            .base("https://api.real-debrid.com/rest/1.0/unrestrict/link")
            .form(&[("link", info.links[0].as_str())])
            .send()
            .await
            .context("real-debrid: unrestrict failed")?
            .error_for_status()
            .context("real-debrid: unrestrict rejected")?
            .json()
            .await?;

        Ok(unrestricted.download)
    }

    /// Resolve a direct file-host link into a stream URL.
    async fn resolve_link(&self, link: &str) -> Result<String> {
        let unrestricted: RdUnrestrict = self
            .base("https://api.real-debrid.com/rest/1.0/unrestrict/link")
            .form(&[("link", link)])
            .send()
            .await
            .context("real-debrid: unrestrict failed")?
            .error_for_status()
            .context("real-debrid: unrestrict rejected")?
            .json()
            .await?;
        Ok(unrestricted.download)
    }
}

impl DebridResolver for RealDebrid {
    fn resolve<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(host_link) = url.strip_prefix("debrid:") {
                self.resolve_link(host_link).await
            } else if url.starts_with("magnet:") {
                self.resolve_magnet(url).await
            } else {
                Ok(url.to_string())
            }
        })
    }
}

/// Build a resolver for the given URL if possible. Magnet links and `debrid:`-prefixed
/// file-host links are resolved; plain HTTP(S) streams are played directly.
pub fn resolver_for(url: &str, debrid_api_key: Option<&str>) -> Option<Box<dyn DebridResolver>> {
    let key = debrid_api_key?;
    if url.starts_with("magnet:") || url.starts_with("debrid:") {
        Some(Box::new(RealDebrid::new(key)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_only_handles_magnets_and_debrid_prefixes() {
        assert!(resolver_for("magnet:?xt=urn:btih:abc", Some("key")).is_some());
        assert!(resolver_for("debrid:https://host.example/file.mkv", Some("key")).is_some());
        assert!(resolver_for("https://example.com/stream.mp4", Some("key")).is_none());
        assert!(resolver_for("placeholder://Some Title", Some("key")).is_none());
        assert!(resolver_for("magnet:?xt=urn:btih:abc", None).is_none());
    }
}

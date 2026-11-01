use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

use crate::media::MediaItem;
use crate::storage::ProviderConfig;

/// Result of loading every enabled provider.
#[derive(Debug, Default)]
pub struct FetchOutcome {
    pub items: Vec<MediaItem>,
    /// Human-readable failures, one per provider/catalog.
    pub errors: Vec<String>,
    /// Number of enabled providers that were attempted.
    pub provider_count: usize,
}

/// Fetch every enabled provider concurrently.
///
/// A failing provider does not prevent the others from loading; its error is
/// collected in [`FetchOutcome::errors`].
pub async fn fetch_all(providers: &[ProviderConfig]) -> FetchOutcome {
    let enabled: Vec<ProviderConfig> = providers
        .iter()
        .filter(|provider| provider.enabled)
        .cloned()
        .collect();

    let mut outcome = FetchOutcome {
        provider_count: enabled.len(),
        ..Default::default()
    };

    let mut tasks = tokio::task::JoinSet::new();
    for provider in enabled {
        tasks.spawn(async move {
            let name = display_name(&provider);
            let result = fetch_provider(&provider).await;
            (name, result)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_name, Ok(mut items))) => outcome.items.append(&mut items),
            Ok((name, Err(err))) => outcome.errors.push(format!("{name}: {err:#}")),
            Err(err) => outcome.errors.push(format!("provider task failed: {err}")),
        }
    }

    outcome
}

/// Fetch a single provider and convert its catalog into [`MediaItem`]s.
///
/// Two endpoint shapes are supported and detected automatically:
///
/// * catalog JSON — `{"items": [...]}` or a bare JSON array of items;
/// * Stremio addon — a manifest (`.../manifest.json`) whose movie catalogs are
///   fetched from `{base}/catalog/{type}/{id}.json`.
///
/// When the supplied URL is a Stremio base URL rather than a manifest, the
/// module falls back to `{base}/manifest.json`.
pub async fn fetch_provider(provider: &ProviderConfig) -> Result<Vec<MediaItem>> {
    let client = http_client()?;
    let requested = normalize_provider_url(&provider.url);
    anyhow::ensure!(!requested.is_empty(), "provider URL is empty");

    let (base, value) = fetch_source(&client, &requested).await?;

    if is_stremio_manifest(&value) {
        fetch_stremio_catalog(&client, &base, &value, provider).await
    } else {
        let items = parse_catalog_items(&value)?;
        Ok(namespace_catalog_items(items, provider))
    }
}

/// Resolve a `stremio:<base>|<type>|<id>` marker into a playable URL.
///
/// Direct HTTP(S) stream URLs are preferred. If the addon only provides an
/// `infoHash`, a magnet link is returned so the existing Real-Debrid resolver
/// can turn it into a direct stream.
pub async fn resolve_stremio_stream(marker: &str) -> Result<String> {
    let (base, kind, id) = parse_stremio_marker(marker).context("invalid Stremio stream marker")?;

    let client = http_client()?;
    let resource = format!("{id}.json");
    let url = stremio_url(base, &["stream", kind, &resource])?;
    let value = get_json(&client, &url).await?;
    let page: StreamPage =
        serde_json::from_value(value).context("could not parse the Stremio stream response")?;

    select_stream(&page.streams).with_context(|| format!("no playable stream found for {id}"))
}

/// Display name for a provider, falling back to its host when no name is set.
pub fn display_name(provider: &ProviderConfig) -> String {
    let name = provider.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }

    let normalized = normalize_provider_url(&provider.url);
    reqwest::Url::parse(&normalized)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| provider.url.trim().to_string())
}

/// Trim whitespace and a trailing `/manifest.json` or `/` from a provider URL.
pub fn normalize_provider_url(url: &str) -> String {
    let trimmed = url.trim();
    let without_manifest = trimmed.strip_suffix("/manifest.json").unwrap_or(trimmed);
    without_manifest.trim_end_matches('/').to_string()
}

// ── HTTP ────────────────────────────────────────────────────────────────

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("russ-tui/0.1 (user-configured provider)")
        .build()
        .context("could not build the provider HTTP client")
}

/// Load the supplied URL, falling back to `{url}/manifest.json` when the URL
/// itself is not valid JSON (for example when it is a Stremio addon base URL).
async fn fetch_source(client: &reqwest::Client, requested: &str) -> Result<(String, Value)> {
    let direct = get_json(client, requested).await;

    let direct_err = match direct {
        Ok(value) if is_stremio_manifest(&value) => {
            return Ok((manifest_base(requested), value));
        }
        Ok(value) if is_catalog_json(&value) => {
            return Ok((requested.to_string(), value));
        }
        Ok(_) => {
            anyhow!("{requested} returned JSON but it was neither a catalog nor a Stremio manifest")
        }
        Err(err) => err,
    };

    let trimmed = requested.trim_end_matches('/');
    if trimmed.ends_with("/manifest.json") {
        return Err(direct_err);
    }

    let manifest_url = format!("{trimmed}/manifest.json");
    match get_json(client, &manifest_url).await {
        Ok(value) if is_stremio_manifest(&value) => Ok((manifest_base(&manifest_url), value)),
        Ok(_) => Err(direct_err),
        Err(manifest_err) => Err(anyhow!(
            "could not load {requested}: {direct_err:#}; \
             manifest fallback failed: {manifest_err:#}"
        )),
    }
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("could not read the response from {url}"))?;

    if !status.is_success() {
        bail!("{url} returned HTTP {status}");
    }

    serde_json::from_str(&body).with_context(|| format!("{url} did not return valid JSON"))
}

fn manifest_base(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed
        .strip_suffix("/manifest.json")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

// ── Catalog JSON ────────────────────────────────────────────────────────

fn is_catalog_json(value: &Value) -> bool {
    match value {
        Value::Array(_) => true,
        Value::Object(object) => object.get("items").is_some_and(Value::is_array),
        _ => false,
    }
}

fn parse_catalog_items(value: &Value) -> Result<Vec<MediaItem>> {
    let entries: &[Value] = match value {
        Value::Array(entries) => entries.as_slice(),
        Value::Object(object) => object
            .get("items")
            .and_then(Value::as_array)
            .map(|entries| entries.as_slice())
            .context("catalog JSON must be an array or an object with an \"items\" array")?,
        _ => bail!("catalog JSON must be an array or an object with an \"items\" array"),
    };

    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Ok(item) = serde_json::from_value::<MediaItem>(entry.clone()) {
            if !item.id.trim().is_empty() && !item.title.trim().is_empty() {
                items.push(item);
            }
        }
    }

    if items.is_empty() && !entries.is_empty() {
        bail!("catalog JSON contained no valid items (each needs at least id and title)");
    }

    Ok(items)
}

/// Namespace item IDs by provider URL and tag items with the provider name.
fn namespace_catalog_items(items: Vec<MediaItem>, provider: &ProviderConfig) -> Vec<MediaItem> {
    let provider_name = display_name(provider);
    let base = normalize_provider_url(&provider.url);

    items
        .into_iter()
        .map(|mut item| {
            item.id = format!("{base}::{}", item.id);
            if let Some(stream) = item.stream_url.as_mut() {
                *stream = absolutize(&base, stream);
            }
            item.provider = Some(provider_name.clone());
            item
        })
        .collect()
}

/// Turn a relative stream path into an absolute URL based on the provider URL.
fn absolutize(base: &str, stream: &str) -> String {
    let trimmed = stream.trim();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("magnet:")
        || trimmed.starts_with("archive:")
        || trimmed.starts_with("stremio:")
        || trimmed.starts_with("debrid:")
        || trimmed.starts_with("placeholder:")
    {
        return trimmed.to_string();
    }

    let Ok(base_url) = reqwest::Url::parse(base) else {
        return trimmed.to_string();
    };

    base_url
        .join(trimmed)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| trimmed.to_string())
}

// ── Stremio addons ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    catalogs: Vec<ManifestCatalog>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestCatalog {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    extra: Vec<ManifestExtra>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestExtra {
    #[serde(default, rename = "isRequired")]
    is_required: bool,
}

fn is_stremio_manifest(value: &Value) -> bool {
    value.get("catalogs").is_some_and(Value::is_array)
        && (value.get("resources").is_some() || value.get("types").is_some())
}

async fn fetch_stremio_catalog(
    client: &reqwest::Client,
    base: &str,
    manifest_value: &Value,
    provider: &ProviderConfig,
) -> Result<Vec<MediaItem>> {
    let base = normalize_provider_url(base);
    let manifest: Manifest = serde_json::from_value(manifest_value.clone())
        .context("could not parse the Stremio manifest")?;

    // Movie catalogs only; skip catalogs that require an extra (for example a
    // search query) because the browse screen has no provider-specific input.
    let catalogs: Vec<ManifestCatalog> = manifest
        .catalogs
        .into_iter()
        .filter(|catalog| catalog.kind == "movie")
        .filter(|catalog| !catalog.extra.iter().any(|extra| extra.is_required))
        .collect();

    if catalogs.is_empty() {
        bail!("the Stremio manifest has no movie catalogs without required extras");
    }

    let mut tasks = tokio::task::JoinSet::new();
    for catalog in catalogs {
        let client = client.clone();
        let base = base.clone();
        tasks.spawn(async move {
            let label = catalog.name.clone().unwrap_or_else(|| catalog.id.clone());
            let result = fetch_stremio_catalog_page(&client, &base, &catalog).await;
            (label, result)
        });
    }

    let mut items = Vec::new();
    let mut errors = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_label, Ok(mut page))) => items.append(&mut page),
            Ok((label, Err(err))) => errors.push(format!("{label}: {err:#}")),
            Err(err) => errors.push(format!("catalog task failed: {err}")),
        }
    }

    if items.is_empty() {
        if let Some(err) = errors.first() {
            bail!("no movie items returned ({err})");
        }
        bail!("the Stremio manifest movie catalogs returned no items");
    }

    // The same meta can appear in several catalogs (for example "top" and
    // "year"); keep only the first occurrence.
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.id.clone()));

    let provider_name = display_name(provider);
    for item in &mut items {
        item.id = format!("{base}::{}", item.id);
        item.provider = Some(provider_name.clone());
    }

    Ok(items)
}

async fn fetch_stremio_catalog_page(
    client: &reqwest::Client,
    base: &str,
    catalog: &ManifestCatalog,
) -> Result<Vec<MediaItem>> {
    let resource = format!("{}.json", catalog.id);
    let url = stremio_url(base, &["catalog", &catalog.kind, &resource])?;
    let value = get_json(client, &url).await?;
    parse_stremio_metas(&value, base, &catalog.kind)
}

fn stremio_url(base: &str, segments: &[&str]) -> Result<String> {
    let mut url = reqwest::Url::parse(base).context("invalid provider URL")?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow!("provider URL cannot be used as a base URL"))?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

#[derive(Debug, Deserialize)]
struct CatalogPage {
    #[serde(default)]
    metas: Vec<StremioMeta>,
}

#[derive(Debug, Deserialize)]
struct StremioMeta {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    poster: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    year: Option<Value>,
    #[serde(default, rename = "releaseInfo")]
    release_info: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "imdbRating")]
    imdb_rating: Option<Value>,
}

fn parse_stremio_metas(value: &Value, base: &str, kind: &str) -> Result<Vec<MediaItem>> {
    let page: CatalogPage =
        serde_json::from_value(value.clone()).context("could not parse the Stremio catalog")?;

    let mut items = Vec::new();
    for meta in page.metas {
        let id = meta.id.trim().to_string();
        if id.is_empty() {
            continue;
        }

        let title = meta.name.unwrap_or_else(|| id.clone());
        if title.trim().is_empty() {
            continue;
        }

        let year = parse_year(meta.year.as_ref())
            .or_else(|| parse_year_from_text(meta.release_info.as_deref()));

        items.push(MediaItem {
            id: id.clone(),
            title,
            year,
            rating: parse_rating(meta.imdb_rating.as_ref()),
            genres: meta.genres,
            overview: meta.description.unwrap_or_default(),
            poster_url: meta.poster.or(meta.logo),
            stream_url: Some(format!("stremio:{base}|{kind}|{id}")),
            provider: None,
        });
    }

    Ok(items)
}

fn parse_year(value: Option<&Value>) -> Option<u16> {
    match value? {
        Value::Number(number) => number.as_u64().and_then(|year| u16::try_from(year).ok()),
        Value::String(text) => parse_year_from_text(Some(text)),
        _ => None,
    }
}

fn parse_year_from_text(text: Option<&str>) -> Option<u16> {
    let text = text?;
    let digits: String = text
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take(4)
        .collect();
    if digits.len() == 4 {
        digits.parse().ok()
    } else {
        None
    }
}

fn parse_rating(value: Option<&Value>) -> Option<f32> {
    match value? {
        Value::Number(number) => number.as_f64().map(|rating| rating as f32),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct StreamPage {
    #[serde(default)]
    streams: Vec<StremioStream>,
}

#[derive(Debug, Deserialize)]
struct StremioStream {
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "infoHash")]
    info_hash: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

fn parse_stremio_marker(marker: &str) -> Option<(&str, &str, &str)> {
    let rest = marker.strip_prefix("stremio:")?;
    let mut parts = rest.splitn(3, '|');
    let base = parts.next()?.trim();
    let kind = parts.next()?.trim();
    let id = parts.next()?.trim();

    if base.is_empty() || kind.is_empty() || id.is_empty() {
        None
    } else {
        Some((base, kind, id))
    }
}

fn select_stream(streams: &[StremioStream]) -> Option<String> {
    let mut magnet = None;

    for stream in streams {
        if let Some(url) = stream
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            if url.starts_with("http://")
                || url.starts_with("https://")
                || url.starts_with("magnet:")
            {
                return Some(url.to_string());
            }
        }

        if magnet.is_none() {
            if let Some(hash) = stream
                .info_hash
                .as_deref()
                .map(str::trim)
                .filter(|hash| !hash.is_empty())
            {
                magnet = Some(magnet_for_hash(hash, stream.title.as_deref()));
            }
        }
    }

    magnet
}

fn magnet_for_hash(hash: &str, title: Option<&str>) -> String {
    let mut magnet = format!("magnet:?xt=urn:btih:{hash}");
    if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
        magnet.push_str("&dn=");
        magnet.push_str(&encode_query_value(title));
    }
    magnet
}

fn encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn item(id: &str, title: &str) -> MediaItem {
        MediaItem {
            id: id.to_string(),
            title: title.to_string(),
            year: None,
            rating: None,
            genres: Vec::new(),
            overview: String::new(),
            poster_url: None,
            stream_url: None,
            provider: None,
        }
    }

    #[test]
    fn parses_catalog_objects_and_arrays() {
        let object = serde_json::json!({
            "items": [
                {"id": "a", "title": "A"}
            ]
        });
        let items = parse_catalog_items(&object).expect("object catalog");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "a");
        assert!(items[0].genres.is_empty());
        assert_eq!(items[0].overview, "");

        let array = serde_json::json!([
            {
                "id": "b",
                "title": "B",
                "genres": ["Drama"],
                "stream_url": "https://example.com/b.mp4"
            }
        ]);
        let items = parse_catalog_items(&array).expect("array catalog");
        assert_eq!(items[0].id, "b");
        assert_eq!(items[0].genres, vec!["Drama"]);
        assert_eq!(
            items[0].stream_url.as_deref(),
            Some("https://example.com/b.mp4")
        );
    }

    #[test]
    fn namespaces_provider_items_and_resolves_relative_streams() {
        let provider = ProviderConfig::new("Test", "https://example.com/api/catalog.json/");
        let mut entry = item("a", "A");
        entry.stream_url = Some("movie.mp4".to_string());

        let items = namespace_catalog_items(vec![entry], &provider);

        assert_eq!(items[0].id, "https://example.com/api/catalog.json::a");
        assert_eq!(
            items[0].stream_url.as_deref(),
            Some("https://example.com/api/movie.mp4")
        );
        assert_eq!(items[0].provider.as_deref(), Some("Test"));
    }

    #[test]
    fn detects_manifest_and_builds_stremio_urls() {
        let manifest = serde_json::json!({
            "id": "org.example",
            "resources": ["catalog", "stream"],
            "types": ["movie"],
            "catalogs": []
        });
        assert!(is_stremio_manifest(&manifest));
        assert!(!is_stremio_manifest(&serde_json::json!({"items": []})));

        assert_eq!(
            stremio_url("https://example.com/addon/", &["catalog", "movie", "top"])
                .expect("catalog URL"),
            "https://example.com/addon/catalog/movie/top"
        );
        assert_eq!(
            stremio_url("https://example.com", &["stream", "movie", "tt1"]).expect("stream URL"),
            "https://example.com/stream/movie/tt1"
        );
    }

    #[test]
    fn parses_stream_markers_and_prefers_direct_urls() {
        let marker = "stremio:https://addon.example/base|movie|tt123";
        assert_eq!(
            parse_stremio_marker(marker),
            Some(("https://addon.example/base", "movie", "tt123"))
        );
        assert!(parse_stremio_marker("archive:foo").is_none());

        let streams: Vec<StremioStream> = serde_json::from_value(serde_json::json!([
            {"infoHash": "abc123", "title": "Torrent"},
            {"url": "https://cdn.example/movie.mp4", "title": "Direct"}
        ]))
        .expect("streams");

        assert_eq!(
            select_stream(&streams).as_deref(),
            Some("https://cdn.example/movie.mp4")
        );
    }

    #[test]
    fn builds_magnets_from_info_hashes() {
        let streams: Vec<StremioStream> = serde_json::from_value(serde_json::json!([
            {"infoHash": "abc123", "title": "A Movie 1080p"}
        ]))
        .expect("streams");

        let stream = select_stream(&streams).expect("magnet");
        assert!(stream.starts_with("magnet:?xt=urn:btih:abc123"));
        assert!(stream.contains("dn=A+Movie+1080p"));
    }

    #[test]
    fn parses_stremio_meta_fields() {
        let value = serde_json::json!({
            "metas": [
                {
                    "id": "tt123",
                    "type": "movie",
                    "name": "Test Movie",
                    "year": "1999",
                    "imdbRating": "8.1",
                    "genres": ["Drama"],
                    "description": "A test.",
                    "poster": "https://example.com/p.jpg"
                }
            ]
        });

        let items = parse_stremio_metas(&value, "https://addon.example", "movie").expect("metas");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Test Movie");
        assert_eq!(items[0].year, Some(1999));
        assert!((items[0].rating.unwrap() - 8.1).abs() < 0.01);
        assert_eq!(
            items[0].stream_url.as_deref(),
            Some("stremio:https://addon.example|movie|tt123")
        );
    }

    /// Serve canned JSON responses until the test process exits.
    async fn spawn_test_server(routes: Vec<(&'static str, String)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let routes: Arc<HashMap<String, String>> = Arc::new(
            routes
                .into_iter()
                .map(|(path, body)| (path.to_string(), body))
                .collect(),
        );

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let routes = Arc::clone(&routes);
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");

                    let (status, body) = match routes.get(path) {
                        Some(body) => ("200 OK", body.clone()),
                        None => ("404 Not Found", "{}".to_string()),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        format!("http://{addr}")
    }

    #[tokio::test]
    async fn fetches_and_resolves_a_stremio_addon_over_http() {
        let manifest = serde_json::json!({
            "id": "org.example.test",
            "name": "Test Addon",
            "resources": ["catalog", "stream"],
            "types": ["movie"],
            "catalogs": [
                {"type": "movie", "id": "top", "name": "Top Movies"}
            ]
        })
        .to_string();

        let catalog = serde_json::json!({
            "metas": [
                {
                    "id": "tt123",
                    "type": "movie",
                    "name": "Test Movie",
                    "year": "1999",
                    "imdbRating": "8.1",
                    "genres": ["Drama"],
                    "description": "A test.",
                    "poster": "https://example.com/p.jpg"
                }
            ]
        })
        .to_string();

        let streams = serde_json::json!({
            "streams": [
                {"infoHash": "abc123", "title": "Torrent"},
                {"url": "https://cdn.example.com/movie.mp4", "title": "Direct"}
            ]
        })
        .to_string();

        let base = spawn_test_server(vec![
            ("/manifest.json", manifest),
            ("/catalog/movie/top.json", catalog),
            ("/stream/movie/tt123.json", streams),
        ])
        .await;

        let provider = ProviderConfig::new("Test Addon", base.clone());
        let items = fetch_provider(&provider).await.expect("provider fetch");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Test Movie");
        assert_eq!(items[0].id, format!("{base}::tt123"));
        assert_eq!(items[0].provider.as_deref(), Some("Test Addon"));
        assert_eq!(items[0].year, Some(1999));

        let marker = items[0].stream_url.as_deref().expect("stream marker");
        assert_eq!(marker, format!("stremio:{base}|movie|tt123"));

        let stream = resolve_stremio_stream(marker)
            .await
            .expect("stream resolution");
        assert_eq!(stream, "https://cdn.example.com/movie.mp4");
    }
}

use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

use crate::media::MediaItem;

const SEARCH_URL: &str = "https://archive.org/advancedsearch.php";
const METADATA_URL: &str = "https://archive.org/metadata";
const DOWNLOAD_URL: &str = "https://archive.org/download";

/// Fetch popular public-domain feature films from the Internet Archive.
///
/// Streams are not resolved here: each item stores an `archive:<identifier>` marker
/// that [`resolve_stream`] turns into a direct download URL when the user presses play.
pub async fn fetch_popular(limit: usize) -> Result<Vec<MediaItem>> {
    let client = http_client()?;

    let params: Vec<(&str, String)> = vec![
        (
            "q",
            "collection:(feature_films) AND mediatype:(movies) AND format:(MPEG4)".to_string(),
        ),
        ("fl[]", "identifier".to_string()),
        ("fl[]", "title".to_string()),
        ("fl[]", "year".to_string()),
        ("fl[]", "description".to_string()),
        ("fl[]", "subject".to_string()),
        ("fl[]", "avg_rating".to_string()),
        ("sort[]", "downloads desc".to_string()),
        ("rows", limit.to_string()),
        ("page", "1".to_string()),
        ("output", "json".to_string()),
    ];

    let response = client
        .get(SEARCH_URL)
        .query(&params)
        .send()
        .await
        .context("Internet Archive search request failed")?
        .error_for_status()
        .context("Internet Archive search was rejected")?;

    let search: SearchResponse = response.json().await?;
    let items: Vec<MediaItem> = search
        .response
        .docs
        .iter()
        .filter_map(media_item_from_doc)
        .collect();

    anyhow::ensure!(!items.is_empty(), "Internet Archive returned no items");
    Ok(items)
}

/// Resolve an Internet Archive identifier to a direct, playable download URL.
pub async fn resolve_stream(identifier: &str) -> Result<String> {
    let client = http_client()?;
    let url = metadata_url(identifier)?;

    let metadata: MetadataResponse = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Internet Archive metadata request failed for {identifier}"))?
        .error_for_status()
        .with_context(|| format!("Internet Archive metadata was rejected for {identifier}"))?
        .json()
        .await?;

    let file = select_video_file(&metadata.files).with_context(|| {
        format!("Internet Archive item {identifier} has no playable video file")
    })?;

    download_url(identifier, &file.name)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("russ-tui/0.1 (Internet Archive catalog)")
        .build()
        .context("could not build the Internet Archive HTTP client")
}

// ── Search response parsing ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponse {
    response: SearchBody,
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    #[serde(default)]
    docs: Vec<serde_json::Value>,
}

fn media_item_from_doc(doc: &serde_json::Value) -> Option<MediaItem> {
    let identifier = doc.get("identifier")?.as_str()?.trim();
    if identifier.is_empty() {
        return None;
    }

    let title = first_string(doc.get("title")).unwrap_or_else(|| identifier.to_string());
    let year = first_string(doc.get("year"))
        .and_then(|value| value.get(0..4)?.parse::<u16>().ok())
        .or_else(|| {
            first_string(doc.get("date")).and_then(|value| value.get(0..4)?.parse::<u16>().ok())
        });

    let overview = first_string(doc.get("description"))
        .map(|description| strip_html(&description))
        .unwrap_or_default();

    Some(MediaItem {
        id: format!("ia:{identifier}"),
        title,
        year,
        rating: doc
            .get("avg_rating")
            .and_then(|value| value.as_f64())
            .map(|rating| rating as f32),
        genres: string_list(doc.get("subject")),
        overview,
        poster_url: Some(format!("https://archive.org/services/img/{identifier}")),
        stream_url: Some(format!("archive:{identifier}")),
        provider: Some("Internet Archive".to_string()),
    })
}

/// Read a field that the search API may return as a string, number, or array.
fn first_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| value.as_str())
            .map(str::to_string),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::to_string)
            .collect(),
        Some(serde_json::Value::String(text)) => text
            .split([';', ','])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Strip the HTML tags and basic entities that show up in Archive descriptions.
fn strip_html(input: &str) -> String {
    let mut text = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }

    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Metadata / stream resolution ────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MetadataResponse {
    #[serde(default)]
    files: Vec<ArchiveFile>,
}

#[derive(Debug, Deserialize)]
struct ArchiveFile {
    name: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

fn select_video_file(files: &[ArchiveFile]) -> Option<&ArchiveFile> {
    files
        .iter()
        .filter_map(|file| {
            let score = video_score(file);
            (score > 0).then_some((score, file_size(file), file))
        })
        // Prefer the best format, then the largest file (usually the full feature).
        .max_by_key(|(score, size, _)| (*score, *size))
        .map(|(_, _, file)| file)
}

fn video_score(file: &ArchiveFile) -> u8 {
    let name = file.name.to_ascii_lowercase();
    let format = file.format.as_deref().unwrap_or("").to_ascii_lowercase();

    // Never pick sidecar/thumbnail derivatives.
    if format.contains("thumbnail")
        || format.contains("metadata")
        || format.contains("item tile")
        || format.contains("archive bittorrent")
        || name.ends_with(".torrent")
        || name.ends_with(".xml")
        || name.ends_with(".json")
        || name.ends_with(".txt")
        || name.ends_with(".jpg")
        || name.ends_with(".png")
    {
        return 0;
    }

    if name.ends_with(".mp4") || format.contains("mpeg4") || format.contains("h.264") {
        100
    } else if name.ends_with(".webm") || format.contains("webm") {
        90
    } else if name.ends_with(".mkv") || format.contains("matroska") {
        80
    } else if name.ends_with(".ogv") || format.contains("ogg video") {
        70
    } else if name.ends_with(".avi") || format.contains("avi") {
        60
    } else {
        0
    }
}

fn file_size(file: &ArchiveFile) -> u64 {
    file.size
        .as_deref()
        .and_then(|size| size.parse().ok())
        .unwrap_or(0)
}

fn metadata_url(identifier: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(METADATA_URL)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid Internet Archive metadata URL"))?
        .push(identifier);
    Ok(url.to_string())
}

fn download_url(identifier: &str, file_name: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(DOWNLOAD_URL)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid Internet Archive download URL"))?;
        segments.push(identifier);
        // Archive file names can contain subdirectories; encode each part separately.
        for part in file_name.split('/') {
            segments.push(part);
        }
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_docs_with_string_and_array_fields() {
        let json = serde_json::json!({
            "response": {
                "docs": [
                    {
                        "identifier": "night_of_the_living_dead",
                        "title": "Night of the Living Dead",
                        "year": "1968",
                        "description": "<p>A <b>classic</b> horror film &amp; more.</p>",
                        "subject": ["Horror", "Public Domain"]
                    },
                    {
                        "identifier": "second_film",
                        "title": ["Second Film"],
                        "year": 1920,
                        "description": "Silent film",
                        "subject": "Drama; Silent"
                    }
                ]
            }
        });

        let search: SearchResponse = serde_json::from_value(json).expect("valid search JSON");
        let items: Vec<MediaItem> = search
            .response
            .docs
            .iter()
            .filter_map(media_item_from_doc)
            .collect();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "ia:night_of_the_living_dead");
        assert_eq!(items[0].year, Some(1968));
        assert_eq!(items[0].genres, vec!["Horror", "Public Domain"]);
        assert_eq!(items[0].overview, "A classic horror film & more.");
        assert_eq!(
            items[0].stream_url.as_deref(),
            Some("archive:night_of_the_living_dead")
        );
        assert_eq!(items[1].title, "Second Film");
        assert_eq!(items[1].year, Some(1920));
        assert_eq!(items[1].genres, vec!["Drama", "Silent"]);
    }

    #[test]
    fn picks_mp4_over_other_video_derivatives() {
        let files = vec![
            ArchiveFile {
                name: "movie.ogv".into(),
                format: Some("Ogg Video".into()),
                size: Some("900000000".into()),
            },
            ArchiveFile {
                name: "movie.mp4".into(),
                format: Some("MPEG4".into()),
                size: Some("500000000".into()),
            },
            ArchiveFile {
                name: "movie_thumb.jpg".into(),
                format: Some("Thumbnail".into()),
                size: Some("10000".into()),
            },
        ];

        let selected = select_video_file(&files).expect("a video file");
        assert_eq!(selected.name, "movie.mp4");
    }

    #[test]
    fn builds_download_urls_with_encoded_segments() {
        let url = download_url("my film", "sub dir/movie file.mp4").expect("valid URL");
        assert_eq!(
            url,
            "https://archive.org/download/my%20film/sub%20dir/movie%20file.mp4"
        );
    }

    #[tokio::test]
    #[ignore = "requires network access to archive.org"]
    async fn live_archive_smoke() {
        let items = fetch_popular(5).await.expect("Internet Archive search");
        assert!(!items.is_empty(), "expected at least one film");

        let first = &items[0];
        println!("item: {} ({:?})", first.title, first.year);

        let identifier = first
            .stream_url
            .as_deref()
            .and_then(|url| url.strip_prefix("archive:"))
            .expect("archive stream marker");
        let url = resolve_stream(identifier)
            .await
            .expect("Internet Archive stream resolution");
        println!("stream: {url}");

        assert!(url.starts_with("https://archive.org/download/"));
        assert!(url.ends_with(".mp4") || url.ends_with(".webm") || url.ends_with(".mkv"));
    }
}

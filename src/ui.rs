use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, AppMode, ProviderField, SettingField};
use crate::media::MediaItem;
use crate::playback::PlaybackStatus;
use crate::provider;

/// Draw a single frame of the TUI.
pub fn ui(f: &mut Frame, app: &mut App) {
    match app.mode {
        AppMode::Playing => playback_ui(f, app),
        AppMode::Library => library_ui(f, app),
        AppMode::Settings => settings_ui(f, app),
        AppMode::Providers => providers_ui(f, app),
        _ => browse_ui(f, app),
    }

    // Overlays are drawn last so they sit on top of the active screen.
    if app.provider_editing.is_some() {
        provider_edit_overlay(f, app);
    }
    toast_overlay(f, app);
    if app.show_help {
        help_overlay(f);
    }
}

/// ── Browsing screen ───────────────────────────────────────────────────
fn browse_ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Header: title + search
    let header = match app.mode {
        AppMode::Searching => format!("russ-tui  [/{}]", app.search_input),
        _ => format!("russ-tui — {}", app.storage.catalog_source.label()),
    };
    let title =
        Paragraph::new(header).block(Block::default().borders(Borders::ALL).title("Browse"));
    f.render_widget(title, chunks[0]);

    // Main area: catalog list (left) + details panel (right)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // Catalog list
    let filtered: Vec<&MediaItem> = app.catalog.filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|item| {
            let year = item.year.map_or_else(String::new, |y| format!(" ({})", y));
            let rating = item
                .rating
                .map_or_else(String::new, |r| format!("  ★{:.1}", r));
            let bookmark = if app.storage.is_bookmarked(&item.id) {
                "♥ "
            } else {
                ""
            };
            ListItem::new(format!("{bookmark}{}{}{}", item.title, year, rating))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Catalog"))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, main_chunks[0], &mut app.list_state);

    // Details panel
    if let Some(item) = app.selected_item() {
        let mut details = details_lines(item);
        // Resume indicator: where the item would resume from, if partially watched.
        if let Some(pos) = app.storage.resume_position(&item.id) {
            let mins = pos as u64 / 60;
            let secs = pos as u64 % 60;
            details.push(Line::from(Span::styled(
                format!("▶ Resume available at {mins}:{secs:02}"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        if app.storage.is_bookmarked(&item.id) {
            details.push(Line::from(Span::styled(
                "♥ Bookmarked",
                Style::default().fg(Color::Magenta),
            )));
        }
        let details_widget =
            Paragraph::new(details).block(Block::default().borders(Borders::ALL).title("Details"));
        f.render_widget(details_widget, main_chunks[1]);
    } else {
        let empty = Paragraph::new(vec![Line::from("No items")])
            .block(Block::default().borders(Borders::ALL).title("Details"));
        f.render_widget(empty, main_chunks[1]);
    }

    // Footer: help
    let help_items = vec![
        Span::styled("↑/↓ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("navigate   "),
        Span::styled("/ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("search   "),
        Span::styled("enter ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("play   "),
        Span::styled("b ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("bookmark   "),
        Span::styled("l ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("library   "),
        Span::styled("s ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("settings   "),
        Span::styled("p ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("providers   "),
        Span::styled("? ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("help   "),
        Span::styled("q ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("quit"),
    ];
    let help =
        Paragraph::new(vec![Line::from(help_items)]).block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

/// ── Library screen ────────────────────────────────────────────────────
fn library_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Header
    let title = Paragraph::new("russ-tui — Library")
        .block(Block::default().borders(Borders::ALL).title("Library"))
        .style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(title, chunks[0]);

    // Main area: watch history (left) + bookmarks (right)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // ── Continue watching: history entries ──────────────────────────────
    let history_items: Vec<ListItem> = app
        .storage
        .history
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let title = if entry.completed {
                format!("✓ {}", entry.title)
            } else if let Some(dur) = entry.duration.filter(|d| *d > 0.0) {
                let pct = (entry.position / dur * 100.0).round() as u64;
                format!("⏸ {} — {}% watched", entry.title, pct)
            } else {
                format!("⏸ {}", entry.title)
            };
            let style = if idx == app.library_selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::styled(title, style))
        })
        .collect();

    let history_list = if history_items.is_empty() {
        List::new(vec![ListItem::new(
            "No watch history yet — press Enter on a title in Browse",
        )])
    } else {
        List::new(history_items)
    };
    f.render_widget(
        history_list.block(
            Block::default()
                .borders(Borders::ALL)
                .title("Continue Watching"),
        ),
        main_chunks[0],
    );

    // ── Bookmarks ───────────────────────────────────────────────────────
    let bookmark_items: Vec<ListItem> = app
        .storage
        .bookmarks
        .iter()
        .map(|id| {
            let title = app
                .catalog
                .items
                .iter()
                .find(|item| item.id == *id)
                .map(|item| item.title.clone())
                .unwrap_or_else(|| format!("unknown ({id})"));
            ListItem::new(format!("♥ {title}"))
        })
        .collect();

    let bookmark_list = if bookmark_items.is_empty() {
        List::new(vec![ListItem::new(
            "No bookmarks — press b on a title in Browse",
        )])
    } else {
        List::new(bookmark_items)
    };
    f.render_widget(
        bookmark_list.block(Block::default().borders(Borders::ALL).title("Bookmarks")),
        main_chunks[1],
    );

    // Footer: help
    let help_items = vec![
        Span::styled("↑/↓ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("navigate   "),
        Span::styled("enter ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("resume   "),
        Span::styled("l ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("back   "),
        Span::styled("q ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("quit"),
    ];
    let help =
        Paragraph::new(vec![Line::from(help_items)]).block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

/// ── Settings screen ───────────────────────────────────────────────────
fn settings_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Header
    let header = Paragraph::new("russ-tui — Settings")
        .block(Block::default().borders(Borders::ALL).title("Settings"))
        .style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(header, chunks[0]);

    // Masked API key for display (never show the full secret on screen).
    let masked_key = app.storage.debrid_api_key.as_deref().map(|key| {
        let visible: String = key.chars().take(4).collect();
        if key.chars().count() > 4 {
            format!("{visible}••••••")
        } else {
            "••••".to_string()
        }
    });

    let catalog_source = app
        .storage
        .catalog_url
        .clone()
        .unwrap_or_else(|| "(bundled data/movies.json)".to_string());

    let rows = [
        (
            "Catalog source",
            format!("{}   (←/→ to change)", app.storage.catalog_source.label()),
        ),
        ("Catalog URL", catalog_source),
        (
            "Default volume",
            format!("{}%   (←/→ to adjust)", app.volume),
        ),
        (
            "Real-Debrid API key",
            masked_key.unwrap_or_else(|| "(not set)".to_string()),
        ),
    ];

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(idx, (label, value))| {
            // While editing this row, show the live text buffer with a cursor.
            let display = match &app.settings_editing {
                Some((field, buffer)) if field_for_index(idx) == Some(*field) => {
                    if *field == SettingField::DefaultVolume {
                        format!("{buffer}▏   (current: {}%)", app.volume)
                    } else {
                        format!("{buffer}▏")
                    }
                }
                _ => value.clone(),
            };
            let style = if idx == app.settings_selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let line = Line::from(vec![
                Span::styled(
                    format!("{label:<22}"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(display),
            ]);
            ListItem::new(line.style(style))
        })
        .collect();

    let fields = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Configuration"),
    );
    f.render_widget(fields, chunks[1]);

    // Help text for each setting
    let notes = vec![
        Line::from(Span::styled(
            "Catalog source",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "Bundled sample reads data/movies.json. Remote URL fetches your own JSON endpoint. \
             Internet Archive streams public-domain films. My providers loads your own catalog \
             endpoints and Stremio addons (manage them with p).",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Catalog URL",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "HTTP endpoint returning {\"items\": [...]} JSON. Only used by the Remote URL source.",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Real-Debrid API key",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(
            "Resolves magnet links and supported file-host URLs into direct streams. Create a \
             key at real-debrid.com/apitoken.",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Enter edits a field, Enter again saves, Esc cancels.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let notes_widget =
        Paragraph::new(notes).block(Block::default().borders(Borders::ALL).title("About"));
    f.render_widget(notes_widget, chunks[2]);

    // Footer: help
    let help_items = vec![
        Span::styled("↑/↓ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("select   "),
        Span::styled("enter ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("edit/save   "),
        Span::styled("←/→ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("source/volume   "),
        Span::styled("esc ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("back   "),
        Span::styled("q ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("quit"),
    ];
    let help =
        Paragraph::new(vec![Line::from(help_items)]).block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[3]);
}

/// Map a Settings row index to its editable field.
fn field_for_index(idx: usize) -> Option<SettingField> {
    match idx {
        1 => Some(SettingField::CatalogUrl),
        2 => Some(SettingField::DefaultVolume),
        3 => Some(SettingField::DebridApiKey),
        _ => None,
    }
}

/// ── Providers screen ──────────────────────────────────────────────────
fn providers_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    let header = Paragraph::new("russ-tui — Providers")
        .block(Block::default().borders(Borders::ALL).title("Providers"))
        .style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(header, chunks[0]);

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    // Provider list
    let items: Vec<ListItem> = app
        .storage
        .providers
        .iter()
        .enumerate()
        .map(|(idx, provider)| {
            let marker = if provider.enabled { "✓" } else { " " };
            let name = provider::display_name(provider);
            let style = if idx == app.providers_selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let mut spans = vec![
                Span::styled(
                    format!("[{marker}] "),
                    Style::default().fg(if provider.enabled {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
            ];
            if !provider.url.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", provider.url),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new("No providers yet — press a to add one")])
    } else {
        List::new(items)
    };
    f.render_widget(
        list.block(Block::default().borders(Borders::ALL).title("My Providers")),
        main_chunks[0],
    );

    // Details for the selected provider
    let details = if let Some(provider) = app.storage.providers.get(app.providers_selected) {
        let status_color = if provider.enabled {
            Color::Green
        } else {
            Color::DarkGray
        };
        vec![
            Line::from(Span::styled(
                provider::display_name(provider),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    if provider.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    Style::default().fg(status_color),
                ),
            ]),
            Line::from(vec![
                Span::styled("URL: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(provider.url.as_str()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Enter or Space toggles this provider.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "e edits · d deletes · r reloads",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Set Catalog source to My providers in Settings to browse these items.",
                Style::default().fg(Color::Cyan),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "No provider selected",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Supported endpoints:"),
            Line::from("• Catalog JSON: {\"items\": [...]}"),
            Line::from("• A bare JSON array of catalog items"),
            Line::from("• A Stremio addon manifest.json"),
            Line::from(""),
            Line::from(Span::styled(
                "Press a to add your first provider.",
                Style::default().fg(Color::Cyan),
            )),
        ]
    };

    let details_widget = Paragraph::new(details)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Details"));
    f.render_widget(details_widget, main_chunks[1]);

    // Footer
    let help_items = vec![
        Span::styled("a ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("add   "),
        Span::styled("e ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("edit   "),
        Span::styled("d ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("delete   "),
        Span::styled(
            "enter/space ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("toggle   "),
        Span::styled("r ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("reload   "),
        Span::styled("esc ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("back   "),
        Span::styled("q ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("quit"),
    ];
    let help =
        Paragraph::new(vec![Line::from(help_items)]).block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[2]);
}

/// Draw the add/edit provider dialog over the current screen.
fn provider_edit_overlay(f: &mut Frame, app: &App) {
    let Some(edit) = &app.provider_editing else {
        return;
    };

    let rect = centered_rect(74, 11, f.area());

    let field_line = |label: &str, value: &str, selected: bool| {
        let cursor = if selected { "▏" } else { "" };
        let label_style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let value_style = if selected {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default()
        };

        Line::from(vec![
            Span::styled(format!("{label:<8}"), label_style),
            Span::styled(format!("{value}{cursor}"), value_style),
        ])
    };

    let lines = vec![
        field_line("Name", &edit.name, edit.field == ProviderField::Name),
        field_line("URL", &edit.url, edit.field == ProviderField::Url),
        Line::from(""),
        Line::from(Span::styled(
            "Catalog JSON: {\"items\": [...]} · Stremio: manifest.json or addon base URL",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Tab / ↑/↓ switch fields · Enter save · Esc cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let title = if edit.index.is_some() {
        "Edit Provider"
    } else {
        "Add Provider"
    };

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Magenta)),
    );

    f.render_widget(Clear, rect);
    f.render_widget(paragraph, rect);
}

/// ── Playback screen ───────────────────────────────────────────────────
fn playback_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Header: now playing
    let title_text = app
        .now_playing
        .as_ref()
        .map(|item| item.title.as_str())
        .or(app.playback_status.media_title.as_deref())
        .unwrap_or("Now playing");
    let status_icon = if app.playback_status.paused.unwrap_or(false) {
        "⏸"
    } else {
        "▶"
    };
    let title = Paragraph::new(format!("{status_icon} {title_text}"))
        .block(Block::default().borders(Borders::ALL).title("Playback"))
        .style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(title, chunks[0]);

    // Progress bar
    let ratio = progress_ratio(&app.playback_status);
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .ratio(ratio)
        .gauge_style(Style::default().fg(Color::Green));
    f.render_widget(gauge, chunks[1]);

    // Time display
    let time_str = format_time(app.playback_status.time_pos)
        + " / "
        + &format_time(app.playback_status.duration);
    let time = Paragraph::new(time_str).block(Block::default().borders(Borders::ALL).title("Time"));
    f.render_widget(time, chunks[2]);

    // Volume
    let vol = Paragraph::new(format!("Volume: {}%", app.volume))
        .block(Block::default().borders(Borders::ALL).title("Volume"));
    f.render_widget(vol, chunks[3]);

    // Footer: controls
    let help_items = vec![
        Span::styled("space ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("pause   "),
        Span::styled("←/→ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("seek   "),
        Span::styled("↑/↓ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("volume   "),
        Span::styled("q ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("back to browse"),
    ];
    let help =
        Paragraph::new(vec![Line::from(help_items)]).block(Block::default().borders(Borders::ALL));
    f.render_widget(help, chunks[4]);
}

/// Compute progress ratio (0.0–1.0) from status.
fn progress_ratio(status: &PlaybackStatus) -> f64 {
    match (status.time_pos, status.duration) {
        (Some(pos), Some(dur)) if dur > 0.0 => (pos / dur).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Format seconds as MM:SS, or "--:--" if unknown.
fn format_time(seconds: Option<f64>) -> String {
    match seconds {
        Some(s) if s.is_finite() => {
            let total = s as i64;
            let mins = total / 60;
            let secs = total % 60;
            format!("{mins}:{secs:02}")
        }
        _ => "--:--".to_string(),
    }
}

/// Build the detail-view lines for a media item.
fn details_lines(item: &MediaItem) -> Vec<Line<'_>> {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(item.title.clone()));
    lines.push(Line::from(format!(
        "Year: {:?}   Rating: {:?}",
        item.year, item.rating
    )));
    lines.push(Line::from(format!("Genres: {}", item.genres.join(", "))));
    if let Some(provider) = &item.provider {
        lines.push(Line::from(format!("Provider: {provider}")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Overview:"));
    lines.push(Line::from(item.overview.as_str()));
    lines.push(Line::from(""));
    lines.push(Line::from(format!("ID: {}", item.id)));

    if let Some(poster) = &item.poster_url {
        lines.push(Line::from(format!("Poster: {}", poster)));
    }
    if let Some(stream) = &item.stream_url {
        lines.push(Line::from(format!("Stream: {}", stream)));
    }

    lines
}

// ── Overlays ──────────────────────────────────────────────────────────

/// Draw a transient toast near the bottom of the screen.
fn toast_overlay(f: &mut Frame, app: &App) {
    let Some((message, _)) = &app.toast else {
        return;
    };

    let area = f.area();
    let max_width = area.width.saturating_sub(2).max(12);
    let width = (message.chars().count() as u16 + 4).min(max_width);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(5),
        width,
        height: 3.min(area.height),
    };

    let toast = Paragraph::new(Line::from(Span::styled(
        message.as_str(),
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );

    f.render_widget(Clear, rect);
    f.render_widget(toast, rect);
}

/// Draw the full-screen keyboard-shortcut help overlay.
fn help_overlay(f: &mut Frame) {
    let area = f.area();
    let rect = centered_rect(64, 30, area);

    let lines = vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        section("Browse"),
        shortcut("↑/↓, j/k", "navigate"),
        shortcut("/", "search"),
        shortcut("Enter", "play selected"),
        shortcut("b", "toggle bookmark"),
        shortcut("l", "library"),
        shortcut("s", "settings"),
        shortcut("p", "providers"),
        shortcut("?", "this help"),
        shortcut("q / Esc", "quit"),
        section("Playback"),
        shortcut("Space", "pause / resume"),
        shortcut("←/→", "seek ±10s"),
        shortcut("↑/↓", "volume"),
        shortcut("q / Esc", "back to browse"),
        section("Settings"),
        shortcut("↑/↓", "select field"),
        shortcut("←/→", "cycle source / volume"),
        shortcut("Enter / Esc", "edit/save · back"),
        section("Providers"),
        shortcut("a", "add provider"),
        shortcut("e / d", "edit / delete"),
        shortcut("Enter / Space", "enable / disable"),
        shortcut("r", "reload catalog"),
        Line::from(""),
        Line::from(Span::styled(
            "Press ? or Esc to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Help")
            .border_style(Style::default().fg(Color::Magenta)),
    );

    f.render_widget(Clear, rect);
    f.render_widget(help, rect);
}

fn section(title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn shortcut(key: &'static str, desc: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {key:<10}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(desc),
    ])
}

/// A rectangle of the given size, centered within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    /// Render one frame off-screen and flatten it into a plain string.
    fn rendered_text(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|f| ui(f, app)).expect("draw frame");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn settings_screen_renders_all_fields() {
        let mut app = App::new();
        app.mode = AppMode::Settings;

        let text = rendered_text(&mut app, 100, 30);
        assert!(text.contains("Default volume"));
        assert!(text.contains("Catalog URL"));
        assert!(text.contains("Real-Debrid API key"));
    }

    #[test]
    fn help_overlay_toggles_with_question_mark_and_esc() {
        let mut app = App::new();

        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.show_help);

        let text = rendered_text(&mut app, 100, 30);
        assert!(text.contains("Keyboard Shortcuts"));
        assert!(text.contains("pause / resume"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.show_help);
        // Esc closed the overlay instead of quitting the app.
        assert_eq!(app.mode, AppMode::Browsing);
    }

    #[test]
    fn toast_overlay_renders_message() {
        let mut app = App::new();
        app.show_toast("Hello toast");

        let text = rendered_text(&mut app, 100, 30);
        assert!(text.contains("Hello toast"));
    }

    #[test]
    fn search_accepts_q_and_question_mark_as_text() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::Searching);

        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

        assert_eq!(app.search_input, "q?");
        assert_eq!(app.mode, AppMode::Searching);
        assert!(!app.show_help);
    }

    #[test]
    fn settings_edit_can_be_started_and_cancelled() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::Settings);

        // Move to the catalog URL field and start editing it.
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.settings_editing.is_some());

        // Esc cancels the edit and stays on the Settings screen.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.settings_editing.is_none());
        assert_eq!(app.mode, AppMode::Settings);
    }

    #[test]
    fn providers_screen_renders_supported_formats() {
        let mut app = App::new();
        app.storage.providers.clear();
        app.mode = AppMode::Providers;

        let text = rendered_text(&mut app, 100, 30);
        assert!(text.contains("Providers"));
        assert!(text.contains("press a to add one"));
        assert!(text.contains("Stremio"));
    }

    #[test]
    fn providers_screen_lists_configured_providers() {
        let mut app = App::new();
        app.storage.providers = vec![crate::storage::ProviderConfig::new(
            "Local Catalog",
            "https://example.com/catalog.json",
        )];

        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::Providers);

        let text = rendered_text(&mut app, 100, 30);
        assert!(text.contains("Local Catalog"));
        assert!(text.contains("https://example.com/catalog.json"));
        assert!(text.contains("enabled"));
    }

    #[test]
    fn provider_editor_overlay_renders_fields() {
        let mut app = App::new();
        app.mode = AppMode::Providers;
        app.provider_editing = Some(crate::app::ProviderEdit {
            index: None,
            name: "Test Addon".to_string(),
            url: "https://addon.example".to_string(),
            field: ProviderField::Url,
        });

        let text = rendered_text(&mut app, 100, 30);
        assert!(text.contains("Add Provider"));
        assert!(text.contains("Test Addon"));
        assert!(text.contains("https://addon.example"));
        assert!(text.contains("Enter save"));
    }
}

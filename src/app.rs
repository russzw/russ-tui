use anyhow::Context;
use crossterm::event::KeyCode;
use ratatui::widgets::ListState;
use tokio::task::JoinHandle;

use crate::debrid::resolver_for;
use crate::media::{Catalog, MediaItem};
use crate::playback::{MpvPlayer, PlaybackStatus, PlayerCommand};
use crate::provider;
use crate::storage::{CatalogSource, PersistedState, ProviderConfig};

/// Current screen mode of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Browsing,
    Searching,
    Playing,
    Library,
    Settings,
    Providers,
    Quit,
}

/// Which settings field is being edited on the Settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingField {
    DefaultVolume,
    CatalogUrl,
    DebridApiKey,
}

/// Which field of the provider editor has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderField {
    Name,
    Url,
}

/// An in-progress add/edit of a provider.
#[derive(Debug, Clone)]
pub struct ProviderEdit {
    /// `None` when adding a new provider, otherwise the index being edited.
    pub index: Option<usize>,
    pub name: String,
    pub url: String,
    pub field: ProviderField,
}

/// Top-level application state.
pub struct App {
    pub mode: AppMode,
    pub catalog: Catalog,
    pub selected: usize,
    pub list_state: ListState,
    pub search_input: String,
    pub playback: Option<MpvPlayer>,
    pub playback_handle: Option<JoinHandle<()>>,
    pub playback_status: PlaybackStatus,
    pub volume: i32,
    pub storage: PersistedState,
    /// Selected row in the Library screen (over the watch history).
    pub library_selected: usize,
    /// Selected row in the Settings screen.
    pub settings_selected: usize,
    /// Active inline text edit on the Settings screen: (field, current buffer).
    pub settings_editing: Option<(SettingField, String)>,
    /// Selected row in the Providers screen.
    pub providers_selected: usize,
    /// Active provider add/edit overlay.
    pub provider_editing: Option<ProviderEdit>,
    /// Toast notification: (message, shown_at).
    pub toast: Option<(String, std::time::Instant)>,
    /// Toggle for the help overlay.
    pub show_help: bool,
    /// Item awaiting playback start (processed in the async main loop).
    pub pending_play: Option<MediaItem>,
    /// The item currently being played (used for history and the playback header).
    pub now_playing: Option<MediaItem>,
    /// Set when the catalog URL changes and the catalog should be re-fetched.
    pub pending_catalog_reload: bool,
}

impl App {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        // Load persisted state (bookmarks, history, volume) from disk.
        let storage = PersistedState::load();
        let volume = storage.volume;

        Self {
            mode: AppMode::Browsing,
            catalog: Catalog::new(),
            selected: 0,
            list_state,
            search_input: String::new(),
            playback: None,
            playback_handle: None,
            playback_status: PlaybackStatus::default(),
            volume,
            storage,
            library_selected: 0,
            settings_selected: 0,
            settings_editing: None,
            providers_selected: 0,
            provider_editing: None,
            toast: None,
            show_help: false,
            pending_play: None,
            now_playing: None,
            pending_catalog_reload: false,
        }
    }

    /// The currently highlighted media item (after filtering), if any.
    pub fn selected_item(&self) -> Option<&MediaItem> {
        let filtered: Vec<&MediaItem> = self.catalog.filtered();
        filtered.get(self.selected).copied()
    }

    /// Number of items currently visible after filtering.
    pub fn visible_len(&self) -> usize {
        self.catalog.filtered().len()
    }

    pub fn next(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1).min(len - 1);
        self.list_state.select(Some(self.selected));
    }

    pub fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.list_state.select(Some(self.selected));
    }

    /// Clamp selection into range after the filtered list changes.
    pub fn clamp_selection(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            self.selected = 0;
            self.list_state.select(None);
        } else if self.selected >= len {
            self.selected = len - 1;
            self.list_state.select(Some(self.selected));
        } else {
            self.list_state.select(Some(self.selected));
        }
    }

    /// Show a transient toast message (auto-cleared after 4 s by the main loop).
    pub fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
    }

    /// Clear the toast if it has expired.
    pub fn tick_toast(&mut self) {
        if let Some((_, shown_at)) = &self.toast {
            if shown_at.elapsed() > std::time::Duration::from_secs(4) {
                self.toast = None;
            }
        }
    }

    /// Cycle the catalog source and schedule a reload in the main loop.
    fn cycle_catalog_source(&mut self, forward: bool) {
        self.storage.catalog_source = if forward {
            self.storage.catalog_source.next()
        } else {
            self.storage.catalog_source.previous()
        };
        let _ = self.storage.save();
        self.pending_catalog_reload = true;
        self.show_toast(format!(
            "Catalog source: {}",
            self.storage.catalog_source.label()
        ));
    }

    /// Start playback of the currently selected item.
    pub fn play_selected(&mut self) -> anyhow::Result<()> {
        let item = match self.selected_item() {
            Some(item) => item.clone(),
            None => return Ok(()),
        };
        // Defer actual playback to the async main loop (debrid resolution may block).
        self.pending_play = Some(item);
        Ok(())
    }

    /// Look up a catalog item by id and start playback (used by the Library screen).
    pub fn play_by_id(&mut self, media_id: &str) -> anyhow::Result<()> {
        let item = self
            .catalog
            .items
            .iter()
            .find(|i| i.id == media_id)
            .cloned();
        match item {
            Some(item) => {
                self.pending_play = Some(item);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Start playback of the given item (resume position comes from history).
    /// Runs in the async main loop so magnet/debrid resolution can block safely.
    pub async fn play_item(&mut self, item: MediaItem) -> anyhow::Result<()> {
        let mut url = match item.stream_url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => url.to_string(),
            _ => {
                self.show_toast("This title has no stream URL");
                return Ok(());
            }
        };

        // Resolve Internet Archive identifiers into a direct download URL.
        if let Some(identifier) = url.strip_prefix("archive:").map(str::to_string) {
            self.show_toast("Resolving Internet Archive stream…");
            match crate::archive::resolve_stream(&identifier).await {
                Ok(direct) => url = direct,
                Err(err) => {
                    self.show_toast(format!("Internet Archive failed: {err:#}"));
                    return Ok(());
                }
            }
        }

        // Resolve Stremio addon stream markers into a direct URL or magnet.
        if url.starts_with("stremio:") {
            self.show_toast("Resolving provider stream…");
            match provider::resolve_stremio_stream(&url).await {
                Ok(direct) => url = direct,
                Err(err) => {
                    self.show_toast(format!("Provider stream failed: {err:#}"));
                    return Ok(());
                }
            }
        }

        // Magnet and file-host links need Real-Debrid; fail with a clear hint
        // instead of handing mpv a URL it cannot open.
        if (url.starts_with("magnet:") || url.starts_with("debrid:"))
            && self.storage.debrid_api_key.is_none()
        {
            self.show_toast("This stream needs Real-Debrid — set an API key in Settings");
            return Ok(());
        }

        // Resolve magnet / file-host URLs via Real-Debrid when an API key is set.
        if let Some(resolver) = resolver_for(&url, self.storage.debrid_api_key.as_deref()) {
            self.show_toast("Resolving via Real-Debrid…");
            match resolver.resolve(&url).await {
                Ok(direct) => url = direct,
                Err(e) => {
                    self.show_toast(format!("Debrid failed: {e:#}"));
                    return Ok(());
                }
            }
        }

        // Resume from the last watched position, if any.
        let start_at = self.storage.resume_position(&item.id);

        if self.playback.is_none() {
            let (player, handle) = MpvPlayer::new()?;
            self.playback = Some(player);
            self.playback_handle = Some(handle);
        }

        self.playback
            .as_mut()
            .context("playback not initialized")?
            .send(PlayerCommand::Play { url, start_at })?;

        // Record the start of a watch session (updated with the final position on exit).
        self.storage
            .record_history(&item.id, &item.title, start_at.unwrap_or(0.0), None);
        let _ = self.storage.save();

        self.now_playing = Some(item);
        self.toast = None;
        self.mode = AppMode::Playing;
        Ok(())
    }

    /// Schedule a catalog reload when provider settings change while providers
    /// are the active catalog source.
    fn refresh_providers(&mut self) {
        if self.storage.catalog_source == CatalogSource::Providers {
            self.pending_catalog_reload = true;
        }
    }

    /// Open the provider editor for an existing provider or a new one.
    fn begin_provider_edit(&mut self, index: Option<usize>) {
        match index.and_then(|idx| self.storage.providers.get(idx)) {
            Some(provider) => {
                self.provider_editing = Some(ProviderEdit {
                    index,
                    name: provider.name.clone(),
                    url: provider.url.clone(),
                    field: ProviderField::Name,
                });
            }
            None => {
                self.provider_editing = Some(ProviderEdit {
                    index: None,
                    name: String::new(),
                    url: String::new(),
                    field: ProviderField::Name,
                });
            }
        }
    }

    /// Validate and save the provider currently being edited.
    fn commit_provider_edit(&mut self) {
        let Some(edit) = self.provider_editing.as_ref() else {
            return;
        };

        let name = edit.name.trim().to_string();
        let url = edit.url.trim().to_string();

        if url.is_empty() {
            self.show_toast("Provider URL is required");
            return;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.show_toast("Provider URL must start with http:// or https://");
            return;
        }

        let index = edit.index;
        self.provider_editing = None;

        let provider = ProviderConfig::new(name, url);

        match index {
            Some(idx) if idx < self.storage.providers.len() => {
                let enabled = self.storage.providers[idx].enabled;
                self.storage.providers[idx] = ProviderConfig {
                    enabled,
                    ..provider
                };
                self.providers_selected = idx;
                self.show_toast("Provider updated");
            }
            _ => {
                self.storage.providers.push(provider);
                self.providers_selected = self.storage.providers.len().saturating_sub(1);
                self.show_toast("Provider added");
            }
        }

        let _ = self.storage.save();
        self.refresh_providers();
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

        // Help overlay: `?` toggles it (except while typing), Esc closes it first.
        let typing = self.mode == AppMode::Searching
            || self.settings_editing.is_some()
            || self.provider_editing.is_some();
        if self.show_help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.show_help = false;
                return;
            }
        } else if !typing && key.code == KeyCode::Char('?') {
            self.show_help = true;
            return;
        }

        match (self.mode, key.code) {
            // Quit playback / return to browsing
            (AppMode::Playing, KeyCode::Char('q')) | (AppMode::Playing, KeyCode::Esc) => {
                // Save the final position so playback can be resumed later.
                let pos = self.playback_status.time_pos.unwrap_or(0.0);
                let dur = self.playback_status.duration;
                let current = self.now_playing.take().map(|item| (item.id, item.title));
                if let Some((id, title)) = current {
                    self.storage.record_history(&id, &title, pos, dur);
                }
                let _ = self.storage.save();

                let _ = self
                    .playback
                    .as_mut()
                    .and_then(|p| p.send(PlayerCommand::Quit).ok());
                self.mode = AppMode::Browsing;
            }

            // Quit app
            (AppMode::Browsing, KeyCode::Char('q')) | (AppMode::Browsing, KeyCode::Esc) => {
                self.mode = AppMode::Quit;
            }

            // Cancel search. While typing, 'q' is search input, not quit.
            (AppMode::Searching, KeyCode::Esc) => {
                self.mode = AppMode::Browsing;
                self.search_input.clear();
                self.catalog.clear_search();
                self.clamp_selection();
            }

            // Navigation (browsing only)
            (AppMode::Browsing, KeyCode::Down) | (AppMode::Browsing, KeyCode::Char('j')) => {
                self.next();
            }
            (AppMode::Browsing, KeyCode::Up) | (AppMode::Browsing, KeyCode::Char('k')) => {
                self.previous();
            }

            // Enter search mode
            (AppMode::Browsing, KeyCode::Char('/')) => {
                self.mode = AppMode::Searching;
                self.search_input.clear();
            }

            // Play selected item
            (AppMode::Browsing, KeyCode::Enter) => {
                let _ = self.play_selected();
            }

            // Toggle bookmark on the selected item
            (AppMode::Browsing, KeyCode::Char('b')) => {
                let selected = self
                    .selected_item()
                    .map(|item| (item.id.clone(), item.title.clone()));
                if let Some((id, title)) = selected {
                    let now_bookmarked = self.storage.toggle_bookmark(&id);
                    let _ = self.storage.save();
                    self.show_toast(if now_bookmarked {
                        format!("♥ Bookmarked {title}")
                    } else {
                        format!("Removed bookmark: {title}")
                    });
                }
            }

            // Open the Library screen
            (AppMode::Browsing, KeyCode::Char('l')) => {
                self.mode = AppMode::Library;
                self.library_selected = 0;
            }

            // Library screen: exit back to browsing / quit
            (AppMode::Library, KeyCode::Esc) | (AppMode::Library, KeyCode::Char('l')) => {
                self.mode = AppMode::Browsing;
            }
            (AppMode::Library, KeyCode::Char('q')) => {
                self.mode = AppMode::Quit;
            }

            // Library screen: navigate watch history
            (AppMode::Library, KeyCode::Down) | (AppMode::Library, KeyCode::Char('j')) => {
                let len = self.storage.history.len();
                if len > 0 {
                    self.library_selected = (self.library_selected + 1).min(len - 1);
                }
            }
            (AppMode::Library, KeyCode::Up) | (AppMode::Library, KeyCode::Char('k')) => {
                self.library_selected = self.library_selected.saturating_sub(1);
            }

            // Library screen: resume the selected history entry
            (AppMode::Library, KeyCode::Enter) => {
                let entry = self.storage.history.get(self.library_selected).cloned();
                if let Some(entry) = entry {
                    let _ = self.play_by_id(&entry.media_id);
                }
            }

            // Open the Settings screen
            (AppMode::Browsing, KeyCode::Char('s')) => {
                self.mode = AppMode::Settings;
                self.settings_selected = 0;
                self.settings_editing = None;
            }

            // Open the Providers screen
            (AppMode::Browsing, KeyCode::Char('p')) => {
                self.mode = AppMode::Providers;
                self.providers_selected = 0;
                self.provider_editing = None;
            }

            // Settings screen: exit back to browsing / quit
            (AppMode::Settings, KeyCode::Esc) if self.settings_editing.is_none() => {
                self.mode = AppMode::Browsing;
            }
            // Settings screen: cancel an in-progress edit
            (AppMode::Settings, KeyCode::Esc) => {
                self.settings_editing = None;
                self.show_toast("Edit cancelled");
            }
            (AppMode::Settings, KeyCode::Char('q')) if self.settings_editing.is_none() => {
                self.mode = AppMode::Quit;
            }

            // Settings screen: navigate between fields
            (AppMode::Settings, KeyCode::Down) | (AppMode::Settings, KeyCode::Char('j'))
                if self.settings_editing.is_none() =>
            {
                self.settings_selected = (self.settings_selected + 1) % 4;
            }
            (AppMode::Settings, KeyCode::Up) | (AppMode::Settings, KeyCode::Char('k'))
                if self.settings_editing.is_none() =>
            {
                self.settings_selected = (self.settings_selected + 3) % 4;
            }

            // Settings screen: left/right cycles the catalog source or adjusts volume
            (AppMode::Settings, KeyCode::Right) if self.settings_editing.is_none() => {
                match self.settings_selected {
                    0 => self.cycle_catalog_source(true),
                    2 => {
                        self.volume = (self.volume + 5).min(100);
                        self.storage.volume = self.volume;
                        let _ = self.storage.save();
                    }
                    _ => {}
                }
            }
            (AppMode::Settings, KeyCode::Left) if self.settings_editing.is_none() => {
                match self.settings_selected {
                    0 => self.cycle_catalog_source(false),
                    2 => {
                        self.volume = (self.volume - 5).max(0);
                        self.storage.volume = self.volume;
                        let _ = self.storage.save();
                    }
                    _ => {}
                }
            }

            // Settings screen: Enter on the source row cycles it
            (AppMode::Settings, KeyCode::Enter)
                if self.settings_editing.is_none() && self.settings_selected == 0 =>
            {
                self.cycle_catalog_source(true);
            }

            // Settings screen: start editing the selected text field
            (AppMode::Settings, KeyCode::Enter) if self.settings_editing.is_none() => {
                let field = match self.settings_selected {
                    1 => SettingField::CatalogUrl,
                    2 => SettingField::DefaultVolume,
                    _ => SettingField::DebridApiKey,
                };
                let current = match field {
                    // Start blank so the user replaces the value rather than appending to it.
                    SettingField::DefaultVolume => String::new(),
                    SettingField::CatalogUrl => {
                        self.storage.catalog_url.clone().unwrap_or_default()
                    }
                    SettingField::DebridApiKey => {
                        self.storage.debrid_api_key.clone().unwrap_or_default()
                    }
                };
                self.settings_editing = Some((field, current));
            }

            // Settings screen: commit an edit
            (AppMode::Settings, KeyCode::Enter) => {
                if let Some((field, value)) = self.settings_editing.take() {
                    match field {
                        SettingField::DefaultVolume => {
                            let trimmed = value.trim();
                            if trimmed.is_empty() {
                                // Empty input keeps the current volume.
                            } else if let Ok(v) = trimmed.parse::<i32>() {
                                self.volume = v.clamp(0, 100);
                                self.storage.volume = self.volume;
                                self.show_toast(format!("Default volume set to {}%", self.volume));
                            } else {
                                self.show_toast("Volume must be a number between 0 and 100");
                            }
                        }
                        SettingField::CatalogUrl => {
                            let trimmed = value.trim().to_string();
                            self.storage.catalog_url = if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed)
                            };
                            // Re-fetch the catalog with the new source.
                            self.pending_catalog_reload = true;
                            self.show_toast("Catalog source saved — reloading…");
                        }
                        SettingField::DebridApiKey => {
                            let trimmed = value.trim().to_string();
                            self.storage.debrid_api_key = if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed)
                            };
                            self.show_toast(if self.storage.debrid_api_key.is_some() {
                                "Real-Debrid API key saved"
                            } else {
                                "Real-Debrid API key cleared"
                            });
                        }
                    }
                    let _ = self.storage.save();
                }
            }

            // Settings screen: text-edit input
            (AppMode::Settings, KeyCode::Char(c)) if self.settings_editing.is_some() => {
                if let Some((_, buf)) = self.settings_editing.as_mut() {
                    buf.push(c);
                }
            }
            (AppMode::Settings, KeyCode::Backspace) if self.settings_editing.is_some() => {
                if let Some((_, buf)) = self.settings_editing.as_mut() {
                    buf.pop();
                }
            }

            // Providers screen: exit back to browsing / quit
            (AppMode::Providers, KeyCode::Esc) | (AppMode::Providers, KeyCode::Char('p'))
                if self.provider_editing.is_none() =>
            {
                self.mode = AppMode::Browsing;
            }
            (AppMode::Providers, KeyCode::Char('q')) if self.provider_editing.is_none() => {
                self.mode = AppMode::Quit;
            }

            // Providers screen: cancel the editor
            (AppMode::Providers, KeyCode::Esc) if self.provider_editing.is_some() => {
                self.provider_editing = None;
                self.show_toast("Provider edit cancelled");
            }

            // Providers screen: save the editor
            (AppMode::Providers, KeyCode::Enter) if self.provider_editing.is_some() => {
                self.commit_provider_edit();
            }

            // Providers screen: switch between the name and URL fields
            (AppMode::Providers, KeyCode::Tab | KeyCode::Down | KeyCode::Up)
                if self.provider_editing.is_some() =>
            {
                if let Some(edit) = self.provider_editing.as_mut() {
                    edit.field = match edit.field {
                        ProviderField::Name => ProviderField::Url,
                        ProviderField::Url => ProviderField::Name,
                    };
                }
            }

            // Providers screen: editor text input
            (AppMode::Providers, KeyCode::Backspace) if self.provider_editing.is_some() => {
                if let Some(edit) = self.provider_editing.as_mut() {
                    match edit.field {
                        ProviderField::Name => {
                            edit.name.pop();
                        }
                        ProviderField::Url => {
                            edit.url.pop();
                        }
                    }
                }
            }
            (AppMode::Providers, KeyCode::Char(c)) if self.provider_editing.is_some() => {
                if let Some(edit) = self.provider_editing.as_mut() {
                    match edit.field {
                        ProviderField::Name => edit.name.push(c),
                        ProviderField::Url => edit.url.push(c),
                    }
                }
            }

            // Providers screen: navigate the list
            (AppMode::Providers, KeyCode::Down) | (AppMode::Providers, KeyCode::Char('j')) => {
                let len = self.storage.providers.len();
                if len > 0 {
                    self.providers_selected = (self.providers_selected + 1).min(len - 1);
                }
            }
            (AppMode::Providers, KeyCode::Up) | (AppMode::Providers, KeyCode::Char('k')) => {
                self.providers_selected = self.providers_selected.saturating_sub(1);
            }

            // Providers screen: toggle the selected provider
            (AppMode::Providers, KeyCode::Enter) | (AppMode::Providers, KeyCode::Char(' ')) => {
                if let Some(provider) = self.storage.providers.get_mut(self.providers_selected) {
                    provider.enabled = !provider.enabled;
                    let label = provider::display_name(provider);
                    let enabled = provider.enabled;
                    let _ = self.storage.save();
                    self.show_toast(format!(
                        "{} {label}",
                        if enabled { "Enabled" } else { "Disabled" }
                    ));
                    self.refresh_providers();
                }
            }

            // Providers screen: add, edit, delete, reload
            (AppMode::Providers, KeyCode::Char('a')) => {
                self.begin_provider_edit(None);
            }
            (AppMode::Providers, KeyCode::Char('e')) => {
                if self.storage.providers.is_empty() {
                    self.show_toast("No providers to edit — press a to add one");
                } else {
                    self.begin_provider_edit(Some(self.providers_selected));
                }
            }
            (AppMode::Providers, KeyCode::Char('d')) => {
                if self.providers_selected < self.storage.providers.len() {
                    let removed = self.storage.providers.remove(self.providers_selected);
                    let label = provider::display_name(&removed);
                    if self.providers_selected >= self.storage.providers.len() {
                        self.providers_selected = self.storage.providers.len().saturating_sub(1);
                    }
                    let _ = self.storage.save();
                    self.show_toast(format!("Removed provider: {label}"));
                    self.refresh_providers();
                }
            }
            (AppMode::Providers, KeyCode::Char('r')) => {
                self.pending_catalog_reload = true;
                self.show_toast("Reloading providers…");
            }

            // Search input
            (AppMode::Searching, KeyCode::Enter) => {
                self.catalog.set_search(self.search_input.clone());
                self.selected = 0;
                self.clamp_selection();
                self.mode = AppMode::Browsing;
            }
            (AppMode::Searching, KeyCode::Backspace) => {
                self.search_input.pop();
            }
            (AppMode::Searching, KeyCode::Char(c)) => {
                self.search_input.push(c);
            }

            // Playback controls
            (AppMode::Playing, KeyCode::Char(' ')) => {
                let _ = self
                    .playback
                    .as_mut()
                    .and_then(|p| p.send(PlayerCommand::TogglePause).ok());
            }
            (AppMode::Playing, KeyCode::Left) => {
                let _ = self
                    .playback
                    .as_mut()
                    .and_then(|p| p.send(PlayerCommand::Seek { seconds: -10.0 }).ok());
            }
            (AppMode::Playing, KeyCode::Right) => {
                let _ = self
                    .playback
                    .as_mut()
                    .and_then(|p| p.send(PlayerCommand::Seek { seconds: 10.0 }).ok());
            }
            (AppMode::Playing, KeyCode::Up) => {
                self.volume = (self.volume + 5).min(100);
                self.storage.volume = self.volume;
                let _ = self.playback.as_mut().and_then(|p| {
                    p.send(PlayerCommand::Volume {
                        percent: self.volume as f64,
                    })
                    .ok()
                });
            }
            (AppMode::Playing, KeyCode::Down) => {
                self.volume = (self.volume - 5).max(0);
                self.storage.volume = self.volume;
                let _ = self.playback.as_mut().and_then(|p| {
                    p.send(PlayerCommand::Volume {
                        percent: self.volume as f64,
                    })
                    .ok()
                });
            }

            _ => {}
        }
    }

    /// Refresh the cached playback status from the player.
    #[allow(dead_code)]
    pub async fn refresh_playback_status(&mut self) {
        if let Some(player) = &self.playback {
            self.playback_status = player.status_sync().unwrap_or_default();
        }
    }
}

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::Duration;

mod app;
mod archive;
mod debrid;
mod media;
mod playback;
mod provider;
mod storage;
mod ui;

use app::App;
use app::AppMode;
use storage::CatalogSource;
use ui::ui;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(err) = run().await {
        let _ = disable_raw_mode();
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run() -> Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    enable_raw_mode()?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut app = App::new();

    // Show a loading state before the (possibly network-bound) catalog fetch.
    app.show_toast("Loading catalog…");
    terminal.draw(|f| ui(f, &mut app))?;
    load_catalog(&mut app).await;
    app.clamp_selection();

    let tick_rate = Duration::from_millis(250);

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }

        // Process deferred playback (debrid resolution may do network I/O).
        if let Some(item) = app.pending_play.take() {
            // Draw once so the user sees the pending state before any network work.
            app.show_toast(format!("Starting {}…", item.title));
            terminal.draw(|f| ui(f, &mut app))?;
            if let Err(err) = app.play_item(item).await {
                app.show_toast(format!("Playback failed: {err:#}"));
            }
        }

        // Re-fetch the catalog after the settings screen changes its source.
        if app.pending_catalog_reload {
            app.pending_catalog_reload = false;
            // Show the new source/loading toast before the network work.
            terminal.draw(|f| ui(f, &mut app))?;
            load_catalog(&mut app).await;
            app.clamp_selection();
        }

        // Expire transient toast messages.
        app.tick_toast();

        // Refresh playback status when playing.
        if app.mode == AppMode::Playing {
            if let Some(status) = app.playback.as_ref().and_then(|p| p.status_sync()) {
                app.playback_status = status;
            }
        }

        if app.mode == AppMode::Quit {
            break;
        }
    }

    // Shut down the mpv playback task.
    if let Some(handle) = app.playback_handle.take() {
        handle.abort();
    }

    // Persist state (history, bookmarks, volume) for the next session.
    let _ = app.storage.save();

    terminal.show_cursor()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    disable_raw_mode()?;

    Ok(())
}

/// Load the catalog from the configured source, falling back to the bundled file.
async fn load_catalog(app: &mut App) {
    let mut error: Option<String> = None;

    match app.storage.catalog_source {
        CatalogSource::InternetArchive => match archive::fetch_popular(60).await {
            Ok(items) => {
                let count = items.len();
                app.catalog.items = items;
                reset_catalog_view(app);
                app.show_toast(format!("Loaded {count} films from the Internet Archive"));
                return;
            }
            Err(err) => error = Some(format!("Internet Archive failed: {err:#}")),
        },
        CatalogSource::RemoteUrl => match app.storage.catalog_url.clone() {
            Some(url) => match app.catalog.fetch_from_url(&url).await {
                Ok(()) => {
                    let count = app.catalog.items.len();
                    reset_catalog_view(app);
                    app.show_toast(format!("Loaded {count} titles from {url}"));
                    return;
                }
                Err(err) => error = Some(format!("Remote catalog failed: {err:#}")),
            },
            None => error = Some("No catalog URL configured; using bundled data".to_string()),
        },
        CatalogSource::Providers => {
            let outcome = provider::fetch_all(&app.storage.providers).await;
            let count = outcome.items.len();

            if count > 0 {
                app.catalog.items = outcome.items;
                reset_catalog_view(app);

                let mut message = format!(
                    "Loaded {count} titles from {} provider(s)",
                    outcome.provider_count
                );
                if !outcome.errors.is_empty() {
                    message.push_str(&format!(" · {} failed", outcome.errors.len()));
                }
                app.show_toast(message);
                return;
            }

            error = Some(if outcome.provider_count == 0 {
                "No providers enabled — press p to add one".to_string()
            } else if outcome.errors.is_empty() {
                "Providers returned no items".to_string()
            } else {
                format!("Provider errors: {}", outcome.errors.join("; "))
            });
        }
        CatalogSource::Bundled => {}
    }

    // Fall back to the bundled sample catalog.
    match app.catalog.load_from_file("data/movies.json").await {
        Ok(()) => {
            reset_catalog_view(app);
            if let Some(message) = error {
                app.show_toast(message);
            } else {
                app.toast = None;
            }
        }
        Err(err) => app.show_toast(format!("Failed to load bundled catalog: {err:#}")),
    }
}

/// Reset search and selection after the catalog contents change.
fn reset_catalog_view(app: &mut App) {
    app.catalog.clear_search();
    app.search_input.clear();
    app.selected = 0;
}

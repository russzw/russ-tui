# russ-tui Guide

This is the complete manual for **russ-tui**, a MovieBox-style media browser
and streamer for the terminal. It covers installation, every keyboard control,
catalog sources, adding your own streams and providers, Real-Debrid, state
storage, and troubleshooting.

- [Install and run](#install-and-run)
- [First run](#first-run)
- [Screens and controls](#screens-and-controls)
- [Catalog sources](#catalog-sources)
- [Adding your own catalog and streams](#adding-your-own-catalog-and-streams)
- [Adding providers](#adding-providers)
- [Real-Debrid](#real-debrid)
- [Playback and mpv](#playback-and-mpv)
- [State and persistence](#state-and-persistence)
- [Updating and uninstalling](#updating-and-uninstalling)
- [Troubleshooting](#troubleshooting)
- [Privacy, content, and license](#privacy-content-and-license)

---

## Install and run

### Requirements

| Component | Windows | Linux / macOS |
| --- | --- | --- |
| Rust | Stable toolchain (edition 2024; Rust 1.85+) | Stable toolchain (1.85+) |
| C++ build tools | Visual Studio C++ Build Tools when using the MSVC Rust host | A normal C/C++ toolchain if your Rust distribution needs one |
| mpv | `mpv.exe` on `PATH`, a common install location, or `RUSS_TUI_MPV` | `mpv` on `PATH` or `RUSS_TUI_MPV` |
| Network | Needed for remote catalogs, providers, Internet Archive, Real-Debrid, and streams | Same |

The Windows installer handles Rust, the MSVC C++ workload, and mpv for you.

### Windows installer

From a cloned repository:

```powershell
git clone https://github.com/russzw/russ-tui.git
cd russ-tui
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Launch
```

Install without launching immediately:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Install directly from the web without cloning:

```powershell
irm https://raw.githubusercontent.com/russzw/russ-tui/main/install.ps1 | iex
```

That installs the app and prints the launcher path. To install **and launch**
in one command, run the downloaded script with `-Launch`:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/russzw/russ-tui/main/install.ps1))) -Launch
```

If PowerShell blocks the script, use `powershell -ExecutionPolicy Bypass -File`
as shown above. You do not need to permanently change your execution policy.

### What the installer does

1. Finds a local source checkout, or downloads the `main` branch archive from
   GitHub when the script is run through `irm | iex`.
2. Checks for Rust. If missing, it tries winget (`Rustlang.Rustup`), then the
   official `rustup-init` download.
3. Checks whether the active Rust host is MSVC. If so, it verifies the Visual
   Studio C++ workload and installs Visual Studio 2022 Build Tools when needed.
4. Checks for mpv. If missing, it installs `shinchiro.mpv` with winget. A
   working `RUSS_TUI_MPV` value or an existing mpv installation is reused.
5. Runs `cargo build --release --locked`.
6. Installs the binary, bundled catalog, README, guide, and license to
   `%LOCALAPPDATA%\russ-tui`.
7. Creates `russ-tui.cmd` and `russ-tui.ps1` launchers in that directory.
8. Sets the user environment variable `RUSS_TUI_MPV` when an mpv executable is
   found.
9. Adds the install directory to your **user** `PATH` unless `-NoPath` is used.
10. Optionally launches the app with `-Launch`.

The installer never writes to `%APPDATA%\russ-tui\state.json`, so reinstalling
does not erase bookmarks, history, providers, or settings.

### Installer options

| Option | Meaning |
| --- | --- |
| `-InstallDir <path>` | Install somewhere other than `%LOCALAPPDATA%\russ-tui`. |
| `-SkipRust` | Do not install Rust; fail if `cargo` is not available. |
| `-SkipMpv` | Do not install, detect, or configure mpv. |
| `-SkipBuildTools` | Do not install Visual Studio C++ Build Tools. The build may fail if they are missing. |
| `-NoPath` | Do not modify the user `PATH`. |
| `-Launch` | Start russ-tui after installation. |
| `-SourceDir <path>` | Build from a specific source checkout. |
| `-MpvPath <path>` | Use a specific `mpv.exe` and skip mpv discovery/installation. |

`-SkipMpv` and `-MpvPath` are mutually exclusive.

Example test installation without touching mpv or `PATH`:

```powershell
.\install.ps1 -InstallDir "$env:TEMP\russ-tui-test" -SkipMpv -NoPath
```

### Run from source

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File .\run.ps1
```

The script builds the release binary and launches it from the repository root so
`data/movies.json` resolves correctly. Use `-NoBuild` to launch an existing
release binary.

Any platform with Cargo and mpv:

```sh
cargo run --release
```

Keep the repository root as the working directory when using the bundled
catalog. The installed Windows build stores its copy beside the executable and
its launcher changes into that directory first. The app also checks beside the
executable when `data/movies.json` is not found in the working directory.

### Linux and macOS

Install Rust and mpv with your package manager, then run `cargo run --release`
from the repository root. The app checks `PATH`, `/usr/bin/mpv`,
`/usr/local/bin/mpv`, and `/opt/homebrew/bin/mpv`. You can always override the
location:

```sh
RUSS_TUI_MPV=/path/to/mpv cargo run --release
```

---

## First run

1. The app starts on **Browse** with the bundled sample catalog. Five of the
   sample titles have public sample video clips; the rest intentionally have no
   stream.
2. Press `s` to open Settings. Choose a catalog source, or configure a remote
   URL or Real-Debrid key.
3. Press `p` to manage providers. The app ships with **no provider URLs**.
4. Press `?` at any time for the in-app shortcut overlay.
5. Select a title and press `Enter`. mpv opens in its own window; the terminal
   remains the remote control and status display.

---

## Screens and controls

The terminal UI has five main screens plus search mode and overlays:

| Screen | Opened with | Purpose |
| --- | --- | --- |
| Browse | start screen | Search and browse the active catalog, inspect details, play, bookmark. |
| Search | `/` from Browse | Type a title/genre filter. |
| Playback | `Enter` on a title | Control the external mpv window and see progress. |
| Library | `l` from Browse | Watch history and bookmarks. |
| Settings | `s` from Browse | Catalog source, remote URL, default volume, Real-Debrid key. |
| Providers | `p` from Browse | Add, edit, enable, delete, and reload user providers. |
| Help | `?` | Shortcut reference overlay. |

`q` generally quits the app from a list screen. On the Playback screen, `q`
stops playback and returns to Browse. `Esc` backs out of the current screen, or
quits from Browse. While typing in a text field, `q` is just the letter `q`.

### Browse

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move the selection. |
| `j` / `k` | Move the selection (vim-style). |
| `/` | Enter search mode. |
| `Enter` | Play the selected title. |
| `b` | Add or remove the selected title from bookmarks. |
| `l` | Open Library. |
| `s` | Open Settings. |
| `p` | Open Providers. |
| `?` | Open the help overlay. |
| `q` / `Esc` | Quit the app. |

The left panel is the filtered catalog. The right panel shows title, year,
rating, genres, overview, provider, ID, poster URL, and stream URL. A green
**Resume available** line appears for partially watched titles, and bookmarked
titles show a heart.

### Search

| Key | Action |
| --- | --- |
| Printable characters | Append to the search query. This includes `q` and `?`. |
| `Backspace` | Delete the last character. |
| `Enter` | Apply the filter and return to Browse. |
| `Esc` | Cancel search, clear the query, and return to Browse. |

Search is case-insensitive and matches the title or any genre. Press `Esc` once
to leave search, then `q` to quit if needed. Changing the catalog source reloads
the catalog and clears the current search.

### Playback

| Key | Action |
| --- | --- |
| `Space` | Pause or resume. |
| `Left` | Seek backward 10 seconds. |
| `Right` | Seek forward 10 seconds. |
| `Up` | Increase volume by 5%. |
| `Down` | Decrease volume by 5%. |
| `q` / `Esc` | Stop mpv, save the current position, and return to Browse. |

The Playback screen shows the title, pause/play indicator, progress gauge,
current time/duration, and volume. The volume value is persisted. If the title
was previously watched past 5 seconds and not completed, playback seeks to the
saved position.

### Library

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move through the watch-history list. |
| `j` / `k` | Move through the watch-history list. |
| `Enter` | Resume the selected history item. |
| `l` / `Esc` | Return to Browse. |
| `q` | Quit the app. |

The left panel is **Continue Watching**, ordered by most recent activity. The
right panel lists bookmarks for reference. Bookmark rows are not selectable;
use Browse to play or remove a bookmark. Resuming looks the item up in the
currently loaded catalog, so switch back to the catalog source that supplied it
if necessary.

History is capped at 100 entries. A title counts as completed when playback is
within 5 seconds of the known duration. Completed titles are not auto-resumed.

### Settings

The four rows are:

1. **Catalog source**
2. **Catalog URL**
3. **Default volume**
4. **Real-Debrid API key**

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move between rows (wraps around). |
| `j` / `k` | Move between rows when not editing. |
| `Left` / `Right` | On Catalog source: cycle source. On Default volume: adjust by 5. |
| `Enter` | On Catalog source: cycle forward. On other rows: start editing. |
| `Enter` again | Save the edited value. |
| `Esc` | Cancel the current edit. Press again to return to Browse. |
| `q` | Quit the app when not editing. |
| Printable characters / `Backspace` | Edit the active text field. |

Field notes:

- **Catalog source** changes immediately and triggers a catalog reload. Search
  is cleared. If the new source fails, the app falls back to the bundled
  catalog and shows a toast.
- **Catalog URL** is only used by the Remote URL source. Saving it triggers a
  reload. An empty value clears it.
- **Default volume** accepts 0-100. An empty value keeps the current volume.
  Invalid text shows a toast.
- **Real-Debrid API key** is masked on screen. Saving a blank value clears the
  stored key.

### Providers

| Key | Action |
| --- | --- |
| `Up` / `Down` | Move through the provider list. |
| `j` / `k` | Move through the provider list. |
| `a` | Add a provider. |
| `e` | Edit the selected provider. |
| `d` | Delete the selected provider immediately. |
| `Enter` / `Space` | Enable or disable the selected provider. |
| `r` | Reload the catalog from the current source. |
| `Esc` / `p` | Return to Browse. |
| `q` | Quit the app. |

Changes are saved immediately. If **My providers** is the active catalog source,
adding, editing, deleting, toggling, or reloading providers schedules a catalog
reload. If another source is active, manage providers here first, then switch to
**My providers** in Settings.

The Details panel shows the provider status and URL, plus a reminder to select
**My providers** as the catalog source.

### Provider editor

| Key | Action |
| --- | --- |
| `Tab` / `Up` / `Down` | Switch between the Name and URL fields. |
| Printable characters | Edit the focused field. |
| `Backspace` | Delete the last character. |
| `Enter` | Validate and save. |
| `Esc` | Cancel without saving. |

The URL is required and must start with `http://` or `https://`. The name is
optional; when blank, the provider's host name is used. There is no confirmation
dialog for deletion, so use `d` carefully.

### Help overlay

| Key | Action |
| --- | --- |
| `?` | Open help when not typing in a text field. |
| `?` / `Esc` | Close help. |

While the help overlay is open, it captures these keys before the screen beneath
it.

---

## Catalog sources

Choose the source in **Settings -> Catalog source**. The options cycle in this
order:

1. Bundled sample
2. Remote URL
3. Internet Archive
4. My providers

### Bundled sample

Loads `data/movies.json` from the working directory, falling back to the copy
beside the executable. The installed app ships a copy at
`%LOCALAPPDATA%\russ-tui\data\movies.json`.

The sample catalog is a demonstration. Some entries point to public sample
clips and some intentionally have no stream. It is not a piracy catalog and is
not a provider.

### Remote URL

Loads a JSON document from an HTTP(S) endpoint configured in
**Settings -> Catalog URL**. The response must be an object with an `items`
array:

```json
{
  "items": [
    {
      "id": "unique-id",
      "title": "Title",
      "stream_url": "https://example.com/movie.mp4"
    }
  ]
}
```

A bare array is **not** accepted in Remote URL mode. Use a provider for that.
Stream URLs in this mode should be absolute; relative paths are not resolved.

The request has a 15-second timeout. Saving the URL or changing the source
reloads immediately. On failure, the app falls back to the bundled catalog and
shows a toast explaining the failure. Clearing the URL and selecting Remote URL
also falls back with a message.

### Internet Archive

Fetches up to 60 popular public-domain feature films from
<https://archive.org> using its public search API. No API key is needed.

The catalog stores an Archive identifier for each film. When you press play,
russ-tui requests the item metadata and chooses a video file, preferring:

1. MP4 / H.264
2. WebM
3. MKV
4. OGV
5. AVI

Within the best format, the largest file is preferred. Thumbnails, metadata
sidecars, torrents, XML, JSON, text, and image files are excluded.

The Internet Archive source requires network access. Provider is shown as
`Internet Archive` in the Details panel.

### My providers

Merges the catalogs of every **enabled** user provider. Fetches run
concurrently. If one provider fails, the others still load; the toast reports
how many titles loaded and how many providers failed.

If no enabled provider returns items, the app falls back to the bundled catalog.
If you have no providers yet, the toast points you to `p`.

Provider item IDs are namespaced with the provider URL, so identical IDs from
different providers cannot collide. The Details panel shows the provider name.

---

## Adding your own catalog and streams

There are three ways to use your own content:

1. Edit the bundled `data/movies.json`.
2. Serve a remote catalog and select **Remote URL**.
3. Add a provider and select **My providers**.

### Catalog JSON schema

Both provider catalog JSON and the Remote URL format use this item shape. Only
`id` and `title` are required; every other field is optional.

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | Required, non-empty. Namespaced automatically for providers. |
| `title` | string | Required, non-empty. |
| `year` | number | Optional release year. |
| `rating` | number | Optional rating, shown with a star. |
| `genres` | array of strings | Optional. Search matches genres. |
| `overview` | string | Optional synopsis. |
| `poster_url` | string or null | Optional poster URL shown in Details. |
| `stream_url` | string or null | Optional playable stream or resolver marker. |
| `provider` | string | Optional. Provider items are tagged automatically. |

Invalid items are skipped. If a provider returns a non-empty catalog but no
valid items, the provider reports an error.

```json
{
  "items": [
    {
      "id": "my-movie-1",
      "title": "My Movie",
      "year": 2024,
      "rating": 8.5,
      "genres": ["Drama"],
      "overview": "A synopsis.",
      "poster_url": "https://example.com/poster.jpg",
      "stream_url": "https://example.com/movie.mp4"
    }
  ]
}
```

Provider catalog JSON may also be a bare array:

```json
[
  {
    "id": "my-movie-1",
    "title": "My Movie",
    "stream_url": "https://example.com/movie.mp4"
  }
]
```

### Stream URL forms

| Form | Example | Behavior |
| --- | --- | --- |
| Absolute HTTP(S) | `https://cdn.example/movie.mp4` | Played directly by mpv. No Real-Debrid call. |
| Relative path | `movie.mp4` | Provider catalogs only. Resolved against the provider URL. |
| Magnet | `magnet:?xt=urn:btih:...` | Requires a Real-Debrid API key. |
| Debrid file host | `debrid:https://host.example/file.mkv` | Requires a Real-Debrid API key. |
| Internet Archive | `archive:night_of_the_living_dead` | Resolved through the Archive metadata API. No Real-Debrid key. |
| Stremio marker | `stremio:https://addon.example|movie|tt123` | Internal marker created by Stremio provider support. Do not hand-write. |
| Missing / empty | `null` | Playback reports that the title has no stream URL. |

An absolute HTTP(S) URL can point to any media or manifest format that your mpv
build supports, including HLS playlists. russ-tui does not transcode or proxy
streams.

**Relative URLs** are resolved against the provider catalog URL using standard
URL joining. For example, a provider at
`https://example.com/api/catalog.json` with `"stream_url": "movie.mp4"` resolves
to `https://example.com/api/movie.mp4`. Prefer absolute URLs to avoid surprises.
Relative URLs are not resolved in Remote URL mode.

### Editing the bundled sample

Source checkout:

```text
data/movies.json
```

Installed Windows app:

```text
%LOCALAPPDATA%\russ-tui\data\movies.json
```

Edit the file, save it, and restart the app while **Bundled sample** is
selected. Reinstalling replaces this file, so keep a backup if you customize it.
For a durable setup, use a remote catalog or provider instead.

### Testing a catalog locally

A quick way to test Remote URL or a catalog JSON provider:

```sh
python -m http.server 8000
```

Put `catalog.json` and a video file in the served directory, then use:

```text
http://127.0.0.1:8000/catalog.json
```

For **Remote URL**, use the object form with `items`. For a provider, either
object or bare array works. The app accepts `http://` for local testing and LAN
endpoints, but avoid sending secrets over plain HTTP.

### Authentication

russ-tui sends plain GET requests with no custom authentication headers. If an
endpoint needs a token, put it in the URL query string and understand that the
URL is stored in your local state file. Prefer HTTPS.

---

## Adding providers

Providers are entirely user-supplied. The app contains only the code that
consumes provider URLs. **No provider URLs, scrapers, or piracy endpoints are
bundled.**

Open Providers with `p`, press `a`, and enter a name and URL. Two endpoint
shapes are detected automatically.

### Catalog JSON providers

A provider URL can return either:

- an object with an `items` array, or
- a bare JSON array of items.

The schema is the same as above. Only `id` and `title` are required. Provider
items are tagged with the provider name and their IDs are namespaced with the
provider URL.

Stream URLs may be absolute, relative to the provider catalog URL, a magnet, a
`debrid:` file-host URL, an `archive:` marker, or an internal Stremio marker.
Provider HTTP requests time out after 20 seconds.

### Stremio addon providers

Enter either the addon base URL or its manifest URL:

```text
https://addon.example/
https://addon.example/manifest.json
```

The app tries the supplied URL, then falls back to `{url}/manifest.json` when
needed. A Stremio manifest is recognized by having a `catalogs` array plus
`resources` or `types`.

Supported behavior:

- Only catalogs with type `movie` are fetched.
- Catalogs that require an extra parameter are skipped, because the Browse
  screen has no provider-specific search input.
- Catalogs are fetched concurrently from
  `{base}/catalog/movie/{catalog-id}.json`.
- Duplicate metadata IDs across multiple catalogs are removed.
- When you press play, the addon's `{base}/stream/movie/{id}.json` endpoint is
  requested.
- Direct HTTP(S) stream URLs are preferred. If only an `infoHash` is present,
  russ-tui builds a magnet link and resolves it through Real-Debrid when a key
  is configured.

Series catalogs, required extra parameters, addon subtitles, and torrent
quality selection are not supported. If a manifest has no usable movie catalog,
the provider reports an error.

### Provider failures

A failing provider does not prevent other providers from loading. Errors are
collected and summarized in a toast. Common causes:

- HTTP status other than 2xx.
- Invalid JSON.
- A catalog with no valid `id`/`title` pairs.
- A Stremio manifest with no movie catalogs that avoid required extras.
- A timeout after 20 seconds.
- A URL that is not HTTP(S).

If every enabled provider fails or returns nothing, the app falls back to the
bundled sample catalog so the UI remains usable.

---

## Real-Debrid

Create an API key at <https://real-debrid.com/apitoken>, then paste it into
**Settings -> Real-Debrid API key**. The key is stored locally and masked on
screen.

With a key configured:

- **Magnet links** go through Real-Debrid: add the magnet, select all files,
  poll the torrent status every 2 seconds for up to about 60 seconds, then
  unrestrict the first returned link.
- **`debrid:` file-host links** are passed to Real-Debrid's unrestrict
  endpoint.
- **Plain HTTP(S) URLs** are played directly and never sent to Real-Debrid.
- **Internet Archive** and **Stremio direct URLs** do not need Real-Debrid.

Limitations:

- The account must be active and allowed to download the requested content.
- russ-tui selects all files and plays the first resulting link. There is no
  per-file or quality picker.
- A magnet that is not cached may take longer than the polling window and fail
  with a status message.
- The Real-Debrid client has a 30-second HTTP timeout per request.

Clearing the API key disables magnet and `debrid:` playback. Direct HTTP(S)
streams continue to work.

---

## Playback and mpv

russ-tui does not embed a video player. It starts mpv with an IPC endpoint and
controls it over JSON IPC:

- Windows: a named pipe.
- Linux/macOS: a Unix domain socket.

mpv is launched with `--idle=yes --force-window=immediate` so it can stay alive
between files and open its window immediately. The terminal shows progress,
time, pause state, and volume. Closing the TUI stops mpv.

### mpv discovery order

1. `RUSS_TUI_MPV` environment variable.
2. `mpv` on `PATH`.
3. Common Windows locations, including:
   - `%LOCALAPPDATA%\Programs\mpv\mpv.exe`
   - `%LOCALAPPDATA%\Microsoft\WinGet\Links\mpv.exe`
   - winget package directories containing `mpv`
   - `%ProgramFiles%\mpv\mpv.exe`
   - `%ProgramFiles%\MPV Player\mpv.exe`
   - `%ProgramFiles(x86)%\MPV Player\mpv.exe`
   - `C:\Program Files\MPV Player\mpv.exe`
4. Common Unix locations: `/usr/bin/mpv`, `/usr/local/bin/mpv`,
   `/opt/homebrew/bin/mpv`.

On Windows, prefer `mpv.exe` over `mpv.com`. The `.com` wrapper opens a console
window; the `.exe` does not.

If mpv is missing, playback shows a toast with installation guidance. Browsing,
search, providers, and settings still work.

### Volume and resume

- Volume starts from the saved default and is adjusted in 5% steps.
- Volume changes during playback are sent to mpv and persisted.
- On leaving playback, the current position and duration are saved to history.
- A title resumes automatically when it has more than 5 seconds of history and
  is not within 5 seconds of the known duration.
- Completed titles are marked with a check mark in Library.

---

## State and persistence

State is saved as JSON in the platform config directory:

| Platform | Path |
| --- | --- |
| Windows | `%APPDATA%\russ-tui\state.json` |
| Linux | `~/.config/russ-tui/state.json` (or `$XDG_CONFIG_HOME/russ-tui/state.json`) |
| macOS | `~/Library/Application Support/russ-tui/state.json` |

The file contains:

- Volume.
- Bookmarked media IDs.
- Watch history, resume positions, and completion flags (up to 100 entries).
- Catalog source.
- Remote catalog URL.
- Real-Debrid API key.
- User provider list (name, URL, enabled flag).

The Real-Debrid key and provider URLs are stored in **plain text** in your user
profile. Protect your user account and do not share the state file. Deleting
the file resets all settings, bookmarks, history, and providers; the app creates
a fresh default state on next launch.

The installer and app do not write provider URLs into the repository or install
directory. Provider configuration lives only in your state file.

---

## Updating and uninstalling

### Update

Rerun the installer from a fresh clone or the web one-liner. It rebuilds and
overwrites the executable, bundled catalog, docs, and launchers. Your state file
in `%APPDATA%\russ-tui` is preserved.

If you customized `data/movies.json` in the install directory, back it up first;
the installer replaces it with the bundled sample.

### Uninstall

1. Close russ-tui and any mpv window it started.
2. Remove the install directory:

   ```powershell
   Remove-Item -Recurse -Force "$env:LOCALAPPDATA\russ-tui"
   ```

3. Remove the install directory from your user `PATH` if the installer added it.
4. Remove the mpv override if you want:

   ```powershell
   [Environment]::SetEnvironmentVariable("RUSS_TUI_MPV", $null, "User")
   ```

5. Optionally remove state:

   ```powershell
   Remove-Item -Recurse -Force "$env:APPDATA\russ-tui"
   ```

Dependencies installed by the installer (Rust, Visual Studio Build Tools, mpv)
are normal system tools and are not removed automatically.

---

## Troubleshooting

### `cargo` is not recognized

Rust is not on `PATH`. Run `install.ps1` again, or install Rust from
<https://rustup.rs>, then open a new terminal. The installer also checks
`%USERPROFILE%\.cargo\bin` directly.

### The build fails with `link.exe not found`

The MSVC Rust host needs the Visual Studio C++ workload. Rerun `install.ps1`
without `-SkipBuildTools`, or open the Visual Studio Installer and add
**Desktop development with C++**.

### PowerShell says running scripts is disabled

Use:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Launch
```

This bypasses the policy for that process only.

### The installer cannot download the source

The web one-liner downloads the `main` branch archive from GitHub. If GitHub is
unreachable, clone the repository over Git or download a ZIP manually, then run
`install.ps1` from the checkout with `-SourceDir`.

### winget is unavailable

The installer can fall back to `rustup-init` for Rust, but mpv and Visual Studio
Build Tools normally use winget. Install **App Installer** from Microsoft Store,
or install mpv and the C++ workload manually, then rerun with `-SkipMpv` and/or
`-SkipBuildTools` as appropriate.

### Playback says mpv could not be found

Install mpv, or point russ-tui at it:

```powershell
[Environment]::SetEnvironmentVariable("RUSS_TUI_MPV", "C:\path\to\mpv.exe", "User")
```

Open a new terminal or restart the app. Check that the path is a file, not a
directory, and prefer `mpv.exe` over `mpv.com` on Windows.

### A provider fails to load

- Confirm the URL starts with `http://` or `https://`.
- Open the URL in a browser or with `curl`; it must return JSON, not HTML.
- The endpoint must be reachable from your machine without a login page.
- Catalog JSON needs valid `id` and `title` fields.
- Stremio addons must expose at least one `movie` catalog without a required
  extra.
- The request timeout is 20 seconds.

Other providers still load when one fails. The toast reports failures.

### Remote URL falls back to the bundled catalog

- The response must be `{"items": [...]}`, not a bare array.
- `items` must be an array.
- Stream URLs should be absolute.
- The endpoint must return HTTP 2xx.
- Check the toast for the specific error.

### A title says it has no stream URL

Add `stream_url` to the catalog item, or use a provider whose Stremio stream
endpoint returns a playable stream. Some sample titles intentionally have no
stream.

### Magnet or `debrid:` playback fails

- Set a Real-Debrid API key in Settings.
- Confirm the account is active and supports the content.
- A magnet may not be cached; the resolver polls for about 60 seconds.
- `debrid:` must be followed by a supported file-host URL.
- Plain HTTP(S) streams do not use Real-Debrid and can be tested independently.

### Search finds nothing

Search matches titles and genres, not overview text. Press `Enter` to apply the
query and `Esc` to clear it. Changing the catalog source also clears the search.

### The terminal display looks wrong

Use a modern terminal such as Windows Terminal, resize the window, and use a
font with box-drawing and Unicode symbol coverage. The UI is designed for a
reasonably large terminal; very small windows clip panels.

### The app exits immediately

Run it from a terminal to see the error:

```powershell
powershell -ExecutionPolicy Bypass -File .\run.ps1 -NoBuild
```

If the bundled catalog is missing, the toast reports the path it tried. Reinstall
the app or confirm that `data/movies.json` exists in the working directory or
beside the executable.

---

## Privacy, content, and license

- russ-tui has no telemetry and no analytics.
- The app contains **no provider URLs, scrapers, or piracy endpoints**.
- Outbound requests happen only when you use a network-backed feature:
  Internet Archive, a remote catalog, a provider, Real-Debrid, or a stream URL
  you supplied.
- Provider URLs and the Real-Debrid key are stored in plain text in your local
  state file. Treat that file as sensitive.
- The project does not host, index, or distribute media. You are responsible
  for ensuring that you have the rights to access anything you play and for
  following the terms of any provider, addon, or service you configure.
- The bundled sample catalog is for demonstration. Some entries point to public
  sample clips; others have no stream.
- Real-Debrid and Stremio addons are third-party services with their own terms,
  availability, and account requirements. They are not affiliated with this
  project.
- russ-tui is released under the MIT License. See [LICENSE](../LICENSE).

The software is provided **as is**, without warranty of any kind. Use it at your
own risk.

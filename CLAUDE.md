# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

WinIsland is a Windows application that displays a Dynamic Island-style interface on Windows, similar to Apple's Dynamic Island feature. It integrates with Windows System Media Transport Controls (SMTC) to show currently playing media with album art, playback controls, audio visualization, and lyrics support.

## Build and Development Commands

Build the project:
```bash
cargo build --release
```

Run in development mode:
```bash
cargo run
```

Run with settings window:
```bash
cargo run -- --settings
```

The release build uses aggressive optimizations (LTO, single codegen unit, stripped symbols) configured in Cargo.toml.

## Architecture

### Module Structure

- **main.rs**: Entry point with single-instance mutex check, tokio runtime initialization, and event loop setup. Supports `--settings` flag to launch settings window instead of main app.

- **window/**: Window management and application lifecycle
  - `app.rs`: Main `App` struct implementing `ApplicationHandler` trait from winit. Manages window state, rendering loop, media info display, spring physics animations, auto-hide behavior, and user interactions.
  - `settings.rs`: Settings UI window with configuration options
  - `tray.rs`: System tray icon and menu management

- **core/**: Core functionality modules
  - `smtc.rs`: Windows System Media Transport Controls integration. `SmtcListener` monitors media sessions, fetches metadata (title, artist, album, thumbnail), and tracks playback state.
  - `audio.rs`: Audio spectrum analysis using CPAL for audio capture and FFT processing. `AudioProcessor` captures system audio output and generates 6-band spectrum data for visualization.
  - `lyrics.rs`: Lyrics fetching and synchronization with playback position
  - `config.rs`: Application configuration structure and constants (window dimensions, offsets, feature flags)
  - `persistence.rs`: Configuration loading/saving to TOML files
  - `render.rs`: Skia-based rendering pipeline for the island UI
  - `i18n.rs`: Internationalization support

- **ui/**: UI components
  - `expanded/`: Expanded view layouts (main view with controls, widget view)

- **utils/**: Utility modules
  - `anim.rs`: Animation helpers
  - `autostart.rs`: Windows autostart registry integration
  - `blur.rs`: Motion blur calculations
  - `color.rs`: Color utilities and adaptive border color detection
  - `font.rs`: Font loading and management
  - `glass.rs`: Windows glass/transparency effects
  - `mouse.rs`: Mouse position and interaction utilities
  - `physics.rs`: Spring physics for smooth animations
  - `scroll.rs`: Scrolling behavior for lyrics
  - `updater.rs`: Auto-update checking functionality
  - `settings_ui/`: Settings window UI components

### Key Technical Details

**Windows-Specific Integration:**
- Uses Windows API extensively via the `windows` crate for SMTC, audio session management, window styling (WS_EX_TOOLWINDOW, WS_EX_NOACTIVATE), and registry access
- Window is always-on-top (HWND_TOPMOST) with no taskbar presence
- Single instance enforcement via named mutex (`Local\\WinIsland_SingleInstance_Mutex`)

**Rendering:**
- Uses `skia-safe` for 2D graphics rendering
- `softbuffer` for presenting rendered frames to the window
- Custom spring physics (`Spring` struct) for smooth width/height/radius animations
- Adaptive border colors based on album art thumbnail analysis

**Async Architecture:**
- Tokio runtime for async operations (SMTC monitoring, lyrics fetching, update checking)
- Uses `tokio::sync::watch` channels for media info updates between SMTC listener and main app
- `CancellationToken` for graceful shutdown of background tasks

**Media Integration:**
- SMTC provides media metadata and playback state from any Windows media app
- Audio spectrum visualization captures system audio output (loopback recording)
- Lyrics fetched from configurable sources with fallback support and timing synchronization

## Configuration

User configuration stored in TOML format at standard config directory (via `dirs` crate). Key settings include:
- Island dimensions and scaling
- SMTC app filtering (whitelist/blacklist)
- Lyrics source and display options
- Auto-hide behavior and timing
- Custom font paths
- Auto-start on Windows login
- Update check intervals

## Testing and Debugging

The application runs as a Windows GUI subsystem (`#![windows_subsystem = "windows"]`), which suppresses console output. For debugging, temporarily remove this attribute to see stdout/stderr.

No formal test suite is present. Manual testing should focus on:
- Media playback integration with various apps (Spotify, Windows Media Player, etc.)
- Window positioning across multiple monitors
- Auto-hide/show transitions
- Settings persistence
- System wake behavior (recent fix for island shifting after wake)

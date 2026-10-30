# sloplight

A compact, keyboard-first launcher for Apollo / Sunshine / GeForce Experience
hosts, written in Rust with [GPUI](https://www.gpui.rs) (Zed's UI framework).
It replaces the Qt/QML UI of this Moonlight fork; the video stream itself is
still rendered by the C++ core, which runs as a helper process
(`moonlight stream-helper`).

Colors come from your Zed theme: the app reads `~/.config/zed/settings.json`,
finds the dark theme named there in `~/.config/zed/themes/*.json` (or the
installed Zed extensions) and derives every color role from it. The buffer
font from Zed is used for monospace text. Without Zed it falls back to the
built-in `gruvbox-poly` values.

## Layout

```
sloplight/
  src/main.rs          app entry: window, key bindings, native menus
  src/app.rs           Root state: hosts, selection, dialogs, stream lifecycle, keyboard
  src/theme.rs         Zed theme loader + color/typography tokens
  src/settings.rs      stream settings (same fields as StreamingPreferences), JSON on disk
  src/stream.rs        spawns `moonlight stream-helper`, relays its JSON events
  src/backend/         Rust port of app/backend: HTTP(S) client with pinned certs,
                       pairing (PIN + Apollo OTP), host polling, mDNS, Wake-on-LAN, box art
  src/ui/              hand-built widgets (button, toggle, segmented, menu, text input, ...)
  src/views/           shell (sidebar + content), host state panel, info popover, dialogs, overlays, settings, quick launch palette
  DESIGN.md            first design spec; DESIGN-v2.md is the simplification pass the UI now follows
  assets/icons/        Lucide icons (SVG)
```

Data lives in `~/Library/Application Support/sloplight` on macOS
(`$XDG_CONFIG_HOME/sloplight` on Linux): `identity.json` (client certificate),
`hosts.json`, `settings.json`. Box art is cached under the user cache dir.
`SLOPLIGHT_CONFIG_DIR` overrides the config directory.

## Building

```sh
cd sloplight
cargo build --release        # needs the Rust nightly toolchain from rust-toolchain / rustup
./target/release/sloplight
```

GPUI is built with its `runtime_shaders` feature so the Metal shader compiler
(`xcrun metal`) is not needed at build time.

The streaming helper is the Moonlight binary from this repository with the
`stream-helper` action. It is looked up in this order: the `helper_path`
setting (Settings > Advanced > Streaming helper), `$SLOPLIGHT_HELPER`, next to
the executable, `../../../result/...` (the `nix build` output when running
from `sloplight/target`), `/Applications/Moonlight.app`, then `moonlight` on
`$PATH`. Build it with `nix build '.?submodules=1'` in the repository root.

### CI builds

`.github/workflows/sloplight-macos.yml` builds `Sloplight.app` for Apple Silicon on
the macOS 14, 15 and 26 GitHub runners (Nix for the helper, cargo for the
launcher). It is slow (15-25 minutes per runner), so it runs only for `sloplight-v*` tags,
where it attaches a zip per runner to the GitHub release, or when started by hand
from the Actions tab ("Run workflow", optionally with a ref), where the zips are
workflow artifacts. The app is ad-hoc signed, not
notarized, so after unzipping run `xattr -dr com.apple.quarantine Sloplight.app`
or right-click → Open once.

## Helper protocol

`moonlight stream-helper` reads one JSON document on stdin (identity, host,
app, settings, optional display origin) and prints one JSON object per line on
stdout: `warnings`, `stage_starting`, `stage_failed`, `connection_started`,
`launch_error`, `quit_starting`, `session_finished`. See `src/stream.rs` and
`app/cli/streamhelper.cpp`.

## Keyboard

`⌘K` quick launch · `⌘,` settings · `⌘N` add host · `⌘R` refresh · `⌘F` filter ·
`⌘I` host info · `⇧⌘C` copy address · `⇧⌘H` hidden apps · `⌘1…9` select host · `⌘[` `⌘]` previous/next host ·
arrows/Enter in the grid · typing filters the grid · `F2` rename · `⌘⌫` remove host / quit app ·
`Esc` back.

`SLOPLIGHT_DEBUG_STATE=settings|info|pairing|otp|add-host|error|confirm|palette|host-menu|app-menu|toast`
opens that state right after startup, for screenshots without a host.

## Not yet ported

Network test (needs moonlight-common-c), STUN external-address lookup, Discord
rich presence, the CLI `pair`/`stream`/`quit` actions (still available in the
Moonlight binary itself), and gamepad navigation of the launcher.

## Credits and licenses

Sloplight is part of this moonlight-qt fork and is licensed under the
[GPL-3.0](../LICENSE), like Moonlight itself. It builds on:

- [Moonlight](https://github.com/moonlight-stream/moonlight-qt) and
  [moonlight-common-c](https://github.com/moonlight-stream/moonlight-common-c)
  by the Moonlight Game Streaming Project (GPL-3.0), which run the stream.
- [Apollo](https://github.com/ClassicOldSong/Apollo) host extensions, re-implemented
  from [Artemis Qt](https://github.com/wjbeckett/artemis).
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the UI
  framework from the Zed editor (Apache-2.0).
- `assets/zed-one.json`, the "One Dark" theme from
  [Zed](https://github.com/zed-industries/zed) (GPL-3.0), used as the base
  that your own Zed theme is layered over.
- `assets/icons/`, from [Lucide](https://lucide.dev) (ISC).

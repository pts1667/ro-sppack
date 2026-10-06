# ro-sppack

A setup dashboard and one-click launcher for a self-hosted **Ragnarok Online**
package. It assembles three open source projects plus your own game assets into
a single folder and runs everything from one window:

| Component | Source | Role |
| --- | --- | --- |
| Game server | [`pts1667/rust-ro`](https://github.com/pts1667/rust-ro) | A Rust Ragnarok Online server (login / char / map in one process, native WebSocket support) |
| Web client | [`MrAntares/roBrowserLegacy`](https://github.com/MrAntares/roBrowserLegacy) | Ragnarok Online in the browser (WebGL) |
| Asset server | [`Flux159/roBrowserLegacy-RemoteClient-Rust`](https://github.com/Flux159/roBrowserLegacy-RemoteClient-Rust) | Serves the built client, extracts game assets from your GRF archives and serves them over HTTP |
| Game assets | You | The kRO client root (`DATA.INI` + `.grf` archives), supplied during setup |

ro-sppack is the launcher itself (a small [egui](https://github.com/emilk/egui)
desktop app). It clones the repositories, builds them, prepares the assets,
writes all configuration, and starts/stops the processes. Because `rust-ro`
accepts WebSocket clients on its game port, no `wsProxy` relay is required:
the browser connects straight to the game server.

## Requirements

- **Windows / Linux / macOS** (the launcher builds on all three; rust-ro and the
  remote client are plain Rust).
- **Git** on `PATH`.
- **Rust toolchain installed via [rustup](https://rustup.rs)** — rust-ro pins a
  nightly toolchain in its `rust-toolchain.toml` and uses nightly-only features.
  Before building the server the launcher runs
  `rustup toolchain install <pinned-channel> --profile minimal` and then builds
  with `cargo +<pinned-channel>`, so the pinned nightly does not need to be your
  default toolchain. A plain non-rustup cargo will only work if it already is a
  compatible nightly.
- **Node.js 22+ and npm** — required by roBrowserLegacy.
- A **Ragnarok Online client folder** (kRO or compatible) containing
  `DATA.INI` and the `.grf` archives. ro-sppack never downloads game assets;
  they are copyrighted by Gravity Co., Ltd.

## Quick start

1. Download/copy the `ro-sppack` executable to a folder of your choice
   (`cargo build --release` produces `target/release/ro-sppack`).
2. Run it. The **Setup** tab opens:
   - choose an **Install folder** (created if missing),
   - choose your **Assets folder** (the client root with `DATA.INI` + GRFs),
   - adjust rates/ports if you like,
   - press **Install Everything**.

   The pipeline runs sequentially and streams every command's output into the
   **Logs** tab:
   ```
   Clone rust-ro server
   Clone roBrowserLegacy client
   Clone remote client
   Build rust-ro server            (cargo build --release)
   Install client dependencies     (npm ci)
   Build web client                (npm run build:all)
   Build remote client             (cargo build --release)
   Prepare game assets             (copy GRFs / write DATA.INI)
   Generate configuration
   ```
3. When everything shows **ready**, press **Launch Game**. The launcher:
   - regenerates the server and client configuration,
   - starts `rust-ro` (game server, default port `6901`),
   - starts the remote client (client + assets, default port `3338`),
   - opens `http://127.0.0.1:3338/` in your browser.

Log in with the seeded account **`admin` / `qwertz`** (account id `2000000`).
That account is handled entirely by rust-ro; other accounts would be proxied
to a separate rAthena/Hercules installation and are not available in this
standalone package.

## Folder layout

```
<install folder>/
  rust-ro/                  cloned server repository (config.json is regenerated)
  roBrowserLegacy/          cloned web client repository (built into dist/Web)
  remote-client/            cloned asset server repository and its release binary
  ROenglishRE/              optional English translation (base, `Ragnarok/` subfolder)
  ROenglishPRE/             optional English translation (pre-renewal overlay)
  translation-re.json       marker written after ROenglishRE is applied
  translation-pre.json      marker written after ROenglishPRE is applied
  webroot/                  runtime data owned by the launcher
    Config.local.js         generated client override (server address, ports, ...)
    .env                    generated remote client configuration
    resources/
      DATA.INI              generated archive manifest
      *.grf                 copied archives (or referenced in place)
    data/                   loose overrides: assets folder, translations, auto-extracted
    System/                 loose System/ overrides (translations, item info)
    BGM/  AI/               only present when "copy" is enabled for them
  ro-sppack.json            launcher settings (working directory)
```

## Command line

The launcher has a headless mode for automation:

```text
ro-sppack --setup [--force] [--install-dir PATH] [--assets-dir PATH]
ro-sppack --generate-configs [--prepare-assets] [--install-dir PATH] [--assets-dir PATH]
ro-sppack --help
```

Everything else is driven by the same `ro-sppack.json` used by the GUI.

On Windows the release build is a GUI-subsystem executable, so redirect the
output when you want to watch a headless run:

```bat
ro-sppack.exe --setup > setup.log 2>&1
```

## What gets generated

**`rust-ro/config.json`** — built from `config.template.json` with:

- `server.host` / `server.port` from your settings,
- `server.enable_websocket = true`, `server.enable_legacy_proxy = false`,
- `server.packetver` (default `20120307`),
- the seeded account `2000000` appended to `server.accounts`,
- `game.*` rates (base/job EXP, drop, card drop, mob density).

**`webroot/Config.local.js`** — the client override:

```js
window.ROConfigLocal = {
  remoteClient: 'http://<host>:<web_port>/',
  servers: [{ address: '<host>', port: <game_port>, packetver: ..., 
              socketProxy: 'ws://<host>:<game_port>', remoteClient: ... }],
  skipServerList: true, skipIntro: true, ...
};
```

**`webroot/.env` and process environment** — remote client settings:
static serving from `roBrowserLegacy/dist/Web`, `resources/` as the archive
folder, auto-extraction of requested assets, `ENABLE_WSPROXY=false`.

## Notes

- **Archive copy vs reference.** Copying is the default and makes the install
  self-contained. If you are short on disk space, switch "Reference archives in
  place": no GRFs are copied, and `DATA.INI` gets absolute paths instead. BGM and
  AI folders are referenced in place unless "Copy BGM/ and AI/ folders" is
  checked.
- **LAN play.** Set *Bind host* to `0.0.0.0`, set *Advertised host* to your
  machine's LAN IP, and open the web port in your firewall. Everyone who joins
  uses `http://<your-ip>:<web_port>/`. (`rust-ro` and the asset server do not
  provide TLS; for internet play put a reverse proxy with HTTPS/WSS in front.)
- **Updating.** The *Update (git pull)* buttons fetch the latest upstream code;
  rebuild afterwards with the per-component **Build** buttons or
  **Rebuild Everything**.
- **Rebuilds.** `Install Everything` only fills in missing pieces.
  *Rebuild Everything* forces every build and re-copies assets.
- **Settings file.** Install and assets paths (plus every other option) are
  remembered in `ro-sppack.json` in the current working directory, rewritten
  whenever something changes. If no file is there yet, a settings file left
  next to the executable by older builds is copied over automatically on first
  run.
- **English translation.** On English systems the setup enables the fan
  translation by default: [ROenglishRE](https://github.com/zackdreaver/ROenglishRE)
  (base) is cloned and copied into the client data, then
  [ROenglishPRE](https://github.com/zackdreaver/ROenglishPRE) is cloned and copied
  on top of it, so the pre-renewal files win where the two overlap. Most of the
  content is Lua table (`.lub`) and name/description/intro `.txt` files, which
  the asset server serves before the GRFs — no archive repacking is needed.
  Turn it off with *Install the English translation* in Setup, or re-apply after
  an update with **Apply into data**.
- **Ports in use.** Startup errors from rust-ro or the remote client appear in
  the Logs tab; change the ports in Setup and relaunch.
- **Log files.** Everything in the Logs tab is also written to
  `<install dir>/logs`: `server.log`, `web.log` and `launcher.log` (setup jobs
  and launcher messages). Lines are flushed to disk about every 10 seconds, and
  the previous run's file is kept as `<name>.previous.log`.
- **Server debug log.** Tick **Server debug log** in the Setup options to start
  rust-ro with `--debug-log`, which also logs failed NPC conversations and
  other script failures into `server.log`. It applies the next time the server
  starts.

## Licensing

The launcher (this repository) is MIT licensed. It clones and builds third
party projects at run time; those keep their own licences (rust-ro, the asset
server and roBrowserLegacy are GPL-3.0-style/copyleft projects). Game assets are
never bundled and remain the property of their respective copyright holders.

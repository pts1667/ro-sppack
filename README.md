# ro-sppack

A setup dashboard and one-click launcher for a self-hosted **Ragnarok Online**
package. It assembles the open source projects plus your own game assets into a
single folder and runs everything from one window:

| Component | Source | Role |
| --- | --- | --- |
| Game server | [`pts1667/rust-ro`](https://github.com/pts1667/rust-ro) | A Rust Ragnarok Online server (login / char / map in one process) |
| Game client | [`pts1667/nostalro-client`](https://github.com/pts1667/nostalro-client) | A native Rust client (wgpu/winit) that reads your GRF archives directly — no browser and no asset server |
| Game assets | You | The kRO client root (`DATA.INI` + `.grf` archives), supplied during setup |
| English translation (optional) | [`zackdreaver/ROenglishRE`](https://github.com/zackdreaver/ROenglishRE) + [`zackdreaver/ROenglishPRE`](https://github.com/zackdreaver/ROenglishPRE) | Loose data files that override the archive contents |

ro-sppack is the launcher itself (a small [egui](https://github.com/emilk/egui)
desktop app). It clones the repositories, builds them, prepares the assets,
writes all configuration, and starts/stops the server and client.

## Requirements

- **Windows / Linux / macOS** (the launcher and the client build on all three).
- **Git** on `PATH`.
- **Rust toolchain installed via [rustup](https://rustup.rs)** — the server pins
  a nightly toolchain and the client pins a recent stable one, both in
  `rust-toolchain.toml`. Before each build the launcher runs
  `rustup toolchain install <pinned-channel> --profile minimal` and then builds
  with `cargo +<pinned-channel>`, so neither toolchain needs to be your default.
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
   Clone nostalro client
   Clone English translation (ROenglishRE)     (optional, on by default)
   Clone English translation (ROenglishPRE)    (optional, on by default)
   Build rust-ro server                        (cargo build --release)
   Build nostalro client                       (cargo build --release -p ragnarok-client)
   Prepare game assets                         (copy/reference GRFs, loose data)
   Apply English translation (ROenglishRE)
   Apply English translation (ROenglishPRE)
   Generate configuration
   ```
3. When **PLAY** lights up, press it. The launcher:
   - regenerates the server configuration and patches the client `config.json`,
   - starts `rust-ro` (game server, default port `6901`),
   - starts the native client, which connects to the login server.

Log in with the seeded account **`admin` / `qwertz`** (account id `2000000`).
That account is handled entirely by rust-ro; other accounts would be proxied
to a separate rAthena/Hercules installation and are not available in this
standalone package.

## Folder layout

```
<install folder>/
  rust-ro/                    cloned server repository (config.json is regenerated)
  nostalro-client/            cloned native client repository
    config.json               managed connection/resource fields + your in-game settings
    data/
      *.grf                   copied archives (none in "reference" mode)
      ro-sppack-assets.json   archive manifest written into config.json
      extracted/              loose overrides; checked before the archives
      BGM/                    only when "copy BGM" is enabled
  ROenglishRE/                optional English translation (base, `Ragnarok/` subfolder)
  ROenglishPRE/               optional English translation (pre-renewal overlay)
  translation-re.json         marker written after ROenglishRE is applied
  translation-pre.json        marker written after ROenglishPRE is applied
  ro-sppack.json              launcher settings (working directory)
```

The client resolves files by priority: `data/extracted/` first, then every
archive in `grf_paths` in order, so a translation or loose override wins
without repacking a `.grf`.

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

**`nostalro-client/config.json`** — the launcher only patches the fields it
owns and preserves everything else the client wrote (window state, keybindings,
volumes, graphic options):

- `packetver`, `login_servers[0]` (host/port from your settings),
- `grf_paths` — `data/<name>.grf` entries in copy mode, absolute paths in
  reference mode,
- `data_dir = data/extracted` when loose overrides were prepared,
- `bgm_path` — `data/BGM` when copied, otherwise the assets folder's `BGM/`
  by absolute path.

## Notes

- **Archive copy vs reference.** Copying is the default and makes the install
  self-contained. If you are short on disk space, switch "Reference archives in
  place": no GRFs are copied and `grf_paths` gets absolute paths instead.
- **English translation.** On English systems the setup enables the fan
  translation by default: [ROenglishRE](https://github.com/zackdreaver/ROenglishRE)
  (base) is cloned and copied into `data/extracted/`, then
  [ROenglishPRE](https://github.com/zackdreaver/ROenglishPRE) is cloned and copied
  on top of it, so the pre-renewal files win where the two overlap. Most of the
  content is Lua table (`.lub`) and item/skill/message `.txt` files — no archive
  repacking is needed. Turn it off with *Install the English translation* in
  Setup, or re-apply after an update with **Apply into data**.
- **LAN play.** Set *Bind host* to `0.0.0.0` and *Advertised host* to your
  machine's LAN IP, then open the game port in your firewall. Every player
  needs their own copy of the client configured with your IP as the login
  server. The client does not provide TLS; for internet play use a
  VPN/tunnel.
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
- **Ports in use.** Startup errors from rust-ro or the client appear in the
  Logs tab; change the game port in Setup and relaunch.
- **Log files.** Everything in the Logs tab is also written to
  `<install dir>/logs`: `server.log`, `client.log` and `launcher.log` (setup
  jobs and launcher messages). Lines are flushed to disk about every 10
  seconds, and the previous run's file is kept as `<name>.previous.log`.
- **Server debug log.** Tick **Server debug log** in the Setup options to start
  rust-ro with `--debug-log`, which also logs failed NPC conversations and
  other script failures into `server.log`. It applies the next time the server
  starts.

## Licensing

The launcher (this repository) is MIT licensed. It clones and builds third
party projects at run time; those keep their own licences (rust-ro and
nostalro-client are GPL-3.0-style/copyleft projects). Game assets are never
bundled and remain the property of their respective copyright holders.

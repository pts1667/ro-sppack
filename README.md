# ro-sppack

A setup dashboard and one-click launcher for a self-hosted **Ragnarok Online**
package. It assembles the open source projects plus your own game assets into a
single folder and runs everything from one window:

| Component | Source | Role |
| --- | --- | --- |
| Game server | [`pts1667/rust-ro`](https://github.com/pts1667/rust-ro) | A Rust Ragnarok Online server (login / char / map in one process) |
| Game client | [`pts1667/nostalro-client`](https://github.com/pts1667/nostalro-client) | A native Rust client (wgpu/winit) that reads your GRF archives directly — no browser and no asset server |
| Bots | [`pts1667/rust-ro-bot`](https://github.com/pts1667/rust-ro-bot) | Characters that play on their own through the server's Bot API: they level, trade, party up and chat |
| Game assets | You | The kRO client root (`DATA.INI` + `.grf` archives). The server reads it once to build the game data it serves on `/data` |
| English translation (optional) | [`llchrisll/ROenglishRE`](https://github.com/llchrisll/ROenglishRE) | Loose data files (renewal base, pre-renewal overlay, old-client compatibility tables) that override the archive contents |

ro-sppack is the launcher itself (a small [egui](https://github.com/emilk/egui)
desktop app). It clones the repositories, keeps them up to date, builds them,
builds the client data, writes all configuration, and starts/stops the server,
the client and the bots.

## Requirements

- **Windows / Linux / macOS** (the launcher and the client build on all three).
- **Git** on `PATH`.
- **Rust toolchain installed via [rustup](https://rustup.rs)** — rust-ro, the
  client and the bots all build with the stable toolchain, so the launcher runs
  `cargo` with your default toolchain and does not install or switch any other.
  The optional [web client](#web-client) also needs the `wasm32` target and `wasm-bindgen-cli`.
- A **Ragnarok Online client folder** (kRO or compatible) containing
  `DATA.INI` and the `.grf` archives. ro-sppack never downloads game assets;
  they are copyrighted by Gravity Co., Ltd.

## Quick start

1. Download/copy the `ro-sppack` executable to a folder of your choice
   (`cargo build --release` produces `target/release/ro-sppack`).
2. Run it. The launcher opens with an update bar and a big button. Open
   **Settings → General** and:
   - choose an **Install folder** (created if missing),
   - choose your **Assets folder** (the client root with `DATA.INI` + GRFs),
   - adjust the server under **Server** and the bots under **Bots** if you like.

   Then close the menu and press **INSTALL**. The update bar shows each step
   ("Step 4 of 10 · Compiling …"), and every command's output goes to **Logs**:
   ```
   Clone rust-ro server
   Clone nostalro client
   Clone rust-ro-bot
   Clone English translation (ROenglishRE)     (optional, on by default)
   Build rust-ro server                        (cargo build --release)
   Build nostalro client                       (cargo build --release -p ragnarok-client)
   Build rust-ro-bot                           (cargo build --release)
   Build client data                           (rust-ro client-data-setup, from the GRFs)
   Apply English translation (ROenglishRE, Pre-Renewal)
   Generate configuration
   ```
3. When the button reads **PLAY**, press it. The launcher:
   - regenerates the server configuration and patches the client `config.json`,
   - starts `rust-ro` (game server, default port `6901`),
   - starts the native client, which connects to the login server,
   - starts 50 bots once the server accepts connections (see [Bots](#bots)).

The client asks only for a name (and a sex): logins from this computer skip
the account system (see [Login](#login)).

## Updates

Every time the launcher starts, it updates in the background before anything
runs: `git pull` in every clone, then `cargo build` of the server, the client
and the bots (cargo only recompiles what changed), then whatever the install is
still missing. A translation that pulled new commits is applied again. While it
runs the button reads **UPDATING…**; afterwards the bar says whether anything
changed, and the commits in use are shown under the button.

- Tick **Play without updating** above the button to skip it: ticking it
  during an update cancels the rest, and the launcher does not update on later
  starts until it is unticked again (unticking updates at once). An install
  that is not complete is always finished.
- A failed update is shown in red above the bar. PLAY then starts the version
  that is installed, and **INSTALL** (when something is missing) tries again.
- **Settings → Components** still has a Build and an Update (git pull) button
  per component, Install Everything and Rebuild Everything.

## Login

With **Login: by IP** ticked in **Settings → Server** (the default),
the server config lists this computer in `login.skip_account_ips`: `127.0.0.1`,
and for LAN play the advertised host when it is an IPv4 address. Logins from
those addresses need no password; the client asks the server
(`CA_SERVER_INFO`) and shows only a name and a sex choice, and a name that is
new gets an account on first use. Other computers still log in with an account,
such as the seeded **`admin` / `qwertz`** (account id `2000000`). That account
is handled entirely by rust-ro; other accounts would be proxied to a separate
rAthena/Hercules installation and are not available in this standalone package.

## Bots

With **Start: with the server** ticked in **Settings → Bots** (the default),
the launcher starts
`rust-ro-bot run --bots 50 --server-dir <install>/rust-ro --stop-on-stdin …` as
soon as the server's game, client data, reference and Bot API ports accept
connections. With `--server-dir`, the bots take the Bot API and reference
addresses and the API key from the server's `config.json`, so they follow
whatever the launcher wrote there.

The Bots page sets the rest of the command line; changes apply the next time
the bots start:

| Setting | Default | Argument |
| --- | --- | --- |
| Model | Decision model | `--voice system-one`, `--voice llm`, or `--no-llm` for none (the rules pick the goals; no chat, no forum) |
| Decision model server | `http://127.0.0.1:9931` | `--s1-url` |
| LLM server, model name, thinking budget, temperature | `http://127.0.0.1:8080`, `local-model`, 2048 tokens, 0.3 | `--llm-url`, `--llm-model`, `--llm-think`, `--llm-temp` |
| Strategist memory (LLM) | fresh prompt each step | `--context skill-state` / `history` |
| Model share of the goal choices (kept per model) | the bots' default: 0.25 decision model, 1 LLM | `--llm-share` |
| Sessions | take turns, 30 to 60 minutes | `--session 30-60`, or `0` to stay in the game |
| Pool | twice the bots | `--pool` |
| Join gap, strategist period | 3 s, 45 s | `--join-gap`, `--strategist-period` |
| Visit the forum, solo | on, off | `--no-forum`, `--solo` |
| Extra arguments | none | anything else of `rust-ro-bot run --help`, e.g. `--role healer` |

**Check** next to a server address tries to connect to it. A model server that
does not answer is no error: the bots then play by the rules alone and don't
chat.

- **Stopping logs the bots out cleanly.** The launcher sends them `stop` on
  their stdin: each finishes its step, saves its memory and leaves the game.
  Bots still running after 60 seconds are killed. If the launcher itself dies,
  the pipe closes and the bots stop the same way.
- Stopping the server (or **Stop All**, or closing the window) logs the bots out
  first and kills the server only after they are gone, so they can still say
  bye. Closing the window can therefore take up to a minute while bots run.
- A run that was killed anyway leaves no characters behind for long: the next
  run logs out the resting characters of its pool that are still in the game.
- The server config gets `bots.max_bots` and `bots.max_registered_bots` of at
  least twice the bot count, and at least the pool: every character of the pool
  must be registered.
- The **Bots** card in Settings → Processes and the **Run** button in Components start and
  stop them by hand while the server runs.
- Their output goes to the Logs tab and `logs/bots.log`.

## Plugins

**Settings → Plugins** lists the server's plugins: every folder with a
`plugin.wasm` in its plugins folder (`rust-ro/config/plugins` unless
`scripting.plugins_dir` says otherwise). See rust-ro's `docs/plugins.md`.

- The checkbox in a plugin's header loads it when the server starts. A plugin
  you haven't ticked or unticked keeps what `config.template.json` says (the
  examples ship off). The launcher sets `enabled` on the plugin's entry in
  `scripting.plugins`, keeping its place and `capabilities`, or appends it.
- Open a plugin to edit its settings. The launcher reads each plugin's
  `info.json` (title, description, settings and defaults) from inside its
  `plugin.wasm`, and the type of a setting's default picks its field: a
  checkbox, a number (whole when the default is whole), a text, or JSON for
  lists and objects, one element per line. **Default** puts a setting back.
- Settings that differ from the defaults are written to
  `config/plugins/<name>.json`, which the server reads; with none, the launcher
  removes that file. A file you wrote by hand is shown on the page and kept
  until you change that plugin's settings there.
- A setting the plugin no longer has after an update is shown in red with a
  **Remove** button. The server would refuse the plugin over it, so while the
  plugin is enabled the launcher does not write the configuration until it is
  removed.
- Changes apply the next time the server starts. **Refresh** looks for plugins
  again after you copy one into the folder.

## Web client

The client also builds for the browser. **Settings → General → Client data**
has two options for it, both off by default:

- **Build the web client** runs nostalro-client's `web/build.ps1` (Windows) or
  `web/build.sh` during setup and updates, like the native client's build. It
  writes the page and its wasm module to `nostalro-client/web/dist`. Once, install
  what the build needs (the client pins `wasm-bindgen-cli` to 0.2.118):
  ```bash
  rustup target add wasm32-unknown-unknown
  cargo install wasm-bindgen-cli --version 0.2.118 --locked
  ```
  The launcher does not install either. When one is missing, the build step
  fails and its output is in the Logs tab. A copy of nostalro-client without the
  web client (no `web/build.ps1` or `web/build.sh`) fails the step with
  "not found" instead.
- **Serve the web client at /** sets `client_data.web_root` in rust-ro's
  `config.json` to that `web/dist` folder. The server then serves the page at
  `/` on the client data port (`6903` by default), next to `/data`, so the page
  and the game data come from the same origin. Off, or while `web/dist` holds no
  built client, `web_root` is `""`, which serves nothing. It applies the next
  time the server starts.

Serving does not need the build option: a `web/dist` made by hand works too. The
server does not compress the ~20 MB `.wasm`, so put a compressing proxy in front
of it for players far from the server.

## Folder layout

```
<install folder>/
  rust-ro/                    cloned server repository (config.json is regenerated)
    config/client-data/       game data the server serves on /data (built from the kRO archives)
  nostalro-client/            cloned native client repository
    web/dist/                 web client build, when built (served at / with the web client option)
  rust-ro-bot/                cloned bot repository (state/ and cache/ hold the bots' memory and the reference cache; an update that brings a new server commit clears the cache)
    config.json               managed connection/data fields + your in-game settings
    data/
      extracted/              English translation; checked before the served data
  ROenglishRE/                optional English translation clone
  translation.json            marker written after the translation is applied
  ro-sppack.json              launcher settings (working directory)
```

The client resolves files by priority: `data/extracted/` first, then the
server's `/data` endpoint (`data_url`), so a translation wins without
repacking anything. No archives are copied into the client.

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
- `login.skip_account_ips` (see [Login](#login)),
- `game.*` rates (base/job EXP, drop, card drop, MVP drop, mob density),
  `game.sim_speed` (**Simulation speed**: game time per real time, 1 by default),
  `game.max_base_level` and `game.max_stat_level`,
- `char_server.server_name` and `login.new_account` (**Registration**),
- `client_data` (the `/data` endpoint: port 6903 by default, bound to the bind host;
  `web_root` is `web/dist` when [serving the web client](#web-client) and `""` otherwise),
- `bots` (the bot API: enabled flag and port 6902 by default; with the API
  on, `max_bots` and `max_registered_bots` of at least twice the bot count),
- `reference` (`/db` and `/wiki`, which the bots plan with: enabled, port 6904 by default),
- `scripting.plugins` (see [Plugins](#plugins)), with each plugin's settings in
  `config/plugins/<name>.json`.

`config.json` is written from the template each time, so edit other server
options under **More server options** on the Server page: a JSON object such
as `{"game": {"pvp_exp": true}, "char_server": {"start_zeny": 1000}}`, merged
over the template before the values above are written (those win). Invalid JSON
keeps the configuration from being written and is shown in red.

**`nostalro-client/config.json`** — the launcher only patches the fields it
owns and preserves everything else the client wrote (window state, keybindings,
volumes, graphic options):

- `packetver`, `login_servers[0]` (host/port from your settings),
- `data_url` — the server's `/data` endpoint, built from the advertised host
  and the client data port,
- `grf_paths` — cleared: the client gets everything from `data_url`,
- `data_dir = data/extracted` when the translation was applied,
- `bgm_path` — the server's `rust-ro/config/client-data/bgm`, read straight from disk, so a track copied there after the client data build plays even though the endpoint's file list does not name it.

## Notes

- **Client data.** *Build client data* runs rust-ro's `client-data-setup` over
  your kRO folder (the archives listed in `DATA.INI`, or every `.grf`). It
  writes `rust-ro/config/client-data`, which the server publishes on the client
  data port. Build it once; *Rebuild Everything* builds it again.
- **Menu changes.** Every value in the Settings menu is checked before it is used:
  rates accept 0.0 to 10000.0, mob density 0.0 to 10.0, and ports 1 to 65535
  (the game, client data, reference and bot ports must differ). A value that is not valid
  is not applied. Valid values are written to both configuration files at once;
  a running server reads its configuration only at start, so restart it to
  apply the change.
- **English translation.** On English systems the setup enables the fan
  translation by default: [llchrisll/ROenglishRE](https://github.com/llchrisll/ROenglishRE)
  is cloned and layered into `data/extracted/`, with later layers overwriting
  earlier ones:
  1. `Translation/Renewal` — the renewal base (message table, interface
     textures, Lua tables),
  2. `Translation/Pre-Renewal` — pre-renewal skills, maps and Lua files,
  3. `Translation/Compatibility/2012-04-10/Pre-Renewal` — the classic
     `*item*table.txt` files that a 2012-era client reads for item names,
     descriptions and slots.

  This mirrors what the project's `Tools/ClientGenerator.bat` produces for
  *Pre-Renewal* and is what the native client expects; most content is Lua
  table (`.lub`) and item/skill/message `.txt` files, so no archive repacking
  is needed. Turn it off with *Install the English translation* in Settings → General, or
  re-apply after an update with **Apply into data**.
- **LAN play.** Set *Bind host* to `0.0.0.0` and *Advertised host* to your
  machine's LAN IP, then open the game port in your firewall. Every player
  needs their own copy of the client configured with your IP as the login
  server. The client does not provide TLS; for internet play use a
  VPN/tunnel.
- **Rebuilds.** *Install Everything* (Settings → Components) only fills in missing pieces.
  *Rebuild Everything* forces every build and rebuilds the client data.
- **Settings file.** Install and assets paths (plus every other option) are
  remembered in `ro-sppack.json` in the current working directory, rewritten
  whenever something changes. If no file is there yet, a settings file left
  next to the executable by older builds is copied over automatically on first
  run.
- **Ports in use.** Before it starts the server, the launcher checks the game,
  client data, reference and Bot API ports. If another program already listens
  on one, often a server started by hand, the game is not started and the home
  screen says which port: stop that program or change the ports in
  Settings → Server. Other startup errors from rust-ro or the client appear in
  red on the home screen too, with the details in Logs.
- **Log files.** Everything in the Logs tab is also written to
  `<install dir>/logs`: `server.log`, `client.log` and `launcher.log` (setup
  jobs and launcher messages). Lines are flushed to disk about every 10
  seconds, and the previous run's file is kept as `<name>.previous.log`.
- **Server debug log.** Tick **Server debug log** in Settings → General to start
  rust-ro with `--debug-log`, which also logs failed NPC conversations and
  other script failures into `server.log`. It applies the next time the server
  starts.

## Licensing

The launcher (this repository) is MIT licensed. It clones and builds third
party projects at run time; those keep their own licences (rust-ro and
nostalro-client are GPL-3.0-style/copyleft projects). Game assets are never
bundled and remain the property of their respective copyright holders.

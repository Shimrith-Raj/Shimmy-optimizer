# Shimmy Optimizer: notes for Claude

Loaded automatically in every session. It is the code map, so a new chat does not need to survey the repo first.
Find things by the function names below (grep them); line numbers drift. Keep this file current when the code changes.

@CONTEXT.md

Plans: `PLAN.md` (Phases 0 to 10, all built) and `PLAN_premium_features.md` (P1 to P3, the current work).

## How to add things

**A command.** In `lib.rs`: `#[tauri::command] fn name(s: State<'_, Shr>, ...) -> Result<Value, String>` (`Shr = Arc<Shared>`).
Slow work goes in `tauri::async_runtime::spawn_blocking`. Add it to `generate_handler![...]` in `run()`, or it is silently
missing. In `app.js`: add `'/api/x': 'name'` to `COMMANDS`, and call it with `api('/api/x', { camelCaseArg })`.
Tauri maps camelCase JS keys to snake_case Rust parameters. Example end to end: `get_restore_points`.

**A page.** In `index.html`: add a `<button data-tab="x">` (inline SVG and a label) to `<nav id="tabs">`, and a
`<section id="tab-x" hidden>` in `<main>`. In `app.js`: add a loader branch in `setTab(t)`, and optionally one in
`refresh()` (runs every 5 s for the visible tab). `data-goto="x"` on any element switches tabs.

**A setting.** `struct Settings` in `lib.rs` is `#[serde(rename_all = "camelCase", default)]`, so a new field needs a
`Default`, and old `settings.json` files still load. Clamp it in `auto::clamp` or `sanitize` (`lib.rs`), which runs on
the page's copy and on `settings.json` at start-up alike (any program can edit that file). **A field Shimmy writes itself must be
added to the carry-over in `set_settings`** (the "Kept by Shimmy, not by the page." block), or the page's stale copy
overwrites it. UI: `renderSettings` with the helpers `block`, `sw`, `num`, `field` and `data-set="dotted.path"`.
The change handler coerces with `Number()`, so a string select needs its own branch.

**A log entry.** `s.store.log(action, target, detail)` (`store.rs`) appends to `actions-log.csv`. `read_log` returns the
last 100 rows, newest first. `log_file()` is private.

**A notification.** `alert(app, s, title, body)` in `lib.rs` notifies and logs it. `notify()` is the bare call.

**A rule or automation.** Decision logic is pure and unit-tested with literal millisecond timestamps (`auto.rs`:
`Rule::step`, `Watch`, `update_due`, `weekly_due`; `heat.rs`: `Watch::step`; `hold.rs`). `automation()` in `lib.rs` runs
them from `start_sampler`, which ticks every 3 s. Anything slow (a disk walk or the network) is spawned on its own thread,
never run in the tick (see the daily update check).

**A Clean category.** `const DEFS: [Def; N]` in `clean.rs` (`Def {id, title, why, default_on}`), paths in `targets(id)`
(base folders only from `clean::known`, Windows' known folders, never from environment variables such as TEMP), sizes
from `scan`, and the delete in `run` through `delete_files`, which is a **permanent** `fs::remove_file`. The
Recycle Bin route is `trash::delete_all`, used by `caches.rs`.

## Where things are

- **Heat protection** (`heat.rs`): 90 °C for 30 s turns processor boost off, and 75 °C for 2 min turns it back on. It
  changes power settings only through `powercfg.exe` (`/qh`, `/setacvalueindex`, `/setdcvalueindex` and `/setactive`).
  The old values are saved in `settings.heat.saved` (`SavedBoost`). Every restore goes through `restore_boost` in
  `lib.rs`: on cool-down, when heat protection is switched off, in `exit_app` (tray Exit and update install), and at
  start-up. Shutdown, a crash or a kill are covered only by the start-up restore.
- **Sensors** (`sensors.rs`): the LibreHardwareMonitor sidecar `shimmy-sensors.exe` prints JSON rows every 2 s.
  `Shared::readings()` returns them while fresh (under 10 s). CPU temperature:
  `sensors::cpu_temp(rows)` ("Core (Tctl/Tdie)", with "Core (Tctl)" as the fallback); every reader uses it. Clocks come
  from PDH (`cpu::Perf`) into `Shared.cpu_live`. Fans come from ASUS ATK only: `Shared::fans()`. `cpu::status` is the
  pure throttle verdict.
- **DISM**: `clean::dism_analyze` and `dism_cleanup`, started from `start_dism` in `lib.rs`.
- **Finder**: `finder::scan` returns `Found {large, installers, dups, ...}` (`Item {path, name, folder, bytes, modified}`).
  It is kept in `S.finder` and returned by `get_clean`. Deletes go through `finder_delete`: the gate is
  `finder::allowed`, files bigger than `bin_capacity` return `needsConfirm`, and `allow_permanent` removes them.
- **Biggest folders**: `storage::scan_usage` builds a `Tree` (after `start_usage_scan`), and `storage_folder` returns
  `Folder {path, bytes, filesHere, children}`, capped at 40 children. Folder deletes go through `folder_delete`, gated by
  `storage::check_folder`.
- **System queries** use no WMI yet. Command-line tools run hidden through `clean::hidden(exe, args)`, or `clean::hidden_for` with a time limit for anything that can hang (winget) (powercfg,
  vssadmin, dism). Shimmy runs as administrator, so it never runs anything a standard user can plant: winget is run
  from its package folder (`programs::winget_exe`, not by name through PATH), and uninstallers only from HKLM (apps
  installed just for the user open Windows' Installed apps page, `programs::per_user`).

- **Premium P1/P2 modules**: `hw.rs` (System page), `power.rs` (power mode follows the charger, hand-linked powrprof
  overlay calls), `cool.rs` (cooling check), `programs.rs` (installed apps, winget updates), `prio.rs` (background
  priority). Restore points: `clean::restore_point` via `Shared::restore_point_first`. CSV: `store::csv_row`, `export_csv`.
  Everything Shimmy changes is put back in `exit_app` (boost, power mode, priorities).
- **Dev screenshots**: the dev build lands in `app/src-tauri/src-tauri/target-dev`, so run
  `tools/shoot.ps1 -Target 'src-tauri\target-dev'` from PowerShell (not `powershell -File`, which drops the `-Clicks`
  array), and stop that dev process afterwards or the next build fails with a locked file.

## Dependencies

`windows` 0.62.2 (features are listed in `Cargo.toml`, and a new API usually needs a new feature there), tauri 2,
tauri-plugin-notification (used from Rust only), single-instance, sysinfo, winreg, trash, nvml-wrapper, ureq, rayon,
blake3 and sha2. There is no clipboard, dialog or WMI crate. The capabilities file grants only `core:default`.

## Versions

Kept in step, by hand, in `app/src-tauri/Cargo.toml` and `app/src-tauri/tauri.conf.json`. `app/package.json` stays 1.0.0
and is not the app version.

## Tests

Run them in `app/src-tauri`: `SHIMMY_NO_ADMIN=1 CARGO_TARGET_DIR=target-dev cargo test`. That is 94 unit tests, plus 2 in
`tests/parity.rs`. The real-PC checks are `--test real_scan -- --ignored`. Every new gate or decision function gets unit
tests in the same style.

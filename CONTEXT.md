# Shimmy Optimizer: project context

Written 2026-10-27. Read this first when picking the project back up; `PLAN.md` has the full decisions and per-phase notes.

## What it is
A Windows 11 system tool for one laptop (ASUS Vivobook M3500QC: Ryzen 7 5800HS, RTX 3050 Laptop + Radeon iGPU, 15.4 GB RAM,
512 GB NVMe, battery). It replaces `Desktop\RAM Optimizer` (v0.2, Node + PowerShell) and imported its history and action log.
Built with **Tauri 2** (Rust core, vanilla HTML/CSS/JS UI). It **always runs as Administrator** and lives in the tray.

## Pages
Home (four readouts and Clean now) · Memory · Processor · Graphics · Storage · Clean · Battery · Apps · Programs · Startup · System · Log · Settings · Updates.

## Layout
| Path | Purpose |
|---|---|
| `app/src/` | UI: `index.html`, `style.css`, `app.js` (calls Rust through `invoke`; the `COMMANDS` map keeps v0.2's `/api/...` names) |
| `app/src-tauri/src/lib.rs` | Commands, shared state, tray, sampler (every 3 s), automation, settings |
| `procs.rs` | v0.2's `classify()` and `recommend()` rules, ported 1:1 and parity-tested |
| `system.rs` | Win32: processes, window titles, close, trim, memory, battery |
| `cpu.rs`, `sensors.rs` | Processor page (cores from `GetLogicalProcessorInformationEx`, effective clocks from Windows' processor counters); the LibreHardwareMonitor sidecar (`resources/sensors/shimmy-sensors.cs`, compiled by `build.rs` with Windows' own csc.exe) |
| `caches.rs` | Old caches on the Storage page (age filter, Recycle Bin, skipped while the app runs) |
| `gpu.rs` | Graphics page, any vendor: Windows' GPU Engine counters (every adapter, per engine and per app), the graphics kernel's D3DKMT readout (temperature, clocks, fan, power share; a sleeping dedicated card is never asked) and NVML for NVIDIA detail (only while a page asks), merged per adapter |
| `storage.rs`, `clean.rs`, `finder.rs`, `jobs.rs` | Storage page, safe clean and DISM, big and duplicate finder with its deletion gate, background jobs |
| `update.rs` | Updates page: reads GitHub's latest release, downloads `Shimmy-Optimizer-Setup.exe` to `%ProgramData%\Shimmy Optimizer\updates` (administrators-only), checks its SHA-256, starts it. The only network use in Shimmy, all in Rust |
| `heat.rs` | Heat protection: processor boost off at 90 °C, back on at 75 °C, through powercfg; saved and restored |
| `hold.rs` | Run from the charger: hold level, plug-in re-hold, override and drain warnings |
| `battery.rs`, `asus.rs` | Battery page; ASUS ATK (`\\.\ATKACPI`) for fans and the charge limit |
| `auto.rs` | Alerts, automatic memory clean, weekly clean: pure decision functions, unit-tested |
| `autostart.rs` | The "Shimmy Optimizer" Task Scheduler task (highest privileges, runs on battery, `--tray`) |
| `store.rs`, `startup.rs`, `tray.rs` | History and CSV log; startup entries; the live RAM ring tray icon |
| `installer-hooks.nsh` | NSIS: installs PawnIO silently, then `shimmy-optimizer.exe --autostart on`; uninstall runs `--autostart off` |
| `design/icon.svg` | Icon master (lime ring and sparkle on a near-black tile); `tauri icon` makes the set |
| `tools/shoot.ps1` | Launches a build and screenshots it, with clicks and mouse-wheel scrolling |
| `spike/` | Phase 0 experiments, kept for reference |

Processor use is refreshed only by the 3 s sampler (`Shared.cpu_live`); everything else reads that.

Data lives in `%APPDATA%\com.shimmy.optimizer\`: `settings.json`, `history.json`, `cpu-history.json`, `actions-log.csv`.

## Building
- **Installer:** `cd app` then `CARGO_TARGET_DIR=target-ship npx tauri build` (the Tauri CLI resolves the folder from `src-tauri`, so
  `src-tauri/target-ship` would land in `src-tauri/src-tauri/target-ship`). Output: `src-tauri/target-ship/release/bundle/nsis/Shimmy Optimizer_<version>_x64-setup.exe`, unsigned, per-machine.
- **Publishing:** installers never go in the repo. Attach the installer to a GitHub release (tag `v<version>`) renamed to
  exactly `Shimmy-Optimizer-Setup.exe`: the download buttons in `README.md` and `DOWNLOAD.md` link to
  `releases/latest/download/Shimmy-Optimizer-Setup.exe`, so any other name breaks them.
- **Dev build:** `SHIMMY_NO_ADMIN=1 CARGO_TARGET_DIR=src-tauri/target-dev npx tauri build --no-bundle --config '{"identifier":"com.shimmy.optimizer.dev"}'`.
  It has no admin manifest and its own data folder, so it runs beside the real Shimmy. It has no CPU temperature, and Start with Windows is refused.
- **Tests:** in `app/src-tauri`, `SHIMMY_NO_ADMIN=1 CARGO_TARGET_DIR=target-dev cargo test` (96 tests). Real-PC checks: add `--test real_scan -- --ignored`. They read only, except `set_limit`, which
  writes the charge limit and runs only when `SHIMMY_LIMIT` is set.
- The frontend is embedded in the exe, so UI changes need a rebuild. An elevated Shimmy that is running locks its exe: quit it from the tray first.

## Safety rules (do not loosen)
- `classify()` in Rust is the only gate for closing processes. It protects system processes, services (session 0), security software, Claude, terminals, editors, Salon Shaila, and Shimmy with its WebView.
- File deletes go to the Recycle Bin. Anything too big for the bin needs a second confirmation. `finder::allowed` re-checks every path against these rules: scanned roots, code projects, Salon Shaila and invoices, hidden or system files, the Keep list, the user's exclusions.
- Everything destructive or automatic is logged to `actions-log.csv`.

## State
Phases 0 to 10 are done and merged (v0.5.1, 2026-10-28). `PLAN_premium_features.md` P1 and P2 are built, bug-swept and merged (v0.8.0, 2026-10-30); P3 is next. Still open: the taste-skill inspection pass on the UI. See `PLAN.md` § Progress.

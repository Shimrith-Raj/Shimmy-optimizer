# Shimmy Optimizer: premium features, free (plan)

Written 2026-10-29. This follows `PLAN.md` (Phases 0 to 10). Read `CONTEXT.md` first for the code map.

Paid Windows optimizers charge about $15 to $70 a year for their Pro tiers. This plan brings the safe, useful parts of those
tiers into Shimmy for free, in Shimmy's own style: plain language, everything logged, everything reversible, and nothing
changes without a click.

**Only the safe features (decided 2026-10-29).** Registry cleaning, driver updaters, game boost, silent auto-updates,
network "boosters", tweak scans, freezing background apps, and defrag or wiping stay in `PLAN.md`'s "Not in scope".

## Research (2026-10-29, do not redo)

Checked against the official pricing and comparison pages, opened in a real browser. CCleaner and AVG blocked every
request, so their rows come from reviews.

| App | Paid tier (list, promo in brackets) | Locked behind payment (relevant here) |
|---|---|---|
| CCleaner Pro / Pro Plus | about $30–60 a year (unverified) | scheduled and real-time cleaning, driver updater, software updater, Speccy, Defraggler, Recuva |
| Advanced SystemCare Pro / Ultimate | $29.99 a year ($16.77) / $36.99 ($25.99) | automatic RAM release (manual is free), deep registry clean, software update, anti-tracking |
| Wise Care 365 Pro | $39.95 a year for 3 PCs ($19.97) | privacy protection, real-time monitoring, silent auto-clean, faster startup |
| Glary Utilities Pro | $39.95 a year for 3 PCs | scheduled maintenance, deep clean, software updates |
| Avast Cleanup Premium | $65.99 a year ($29.88 first year) | Sleep Mode (freezing background apps), automatic maintenance, software and driver updater |
| AVG TuneUp | unverified | Sleep Mode, automatic maintenance, updaters |
| Norton Utilities Ultimate | $69.99 ($49.99) | registry clean, startup, privacy, optimization |
| System Mechanic | $54.95 to $84.95 | NetBooster, LiveBoost, DriveScrubber (Ultimate) |
| Process Lasso | $39.95 lifetime or $24.95 a year | Application Power Profiles, CPU Limiter, Watchdog, Keep Running (priorities, affinity, ProBalance and Efficiency Mode are free) |
| HWiNFO Pro | $29 first year | logging and reporting beyond the free limits, commercial use |
| AIDA64 Extreme | $65.95 for 3 PCs | stress tests, sensor panel, reports |
| TreeSize Personal / Pro | $25.20 a year / from $49 a year | duplicate search, largest-files filter, scheduler, exports |
| Duplicate Cleaner Pro | $54.95 | full duplicate finder |
| Revo Uninstaller Pro | $24.95 ($12.47) | forced uninstall, batch uninstall, install monitor, backups |
| GlassWire | $35.88 a year | history beyond 24 h, export, firewall |

**Shimmy already gives these away free:** the automatic RAM clean (needs 10a's Safe marks), scheduled cleaning, the
duplicate and big-file finder, the startup manager, old caches and Windows Update leftovers, battery health and the
charge limit (no paid suite even offers this), and no nags or licence. The commercial-use rights that HWiNFO, WizTree and
TreeSize sell are already granted by the MIT licence.

## Rules for everything in this plan

1. **Ask first.** Nothing changes the system without a confirm, except what you have switched on in Settings yourself.
2. **Backup, then change, then Undo.** Every system change records the old value first, in `settings.json`. There is an Undo button, and the change is restored on
   Exit where it is temporary (as heat protection already does).
3. **Logged.** Every change goes into `actions-log.csv` with its old and new value.
4. **Honest copy.** If a feature does not make the PC faster, Shimmy does not say it does. Measured gains are quoted only
   when measured on the Vivobook, as Phase 9 did for boost off.
5. **Existing gates stay.** `classify()` gates closing processes, and `finder::allowed` and
   `storage::check_folder` gate deleting. New features get their own unit-tested gates in the same style.
6. **Antivirus reputation.** The build is unsigned, so nothing here writes the registry in bulk, suspends processes or
   changes the network.

## Features

| # | Feature | Paid in | Effort | How |
|---|---|---|---|---|
| A1 | **System info** page with Copy | CCleaner Pro Plus (Speccy) | S | Read-only WMI: `Win32_Processor`, `Win32_PhysicalMemory` (each stick, speed, slots free), `Win32_VideoController`, `Win32_DiskDrive`, `Win32_BaseBoard`, `Win32_BIOS`, Windows build. "Copy for a bug report" puts a plain-text summary on the clipboard. |
| A2 | **Power mode follows the charger** | Process Lasso Pro (Application Power Profiles) | S | Choose a mode for the charger and one for battery, through `PowerSetActiveOverlayPowerScheme`. Heat protection keeps priority: while boost is off, a switch waits until it is restored. |
| A3 | **Restore point first** | Wise Care Pro, Revo Pro | S | `SRSetRestorePointW` before DISM clean-up and any delete bigger than the Recycle Bin. Windows' one-per-24 h limit is accepted, and the Log says when a point was skipped. |
| A4 | **Export to CSV** | TreeSize Pro, GlassWire, CCleaner Pro | S | Finder results, the biggest folders and the Log. |
| A5 | **Cooling check**, a stress test | AIDA64 | S | You start it, and it runs 60 to 120 s of all-thread load. It stops at 95 °C or when you press Stop, then shows peak temperature, clocks, fan speed and whether it throttled. Logged. |
| A6 | **Junk alert** | CCleaner Pro, Wise Care Pro | S | A notification when cleanable junk passes X GB (Settings), checked hourly from `clean::scan` sizes. |
| A7 | **Clear Recent items and Jump Lists** | Wise, Glary, CCleaner Pro, ASC Pro | S | A Clean category through the Recycle Bin route. Browser history, cookies and passwords stay out, as the cache promise already says. |
| A8 | **App updates**, ask first | CCleaner Pro, ASC Pro, Glary Pro, Avast | M | List from `winget upgrade` (parsed). One Update button per app runs `winget upgrade --id <id> --exact` after a confirm, logged. Hidden when winget is missing. |
| A9 | **Installed apps** by size, with each app's own uninstaller | Revo Pro (batch, monitor) | M | Read-only list from the `Uninstall` registry keys (`EstimatedSize`, date, publisher). Uninstall runs the app's own `UninstallString` after a confirm. |
| A10 | **Sensor log** and a `--report` snapshot | HWiNFO Pro, AIDA64 | M | CSV of chosen sensors at an interval, and `shimmy-optimizer.exe --report` prints a one-page text summary. |
| A11 | **Background priority** for chosen apps | Process Lasso (free there, a "Pro" item elsewhere) | M | Below Normal or Idle through `SetPriorityClass`, plus Efficiency mode (EcoQoS, `SetProcessInformation` `ProcessPowerThrottling`). Reverted on Exit, logged. |
| A12 | **Connections now**, read-only | GlassWire Premium (history, export) | M | Which app holds which connection, from `GetExtendedTcpTable` and `GetExtendedUdpTable`. No driver, no firewall changes. |

## Phases

| # | What | Release |
|---|---|---|
| P1 | **Quick wins:** A1 System info, A2 Power mode follows the charger, A3 Restore point first, A4 Export, A5 Cooling check, A6 Junk alert, A7 Recent items | v0.7.0 |
| P2 | **Apps:** A8 App updates, A9 Installed apps, A11 Background priority | v0.8.0 |
| P3 | **Monitoring extras:** A10 Sensor log and report, A12 Connections now | v0.9.0 |

Where a Windows API's behaviour is not yet proven, a phase starts with a short **Step 1 test on the Vivobook**, as Phases 7
and 9 did. The API names here are from the docs and still need checking against the `windows` crate version in `Cargo.toml`.
Each phase ends with unit tests for any new gate, an installer, and screenshots.

**Before P1:** merge Phase 10. 10a (marks), 10b+10c and 10d sit on three branches with overlapping version numbers and
changes to `lib.rs`, `app.js` and `PLAN.md`.

## P1 Step 1 results (run 2026-10-29, on battery, not elevated)

- **A2:** `PowerSetActiveOverlayPowerScheme` does not exist. `powrprof.dll` exports `PowerSetActiveOverlayScheme(GUID)`,
  `PowerGetEffectiveOverlayScheme`, `PowerGetActualOverlayScheme` and `PowerGetOverlaySchemes`, and the `windows` crate
  has none of them, so they are linked by hand. Setting Best performance (`ded574b5-…`) and back returned 0 without
  elevation, and the base plan stayed Balanced. The overlay survives heat protection's `powercfg /setactive`, so the two
  do not fight. Overlay GUIDs: Best efficiency `961cc777-2547-4f9d-8174-7d86181b8a7a`, Balanced is the empty GUID, Best
  performance `ded574b5-45a0-4f42-8737-46345c09c238`.
- **A2 defaults (owner):** Best performance on the charger, Balanced on battery. Off until switched on.
- **A3:** not testable without elevation. `SRSetRestorePointW` needs the `Win32_System_Restore` feature. Failures
  (protection off, one-per-24 h) are logged and shown, and the clean-up still runs.

## P1 built (2026-10-29, released in v0.8.0)

- **A1** `hw.rs` + System page: one hidden PowerShell CIM query (about 5 s, kept until the window closes), VRAM from the
  display class keys, drives from `Get-PhysicalDisk`. No serials. Copy uses the WebView clipboard. Sticks are numbered by
  Shimmy because this BIOS calls both "DIMM 0".
- **A2** `power.rs`: `PowerPrefs` in settings (off by default), `power_watch` in `automation`, restore in `exit_app`, at
  start-up when follow is off, and when switched off.
- **A3** `clean::restore_point` + `restore_point_first`: before DISM clean-up, big finder deletes and folder deletes for
  good. Shimmy keeps to one a day (`restore_point_at`). Not yet run elevated: check the Log after the first real use.
- **A4** `export_csv`: finder results, the current biggest-folders view and the whole Log, to Downloads (UTF-8 with BOM).
- **A5** `cool.rs` + Processor page: refused without a temperature or above 85 °C. Not run on the real elevated build yet.
- **A6** `alerts.junkGb` (default 5 GB, 0 is off): hourly scan on its own thread, at most once a day.
- **A7** clean category `recent`, off by default, to the Recycle Bin, keeps Explorer's Quick access jump list.

## P2 built (2026-10-30, released in v0.8.0)

- **Step 1:** winget 1.29 lists 24 updates in about 5 s; 66 apps in the Uninstall keys (46 with a size, 12 MSI); install
  dates come as `20261028` or `5/13/2026`.
- **A9** `programs.rs` + Programs page: installed apps by size. The page sends only the registry key; Rust re-reads the
  uninstall command, turns `msiexec /I` into `/X`, refuses Shimmy itself, and runs one uninstaller at a time.
- **A8** winget table parsed by header word positions (language-independent), cut-off ids dropped, ids checked by regex
  and against the last list. One update at a time, logged. Hidden when winget is missing.
- **A11** `prio.rs`: Below normal, Low, Efficiency mode (Low + EcoQoS). Set per app in its expanded row on the Apps page,
  same built-in gate as Safe marks, applied every 30 s to new processes, put back on Exit. Not reverted after a crash
  (Windows keeps a process's priority until it exits).

## P3 Step 1 results (run 2026-10-30, not elevated)

- **A12:** `GetExtendedTcpTable` and `GetExtendedUdpTable` are in the `windows` crate (`Win32_NetworkManagement_IpHelper`).
  The tables gave 117 sockets in about 3 ms and matched `netstat -ano` exactly: 39 connected, 27 listening, 9 closing, 42 UDP.

## P3 built (2026-10-30, v0.9.0, branch `premium-p3`, not committed or released)

- **A12** `net.rs` + Connections page: every TCP and UDP socket, IPv4 and IPv6, by app, with the internet first. Addresses are
  not looked up, so the page makes no network use, and traffic is not measured. Sockets of this PC talking to itself are
  hidden behind a tick. Also exported as CSV. Refreshes every 5 s, only while the page is open.
- **A10 sensor log** `senselog.rs` + System page: ticked readings to a new CSV in Downloads, every 2 s to 5 min, up to 40
  columns. Every row is flushed. It stops on Stop, on Exit, on a write error or at 100 MB, and does not resume after a
  restart. Graphics chips are left out, because the sensor reader leaves them out to let the RTX sleep. Shimmy's own
  readings (use, clock, memory, battery, fans) work without administrator rights.
- **A10 report** `report.rs`: `--report` and Save a report on the System page. No serials, user name or addresses. It takes
  about 15 s from the command line, because it reads the inventory and starts its own sensor reader. It prints to a redirect,
  a pipe or the terminal that started it. A windowed program does not hold the prompt, so the prompt comes back first and
  the report follows. With nowhere to print, it saves to Downloads and shows the file.
- **Not yet run elevated:** temperatures and package power in the log and report. From a non-elevated terminal the installed
  build asks for elevation and has no console to borrow, so that path should save to Downloads: check it on first real use.

## Risks

1. **Overpromising.** The copy rule (rule 4) applies to the README too. "Pro features, free" is the pitch, not "faster PC".
2. **A2 and heat protection** both change power settings. Heat protection keeps priority, and both are restored on every exit path.
3. **A5 runs the processor hot on purpose.** It stops at 95 °C, after 120 s at most, or on Stop, and never runs on its own.
4. **A8 and A9 run other programs' installers and uninstallers.** Each runs only after a confirm, one at a time, and is logged.

## Questions for the owner
1. A2: which power mode for the charger and which for battery by default?
2. A8: is `winget` acceptable, given it is missing on some older Windows 10 installs (the feature hides there)?

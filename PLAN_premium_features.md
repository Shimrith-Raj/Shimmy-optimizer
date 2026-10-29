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

## Risks

1. **Overpromising.** The copy rule (rule 4) applies to the README too. "Pro features, free" is the pitch, not "faster PC".
2. **A2 and heat protection** both change power settings. Heat protection keeps priority, and both are restored on every exit path.
3. **A5 runs the processor hot on purpose.** It stops at 95 °C, after 120 s at most, or on Stop, and never runs on its own.
4. **A8 and A9 run other programs' installers and uninstallers.** Each runs only after a confirm, one at a time, and is logged.

## Questions for the owner
1. A2: which power mode for the charger and which for battery by default?
2. A8: is `winget` acceptable, given it is missing on some older Windows 10 installs (the feature hides there)?

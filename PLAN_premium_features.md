# Shimmy Optimizer: premium features, free (plan)

Written 2026-10-29. This follows `PLAN.md` (Phases 0 to 10). Read `CONTEXT.md` first for the code map.

Paid Windows optimizers charge about $15 to $70 a year for their Pro tiers. This plan brings the useful parts of those
tiers into Shimmy for free, in Shimmy's own style: plain language, everything logged, everything reversible, nothing silent
unless you switch it on. It has two parts:
- **Part A:** features that are safe and worth having as they are.
- **Part B:** the features Shimmy has kept out until now (registry cleaning, driver updates, game boost, automatic
  updates, network tweaks, tweak scans, freezing background apps, defrag and wiping). They are **done the honest way**:
  each gets the narrow version that really works, a backup before any change, an Undo, and copy that never promises speed it
  cannot deliver. Building Part B means editing `PLAN.md`'s "Not in scope" list, one line per feature, as each one ships.

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
2. **Backup, then change, then Undo.** Every system change records the old value first, in `settings.json` or a `.reg`
   or file backup under `%APPDATA%\com.shimmy.optimizer\backups\`. There is an Undo button, and the change is restored on
   Exit where it is temporary (as heat protection already does).
3. **Logged.** Every change goes into `actions-log.csv` with its old and new value.
4. **Honest copy.** If a feature does not make the PC faster, Shimmy does not say it does. Measured gains are quoted only
   when measured on the Vivobook, as Phase 9 did for boost off.
5. **Existing gates stay.** `classify()` gates closing and freezing processes, and `finder::allowed` and
   `storage::check_folder` gate deleting. New features get their own unit-tested gates in the same style.
6. **Antivirus reputation.** The build is unsigned, so bulk registry writes, process suspension and network changes are
   kept narrow, run only on a click, and never run hidden at start-up.

## Part A: safe features

| # | Feature | Paid in | Effort | How |
|---|---|---|---|---|
| A1 | **System info** page with Copy | CCleaner Pro Plus (Speccy) | S | Read-only WMI: `Win32_Processor`, `Win32_PhysicalMemory` (each stick, speed, slots free), `Win32_VideoController`, `Win32_DiskDrive`, `Win32_BaseBoard`, `Win32_BIOS`, Windows build. "Copy for a bug report" puts a plain-text summary on the clipboard. |
| A2 | **Power mode follows the charger** | Process Lasso Pro (Application Power Profiles) | S | Choose a mode for the charger and one for battery, through `PowerSetActiveOverlayPowerScheme`. Heat protection keeps priority: while boost is off, a switch waits until it is restored. |
| A3 | **Restore point first** | Wise Care Pro, Revo Pro | S | `SRSetRestorePointW` before DISM clean-up, any Part B change and any delete bigger than the Recycle Bin. Windows' one-per-24 h limit is accepted, and the Log says when a point was skipped. |
| A4 | **Export to CSV** | TreeSize Pro, GlassWire, CCleaner Pro | S | Finder results, the biggest folders and the Log. |
| A5 | **Cooling check**, a stress test | AIDA64 | S | You start it, and it runs 60 to 120 s of all-thread load. It stops at 95 °C or when you press Stop, then shows peak temperature, clocks, fan speed and whether it throttled. Logged. |
| A6 | **Junk alert** | CCleaner Pro, Wise Care Pro | S | A notification when cleanable junk passes X GB (Settings), checked hourly from `clean::scan` sizes. |
| A7 | **Clear Recent items and Jump Lists** | Wise, Glary, CCleaner Pro, ASC Pro | S | A Clean category through the Recycle Bin route. Browser history, cookies and passwords stay out, as the cache promise already says. |
| A8 | **App updates**, ask first | CCleaner Pro, ASC Pro, Glary Pro, Avast | M | List from `winget upgrade` (parsed). One Update button per app runs `winget upgrade --id <id> --exact` after a confirm, logged. Hidden when winget is missing. |
| A9 | **Installed apps** by size, with each app's own uninstaller | Revo Pro (batch, monitor) | M | Read-only list from the `Uninstall` registry keys (`EstimatedSize`, date, publisher). Uninstall runs the app's own `UninstallString` after a confirm. |
| A10 | **Sensor log** and a `--report` snapshot | HWiNFO Pro, AIDA64 | M | CSV of chosen sensors at an interval, and `shimmy-optimizer.exe --report` prints a one-page text summary. |
| A11 | **Background priority** for chosen apps | Process Lasso (free there, a "Pro" item elsewhere) | M | Below Normal or Idle through `SetPriorityClass`, plus Efficiency mode (EcoQoS, `SetProcessInformation` `ProcessPowerThrottling`). Reverted on Exit, logged. |
| A12 | **Connections now**, read-only | GlassWire Premium (history, export) | M | Which app holds which connection, from `GetExtendedTcpTable` and `GetExtendedUdpTable`. No driver, no firewall changes. |

## Part B: the kept-out features, done the honest way

### B1 Registry tidy-up (not a "registry cleaner")
Paid in: Advanced SystemCare, Wise, Glary, CCleaner, Norton, Iolo.
- **Scope:** only entries that are provably broken, each checked by a pure function with tests. A broken entry is one of:
  - an `Uninstall` entry whose install folder and uninstaller are both gone
  - a Run or RunOnce entry pointing to a missing exe
  - an App Paths entry pointing to a missing exe
  - a file association pointing to a missing program
  - a `MUICache` or `SharedDLLs` entry for a missing file
- **Never touched:** COM/CLSID, services, drivers, policies, `HKLM\SYSTEM`, anything owned by Microsoft, Shimmy or security
  software, or anything else outside the list above.
- **How:** a scan lists each item with why it is broken. Before a delete, `reg export` writes a backup for that key and a
  restore point is made (A3). One Undo imports the backup again.
- **Copy:** "Tidies leftovers from removed apps. It will not make Windows faster." Microsoft's position on registry cleaners
  is linked.

### B2 Driver updates from Windows Update only
Paid in: CCleaner Pro, IObit Driver Booster, AVG, Glary, Ashampoo.
- **Source:** the Windows Update Agent API (`IUpdateSearcher` with `IsInstalled=0 and Type='Driver'`) lists the drivers
  Microsoft has published for this PC's hardware IDs. **No third-party driver database.**
- **Install:** one driver at a time after a confirm, with a restore point first, through `IUpdateInstaller`, and logged. Rolling back
  opens Device Manager's Roll Back Driver for that device.
- **Graphics:** a "Newer graphics driver from NVIDIA / AMD / Intel" link to the vendor's own page and app, read from the
  driver version that 10d already shows. Shimmy never downloads GPU drivers itself.
- **ASUS:** a link to the model's support page (the model is already read in `asus.rs`).

### B3 Game mode (not "game boost")
Paid in: Advanced SystemCare (unverified placement), System Mechanic LiveBoost.
- **Trigger:** a full-screen game, detected by `SHQueryUserNotificationState` (`QUNS_RUNNING_D3D_FULL_SCREEN`), or a list of
  games you choose. Switch in Settings, off by default.
- **While a game runs:**
  - the charger's power mode (A2) is set to Performance
  - the weekly clean, finder and cache scans and the automatic memory clean are paused
  - apps you marked Safe drop to Efficiency mode (A11)
  - Shimmy's own sampler slows to every 10 s
  - heat protection stays on
- **Never:** killing services, closing apps, changing the network, or touching Windows' own Game Mode.
- **Afterwards:** everything is put back when the game ends, and on Exit. Logged as `game-mode-on` and `game-mode-off`.
- **Copy:** "Keeps Shimmy and your background apps out of the way while you play." The gain, if any, is measured on the Vivobook
  before it is quoted.

### B4 Automatic updates (opt-in)
Builds on 10b's Updates page.
- **Switch:** "Install Shimmy updates automatically", **off by default**.
- **When on:** a new release is downloaded only while plugged in. It installs only if its SHA-256 matches, at the next
  moment you are idle for 10 minutes and no game is running, or when you choose Exit. The installer runs passive (progress
  shown, no questions), and Shimmy restarts in the tray afterwards.
- **Logged** as `update-auto`. After installing, a notification says what changed.
- Silent updates of **other apps** stay out. A8 always asks.

### B5 Network: honest tools, not an "internet booster"
Paid in: Advanced SystemCare (unverified), Norton, System Mechanic NetBooster.
- **DNS check:** times your current DNS against a short list (Cloudflare, Google, Quad9, your router) with a few lookups
  each, and offers to switch the active adapter to the fastest, using `SetInterfaceDnsSettings` (Windows 10 2004+). The old
  setting is saved for Undo. Nothing changes without a click.
- **Network health:** shows TCP auto-tuning, the RSS and ECN state, and the adapter's link speed and power-saving setting.
  If a setting differs from Windows' default (auto-tuning "disabled", often left by old boosters), it offers to put the
  default back. Shimmy never sets non-default values.
- **Reset network:** a troubleshooting button that runs `netsh winsock reset` and `netsh int ip reset` after a confirm, and
  says a restart is needed.
- **Copy:** never promises faster internet. DNS lookups can be faster, and the rest is repair.

### B6 Tweaks: a short, reversible list, not a "deep optimize scan"
Paid in: Advanced SystemCare, Glary, System Mechanic.
- A **fixed, curated list** in `tweaks.rs`. Each tweak has what it does, what you lose, its registry or setting path, the
  value it sets, and the original value it saves. Candidates, each kept only if it holds up on the Vivobook:
  - background apps off
  - Game DVR background recording off
  - visual effects set to performance
  - start-up delay for Run entries removed
  - tips and suggestions notifications off
  - hibernation (already on the Storage page)
- Each is one switch with Undo. There is no "scan found 214 problems" screen and no score.

### B7 Freeze background apps (opt-in, per app)
Paid in: Avast Cleanup Premium and AVG TuneUp (Sleep Mode).
- **Which apps:** only apps you mark **Freeze**. The mark is a fourth mark beside 10a's Protected, Safe and Default, and it
  is never set by default. `classify()` refuses Protected and system apps, and the existing protection list (security
  software, terminals, editors, Claude, Salon Shaila, Shimmy) can never be frozen.
- **When:** after the app has had no window in front for N minutes (Settings, default 10), and only if it is not playing
  sound, checked with `IAudioSessionManager2`. Freezing uses Efficiency mode first (A11). Full suspension
  (`NtSuspendProcess`) is a second, separate tick per app.
- **Thawing:** resumed the moment you open it from the taskbar or tray (a foreground-window hook), at Exit, at start-up for
  anything left frozen by a crash (saved in `settings.json`), and on a "Thaw all" button in the tray. Logged both ways.
- **Copy:** the risk is stated plainly ("frozen apps miss messages and calls until you open them").

### B8 Drive optimise and wiping, matched to the drive
Paid in: CCleaner Pro Plus (Defraggler), Ashampoo, IObit, Wise, System Mechanic DriveScrubber.
- **Optimise:** reads the media type (`MSFT_PhysicalDisk.MediaType`). An **HDD** gets defragmentation, an **SSD** gets
  TRIM (retrim). Both go through Windows' own engine (`defrag <drive> /O`, with progress from `/U`). Shimmy writes no
  defragmenter of its own.
- **Shred files:**
  - on an HDD, the file is overwritten once and then deleted, through the deletion gate, with a second confirm
  - on an SSD, Shimmy says plainly that overwriting cannot be guaranteed, deletes the file for good, then runs retrim.
    BitLocker (device encryption) is pointed to as the real protection
- **Wipe free space:** on an HDD it runs `cipher /w`. On an SSD it runs retrim with the same honest note.
- Logged, and never part of any scheduled clean.

## Phases

| # | What | Release |
|---|---|---|
| P1 | **Quick wins:** A1 System info, A2 Power mode follows the charger, A3 Restore point first, A4 Export, A5 Cooling check, A6 Junk alert, A7 Recent items | v0.7.0 |
| P2 | **Apps:** A8 App updates, A9 Installed apps, A11 Background priority, B4 Automatic Shimmy updates | v0.8.0 |
| P3 | **Play and drives:** B3 Game mode (needs A2 and A11), B7 Freeze background apps (needs 10a's marks), B8 Drive optimise and wiping | v0.9.0 |
| P4 | **System:** B1 Registry tidy-up, B2 Driver updates, B5 Network tools, B6 Tweaks (all need A3) | v0.10.0 |
| P5 | **Monitoring extras:** A10 Sensor log and report, A12 Connections now | v0.11.0 |

Each phase starts with a short **Step 1 firmware/OS test on the Vivobook** where a Windows API's behaviour is not yet
proven, as Phases 7 and 9 did. The API names here are from the docs and still need checking against the `windows` crate version in
`Cargo.toml`. Each phase ends with unit tests for the new gates, an installer, and screenshots, and with the matching line removed from
`PLAN.md`'s "Not in scope".

**Before P1:** merge Phase 10. 10a (marks), 10b+10c and 10d sit on three branches with overlapping version numbers and
changes to `lib.rs`, `app.js` and `PLAN.md`.

## Risks

1. **Antivirus flags.** B1, B5, B7 and B8 are the behaviours heuristics watch for (registry deletes, network changes, process
   suspension, raw overwrites). Each runs only on a click (B7 only for apps you marked), never at start-up, and is listed
   in the Log. Each phase's installer gets a VirusTotal check before release, and a false-positive report is sent to
   Microsoft if needed.
2. **Breaking something.** Every Part B change is backed up and has an Undo, and B1, B2, B5 and B6 also make a restore point.
   B7 thaws on every exit path.
3. **Overpromising.** The copy rule (rule 4) applies to the README too. "Pro features, free" is the pitch, not "faster PC".
4. **Unsigned automatic updates (B4).** The installer is checked against GitHub's own checksum. A compromised GitHub
   account would still win, so B4 stays off by default, and signing remains the real fix.

## Questions for the owner
1. Should Part B ship at all, or only Part A? This plan assumes both, in the phase order above.
2. B7 full suspension: offer it as a second tick, or Efficiency mode only?
3. B4: is off by default right?
4. B6: which tweaks to keep, once each is measured?

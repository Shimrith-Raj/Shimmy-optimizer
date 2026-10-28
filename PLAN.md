# Shimmy Optimizer: build plan

Written 2026-10-27. Successor to `Desktop\RAM Optimizer` (v0.2). Windows 11 only, built for one machine first:
ASUS Vivobook M3500QC · Ryzen 7 5800HS (8C/16T) · RTX 3050 Laptop + Radeon iGPU · 15.4 GB RAM · Intel 512 GB NVMe (110 GB free) · battery.

## Decisions (from the grilling)

| Topic | Decision |
|---|---|
| Framework | **Tauri 2 (Rust)**. UI carried over; `server.js` rewritten as Rust commands. |
| Privileges | **Always runs as Administrator** (`requireAdministrator` manifest). Starts at sign-in through a Task Scheduler task with highest privileges, so no UAC prompt at boot. |
| CPU health | **Bundle LibreHardwareMonitor + signed PawnIO driver** for real Ryzen temps, clocks, power and fans; RTX 3050 through NVIDIA's NVML. |
| Cleanup scope | Temp & caches, browser caches, Windows Update leftovers, big & duplicate file finder (review only). |
| Deleting | **Recycle Bin always.** Items too big for the bin are flagged and need a second explicit confirmation. |
| Tray | Left-click **opens the window**. Right-click menu: Clean now, Trim, Open, Settings, Exit. Closing the window hides it to the tray. |
| Automation | Alerts, auto-clean RAM, weekly storage clean, ASUS battery charge limit. Each one has its own switch. |
| Audience | **Just me.** Unsigned NSIS installer (SmartScreen warns once), no auto-updater. |
| Layout | **Left rail + Home** with four big readouts and one Clean button. |
| Old app | **Replace and import** its history and action log; retire the old tray; leave the folder until you delete it. |
| Icon | **Shimmer ring:** a thick lime ring on a near-black tile with a four-point sparkle cut in at top right. In the tray the ring fills live with RAM %. |

## Architecture

```
Shimmy Optimizer.exe  (Tauri 2, elevated)
├─ WebView2 window: the UI (vanilla HTML/CSS/JS, today's design tokens)
│    calls Rust with invoke("get_state"), invoke("quick_clean"), ...; live data pushed by events
├─ Rust core
│   ├─ procs      sysinfo + EnumWindows (titles) → classify() / recommend()   (ported 1:1 from server.js)
│   ├─ memory     GlobalMemoryStatusEx, EmptyWorkingSet, standby-list purge (NtSetSystemInformation, admin)
│   ├─ cpu        sysinfo per-core load + clocks; temps/power from the sensor sidecar
│   ├─ gpu        nvml-wrapper (RTX 3050 temp, load, VRAM); falls back to nvidia-smi
│   ├─ storage    volumes, SMART/wear (MSFT_PhysicalDisk + StorageReliabilityCounter), folder sizes, cleanup engine
│   ├─ battery    GetSystemPowerStatus (charge %, plugged, charging), sidecar LHM (capacities, rate), powercfg /batteryreport
│   │             (cycles, history), ASUS ATK \.\ATKACPI charge limit. root\wmi failed in Phase 0 and is not used.
│   ├─ startup    winreg (StartupApproved, same as today)
│   ├─ scheduler  history sampler, alerts, auto-clean, weekly clean
│   ├─ store      %ProgramData%\Shimmy Optimizer\{settings.json, history.sqlite, actions-log.csv}
│   └─ tray       dynamic ring icon drawn with tiny-skia, redrawn when RAM % changes
└─ shimmy-sensors (sidecar): LibreHardwareMonitorLib + PawnIO, JSON lines over stdout every 2 s
```

Tauri plugins: `single-instance`, `notification`, `dialog`, `shell` (sidecar). Crates: `sysinfo`, `windows`, `wmi`, `winreg`,
`nvml-wrapper`, `trash`, `jwalk` (fast directory walk), `blake3` (duplicate hashing), `tiny-skia`, `rusqlite`, `serde`.

**Security model** (more important now that it is elevated): no HTTP server and no token any more, because the UI talks to Rust over Tauri IPC
with a capability allow-list. `classify()` stays the single gatekeeper and is enforced in Rust on every close, not in the UI.
New protections on top of today's list: anything in session 0 (services), Shimmy and its sidecar, and security software.
Every destructive action is logged with original paths so it can be undone from the Recycle Bin.

## What each section shows

**Home**: four readouts at display size (RAM %, CPU °C, C: free, battery health %), each coloured lime/amber/red and each opening
its section; one Clean now button; one sentence naming the single biggest problem right now.

**RAM**: today's Overview: percentage, memory map, history chart, "What to close", Trim; plus a standby-list flush (admin).

**CPU**: package temperature with a history chart, per-core load as a row of cells (same language as the memory map), effective clock vs
base, package power, fan RPM if exposed, a **throttling detector** (high temperature plus a clock drop under load), top CPU-hungry apps
with Close, and the RTX 3050 temperature and load.

**Storage**: C: as one big bar coloured by category (apps, user files, system, reclaimable); drive health from SMART (wear %,
temperature, power-on hours, media errors); the largest folders in your profile, drillable.

**Clean**:
- *Safe clean* (scan shows size per category, tick and run): user and Windows temp, thumbnail cache, crash dumps, Delivery
  Optimization cache, Recycle Bin, browser caches for Brave/Chrome/Edge (cache only, never history, cookies or passwords; skipped while
  that browser is open).
- *Windows Update leftovers*: `SoftwareDistribution\Download` (pauses wuauserv during the clean) and `DISM /StartComponentCleanup`,
  analysed first with `/AnalyzeComponentStore` so the real size is shown.
- *Big & duplicate finder*: scans Downloads, Desktop, Documents, Videos and Pictures. Large files (≥ 500 MB, adjustable), old installers,
  and duplicates (same size → first-64 KB hash → full blake3). Nothing is pre-ticked; you choose; everything goes to the Recycle Bin.
  **Hard exclusions** (from your CLAUDE.md): `Salon Shaila` anywhere, any folder with `.git`, `package.json`, `pyproject.toml` or
  `node_modules`, AppData, Program Files, Windows, hidden/system files, `desktop.ini`, and the Keep list (3uTools/3uAirpalyer installers,
  `Cowork data 2026-10-11.zip`, the Physics textbook PDFs). `(1)`/`- Copy` files count as duplicates only when the original exists;
  `U4_*_1/_2` papers are compared by hash, never by name.

**Battery**: health % (full-charge capacity ÷ design capacity), cycle count, current charge/discharge rate in watts, time remaining,
a wear history chart (sampled daily), the `powercfg /batteryreport` history imported, and the **charge limit** switch (60/80/100 %).

**Apps · Startup · Log**: as today, restyled into the rail layout. The Log also lists cleanup runs and alerts.

**Settings**: alert thresholds, auto-clean level and duration, weekly clean day, start with Windows, finder folders and exclusions.

## Design

The same system as RAM Optimizer v0.2: near-black ground, tone-step surfaces, one ink, the lime accent `#c6f432`, amber/red levels,
Space Grotesk (now **bundled locally**, not fetched from Google), square corners, no gradients, glows, pills or emoji, and the taste-skill
house rules. The rail is plain text with small line icons, and the active item is shown as an ink block (like today's active tab). Charts stay
hand-drawn canvas. Window: 1200×800 default, minimum 900×600, a custom dark title bar with Windows' own min/max/close.

**Icon set** (master SVG drawn by hand, then `tauri icon` produces .ico/.png at every size):
- App icon (16→256 px, plus 1024 px master): near-black rounded tile, thick lime ring at about 70 % fill, four-point sparkle cut into the gap at top right.
- Tray icon: no tile, ring only, drawn live at 16/20/24/32 px for the display scaling; the fill follows RAM %, colour follows level;
  the sparkle appears only at 24 px and up, where it stays readable.
- Installer and notification icons come from the same master.

## Phases

| # | Phase | What "done" means |
|---|---|---|
| 0 | **Spike the risky bits** (do first) | Elevated Tauri window opens; LHM + PawnIO read Ryzen Tctl on this laptop; NVML reads the RTX; ASUS WMI charge limit can be read and written; scheduled-task autostart launches elevated without a prompt. Any failure changes the plan *before* the big build. |
| 1 | Shell + port | Tauri project, design tokens, rail, tray with live ring. `classify`/`recommend` ported to Rust, **unit-tested against a fixture of today's real process list** so the suggestions match v0.2 exactly. RAM, Apps, Startup, Log working. Old history and log imported. |
| 2 | CPU + GPU | Sensor sidecar, CPU section, throttling detector, GPU readout. |
| 3 | Storage + Clean | Volumes, SMART, folder sizes, safe clean, Windows Update clean, big/duplicate finder with exclusions; each category dry-run tested on fixture folders before touching real ones. |
| 4 | Battery | Health, cycles, rate, wear history, charge limit. |
| 5 | Automation | Alerts, auto-clean, weekly clean, settings page. |
| 6 | Icon, polish, ship | Final icon set, taste-skill lint/inspect on the UI, NSIS installer, scheduled-task autostart, old tray retired, CONTEXT.md written. |

## Risks and how they are handled

1. **Sensor driver and antivirus.** LHM's old WinRing0 driver is on Microsoft's blocklist; the current PawnIO driver is signed, but
   Defender may still flag an elevated tool that loads a kernel driver. Phase 0 tests it. Fallback: CPU temperature hidden, everything else kept.
2. **LHM is .NET; Tauri is Rust.** No .NET SDK is installed. Plan A: the sidecar is a long-lived PowerShell 5.1 host loading
   `LibreHardwareMonitorLib.dll` (its .NET Framework build), needing nothing new. Plan B: install the .NET 8 SDK and compile a small `shimmy-sensors.exe`.
3. **ASUS charge limit.** The ATK WMI class did not answer without admin; it may not be exposed on this model, or MyASUS may override it.
   Phase 0 tests it; if unsupported, the switch is hidden and the plan says so.
4. **Rewriting server.js in Rust.** The suggestion rules are the product. They are ported with tests that compare against v0.2's output on
   the same process snapshot.
5. **Always elevated.** This is a larger blast radius, which is why closing stays behind `classify()` in Rust, deletes go to the Recycle Bin,
   and nothing is deleted without a preview.
6. **SmartScreen.** It is unsigned, so Windows warns once at install ("More info → Run anyway"). Accepted, since it is for you only.

## Not in scope (unless you ask)

Registry cleaners (they help nothing and can break things), driver updaters, "game boost" modes, macOS/Linux, auto-updates, code signing.

## Phase 0 results (run 2026-10-27)

| Check | Result | Consequence for the build |
|---|---|---|
| Elevated Tauri 2 window | **Pass.** Release exe 8.5 MB with the `requireAdministrator` manifest; WebView2 draws elevated. | Go ahead with Tauri. |
| Silent elevated autostart | **Pass.** A scheduled task (highest privileges) launched the exe elevated with no prompt; the task was removed afterwards. Default task settings refuse to start on battery, so the real task must set `AllowStartIfOnBatteries` and `DontStopIfGoingOnBatteries`. | Use the task, with those two settings. |
| GPU via NVML (Rust `nvml-wrapper` 0.13) | **Pass.** RTX 3050: 46 °C, load, VRAM. | Use NVML; nvidia-smi fallback not needed. |
| PawnIO driver | **Pass.** 2.2.0 (signed by namazso.eu) installed silently with `-install -silent` and running. No Defender alert. | Installer bundles it. It now shows in Apps if you ever want to remove it. |
| CPU sensors (LibreHardwareMonitor 0.9.6 in PowerShell 5.1) | **Pass.** Tctl/Tdie 61.25 °C, package power 4.8 W, per-core clocks, power and VID. **No fan sensors.** | Plan A sidecar confirmed (needs the C# assembly resolver; a PowerShell resolver overflowed the stack). Fans: try ASUS ATK fan IDs in Phase 2, else leave them out. |
| ACPI thermal zone | 46.1 °C, but it is a board sensor, not the CPU (CPU was 61 °C). | Not used for "CPU temperature". |
| SSD health | **Pass.** LHM: life 96 %, 4 % used, 37 °C (warning 76, critical 79). Storage module: Healthy, wear 0, 37 °C; power-on hours not exposed there. | Use LHM's NVMe values; Storage module as backup. |
| Battery | WMI `BatteryStaticData`: "Generic failure". **LHM works:** degradation 26.6 % (so health about 73 %), charge level, voltage, charge rate. Cycle count not found yet. | Battery section reads LHM; `powercfg /batteryreport` for history and possibly cycles. |
| ASUS charge limit | WMI `DSTS(0x00120057)` returned 0 (not reported). The "ASUS System Control Interface v3" driver **is** present and MyASUS services are running. | Phase 4: retry through the `\.\ATKACPI` device (how G-Helper does it). If that fails too, hide the switch and point to MyASUS's Battery Care. Watch for MyASUS overriding the value. |

## Progress

### Phase 1: shell and port (done 2026-10-27)
- Tauri 2 app in `app/`. Rust modules: `procs.rs` (v0.2 rules), `system.rs` (Win32: processes, window titles, sessions, close, trim, Home readouts), `startup.rs`, `store.rs`, `tray.rs` (live ring icon), `lib.rs` (commands, tray menu, close-to-tray, sampler).
- **Parity test passes:** on a real 293-process snapshot, Rust gives exactly v0.2's 59 groups and 7 suggestions (`tests/parity.rs`; regenerate with `node tools/make-fixture.js`).
- New protections for elevated mode: Shimmy and all its descendants (its own WebView2), services (session 0), security software. All startup entries, including all-users ones, can be switched.
- RAM history and action log imported from `Desktop\RAM Optimizer\data` into `%APPDATA%\com.shimmy.optimizer`; the old folder is untouched.
- Space Grotesk bundled locally (OFL licence in `src/fonts`). Native dark title bar instead of a custom one, so Windows snap layouts keep working.
- Deviation: history stays JSON (same format as v0.2) instead of SQLite; it is small enough.
- Dev builds: `SHIMMY_NO_ADMIN=1` builds without the admin manifest, for tests and screenshots. `tools/shoot.ps1` screenshots the window.

### Phase 2: CPU and GPU (done 2026-10-27)
- Sensor sidecar `resources/sensors/sensors.ps1` (LibreHardwareMonitor 0.9.6, MPL-2.0 licence included) inside a kill-on-close job object, restarted with backoff if it dies. Storage re-read every 30 s.
- Processor page: temperature or load at display size, plain-language status with the throttling detector, load/clock/power/fan facts, 8 core tiles (real clock, load, power), temperature and load history (`cpu-history.json`), top CPU apps with Close, NVIDIA GPU.
- **Fans work** through `\.\ATKACPI` (CPU and GPU fan RPM), even without admin, unlike the WMI route in Phase 0. That bodes well for the charge limit in Phase 4.
- Deviation: the sidecar does **not** read GPUs, and NVML is read only while the Processor page is open, so the RTX 3050 can sleep (constant polling keeps a laptop dGPU awake and costs battery). So there is no GPU hot-spot temperature.
- Fixed along the way: Apps table bars replayed their grow animation on every 5 s refresh (also fixed in RAM Optimizer v0.2).
- 7 Rust tests pass (parity, protections, sensor parsing from Phase 0's dump, status rules, ring icon, ISO dates).

### Phase 3: storage and clean (done 2026-10-27)
- Storage page: free space at display size, SSD health from the sidecar (life, temperature, data written, spare), "What fills C:" in five categories, drillable biggest folders with Show in Explorer, other drives.
- The C: measurement reads each directory listing in parallel (rayon) and takes the size from the listing itself: **about 1 million files in 6 s** on this laptop (the first version with jwalk took 4 min). It skips OneDrive placeholders and does not follow junctions.
- Clean page: temp (files older than a day only), Windows temp, thumbnail cache, crash dumps, Delivery Optimization, Windows Update downloads (wuauserv paused meanwhile), Brave/Chrome/Edge caches (skipped while the browser runs), Recycle Bin (off by default). DISM analyse and clean-up as a background job.
- Big & duplicate finder over Downloads, Desktop, Documents, Videos, Pictures: files over 500 MB, installers in Downloads older than 60 days, duplicates by content (size, then first 64 KB, then full BLAKE3). Real run: 22,344 files, 30 large files, 200 duplicate groups (1.7 GB in extra copies), 11 project folders skipped. Every offered file passed the deletion gate.
- Deletion gate (`finder::allowed`, unit-tested): inside a scanned root; not in a code project; not Salon Shaila/invoice; not hidden/system; not on the Keep list. Deletes go to the Recycle Bin; anything over the bin's size limit (read from Explorer's BitBucket settings) needs a second, explicit confirmation. Every file is logged.
- Dev builds now use their own identifier (`com.shimmy.optimizer.dev`) and target folder (`target-dev`), so they never collide with the real, elevated Shimmy.

### Phase 4: battery (done 2026-10-27)
- Battery page: health at display size with plain-language wear status, charge and rate, cycle count (312), full vs design capacity, Windows' full-charge runtime estimate, and a capacity-over-time chart from `powercfg /batteryreport` (44 points since November 2025).
- Charge care: 60 / 80 / 100 % through ATK DEVS `0x00120057`. The firmware reports the feature present (DSTS `0x00090000`) but not the current value, so Shimmy remembers the choice (`settings.json`) and re-applies it at start-up and every 30 minutes. The page confirms when charging has visibly stopped at the limit.
- Fixed: CPU temperature could show 0 °C when another program read the Ryzen sensor at the same moment. Impossible readings are now dropped and the last good value is held for 30 s (`sensors::Steady`, unit-tested).
- 14 Rust tests pass; 5 read-only real-PC checks in `tests/real_scan.rs` (run with `--ignored`).

### Phase 5: automation, plus a Graphics page (done 2026-10-27)
- **Settings page** (new rail item). Saved as you change it, in `settings.json`, through one `set_settings` command that clamps every value in Rust (`auto::clamp`).
- **Alerts** (on by default): memory ≥ 90 % for a minute, CPU ≥ 90 °C for 30 s, C: below 10 GB free, battery ≤ 15 % while unplugged. Each fires once, then repeats at most every 30 min (6 h for disk, 1 h for battery) while the condition lasts. Every alert is written to the Log.
- **Automatic memory clean** (off by default): when memory stays above 85 % for 5 min, run Trim or Clean now (your choice), at most every 15 min, with a notification. Logged as `auto-clean`.
- **Weekly clean** (off by default): a day, an hour and the categories to clean (Recycle Bin off). If the laptop was off at that time, it catches up the next time Shimmy runs. Turning it on or moving it counts from now, so it never fires the moment it is switched on.
- **Start with Windows**: creates or removes the `Shimmy Optimizer` scheduled task (highest privileges, logon trigger, runs on battery, `--tray` so it starts hidden). Refused in dev builds. The window now starts hidden and is shown unless `--tray` is passed.
- **Finder settings**: large-file size, turning the standard folders off, extra folders (Windows, Program Files, ProgramData, AppData and the whole of C: are refused), and extra exclusions matched anywhere in the path. The deletion gate checks these exclusions too.
- **Graphics page** (asked for during this phase): RTX 3050 temperature, load, clocks against their maximum, power against the 30 W limit, video memory, memory-bus load, encoder/decoder, P-state, what is holding the clock back (power, heat, power brake), PCIe link and driver, through NVML (still only while a page is open, so the RTX can sleep). Per-engine load (3D, copy, video decode/encode), **per-app GPU use on both GPUs**, and the Radeon's load and memory come from Windows' "GPU Engine" counters, which do not wake the RTX. The temperature/load chart is recorded only while the page is open.
- Deviation: the plan's `cpu.rs` GPU readout moved into `gpu.rs`; the Processor page keeps a summary with a link to Graphics.
- New modules: `auto.rs` (rules, weekly slots, clamping), `autostart.rs`, `gpu.rs`. 21 Rust tests pass; 2 new real-PC checks (`real_gpu_counters`, `real_nvidia`). `tools/shoot.ps1` can now scroll (`name:x,y:secs:wheel`).

### Phase 6: ship (installed 2026-10-27)
- Icon: the `design/icon.svg` master already matched the spec, kept as final.
- NSIS installer (`target-ship`, 7.5 MB, unsigned, per-machine) into `C:\Program Files\Shimmy Optimizer`. Hooks (`installer-hooks.nsh`) install PawnIO silently and create the sign-in task through `shimmy-optimizer.exe --autostart on`; uninstall removes the task.
- **Verified on this laptop:** installed, task "Shimmy Optimizer" created (highest privileges, runs on battery, `--tray`), PawnIO driver running, installed Shimmy elevated and reading CPU temperature (53–60 °C).
- Old RAM Optimizer tray retired: its tray and `node server.js` had already exited and it has no autostart entry. Its folder is left in place for you to delete.
- `CONTEXT.md` written.
- Still to do: the taste-skill inspection pass on the UI.

## Phase 7: Run from the charger (built 2026-10-28, v0.2.0)

Goal: while plugged in, the battery neither charges nor drains, so the charger powers the laptop and charge cycles are saved.
Software cannot disconnect the battery or stop the laptop running on it when unplugged. This is done with the same ASUS
firmware charge limit (ATK DEVS `0x00120057`) by setting the limit to the current charge.

### Decisions (from the grilling)
| Topic | Decision |
|---|---|
| Hold level | The charge at the moment the switch is turned on. |
| Floor | Never hold below **40 %**: under that it charges to 40 % first, then holds. |
| Re-plug | Holds at the **new** level on every plug-in (unplug at 70 %, use down to 45 %, plug in, hold at 45 %), still with the 40 % floor. |
| Firmware drains above the limit | Round the limit **up** to the next value the firmware accepts, so nothing is drained. |
| Heavy load (charger cannot keep up) | One notification per session saying the battery is helping and how many watts. Nothing automatic. |
| MyASUS Battery Care | Detect when the limit is being overridden and warn on the Battery page. |
| Travel day | Turn the mode off and pick 100 % with the existing buttons. No extra control. |
| Where | Battery page switch above 60/80/100 (those grey out while on), tray menu tick item, Home Battery tile says "Running from the charger". |
| Off | Turning it off restores the previous 60/80/100 choice. |

### Step 1: firmware test (reversible, needs the charger plugged in)
1. Read only: charge, rate, the firmware's answer, the MyASUS Battery Care setting.
2. Limit = current charge; watch ~10 min. Pass: rate about 0 W, level steady, Windows says "plugged in, not charging".
3. Limit a few % below the charge: does the firmware hold or drain?
4. Which values it accepts (e.g. 45, 55, 72): continuous or steps (20/40/60…).
5. Restore the previous limit. Every write is logged.

### Step 1 results (run 2026-10-27, plugged in, battery at 60 %)
| Check | Result |
|---|---|
| Limit = current charge | **Holds.** Shimmy's 60 % limit already had the battery at 60 %, plugged in, 0 W in or out. |
| Limit below the charge (55 % at 60 %) | **Holds, does not drain.** 4 min: 27,565 → 27,541 mWh (about 0.3 W, normal self-discharge). The "round up" fallback is not needed. |
| Odd value (61 %) | **Accepted, exact.** About 80 s after the write it charged at ~50 W for ~40 s and stopped at 61 %, then held still. Any whole % works. |
| MyASUS | Battery Care is set to **80 %** with smart charging on (`HKLM\SOFTWARE\ASUS\ASUS System Control Interface\AsusOptimization\ASUS Keyboard Hotkeys`, `ChargingRate`/`ChargingSmart`). Shimmy's write currently wins; MyASUS may restore 80 % at boot, so the override warning reads `ChargingRate`. |
| Restored | Limit put back to 60 % afterwards. All writes are in the action log as `charge-limit-test`. |

Consequence: hold mode is simply "limit = current charge, at least 40 %", rewritten on every plug-in. The firmware takes about a minute to act on a new limit.

### Step 2: build (only if Step 1 passes) (Step 1 passed)
- Settings `pluggedMode { on, holdPct }`. The effective limit is the hold level when the mode is on, otherwise the 60/80/100 choice. It is re-applied at start-up, every 30 min and **on every plug-in** (re-hold at the new level, floor 40 %).
- Watchdog: charging above the hold level while plugged in → re-apply, then warn after two failures ("MyASUS may be overriding it"). Draining while plugged in under load → one notification per session.
- Status line, for example: "Holding at 72 %. The charger is powering the laptop and the battery is resting (0 W)."
- Unit tests for limit choice, floor, snapping, re-plug and status text; a read-only real-PC check; screenshots; the installer rebuilt.
- Known gap: after a reboot the firmware may charge for a few seconds before Shimmy starts and re-applies the hold.

### Built (2026-10-28)
- `hold.rs`: hold level (whole % below the charge, floor 40), effective limit, the watchdog (re-hold on every plug-in including a
  start-up while plugged in; rewrite the limit if the battery charges above the hold for 3 min; warn after two rewrites; warn once per
  plug-in if the battery drains over 3 W for 30 s), the status sentence, and MyASUS's `ChargingRate`. 5 unit tests.
- `lib.rs`: setting `plugged`, command `set_plugged_mode`, `plugged_watch` every 3 s, the start-up and 30-min re-apply use the effective
  limit, the 60/80/100 command refuses while the mode is on, tray tick item "Run from the charger", `get_state` carries the mode for Home.
- UI: switch and status on the Battery page above 60/80/100 (greyed while on), MyASUS warning, Home battery tile says "Running from
  the charger, held at N%".
- 26 Rust tests pass. Installer `Shimmy Optimizer_0.2.0_x64-setup.exe`.
- Firmware test helper: `SHIMMY_LIMIT=<pct> cargo test --test real_scan set_limit -- --ignored` writes the limit by hand.

## Phase 8: lean Shimmy, and delete from Storage (built 2026-10-28, v0.3.0)

Measured before (installed 0.2.0, window hidden in the tray): **327 MB working set, 486 MB private.** WebView2: 183 MB WS / 275 MB
private (of which the GPU process alone is 57 / 166); `shimmy-optimizer.exe`: 118 / 134; PowerShell sensor sidecar: 24 / 75.

### Decisions (from the grilling)
| Topic | Decision |
|---|---|
| Window engine | **Closed when the window closes**, rebuilt when opened from the tray (~1 s). `--tray` starts create no window at all. |
| GPU acceleration | **Kept on** (no `--disable-gpu`). |
| Sensor sidecar | **Compiled C# program** (`csc.exe` from .NET Framework 4, already in Windows) instead of the PowerShell host; LHM limited to CPU, battery and storage. |
| Self readout | "Shimmy itself: N MB" line on the Memory page (core, window engine and sidecar). |
| Storage delete scope | **User folder only** (`C:\Users\Studio RedPix`), never AppData except the listed caches. |
| Folder deletes | **Recycle Bin by default**, with a "Delete for good" tick in the confirm dialog. |
| Cache deletes | **Recycle Bin** (undo possible). Anything too big for the bin keeps the existing second confirmation. |
| Cache families | Editing apps, Chromium-based apps, developer caches (off by default; the Cargo cache builds Shimmy), Windows extras. |
| "Old" | Files untouched for **30 days**, adjustable in Settings. |
| Hibernation file | A switch with its size and what is lost (Hibernate, Fast Startup); reversible (`powercfg /h on|off`). |
| Recycle Bin | Size plus an **Empty** button on the Storage page (asks once, logged). |
| Placement | Old caches live on the **Storage** page; Clean keeps the quick safe clean. |

### Part 1: RAM (target about 30–50 MB in the tray)
1. Window: on close, destroy the webview window instead of hiding it; `show_main` rebuilds it from the same config. Check the tray,
   alerts, auto-clean, weekly clean and Run from the charger keep working with no window.
2. Core: measure first, then drop caches when the window closes (C: tree, finder, clean scans, battery report), keep a lean process
   list (no full command lines between page loads), shut NVML down when no page needs it, close the PDH query, and return freed
   memory to Windows (heap compaction plus trimming its own working set).
3. Sidecar: `shimmy-sensors.exe` built from C# at build time (or checked in as source plus a prebuilt exe), same JSON lines over
   stdout, same kill-on-close job object; LHM hardware groups limited to CPU, battery and storage.
4. Self readout on the Memory page; before and after numbers recorded here.

### Part 2: Storage deletes
- **Gate** `storage::deletable`: inside the user folder; not the special folders themselves; not AppData except listed caches; not
  code projects, Salon Shaila, hidden or system files, the Keep list or the user's exclusions; re-checked right before deleting.
  Unit-tested like `finder::allowed`.
- **Biggest folders**: Delete per row, with a preview (size, file count) and confirmation; Recycle Bin, or "Delete for good".
- **Old caches** list: per-family size of files older than the age setting, ticks, skipped while the owning app runs:
  - Editing: Premiere / Media Encoder media cache and peak files, CapCut cache.
  - Chromium-based apps: Claude desktop, Discord, Spotify, Telegram and similar, cache folders only (Cache, Code Cache, GPUCache).
  - Developer (off by default): npm, pip, Cargo registry cache.
  - Windows extras: old Windows Update logs and other safe leftovers, after checking each one exists and is safe on this PC.
- "What fills C:" categories clickable into the folder list; Recycle Bin size and Empty; hibernation switch; restore points shown
  only; Apps links to Windows' own uninstall page.
- Testing: dry runs on fixture folders, then a real scan that deletes nothing, then you try it.

### Built (2026-10-28)
**RAM, measured with `tools/memory.ps1` (dev build):**

| State | RAM (working set) | Private |
|---|---|---|
| Before (0.2.0, in the tray) | 327 MB | 486 MB |
| **After, window closed (tray)** | **37 MB** | **29 MB** |
| After, window open | ~420 MB | ~245 MB |

- The window (`create: false`) is built by `show_main` from a separate thread and destroyed on close. `RunEvent::ExitRequested`
  without a code is prevented, so only the tray's Exit quits. On `WindowEvent::Destroyed`, `Shared::release()` drops the C: tree,
  every job result, the battery report, NVML, the PDH query and the process list, then `system::compact_self()` (HeapCompact plus
  EmptyWorkingSet). The core is 10 MB / 11 MB with no window.
- Sidecar: `resources/sensors/shimmy-sensors.cs`, compiled by `build.rs` with .NET Framework 4's `csc.exe` as a windowless exe
  (no console host), with `gcConcurrent` off in its `.exe.config`. CPU, battery and storage only (motherboard dropped, unused):
  18 MB private instead of 75. `sensors.ps1` removed.
- Memory page: "Shimmy itself is using N MB" (its own process tree).

**Storage:**
- `storage::check_folder`: the folder gate (unit-tested), also returning size, file and folder counts for the preview.
  `Tree::remove` updates the list without a rescan.
- `folder_preview` and `folder_delete`: Recycle Bin or "Delete for good"; bigger than the bin means for good, only after a second
  question. Logged.
- `caches.rs`: CapCut, Premiere/Media Encoder, Telegram, Spotify, Discord, Claude desktop, graphics shader caches (on by default);
  npm, pip and Cargo (off). Files older than `caches.ageDays` (30, Settings); a folder whose files are all old is recycled as one
  item; batches of 500 through `trash::delete_all`. Measured on this laptop: npm 5.2 GB old, CapCut 1.5 GB, Cargo 0.9 GB,
  NVIDIA shader cache 0.4 GB, pip 0.2 GB.
- Recycle Bin size, capacity (25.8 GB on C:) and Empty; hibernation file (6.2 GB) with an off/on switch (`powercfg /hibernate`);
  restore points read only (`vssadmin`, needs admin); Installed apps opens Windows' page. "What fills C:" categories are clickable.
- 28 Rust tests pass. Installer `Shimmy Optimizer_0.3.0_x64-setup.exe`.
- Not exercised end to end yet: an actual folder or cache delete on the real PC (left for you to try from the installed build).

## Phase 9: fan modes, heat protection, battery watts (planned 2026-10-28)

Goal: choose the fan mode from the Processor tab, have Shimmy turn CPU boost off when the processor runs hot, and show on the
Battery tab where the power is going and whether the battery is helping the charger while Run from the charger is on.

### What the firmware allows (read-only probe, 2026-10-28)

| ATK device | DSTS answer | Meaning |
|---|---|---|
| `0x00110019` VivoBook fan mode | `0x00030000` | Present; low word 0 = Standard. The Fn+F modes (Silent / Standard / Performance). |
| `0x00120075` ROG performance mode | call failed | Not on this model. |
| `0x00110024` / `0x00110025` CPU / GPU fan curves | call failed | **No custom fan curves, so no direct RPM control.** |
| `0x001200A0`, `A3`, `B0`, `B1` power limits (PPT) | call failed | No ASUS CPU power limits. |
| `0x00110013` CPU fan | `0x00010020` | 3200 RPM, already read. |

Windows' processor settings work on the active plan (Balanced): max processor state 100 % (AC and DC), boost mode on.

### Decisions (from the grilling)

| Question | Decision |
|---|---|
| Fan control | Three buttons on the Processor tab: **Silent / Standard / Performance** (the ASUS modes), live CPU fan RPM beside them. |
| Fn+F or MyASUS changes the mode | **Follow it.** Shimmy reads the firmware and shows the active mode; nothing is re-applied. |
| How to cool a hot CPU | **Turn CPU boost off** (Windows processor boost mode = disabled, AC and DC) and nothing else. |
| Trigger | **Automatic, with a switch, on by default.** Boost off after 30 s at **≥ 90 °C**; back on after 2 min at **≤ 75 °C**. |
| Fan mode Performance | Heat protection still applies. |
| Telling you | **Quiet:** written to the Log and shown on the Processor tab ("Boost off: cooling down"). No notification. |
| Battery watts | Battery in/out W; CPU package W; RTX W; estimated total draw; a drain history chart. |
| RTX watts | **Only when it is already awake** (Windows' GPU engine counters show it in use); otherwise "asleep, about 0 W". Never wakes it. |
| Total draw | Exact on battery (the discharge rate). On the charger it is an estimate (no charger sensor), labelled as such. |
| History | **Last hour, in memory**, sampled every 3 s by the sampler (also in the tray), cleared on restart. Drain shaded. |
| Battery draining while holding | **Warn only:** keep the one notification per plug-in (more than 3 W for 30 s), plus the live state and chart. |

### Step 1: firmware test (reversible)
1. Write each fan mode through DEVS `0x00110019` and read it back; confirm the value mapping (expected, from G-Helper:
   0 Standard, 1 Performance, 2 Silent) by fan RPM and CPU power under the same load. Put it back to Standard.
2. Boost: save `PERFBOOSTMODE` (AC and DC) for the active plan, set 0, confirm the clock stays at or below base under load, restore.

### Step 2: build
- `asus.rs`: fan mode read and write. `cpu.rs`/Processor page: mode buttons, active mode, RPM.
- `heat.rs`: pure decision functions (like `hold.rs`), unit-tested: thresholds, timings, hysteresis.
- Boost off records the original values in `settings.json` and restores them when the heat clears, on Exit, and at start-up if
  Shimmy was closed while boost was off (crash or reboot), so boost is never left off by accident. If the power plan changes
  while boost is off, the new plan is left alone.
- Battery page: power section (in/out, CPU, RTX, total), and a one-hour chart from a 1,200-point ring buffer in `Shared`.
- Settings: the heat protection switch. Log: `fan-mode`, `boost-off`, `boost-on`.
- Tests; screenshots; installer rebuilt as v0.4.0.

### Step 1 results, part 1 (run 2026-10-28, plugged in, not elevated)
Script: `tools/phase9-step1.ps1` (45 s full load per setting on 16 threads, 20 s rest between; restores in `finally`).

| Setting | Firmware | CPU fan under load |
|---|---|---|
| Fan mode value 2 | DEVS answered 1, reads back `0x00030002` | 5700 RPM |
| Fan mode value 1 | DEVS answered 1, reads back `0x00030001` | 3400 RPM |
| Fan mode value 0 | DEVS answered 1, reads back `0x00030000` | 5100 RPM |
| Boost off (`PERFBOOSTMODE` 0/0) | powercfg took it | 5100 RPM |
| Afterwards | Mode back to `0x00030000`, boost back to 2/2 (Aggressive) | |

- All three fan modes are accepted and read back, so Shimmy can both set and show the mode.
- The fan speeds suggest **1 = Silent, 0 = Standard, 2 = Performance** on this Vivobook, not G-Helper's ROG order. Not yet
  confirmed by CPU power: without admin rights the sensor reader returns no temperature, power or clocks.
- Still to do (needs an elevated run): CPU package watts and clocks per mode, and the clock staying at or below 3.2 GHz with
  boost off.

### Step 1 results, part 2 (run 2026-10-28, elevated, plugged in)
Same script, run as admin so the sensor reader delivers. Full load on 16 threads, read after about 40 s.

| Setting | CPU package | Tctl | Average core clock | CPU fan |
|---|---|---|---|---|
| Idle, Standard | 8.0 W | 69.6 °C | 1011 MHz | 5100 RPM |
| Fan mode 2 | **45.1 W** | 95.4 °C | 3177 MHz | 6300 RPM |
| Fan mode 1 | **33.8 W** | 90.0 °C | 2599 MHz | 3500 RPM |
| Fan mode 0 | **38.1 W** | 91.9 °C | 2753 MHz | 5200 RPM |
| Mode 0 + boost off | 34.9 W | **84.1 °C** | 2738 MHz | 5100 RPM |
| Afterwards | Mode back to `0x00030000`, boost back to 2/2 | | | |

- **Confirmed: 1 = Silent, 0 = Standard, 2 = Performance** on this Vivobook (power and fan both rise in that order). The firmware
  also changes the CPU power limit with the mode, so Silent is itself a cooling step.
- **Boost off works:** about 8 °C cooler than Standard under the same all-core load, for about 0.5 % less average clock. Under
  all-core load the clock is already held near base by power, so the speed cost shows mostly in light, few-core work.
- Under full load Standard reaches about 92 °C and Performance 95 °C (the chip's limit), so heat protection at 90 °C will trigger
  during long heavy loads. With boost off it settles around 84 °C, above the 75 °C needed to turn boost back on, so boost stays
  off until the load ends. That is the intended behaviour.

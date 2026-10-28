# Shimmy Optimizer v2: Linux and macOS

Written 2026-10-28. Follows `PLAN.md` (Phases 0 to 10, Windows). Read `CONTEXT.md` first for the code map.

Goal: the same Shimmy (one codebase, one UI, the same safety rules) on **Linux** and **macOS**, alongside Windows. The
plan has three parts. Shimmy is reshaped so that everything Windows-specific sits behind one platform layer. Linux ships first
because its kernel exposes almost everything Shimmy needs through plain files. macOS ships last because its sensors and
firmware controls are mostly private APIs and its privileged helpers need paid code signing.

## Where Shimmy is Windows-only today

Measured from the code on 2026-10-28. About two-thirds of the Rust core calls Windows directly.

| Module | Windows dependency | Portable already |
|---|---|---|
| `system.rs` | `EnumWindows` titles, session ids, `EmptyWorkingSet`, `GlobalMemoryStatusEx`, `GetSystemPowerStatus`, `GetDiskFreeSpaceExW(C:\)`, heap compaction | process list through `sysinfo` |
| `procs.rs` | protection lists are Windows exe names; "session 0 = service" | `classify` / `recommend` logic, grouping |
| `cpu.rs`, `sensors.rs` | LibreHardwareMonitor C# sidecar + PawnIO driver; PDH; `GetLogicalProcessorInformationEx` (Phase 10c) | the status rules, `Steady` |
| `heat.rs` | `powercfg` `PERFBOOSTMODE` | the decision functions |
| `asus.rs`, `hold.rs` | `\\.\ATKACPI` fan mode and charge limit; MyASUS registry | hold-level maths, watchdog rules |
| `gpu.rs` | Windows "GPU Engine" PDH counters, DXGI; D3DKMT (Phase 10d) | NVML (works on Linux too) |
| `battery.rs` | `powercfg /batteryreport` XML | rate, draw, health maths |
| `storage.rs` | drive letters, `C:\`, Program Files, hiberfil, `vssadmin` | the parallel walk, tree, folder gate logic |
| `clean.rs` | `%TEMP%`, Windows Update, Delivery Optimization, DISM, `wuauserv`, browser paths | scan / delete engine |
| `caches.rs` | `%APPDATA%` / `%LOCALAPPDATA%` paths and `.exe` names | the age filter, batching |
| `finder.rs` | `C:\` roots, Windows hidden/system attributes, Program Files refusal | hashing, duplicate logic, the gate structure |
| `startup.rs` | registry Run keys and `StartupApproved` | |
| `autostart.rs` | `schtasks` with highest privileges | |
| `tray.rs` | | the ring icon drawing |
| `auto.rs`, `jobs.rs`, `store.rs` | | portable (store already uses Tauri's per-OS data folder) |
| Installer | NSIS, PawnIO, `requireAdministrator` manifest | |
| Updater (Phase 10b) | one asset, `Shimmy-Optimizer-Setup.exe` | GitHub check, SHA-256 |

## Decisions (proposed, to grill before building)

| Topic | Proposal | Why |
|---|---|---|
| Privileges | **The app runs as the normal user on Linux and macOS.** A small privileged helper does the few root-only actions. | GUI toolkits refuse or misbehave as root (Wayland, WebKitGTK), and a root WebView has a far bigger blast radius than Windows' elevated one. |
| Helper | Linux: `shimmy-helperd`, a systemd system service on a Unix socket, which checks the caller's uid with `SO_PEERCRED`. macOS: an `SMAppService` daemon over XPC that checks the caller's code signature. Both accept only a fixed list of commands with clamped arguments. | Least privilege, and each action can be audited. Every helper call is logged in `actions-log.csv`, as on Windows. |
| No helper installed | Shimmy still runs as a monitor and cleaner. Root-only controls are hidden with one line ("Needs Shimmy's helper"), the same way non-ASUS PCs hide fan modes today. | AppImage and unsigned macOS builds cannot install a service. |
| Sensors | **Native Rust readers, no sidecar.** Linux reads hwmon, cpufreq, powercap and power_supply in sysfs. macOS reads IOKit, SMC and IOReport. | The LibreHardwareMonitor sidecar is .NET and Windows-only. Plain file reads keep Shimmy light. |
| Memory "Trim" | **Windows only.** Linux and macOS have no honest per-process equivalent (`drop_caches` and `purge` throw away the file cache, which makes things slower). Clean now only closes apps marked Safe (Phase 10a). | The README promises honesty, and the 10a marks are cross-platform. |
| Fan modes | Linux: `platform_profile` (quiet / balanced / performance), which is vendor-neutral and includes ASUS through `asus-wmi`. macOS: none. | Writing fans through the SMC on a Mac is risky and undocumented. |
| Heat protection | Linux: boost off through cpufreq (`boost`, `intel_pstate/no_turbo`, `amd_pstate` boost), with the same thresholds and restore rules as `heat.rs`. macOS: **Low Power Mode** (`pmset -a lowpowermode 1`) as the cooling step, on laptops with macOS 12+. | It is the same idea as Windows: a reversible step that is logged and always restored. |
| Charge limit and Run from the charger | Linux: `charge_control_end_threshold` (ASUS, ThinkPad, Dell, Framework and others, read from sysfs). `hold.rs` logic reused unchanged. macOS: **not in v2.** Show Apple's own Optimized Charging and 80 % limit state, read-only. | The kernel exposes the Linux limit for most laptops. Mac charge control needs private SMC keys. |
| GPU | NVML on any OS for NVIDIA. Linux: amdgpu sysfs (busy %, hwmon temperature and power, VRAM), i915/xe frequency, per-app use from `/proc/<pid>/fdinfo` `drm-engine-*`. macOS: IOAccelerator `PerformanceStatistics` (device use %, memory), no per-app. | The rule from Phase 2 holds everywhere: **never wake a sleeping dGPU.** Linux checks `/sys/bus/pci/devices/*/power/runtime_status` first. |
| Packaging | Windows: NSIS (unchanged). Linux: `.deb` and `.rpm` (installing the helper in postinst) plus `.AppImage` (monitor-only, with an "Install helper" button that runs one `pkexec` script). macOS: universal `.dmg` (Apple Silicon + Intel). | These are the Tauri bundler's standard targets. |
| macOS signing | **Needs the owner:** Apple Developer Program ($99 a year) for Developer ID signing and notarization. Without it, Gatekeeper blocks the app and the privileged helper cannot be registered. | The macOS release waits on this decision. |
| Updater (10b) | One asset per OS and architecture: `Shimmy-Optimizer-Setup.exe`, `Shimmy-Optimizer.dmg`, `Shimmy-Optimizer_amd64.deb`, `Shimmy-Optimizer_x86_64.rpm`, `Shimmy-Optimizer_x86_64.AppImage`, each with a SHA-256 digest. Install step per format: run the NSIS installer; open the .deb/.rpm with `pkexec`; replace the AppImage in place; open the .dmg and show the drag-to-Applications step. | It keeps 10b's checksum gate on every OS. |
| Distros | Ubuntu 24.04 LTS (GNOME, Wayland) and Fedora 41 (KDE, Wayland) are tested. Arch is best-effort. | They cover both WebKitGTK stacks and both tray hosts. |
| CI | **GitHub Actions matrix** (windows-latest, ubuntu-24.04, macos-14) builds and runs the unit tests on every push. There is no CI today. | Each OS is checked automatically. Today, tests run only on the Vivobook. |

## Architecture

```
src-tauri/src/
├─ lib.rs, auto.rs, jobs.rs, store.rs, tray.rs     shared (no cfg)
├─ rules/        procs (classify/recommend), heat, hold, finder gate, storage gate, battery maths, semver   pure, tested on every OS
├─ platform/
│   ├─ mod.rs     the Platform trait: procs, windows/titles, close, trim?, memory, cpu topology + clocks + temps,
│   │             power, fan modes?, boost?, charge limit?, battery, gpu, drives, smart, clean targets, cache families,
│   │             startup entries, autostart, protections, installer kind. Optional features return None and the UI hides them.
│   ├─ windows/   today's code moved here unchanged (system, asus, sensors sidecar, pdh, powercfg, schtasks, registry)
│   ├─ linux/     procfs, sysfs readers, XDG autostart, systemd --user, helper client
│   └─ macos/     IOKit, SMC/IOReport, launchd, SMAppService, helper client
└─ helper/       shimmy-helperd (Linux) / Shimmy Helper (macOS): fixed command set, clamped, logged
```

- Each capability is an `Option`: the UI already hides fan modes and the charge limit on unsupported PCs, and v2 extends that
  pattern to every root-only or OS-only control. A `capabilities` command tells the UI what exists.
- Paths come from one place per OS (`dirs` crate plus per-OS tables) instead of `%APPDATA%`, `C:\` and `.exe` names spread
  through `caches.rs`, `clean.rs`, `finder.rs` and `storage.rs`.
- **Safety rules do not loosen.** `classify()` stays the single gate for closing, with per-OS protection tables:
  - Linux: pid 1, kernel threads, `systemd*`, display servers and compositors (`Xorg`, `gnome-shell`, `kwin_*`, `plasmashell`),
    `pipewire`/`wireplumber`/`pulseaudio`, `dbus-*`, `polkitd`, `NetworkManager`, `sshd`, and processes owned by other users or
    system users (uid < 1000) in place of "session 0".
  - macOS: `launchd`, `kernel_task`, `WindowServer`, `loginwindow`, `Finder`, `Dock`, `SystemUIServer`, `coreaudiod`,
    anything under `/System/` or `/usr/libexec/`, and processes owned by root or `_`-prefixed system users.
  - Shared: Shimmy and its WebView (`WebKitWebProcess` / `com.apple.WebKit.*` children), terminals, editors, Claude, Salon Shaila, security software (per-OS names).
  - Each table gets a parity-style fixture test from a real process snapshot of that OS.
- The deletion gates (`finder::allowed`, `storage::check_folder`) gain per-OS refusals: Linux `/usr`, `/etc`, `/var`, `/opt`,
  `/proc`, `/sys`, `/boot`, other mounts, `~/.local/share` except listed caches, dotfiles (hidden = name starts with `.`); macOS `/System`,
  `/Library`, `/Applications`, `~/Library` except listed caches, `.app` bundles, iCloud placeholders (dataless files),
  `/System/Volumes/Data` firmlinks walked once only.
- Trash: the `trash` crate already supports freedesktop Trash and the macOS Finder Trash (with Put Back), so "Recycle Bin always" carries over.

## What each page becomes

| Page | Linux | macOS |
|---|---|---|
| Home | RAM %, CPU °C (hwmon `k10temp` Tctl / `coretemp` package), free space on `/` (or `/home` if separate), battery health | RAM % (memory pressure, as in Activity Monitor), CPU/SoC °C, free space on the data volume, battery health |
| Memory | used, available, swap, PSI memory pressure (`/proc/pressure/memory`); map and suggestions; window titles on X11 via `_NET_CLIENT_LIST`; on Wayland, apps are matched to `.desktop` files instead (Wayland exposes no titles) | used, wired, compressed, app memory; titles through `CGWindowListCopyWindowInfo` (owner names need no permission; titles need Screen Recording, so Shimmy uses owner names only) |
| Processor | load, effective clocks (`scaling_cur_freq`), real topology (`topology/core_id`, `thread_siblings`, `cpu_capacity` and `/sys/devices/cpu_core`/`cpu_atom` for hybrid), temperature, package power from powercap RAPL (`energy_uj` is root-only since kernel 5.10 → helper), fan RPM from hwmon, fan modes via `platform_profile`, heat protection via boost | load, clocks and P/E clusters (`hw.perflevel*` sysctl), temperature and power through IOReport / SMC (the sampler stays read-only), Low Power Mode as heat protection, no fan modes |
| Graphics | NVML; amdgpu and Intel from sysfs; per-app from fdinfo; never wakes a suspended dGPU | IOAccelerator use and memory; Intel Macs with AMD dGPU the same way |
| Storage | `/` and other real mounts (skip tmpfs, squashfs, snap loops); NVMe health through the helper (NVMe SMART log ioctl), temperature from hwmon without root; usage categories: system (`/usr`), apps (Flatpak, Snap, `/opt`), user, reclaimable | data volume; APFS snapshots shown read-only (`tmutil listlocalsnapshots`); SMART "Verified" from `diskutil`; categories: system, Applications, user, reclaimable |
| Clean | `~/.cache` thumbnails, browser caches (Chrome, Chromium, Brave, Edge, Firefox `cache2`; native, Flatpak and Snap paths), Trash, crash reports (`/var/crash`, systemd-coredump via helper), journal vacuum and package caches (`apt`, `dnf`, `pacman -Sc`) through the helper and off by default. No DISM equivalent; old kernels are **never** touched. | `~/Library/Caches` for listed apps only, browser caches, Trash, `~/Library/Logs/DiagnosticReports`, Homebrew cache (`brew cleanup -n` to preview), Xcode DerivedData and old iOS DeviceSupport (developer family, off by default). Paths behind Full Disk Access are skipped with a note, never forced. |
| Old caches | the same families with Linux paths (`~/.config/discord/Cache`, `~/.npm/_cacache`, `~/.cache/pip`, `~/.cargo/registry`…) | `~/Library/Application Support/discord/Cache`, `~/Library/Caches/pip`, … |
| Battery | `power_supply/BAT*`: capacity, `energy_full` / `energy_full_design` (or `charge_*`), `cycle_count`, `power_now`; charge limit, hold mode | IOKit `AppleSmartBattery`: cycle count, max/design capacity, instant amperage × voltage; Apple's own limit shown read-only |
| Battery history | Shimmy records its own daily capacity sample (there is no `batteryreport`) | Same |
| Apps · Log | unchanged | unchanged |
| Startup | XDG autostart `.desktop` files (`Hidden=true` to disable) and `systemd --user` units | `~/Library/LaunchAgents` and `/Library/LaunchAgents` (via `launchctl disable`), login items listed read-only |
| Start with Windows → "Start at sign-in" | XDG autostart entry with `--tray` | `SMAppService.mainApp` |
| Updates | per 10b, per package format | per 10b, .dmg |

## Tray and window
- **macOS:** menu-bar icon. The live ring stays coloured (no template image) so the level colour is kept. When the window
  closes, the app switches to `ActivationPolicy::Accessory` so it leaves the Dock and runs from the menu bar only. The
  "destroy the WebView on close" saving from Phase 8 carries over.
- **Linux:** the tray goes through AppIndicator / StatusNotifierItem. **Left-click is not delivered** on most desktops, so
  the menu gains "Open Shimmy" as its first item. GNOME needs the AppIndicator extension (preinstalled on Ubuntu, not on
  Fedora Workstation): without a tray host, closing the window quits instead of hiding, and Shimmy says so once.
- WebKitGTK and WKWebView: check the bundled Space Grotesk, canvas charts and CSS on both. Keep the CSP as is.

## Phases

| # | Phase | What "done" means | Release |
|---|---|---|---|
| v2-0 | **Spike the risky bits** on real machines | One Linux laptop (ideally the Vivobook from a live USB: `asus-wmi` `platform_profile`, `charge_control_end_threshold`, `k10temp`, RAPL, NVIDIA runtime suspend) and one Apple Silicon Mac (IOKit battery, IOReport/SMC temperature and power without root, Low Power Mode via `pmset`, unsigned-app behaviour, tray). Results recorded here, as in Phase 0. Any failure changes the plan before the refactor. | none |
| v2-1 | **Platform layer on Windows only** | Today's code moved behind `platform/windows`, pure rules in `rules/`. **No behaviour change**: all Windows tests and the parity test pass. CI matrix builds Windows and runs the pure tests on all three OS. | v0.7.0 |
| v2-2 | **Linux, monitor and clean** (no helper) | Every page reads real values on Ubuntu and Fedora; close (SIGTERM, then SIGKILL after a timeout, through `classify`), user-level clean and old caches to Trash, XDG startup and autostart, tray, updater for .deb/.rpm/AppImage. Linux protection table with a fixture test. | v0.8.0 (Linux beta) |
| v2-3 | **Linux controls** (helper) | `shimmy-helperd` with fixed commands: set charge limit, set platform profile, boost on/off, read RAPL, read NVMe SMART, journal vacuum, package cache clean. Hold mode, fan modes, heat protection, power section and SSD health on Linux. Helper fuzzed against bad input; every call logged. | v0.9.0 |
| v2-4 | **macOS, monitor and clean** | Same as v2-2 for macOS, universal .dmg, menu-bar mode, launchd startup, macOS protection table with a fixture test. Unsigned build only if the owner declines signing (documented "right-click → Open"). | v0.10.0 (macOS beta) |
| v2-5 | **macOS controls** (needs signing) | Signed and notarised; `SMAppService` helper for the root-only reads; Low Power Mode as heat protection. | v0.11.0 |
| v2-6 | **Polish and 1.0** | README, DOWNLOAD and screenshots for three OS; the taste-skill pass on WebKitGTK and WKWebView; per-OS install and uninstall checked end to end. | v1.0.0 |

**Ordering with Phase 10:** finish 10a to 10d first. 10c's topology code and 10d's vendor-neutral GPU code become the
Windows side of the platform trait in v2-1, so writing them now is not wasted. 10b's updater gains the per-format install step in v2-2 and v2-4.

## Risks and how they are handled

1. **Root on Linux.** A root helper is the biggest new attack surface. It takes a fixed command set with numbers clamped in the
   helper, no paths or shell strings from the caller, a peer-uid check, and systemd hardening (`ProtectSystem=strict`, `ReadWritePaths`
   limited to the sysfs files it writes, `NoNewPrivileges`). It gets its own tests and a fuzz target.
2. **Firmware writes on unknown laptops.** v0.4.2 already asks before the first ASUS firmware write on an untested model. On Linux the kernel
   driver has validated the call, so the same one-time confirmation applies, and Shimmy writes nothing the driver does not
   expose.
3. **macOS private APIs** (IOReport, SMC keys) can change with any macOS release. Shimmy only reads them, tolerates missing keys (shown as unavailable,
   never 0), and is tested on the newest macOS before each release.
4. **Wayland gives no window titles**, which `quick_clean` uses as its "has a window" signal. After 10a, closing is driven by the
   Safe mark, not by windowlessness, so Linux is not weaker.
5. **Tray without a host** (stock GNOME): handled by quitting on close and saying so.
6. **Three OS × testing.** CI covers builds and pure tests. Real-hardware checks stay manual: `tests/real_scan.rs` gets
   per-OS read-only checks, run with `--ignored` on each machine.
7. **Gatekeeper and SmartScreen.** Unsigned builds warn on Windows and are blocked by default on macOS. Signing is an owner decision
   (see PLAN.md Phase 10 for Windows, and above for Apple).

## Not in scope (unless you ask)

ChromeOS, BSDs, 32-bit or ARM Linux, Linux without systemd for the helper (monitor-only still works), writing Mac fan speeds or
SMC charge keys, `drop_caches`/`purge` as a "RAM clean", removing old kernels, registry-cleaner-style Linux "optimisations"
(swappiness tweaks and so on).

## Questions for the owner
1. Is there a Linux machine to test on (or is a live USB on the Vivobook fine)? Which Mac (Apple Silicon model, macOS version)?
2. Apple Developer Program ($99 a year): yes, later, or ship macOS unsigned?
3. Is the Linux helper acceptable as a root systemd service, or monitor-only on Linux for now?
4. Should Trim stay Windows-only, as proposed?

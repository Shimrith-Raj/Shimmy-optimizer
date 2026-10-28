# Shimmy Optimizer v2: Linux

Written 2026-10-28. Follows `PLAN.md` (Phases 0 to 10, Windows). Read `CONTEXT.md` first for the code map.

Goal: the same Shimmy (one codebase, one UI, the same safety rules) on **Linux**, alongside Windows. Shimmy is reshaped so
that everything Windows-specific sits behind one platform layer, then a Linux side is added. Linux suits Shimmy well: its
kernel exposes almost everything Shimmy needs through plain files.

**macOS: dropped (owner, 2026-10-28).** No Apple signing will be paid for, and without it Gatekeeper blocks the app and no
privileged helper can be installed; Mac sensors and controls are also mostly private APIs. The platform layer leaves room
for it if that changes.

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
| Privileges | **The app runs as the normal user on Linux.** A small privileged helper does the few root-only actions. | GUI toolkits refuse or misbehave as root (Wayland, WebKitGTK), and a root WebView has a far bigger blast radius than Windows' elevated one. |
| Helper | `shimmy-helperd`, a systemd system service on a Unix socket, which checks the caller's uid with `SO_PEERCRED`. It accepts only a fixed list of commands with clamped arguments. | Least privilege, and each action can be audited. Every helper call is logged in `actions-log.csv`, as on Windows. |
| No helper installed | Shimmy still runs as a monitor and cleaner. Root-only controls are hidden with one line ("Needs Shimmy's helper"), the same way non-ASUS PCs hide fan modes today. | An AppImage cannot install a service. |
| Sensors | **Native Rust readers, no sidecar.** hwmon, cpufreq, powercap and power_supply in sysfs. | The LibreHardwareMonitor sidecar is .NET and Windows-only. Plain file reads keep Shimmy light. |
| Memory "Trim" | **Windows only.** Linux has no honest per-process equivalent (`drop_caches` throws away the file cache, which makes things slower). Clean now only closes apps marked Safe (Phase 10a). | The README promises honesty, and the 10a marks are cross-platform. |
| Fan modes | `platform_profile` (quiet / balanced / performance), which is vendor-neutral and includes ASUS through `asus-wmi`. | One kernel interface for most laptops. |
| Heat protection | Boost off through cpufreq (`boost`, `intel_pstate/no_turbo`, `amd_pstate` boost), with the same thresholds and restore rules as `heat.rs`. | It is the same idea as Windows: a reversible step that is logged and always restored. |
| Charge limit and Run from the charger | `charge_control_end_threshold` (ASUS, ThinkPad, Dell, Framework and others, read from sysfs). `hold.rs` logic reused unchanged. | The kernel exposes the limit for most laptops. |
| GPU | NVML for NVIDIA (works on Linux too). amdgpu sysfs (busy %, hwmon temperature and power, VRAM), i915/xe frequency, per-app use from `/proc/<pid>/fdinfo` `drm-engine-*`. | The rule from Phase 2 holds everywhere: **never wake a sleeping dGPU.** Linux checks `/sys/bus/pci/devices/*/power/runtime_status` first. |
| Packaging | Windows: NSIS (unchanged). Linux: `.deb` and `.rpm` (installing the helper in postinst) plus `.AppImage` (monitor-only, with an "Install helper" button that runs one `pkexec` script). | These are the Tauri bundler's standard targets. |
| Updater (10b) | One asset per format: `Shimmy-Optimizer-Setup.exe`, `Shimmy-Optimizer_amd64.deb`, `Shimmy-Optimizer_x86_64.rpm`, `Shimmy-Optimizer_x86_64.AppImage`, each with a SHA-256 digest. Install step per format: run the NSIS installer; open the .deb/.rpm with `pkexec`; replace the AppImage in place. | It keeps 10b's checksum gate on every OS. |
| Distros | Ubuntu 24.04 LTS (GNOME, Wayland) is tested by hand from a live USB on the Vivobook. Fedora 41 (KDE) builds in CI only; Arch is best-effort. | No Linux machine is owned; Ubuntu is the most common and its live session needs no install. |
| CI | **GitHub Actions matrix** (windows-latest, ubuntu-24.04, fedora container) builds and runs the unit tests on every push. There is no CI today. | Each OS is checked automatically. Today, tests run only on the Vivobook. |

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
│   └─ linux/     procfs, sysfs readers, XDG autostart, systemd --user, helper client
└─ helper/       shimmy-helperd: fixed command set, clamped, logged
```

- Each capability is an `Option`: the UI already hides fan modes and the charge limit on unsupported PCs, and v2 extends that
  pattern to every root-only or OS-only control. A `capabilities` command tells the UI what exists.
- Paths come from one place per OS (`dirs` crate plus per-OS tables) instead of `%APPDATA%`, `C:\` and `.exe` names spread
  through `caches.rs`, `clean.rs`, `finder.rs` and `storage.rs`.
- **Safety rules do not loosen.** `classify()` stays the single gate for closing, with per-OS protection tables:
  - Linux: pid 1, kernel threads, `systemd*`, display servers and compositors (`Xorg`, `gnome-shell`, `kwin_*`, `plasmashell`),
    `pipewire`/`wireplumber`/`pulseaudio`, `dbus-*`, `polkitd`, `NetworkManager`, `sshd`, and processes owned by other users or
    system users (uid < 1000) in place of "session 0".
  - Shared: Shimmy and its WebView (`WebKitWebProcess` children), terminals, editors, Claude, Salon Shaila, security software (per-OS names).
  - Each table gets a parity-style fixture test from a real process snapshot of that OS.
- The deletion gates (`finder::allowed`, `storage::check_folder`) gain per-OS refusals: Linux `/usr`, `/etc`, `/var`, `/opt`,
  `/proc`, `/sys`, `/boot`, other mounts, `~/.local/share` except listed caches, dotfiles (hidden = name starts with `.`).
- Trash: the `trash` crate already supports the freedesktop Trash, so "Recycle Bin always" carries over.

## What each page becomes

| Page | Linux |
|---|---|
| Home | RAM %, CPU °C (hwmon `k10temp` Tctl / `coretemp` package), free space on `/` (or `/home` if separate), battery health |
| Memory | used, available, swap, PSI memory pressure (`/proc/pressure/memory`); map and suggestions; window titles on X11 via `_NET_CLIENT_LIST`; on Wayland, apps are matched to `.desktop` files instead (Wayland exposes no titles) |
| Processor | load, effective clocks (`scaling_cur_freq`), real topology (`topology/core_id`, `thread_siblings`, `cpu_capacity` and `/sys/devices/cpu_core`/`cpu_atom` for hybrid), temperature, package power from powercap RAPL (`energy_uj` is root-only since kernel 5.10 → helper), fan RPM from hwmon, fan modes via `platform_profile`, heat protection via boost |
| Graphics | NVML; amdgpu and Intel from sysfs; per-app from fdinfo; never wakes a suspended dGPU |
| Storage | `/` and other real mounts (skip tmpfs, squashfs, snap loops); NVMe health through the helper (NVMe SMART log ioctl), temperature from hwmon without root; usage categories: system (`/usr`), apps (Flatpak, Snap, `/opt`), user, reclaimable |
| Clean | `~/.cache` thumbnails, browser caches (Chrome, Chromium, Brave, Edge, Firefox `cache2`; native, Flatpak and Snap paths), Trash, crash reports (`/var/crash`, systemd-coredump via helper), journal vacuum and package caches (`apt`, `dnf`, `pacman -Sc`) through the helper and off by default. No DISM equivalent; old kernels are **never** touched. |
| Old caches | the same families with Linux paths (`~/.config/discord/Cache`, `~/.npm/_cacache`, `~/.cache/pip`, `~/.cargo/registry`…) |
| Battery | `power_supply/BAT*`: capacity, `energy_full` / `energy_full_design` (or `charge_*`), `cycle_count`, `power_now`; charge limit, hold mode |
| Battery history | Shimmy records its own daily capacity sample (there is no `batteryreport`) |
| Apps · Log | unchanged |
| Startup | XDG autostart `.desktop` files (`Hidden=true` to disable) and `systemd --user` units |
| Start with Windows → "Start at sign-in" | XDG autostart entry with `--tray` |
| Updates | per 10b, per package format |

## Tray and window
- **Linux:** the tray goes through AppIndicator / StatusNotifierItem. **Left-click is not delivered** on most desktops, so
  the menu gains "Open Shimmy" as its first item. GNOME needs the AppIndicator extension (preinstalled on Ubuntu, not on
  Fedora Workstation): without a tray host, closing the window quits instead of hiding, and Shimmy says so once.
- WebKitGTK: check the bundled Space Grotesk, canvas charts and CSS. Keep the CSP as is. The Phase 8 "destroy the WebView on close" saving carries over.

## Phases

| # | Phase | What "done" means | Release |
|---|---|---|---|
| v2-0 | **Spike the risky bits** on real hardware | No Linux machine is owned, so the Vivobook boots Ubuntu 24.04 from a **live USB** (Windows untouched): `asus-wmi` `platform_profile`, `charge_control_end_threshold`, `k10temp`, RAPL, NVIDIA runtime suspend, AppIndicator tray, WebKitGTK rendering. Results recorded here, as in Phase 0. Any failure changes the plan before the refactor. | none |
| v2-1 | **Platform layer on Windows only** | Today's code moved behind `platform/windows`, pure rules in `rules/`. **No behaviour change**: all Windows tests and the parity test pass. CI matrix builds Windows and runs the pure tests on Windows and Linux. | v0.7.0 |
| v2-2 | **Linux, monitor and clean** (no helper) | Every page reads real values on Ubuntu (live USB on the Vivobook; Fedora best-effort through CI); close (SIGTERM, then SIGKILL after a timeout, through `classify`), user-level clean and old caches to Trash, XDG startup and autostart, tray, updater for .deb/.rpm/AppImage. Linux protection table with a fixture test. | v0.8.0 (Linux beta) |
| v2-3 | **Linux controls** (helper) | `shimmy-helperd` with fixed commands: set charge limit, set platform profile, boost on/off, read RAPL, read NVMe SMART, journal vacuum, package cache clean. Hold mode, fan modes, heat protection, power section and SSD health on Linux. Helper fuzzed against bad input; every call logged. | v0.9.0 |
| v2-4 | **Polish and 1.0** | README, DOWNLOAD and screenshots for Windows and Linux; the taste-skill pass on WebKitGTK; install and uninstall checked end to end on both. | v1.0.0 |

**Ordering with Phase 10:** finish 10a to 10d first. 10c's topology code and 10d's vendor-neutral GPU code become the
Windows side of the platform trait in v2-1, so writing them now is not wasted. 10b's updater gains the per-format install step in v2-2.

## Risks and how they are handled

1. **Root on Linux.** A root helper is the biggest new attack surface. It takes a fixed command set with numbers clamped in the
   helper, no paths or shell strings from the caller, a peer-uid check, and systemd hardening (`ProtectSystem=strict`, `ReadWritePaths`
   limited to the sysfs files it writes, `NoNewPrivileges`). It gets its own tests and a fuzz target.
2. **Firmware writes on unknown laptops.** v0.4.2 already asks before the first ASUS firmware write on an untested model. On Linux the kernel
   driver has validated the call, so the same one-time confirmation applies, and Shimmy writes nothing the driver does not
   expose.
3. **Wayland gives no window titles**, which `quick_clean` uses as its "has a window" signal. After 10a, closing is driven by the
   Safe mark, not by windowlessness, so Linux is not weaker.
4. **Tray without a host** (stock GNOME): handled by quitting on close and saying so.
5. **Testing without a Linux machine.** CI covers builds and pure tests. Real-hardware checks stay manual: `tests/real_scan.rs` gets
   Linux read-only checks, run with `--ignored` from the live USB on the Vivobook.
6. **SmartScreen.** Unsigned builds warn once on Windows (see PLAN.md Phase 10). Linux packages are unsigned too; checksums come from the release.

## Not in scope (unless you ask)

macOS (dropped, see the top), ChromeOS, BSDs, 32-bit or ARM Linux, Linux without systemd for the helper (monitor-only still works), `drop_caches` as a "RAM clean", removing old kernels, registry-cleaner-style Linux "optimisations"
(swappiness tweaks and so on).

## Questions for the owner
1. Answered 2026-10-28: no Linux machine (a live USB on the Vivobook is proposed); macOS dropped; no Apple signing.
2. Is the Linux helper acceptable as a root systemd service, or monitor-only on Linux for now?
3. Should Trim stay Windows-only, as proposed?

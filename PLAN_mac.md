# Shimmy Optimizer for Mac: plan

Written 2026-10-31. One repo and one shared UI, with separate Windows and Mac system code. A real Mac app, not an exact
copy: macOS blocks several things Shimmy does on Windows. Mac behaviour below comes from general knowledge until Phase 1
tests it.

## Decisions
- **No root.** Shimmy runs as a normal app with no privileged helper. The main thing a helper would unlock is undocumented
  fan and charging control, which could damage the hardware.
- **Heat protection is a warning only**, because macOS has no supported way to turn off boost.
- **Full Disk Access is asked for only when a scan needs it.** Without it, Shimmy still works with reduced results.
- **Intel and Apple Silicon are both supported, macOS 13 or later**, shipped as one universal app.
- **Outside the Mac App Store**, whose sandbox would block most of the app.

## Pages on the Mac
| Page | On Mac | What changes |
|---|---|---|
| Home | Yes | Free space from the main disk instead of `C:` |
| Memory | Partly | Memory pressure, swap and closing apps; Trim is removed |
| Processor | Partly | Load and thermal state; fan and boost control are removed |
| Graphics | Partly | Overall GPU use; per-app use probably goes, temperatures need testing |
| Storage | Yes | Trash and APFS volumes; hibernation and restore-point controls are removed |
| Clean | Yes | New Mac list (`~/Library/Caches`, logs, Xcode and Homebrew caches); DISM is removed |
| Battery | Partly | Health and cycle count; charge limit and Run from the charger are removed |
| Apps | Partly | Closing apps stays; background priority through the Darwin background policy, not `nice` |
| Programs | Yes | Scans `/Applications`; uninstall moves apps to the Trash; updates through Homebrew if installed |
| Startup | Yes | Login Items and LaunchAgents; some can only be viewed or changed in System Settings |
| Connections | Partly | Without root, it may show only your own apps' connections |
| System | Yes | `system_profiler` and `sysctl` |
| Log, Settings | Yes | Mostly shared; Windows-only settings hidden |
| Updates | Yes | A Mac build and a new install step |

## Phases
0. **Prepare the repo** (done on branch `macos-version`). Windows code sits behind platform checks and the Mac target passes a
   compile check. Mac placeholders refuse to act (`platform.rs`).
1. **Test the riskiest parts on a real Mac:** sensors, GPU readings, safe deletes, Connections without root, Full Disk
   Access. What fails moves to the removed list before more is built. Run the shared tests there too; some were written
   with Windows paths in mind (`net.rs` labels, the `lib.rs` settings test, `tests/parity.rs`) and may need Mac versions.
   The Phase 1 probes live in `spike/mac` and run in `.github/workflows/macos.yml`. **Done on an Intel desktop Mac; Apple
   Silicon and battery are still open:** see "Phase 1 results".
2. **Mac foundation:** window, menu bar icon, data folder, settings, local time, start at login.
3. **Safety code, before any feature can delete or close anything:** `really_at`, `delete_exact` and `create_new` for Mac
   file handling; `classify()` with Mac protected-process lists and a rule for root-owned processes (the Windows service
   rule never fires on a Mac, so system processes would lose that protection); Mac rules for iCloud placeholders and hidden
   files; a redesign of the "too big for the Recycle Bin" path.
4. **Read-only pages:** Home, Memory, Processor, Graphics, System, Connections, Battery.
5. **Pages that change things:** Storage, Clean, Finder, Programs, Startup, Apps, automation.
6. **UI per platform:** a capabilities check so pages hide what the Mac cannot do, path handling, and the Windows wording
   (about 70 places). Test in WebKit.
7. **Package and release:** `.dmg`, a Mac update path with its own download name, Mac tests and GitHub Actions, README and
   website links. An Apple Developer account ($99 a year) is needed to sign and notarize for anyone else to use it.

## Phase 1 results
Written 2026-11-02, from CI run 41349971274 (two GitHub virtual machines, Apple Silicon and Intel) and from `spike/mac` on one
real Mac (iMac20,1, Intel i5-10600, AMD Radeon Pro 5300, macOS 26.7.1, no battery). Shared tests on that Mac: 117 unit tests
and 2 in `tests/parity.rs`, all pass. The reports stay out of the repo, because a real Mac's list its open ports and disks.

**Still open**
- Apple Silicon on real hardware, and any battery. This iMac has neither, so both need another Mac. The Battery page stays
  unproven, and temperatures on Apple Silicon are unknown.
- Permissions. macOS may have judged the iMac's run by the program that started the probe, not the probe. Check Full Disk
  Access, Desktop, Documents, Downloads and Finder automation again from the built app.
- The app has only been started on the virtual machines. The `prio.rs` fix and `--no-fail-fast` have not run in CI yet.

**Removed list**
- `nice` for background priority: a normal user cannot raise it back. Use the Darwin background policy.
- Finder as the way to reach the Trash: no answer in 25 s on the iMac. Use `NsFileManager`.
- `lsof` for Connections: it sees only your own apps. Use `netstat -anv`.

| Part | Virtual machines | Real iMac | What it means |
|---|---|---|---|
| App builds and starts | Works on both, and the universal build holds both architectures | Not run | Keep |
| Full Disk Access | Reported allowed | **Denied** for Mail, Safari, Messages and TCC | The virtual machines cannot test this. Ask only when a scan needs it |
| Thermal state | Works | Works | Keep |
| Temperatures and fans (SMC) | Intel only; no SMC on Apple Silicon | **Works without root**: 86 plausible temperatures, fan speed and limits | Read only, Intel only so far. Use a list of known keys: some `T` keys are not temperatures (`TC0E` reads -10.8) |
| GPU use | Not reported | **Works, but is not one number**: 3 accelerators, with the AMD chip listed twice (3 and 38 % at once) and the unused Intel chip as well | Keep, and use `PerformanceStatistics`, never `PerformanceStatisticsAccum`. Choose the number in Phase 4 by testing under a known load |
| Battery | No battery | No battery | **Unproven** |
| Memory | Works | Works | Keep |
| Connections | `netstat` sees root's listener with a process id, `lsof` does not | `netstat` 24 listening ports, `lsof` 10 | Parse both formats: macOS 26 prints `name:pid` under `process:pid`, names cut to 16 characters and possibly with spaces |
| Process owner (`sysinfo`) | No user id for 217 of 476 and 219 of 450 | **No user id for 310 of 823**, including `launchd` and `WindowServer` | See Phase 3 below |
| Priority with `nice` | Lowering works, and raising it back is refused | Same | **Removed** |
| Priority with the Darwin background policy | Set and put back both work | Same | Keep, on a throwaway process so far |
| Safe deletes | All work, including the swap attacks | All work | Keep |
| Trash with `NsFileManager` | Works | The file leaves its folder in 0.2 s, but `~/.Trash` is denied, so its arrival is unconfirmed | Keep. Reading the Trash's size needs Full Disk Access |
| Disks | Works | Works; adding up volumes gives a wrong total | Free space comes from the volume holding the home folder, and a scan skips `/System/Volumes/Data` |
| Start at login | Works | Works; nothing was added | Keep |

**For Phase 3:** the root-owned rule in `classify()` cannot rely on `sysinfo`'s user id, because it is missing for system
processes. A process with no readable owner must count as protected, never as closable. `delete_exact` should compare device
and inode just before `unlinkat`, and name comparisons must fold case and Unicode form, or compare inodes instead.

## Checking the Mac target from Windows
Rust alone does not compile for a Mac from Windows, because some dependencies build C code with Apple's tools. A check links
nothing, so it can use a stand-in `cc` and `ar` that write empty outputs (set `CC_<target>` and `AR_<target>`, with `-` as
`_`), with `rustup target add aarch64-apple-darwin x86_64-apple-darwin`, then in `app/src-tauri`:
`SHIMMY_NO_ADMIN=1 CARGO_TARGET_DIR=target-mac cargo check --all-targets --target <target>`, once for each of the two.
This proves the Rust types line up. It does not build an app, link, or run anything.

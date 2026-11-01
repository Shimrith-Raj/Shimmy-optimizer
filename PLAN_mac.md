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
| Apps | Partly | Closing apps stays; background priority only if it can be reliably undone |
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

## Checking the Mac target from Windows
Rust alone does not compile for a Mac from Windows, because some dependencies build C code with Apple's tools. A check links
nothing, so it can use a stand-in `cc` and `ar` that write empty outputs (set `CC_<target>` and `AR_<target>`, with `-` as
`_`), with `rustup target add aarch64-apple-darwin x86_64-apple-darwin`, then in `app/src-tauri`:
`SHIMMY_NO_ADMIN=1 CARGO_TARGET_DIR=target-mac cargo check --all-targets --target <target>`, once for each of the two.
This proves the Rust types line up. It does not build an app, link, or run anything.

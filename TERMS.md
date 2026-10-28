SHIMMY OPTIMIZER: TERMS OF USE

Last updated: 28 October 2026

Shimmy Optimizer ("Shimmy") is free, open-source software founded and built by Shimrith Raj. By installing or using Shimmy
you agree to these terms. If you do not agree, do not install or use it.


1. LICENCE

Shimmy's source code is licensed under the MIT licence (the LICENSE file). These terms explain how the app behaves and what
you accept by using it. They do not take away any right the MIT licence gives you. Third-party components included with
Shimmy are under their own licences (THIRD-PARTY-NOTICES.md).


2. WHAT SHIMMY DOES TO YOUR PC

Shimmy is a system tool and needs administrator rights. When you install and use it:

- The installer installs the PawnIO driver, a signed kernel driver made by a third party, so that CPU temperature, clocks
  and power can be read. PawnIO stays installed if you remove Shimmy; you can uninstall it separately from Windows' Installed
  apps.
- The installer creates a Task Scheduler task that starts Shimmy with administrator rights when you sign in. You can turn
  this off in Shimmy's Settings.
- Only when you ask it to (or when you turn on an automatic feature), Shimmy can:
  - close running programs, which can lose unsaved work in them;
  - delete files, folders and caches, by default to the Recycle Bin, and permanently only after a second confirmation;
  - change Windows power settings (CPU boost), restoring your original setting afterwards;
  - write settings to your laptop's firmware: the battery charge limit and the fan mode on supported ASUS laptops;
  - turn startup programs and hibernation on or off.

Every action Shimmy takes is written to its action log so you can see what happened.


3. YOUR RESPONSIBILITY

You are responsible for how you use Shimmy and for keeping backups of anything important. Check what you select before you
close programs or delete files. The ASUS controls were built and tested on one model (ASUS Vivobook M3500QC). On any other
model, Shimmy shows a warning before the first fan-mode or charge-limit change and writes nothing to the firmware unless
you confirm. If you confirm, you use these controls on that model at your own risk.


4. NO WARRANTY

Shimmy is provided "as is", without warranty of any kind, express or implied, including fitness for a particular purpose. It
may contain bugs. Readings such as temperature, battery health and power are estimates from your hardware's sensors and may
be inaccurate.


5. LIMITATION OF LIABILITY

To the fullest extent the law allows, Shimrith Raj and the contributors to Shimmy are not liable for any loss or damage
arising from installing or using Shimmy. This includes lost data, lost work, hardware or battery wear, and system problems.
Nothing in these terms limits liability that cannot be limited by law.


6. PRIVACY

Shimmy has no accounts, no telemetry and no ads. The only network use is the update check and the update download from
GitHub, which sees your IP address and the version of Shimmy you run, under GitHub's own privacy policy. Nothing else about
your PC is sent. The automatic daily check can be turned off on the Updates page. Shimmy's settings, history and action log
are stored only on your PC, in Shimmy's app-data folder, and you can delete them at any time.


7. TRADEMARKS

ASUS, Vivobook, MyASUS, NVIDIA, GeForce, RTX, AMD, Ryzen, Radeon, Windows and other names belong to their owners. Shimmy is
an independent project and is not affiliated with or endorsed by any of them.


8. CHANGES

These terms may be updated with new versions of Shimmy. The version included with each release applies to that release.


9. CONTACT

Questions, bug reports and feedback: https://github.com/Shimrith-Raj/Shimmy-optimizer/issues

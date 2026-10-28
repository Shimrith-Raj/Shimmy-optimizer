# Third-party notices

Shimmy Optimizer's own code is MIT-licensed (see [LICENSE](LICENSE)). The installer also ships the components below.
Each keeps its own licence; none of them is covered by Shimmy's MIT licence.

## Sensor reader (`resources/sensors/`)

Shimmy's sensor reader (`shimmy-sensors.cs`, MIT, part of Shimmy) uses these unmodified libraries:

| Component | Version | Licence | Source |
|---|---|---|---|
| LibreHardwareMonitorLib | 0.9.6 | MPL-2.0 | https://github.com/LibreHardwareMonitor/LibreHardwareMonitor |
| DiskInfoToolkit | 1.1.2 | MPL-2.0 | https://github.com/Blacktempel/DiskInfoToolkit |
| RAMSPDToolkit-NDD | 1.4.2 | MPL-2.0 | https://www.nuget.org/packages/RAMSPDToolkit-NDD |
| BlackSharp.Core | 1.0.7 | MPL-2.0 | https://github.com/Blacktempel/BlackSharp |
| HidSharp | 2.6.4 | Apache-2.0 | https://www.zer7.com/software/hidsharp |
| Microsoft .NET libraries (`System.*`, `Microsoft.Bcl.*`) | see files | MIT | https://github.com/dotnet/runtime |

The MPL-2.0 text ships next to them as `LICENSE-LibreHardwareMonitor.txt`. Under MPL-2.0 the source of these libraries is
available from the links above; Shimmy does not modify them.

## PawnIO driver (`resources/pawnio/PawnIO_setup.exe`)

| Component | Version | Licence | Source |
|---|---|---|---|
| PawnIO | 2.2.0 | GPL-2.0-or-later, with a special exception | https://github.com/namazso/PawnIO · https://pawnio.eu |

Copyright © namazso. PawnIO is a signed kernel driver that LibreHardwareMonitor uses to read CPU temperature, clocks and
power. Shimmy ships PawnIO's own unmodified installer, runs it during setup, and talks to the driver only through
LibreHardwareMonitor. PawnIO's licence includes an exception for "independent modules that communicate with PawnIO solely
through the device IO control". PawnIO stays installed as its own entry in Windows' Installed apps, and its complete source
code is available at the link above.

## Fonts

| Component | Licence | Source |
|---|---|---|
| Space Grotesk | SIL Open Font License 1.1 (`app/src/fonts/OFL.txt`) | https://github.com/floriankarsten/space-grotesk |

## Rust and JavaScript dependencies

The app is built with [Tauri](https://tauri.app) and Rust crates from crates.io (among them `windows`, `sysinfo`,
`nvml-wrapper`, `trash`, `rayon`, `blake3`, `regex` and `winreg`). These are under MIT and/or Apache-2.0 licences; the exact
list is in `app/src-tauri/Cargo.lock`.

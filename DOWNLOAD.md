# Download Shimmy Optimizer

## ⬇️ [Download Shimmy-Optimizer-Setup.exe](https://github.com/Shimrith-Raj/Shimmy-optimizer/releases/latest/download/Shimmy-Optimizer-Setup.exe)

This link always gets the newest version. You need Windows 10 or 11, 64-bit, and the file is about 8 MB.

**You won't find the program among the files in this repository.** This repository holds the source code. The ready-to-run
installer is attached to each [GitHub release](https://github.com/Shimrith-Raj/Shimmy-optimizer/releases), which is where
the link above points.

## Installing

1. Run `Shimmy-Optimizer-Setup.exe` and click **Yes** when Windows asks for administrator rights.
2. If Windows SmartScreen says *"Windows protected your PC"*, click **More info → Run anyway**. It appears because the
   installer isn't code-signed yet.
3. Shimmy opens and adds a tray icon next to the clock. The [README](README.md) has the full details, including how
   to uninstall.

## Older versions

Every version is on the [releases page](https://github.com/Shimrith-Raj/Shimmy-optimizer/releases). Open a release and
download `Shimmy-Optimizer-Setup.exe` under **Assets**.

## Checking the download

Each file on the releases page shows its SHA-256 checksum next to it. To compare, run this in PowerShell in the folder
where you saved the installer:

```powershell
Get-FileHash .\Shimmy-Optimizer-Setup.exe -Algorithm SHA256
```

The hash it prints should match the one on the releases page.

## Building it yourself

If you'd rather build the installer from source, see [Build it yourself](README.md#build-it-yourself) in the README.

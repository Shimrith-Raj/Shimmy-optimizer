// Shimmy sensor sidecar: reads LibreHardwareMonitor (needs admin and the PawnIO driver for CPU temperatures) and writes
// every reading as one JSON line per interval to stdout. It exits when Shimmy closes the pipe.
// Replaces sensors.ps1: the same output, without a PowerShell host (about 75 MB private before).
// Built by build.rs with the C# 5 compiler that ships in .NET Framework 4 (csc.exe); no SDK needed.
using System;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Text;
using System.Threading;
using LibreHardwareMonitor.Hardware;

static class ShimmySensors
{
  static string dir;

  static int Main(string[] args)
  {
    dir = AppDomain.CurrentDomain.BaseDirectory;
    // Any version of a dependency in this folder is accepted (LHM's dependencies ask for versions that differ slightly).
    AppDomain.CurrentDomain.AssemblyResolve += (s, e) =>
    {
      var n = new AssemblyName(e.Name).Name;
      if (n.EndsWith(".resources")) return null;
      var f = Path.Combine(dir, n + ".dll");
      return File.Exists(f) ? Assembly.LoadFrom(f) : null;
    };
    int interval = args.Length > 0 ? int.Parse(args[0], CultureInfo.InvariantCulture) : 2000;
    return Run(interval);
  }

  // Kept separate so LibreHardwareMonitor types are only loaded after the resolver is in place.
  static int Run(int interval)
  {
    var c = new Computer();
    // GPUs are left out on purpose: polling the RTX keeps it awake. Only what Shimmy reads is enabled.
    c.IsCpuEnabled = true;
    c.IsStorageEnabled = true;
    c.IsBatteryEnabled = true;
    c.Open();
    var stdout = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false));
    var sb = new StringBuilder(8192);
    for (long tick = 0; ; tick++)
    {
      sb.Clear();
      sb.Append('[');
      bool first = true;
      foreach (var h in c.Hardware)
      {
        // Drive health changes slowly; re-read it every 15th pass (30 s).
        if (h.HardwareType != HardwareType.Storage || tick % 15 == 0) h.Update();
        foreach (var sub in h.SubHardware) sub.Update();
        Emit(sb, h, ref first);
        foreach (var sub in h.SubHardware) Emit(sb, sub, ref first);
      }
      sb.Append(']');
      try
      {
        stdout.WriteLine(sb.ToString());
        stdout.Flush();
      }
      catch (IOException)
      {
        c.Close();
        return 0;
      }
      Thread.Sleep(interval);
    }
  }

  static void Emit(StringBuilder sb, IHardware x, ref bool first)
  {
    foreach (var s in x.Sensors)
    {
      if (!s.Value.HasValue || float.IsNaN(s.Value.Value) || float.IsInfinity(s.Value.Value)) continue;
      if (!first) sb.Append(',');
      first = false;
      sb.Append("{\"hw\":").Append(Json(x.Name))
        .Append(",\"type\":").Append(Json(x.HardwareType.ToString()))
        .Append(",\"sensor\":").Append(Json(s.Name))
        .Append(",\"kind\":").Append(Json(s.SensorType.ToString()))
        .Append(",\"value\":").Append(Math.Round((double)s.Value.Value, 2).ToString("R", CultureInfo.InvariantCulture))
        .Append('}');
    }
  }

  static string Json(string v)
  {
    var sb = new StringBuilder("\"");
    foreach (var ch in v ?? "")
    {
      if (ch == '"' || ch == '\\') sb.Append('\\').Append(ch);
      else if (ch < ' ') sb.Append("\\u").Append(((int)ch).ToString("x4"));
      else sb.Append(ch);
    }
    return sb.Append('"').ToString();
  }
}

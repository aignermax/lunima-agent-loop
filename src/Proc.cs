using System.Diagnostics;
using System.Text;

namespace AgentLoop;

public sealed record ProcResult(int ExitCode, string StdOut, string StdErr, bool TimedOut);

public static class Proc
{
    /// <summary>
    /// Full path of <paramref name="command"/> on PATH, or null. Searched in-process (PATH order,
    /// then PATHEXT order on Windows — same result as where.exe) so no `which` binary is needed.
    /// </summary>
    public static string? FindOnPath(string command)
    {
        const StringSplitOptions split = StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries;
        string[] extensions = OperatingSystem.IsWindows()
            ? (Environment.GetEnvironmentVariable("PATHEXT") ?? ".COM;.EXE;.BAT;.CMD").Split(';', split)
            : [""];
        foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? "").Split(Path.PathSeparator, split))
        {
            foreach (var extension in extensions)
            {
                var candidate = Path.Combine(dir.Trim('"'), command + extension);
                if (File.Exists(candidate)) return candidate;
            }
        }
        return null;
    }

    public static async Task<ProcResult> RunAsync(
        string fileName,
        string arguments,
        string? workingDirectory = null,
        TimeSpan? timeout = null,
        string? stdOutFile = null,
        IReadOnlyDictionary<string, string>? environment = null)
    {
        var psi = new ProcessStartInfo
        {
            FileName = fileName,
            Arguments = arguments,
            WorkingDirectory = workingDirectory ?? Environment.CurrentDirectory,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var (key, value) in environment ?? new Dictionary<string, string>())
            psi.Environment[key] = value;

        using var proc = new Process { StartInfo = psi, EnableRaisingEvents = true };
        var stdout = new StringBuilder();
        var stderr = new StringBuilder();
        StreamWriter? fileWriter = null;
        if (stdOutFile is not null)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(stdOutFile))!);
            fileWriter = new StreamWriter(stdOutFile, append: false) { AutoFlush = true };
        }

        proc.OutputDataReceived += (_, e) =>
        {
            if (e.Data is null) return;
            stdout.AppendLine(e.Data);
            fileWriter?.WriteLine(e.Data);
        };
        proc.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is null) return;
            stderr.AppendLine(e.Data);
            fileWriter?.WriteLine("[stderr] " + e.Data);
        };

        proc.Start();
        proc.BeginOutputReadLine();
        proc.BeginErrorReadLine();

        var effectiveTimeout = timeout ?? TimeSpan.FromMinutes(10);
        var wait = proc.WaitForExitAsync();
        var timedOut = false;
        if (await Task.WhenAny(wait, Task.Delay(effectiveTimeout)) != wait)
        {
            timedOut = true;
            try { proc.Kill(entireProcessTree: true); }
            catch (InvalidOperationException) { /* process already exited */ }
        }

        await wait; // lets remaining async output flush
        fileWriter?.Dispose();
        return new ProcResult(timedOut ? -1 : proc.ExitCode, stdout.ToString(), stderr.ToString(), timedOut);
    }
}

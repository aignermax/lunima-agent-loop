using System.Text;

namespace AgentLoop;

/// <summary>
/// Mirrors everything the loop prints (stdout + stderr) into logs/loop-yyyy-MM-dd.log.
/// The scheduled task runs windowless, so without this the actual failure reason
/// (e.g. the git error behind "branch setup failed") would be lost. The Control Center
/// shows these files.
/// </summary>
public static class LoopLog
{
    /// <summary>Starts mirroring console output for this process into the daily loop log.</summary>
    public static void Attach(string rootDir, string command)
    {
        try
        {
            var dir = Path.Combine(rootDir, "logs");
            Directory.CreateDirectory(dir);
            var path = Path.Combine(dir, $"loop-{DateTime.Now:yyyy-MM-dd}.log");
            var file = new StreamWriter(new FileStream(path, FileMode.Append, FileAccess.Write, FileShare.ReadWrite),
                new UTF8Encoding(false)) { AutoFlush = true };
            file.WriteLine($"[{DateTime.Now:HH:mm:ss}] ===== lunima-agent-loop {command} (pid {Environment.ProcessId}) =====");
            Console.SetOut(new TeeWriter(Console.Out, file, null));
            Console.SetError(new TeeWriter(Console.Error, file, "[stderr] "));
        }
        catch (IOException)
        {
            // logging must never stop the loop
        }
        catch (UnauthorizedAccessException)
        {
        }
    }

    private sealed class TeeWriter(TextWriter console, TextWriter file, string? prefix) : TextWriter
    {
        private static readonly object Gate = new();
        private bool _lineStart = true;

        public override Encoding Encoding => console.Encoding;

        public override void Write(char value)
        {
            console.Write(value);
            lock (Gate)
            {
                if (_lineStart && prefix is not null) file.Write(prefix);
                file.Write(value);
                _lineStart = value == '\n';
            }
        }

        public override void Write(string? value)
        {
            if (value is null) return;
            foreach (var c in value) Write(c);
        }

        public override void WriteLine(string? value)
        {
            Write(value);
            Write(Environment.NewLine);
        }
    }
}

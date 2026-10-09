using System.Text;

namespace AgentLoop;

/// <summary>
/// Mirrors everything the loop prints (stdout + stderr) into logs/loop-yyyy-MM-dd.log.
/// The scheduled task runs windowless, so without this the actual failure reason
/// (e.g. the git error behind "branch setup failed") would be lost. The Control Center
/// shows these files. Logging never stops the loop: file errors just disable the mirror.
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
                new UTF8Encoding(false));
            var sink = new FileSink(file);
            sink.WriteLine($"[{DateTime.Now:HH:mm:ss}] ===== lunima-agent-loop {command} (pid {Environment.ProcessId}) =====");
            Console.SetOut(new TeeWriter(Console.Out, sink, ""));
            Console.SetError(new TeeWriter(Console.Error, sink, "[stderr] "));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            // no mirror this run
        }
    }

    /// <summary>Whole-line writes (one syscall per line, so concurrent processes don't interleave mid-line).</summary>
    private sealed class FileSink(StreamWriter file)
    {
        private readonly object _gate = new();
        private bool _broken;

        public void WriteLine(string line)
        {
            lock (_gate)
            {
                if (_broken) return;
                try
                {
                    file.Write(line + Environment.NewLine);
                    file.Flush();
                }
                catch (Exception e) when (e is IOException or ObjectDisposedException or UnauthorizedAccessException)
                {
                    _broken = true;
                }
            }
        }
    }

    /// <summary>Passes everything to the console and buffers it into complete lines for the file.</summary>
    private sealed class TeeWriter(TextWriter console, FileSink sink, string prefix) : TextWriter
    {
        private readonly StringBuilder _line = new();

        public override Encoding Encoding => console.Encoding;

        public override void Write(char value)
        {
            console.Write(value);
            if (value == '\n')
            {
                sink.WriteLine(prefix + _line.ToString().TrimEnd('\r'));
                _line.Clear();
            }
            else
            {
                _line.Append(value);
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
            Write('\n');
        }

        public override void Flush() => console.Flush();
    }
}

namespace AgentLoop;

/// <summary>
/// Runs one headless Kimi Code CLI invocation:
///   kimi -p "&lt;short prompt&gt;" --output-format stream-json -m &lt;model&gt;
/// -p mode never asks for approvals (auto permission policy) — perfect for unattended runs.
/// The full task description lives in a file inside the clone; the -p prompt only points at it,
/// so we never hit command-line length limits.
/// </summary>
public sealed class KimiRunner
{
    private readonly string _kimiExe;

    private KimiRunner(string kimiExe) => _kimiExe = kimiExe;

    public static async Task<KimiRunner> CreateAsync()
    {
        var where = await Proc.RunAsync("where.exe", "kimi");
        var exe = where.StdOut.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .FirstOrDefault();
        if (where.ExitCode != 0 || exe is null)
            throw new InvalidOperationException("kimi CLI not found on PATH (where.exe kimi failed).");
        return new KimiRunner(exe);
    }

    public Task<ProcResult> RunAsync(
        string model,
        string shortPrompt,
        string workingDirectory,
        string logFile,
        int timeoutMinutes)
    {
        // shortPrompt is authored by us and contains no quotes.
        var args = $"-p \"{shortPrompt}\" --output-format stream-json -m \"{model}\"";
        return Proc.RunAsync(_kimiExe, args, workingDirectory, TimeSpan.FromMinutes(timeoutMinutes), logFile);
    }
}

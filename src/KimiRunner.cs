namespace AgentLoop;

/// <summary>
/// Runs one headless Kimi Code CLI invocation:
///   kimi -p "&lt;short prompt&gt;" --output-format stream-json -m &lt;model&gt;
/// -p mode never asks for approvals (auto permission policy) — perfect for unattended runs.
/// The full task description lives in a file inside the clone; the -p prompt only points at it,
/// so we never hit command-line length limits.
/// </summary>
public sealed class KimiRunner : IAgentRunner
{
    private readonly string _kimiExe;

    private KimiRunner(string kimiExe) => _kimiExe = kimiExe;

    public static Task<KimiRunner> CreateAsync()
    {
        var exe = Proc.FindOnPath("kimi")
            ?? throw new InvalidOperationException("kimi CLI not found on PATH.");
        return Task.FromResult(new KimiRunner(exe));
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

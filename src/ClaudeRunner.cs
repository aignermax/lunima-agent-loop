namespace AgentLoop;

/// <summary>
/// Runs one headless Claude Code CLI invocation:
///   claude -p "&lt;short prompt&gt;" --model &lt;model&gt; --output-format stream-json --verbose
///          --dangerously-skip-permissions
/// Used for the Product-Owner pass: premium model quality plus Claude Code's
/// built-in WebSearch/WebFetch, so the PO can research product-market fit
/// like a founder — and make small fixes directly (it can edit + commit).
/// </summary>
public sealed class ClaudeRunner : IAgentRunner
{
    private readonly string _claudeExe;

    private ClaudeRunner(string claudeExe) => _claudeExe = claudeExe;

    public static async Task<ClaudeRunner> CreateAsync()
    {
        var where = await Proc.RunAsync("where.exe", "claude");
        var exe = where.StdOut.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .FirstOrDefault();
        if (where.ExitCode != 0 || exe is null)
            throw new InvalidOperationException("claude CLI not found on PATH (where.exe claude failed).");
        return new ClaudeRunner(exe);
    }

    public Task<ProcResult> RunAsync(
        string model,
        string shortPrompt,
        string workingDirectory,
        string logFile,
        int timeoutMinutes)
    {
        // shortPrompt is authored by us and contains no quotes.
        // stream-json in -p mode requires --verbose; --max-turns bounds runaway passes.
        var args = $"-p \"{shortPrompt}\" --model \"{model}\" --output-format stream-json --verbose " +
                   "--max-turns 150 --dangerously-skip-permissions";
        return Proc.RunAsync(_claudeExe, args, workingDirectory, TimeSpan.FromMinutes(timeoutMinutes), logFile);
    }
}

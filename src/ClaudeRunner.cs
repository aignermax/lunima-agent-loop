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
    /// <summary>
    /// A headless -p session ends as soon as the model ends its turn, so a command left running
    /// in the background loses its result. Disable background tasks and let foreground Bash
    /// commands run up to 30 min (the tool's default cap is 2 min, max 10 min) — long enough
    /// for a full test suite or bake.
    /// </summary>
    private static readonly Dictionary<string, string> HeadlessEnvironment = new()
    {
        ["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"] = "1",
        ["BASH_DEFAULT_TIMEOUT_MS"] = "1800000",
        ["BASH_MAX_TIMEOUT_MS"] = "1800000",
    };

    private readonly string _claudeExe;

    private ClaudeRunner(string claudeExe) => _claudeExe = claudeExe;

    public static Task<ClaudeRunner> CreateAsync()
    {
        var exe = Proc.FindOnPath("claude")
            ?? throw new InvalidOperationException("claude CLI not found on PATH.");
        return Task.FromResult(new ClaudeRunner(exe));
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
        return Proc.RunAsync(_claudeExe, args, workingDirectory, TimeSpan.FromMinutes(timeoutMinutes), logFile, HeadlessEnvironment);
    }
}

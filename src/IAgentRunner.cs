namespace AgentLoop;

/// <summary>
/// One headless agent-CLI invocation. Implementations wrap a specific CLI
/// (Kimi Code, Claude Code); the orchestrator picks per pass — e.g. cheap
/// worker model for implementation, premium Claude for the Product-Owner
/// pass (which also gains Claude Code's built-in web research tools).
/// </summary>
public interface IAgentRunner
{
    Task<ProcResult> RunAsync(
        string model,
        string shortPrompt,
        string workingDirectory,
        string logFile,
        int timeoutMinutes);
}

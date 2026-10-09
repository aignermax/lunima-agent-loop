using System.Text.Json;

namespace AgentLoop;

public sealed class LoopConfig
{
    public string GitHubRepo { get; set; } = "aignermax/Lunima";
    public string ClonePath { get; set; } = "";
    public string IntegrationBranch { get; set; } = "dev";
    public string BaseBranch { get; set; } = "main";
    public int MaxTasksPerDay { get; set; } = 2;
    /// <summary>Minimum minutes between Product-Owner passes. Idle passes are skipped entirely (no API cost).</summary>
    public int OwnerIntervalMinutes { get; set; } = 60;
    public string WorkerModel { get; set; } = "moonshot-ai/kimi-k2.7-code";
    public string OwnerModel { get; set; } = "moonshot-ai/kimi-k3";
    /// <summary>CLI for the Product-Owner pass: "kimi" or "claude". Claude gives the PO
    /// premium model quality plus built-in web research (product-market-fit work).</summary>
    public string OwnerRunner { get; set; } = "kimi";
    public int WorkerTimeoutMinutes { get; set; } = 120;
    public int OwnerTimeoutMinutes { get; set; } = 60;
    public string TaskLabel { get; set; } = "agent-task";
    public string PrLabel { get; set; } = "agent-pr";
    public string BlockedLabel { get; set; } = "needs-human";
    /// <summary>Claim label: set on an issue while a worker runs, so a second machine won't start the same issue.</summary>
    public string RunningLabel { get; set; } = "agent-running";
    public bool Enabled { get; set; } = true;
    /// <summary>False = Product-Owner only: this loop's own workers stay idle because another
    /// coder (the autonomous issue agent) implements the agent-task issues. The PO still reviews
    /// and merges every agent-pr PR.</summary>
    public bool WorkersEnabled { get; set; } = true;
    /// <summary>Opt in on an unlocked dedicated desktop with the UX Python dependencies.</summary>
    public bool CustomerEnabled { get; set; }
    public string CustomerPython { get; set; } = "python";
    public string CustomerModel { get; set; } = "claude-fable-5-1";
    public string CustomerProject { get; set; } = "CAP.Desktop/CAP.Desktop.csproj";
    public int CustomerMaxReviewsPerCycle { get; set; } = 2;
    public int CustomerMaxAgeHours { get; set; } = 24;
    public int CustomerMaxSteps { get; set; } = 80;
    public int CustomerMaxTurns { get; set; } = 60;
    public int CustomerTimeoutMinutes { get; set; } = 20;

    public static LoopConfig Load(string path)
    {
        var json = File.ReadAllText(path);
        var config = JsonSerializer.Deserialize(json, JsonContext.Default.LoopConfig);
        if (config is null) throw new InvalidOperationException($"Could not parse config: {path}");
        if (string.IsNullOrWhiteSpace(config.ClonePath))
            throw new InvalidOperationException($"clonePath is not set in {path}");
        if (config.CustomerEnabled && (config.CustomerMaxReviewsPerCycle is < 1 or > 10 ||
            config.CustomerMaxAgeHours is < 1 or > 168 || config.CustomerMaxSteps is < 1 or > 500 ||
            config.CustomerMaxTurns is < 1 or > 500 || config.CustomerTimeoutMinutes is < 1 or > 120 ||
            string.IsNullOrWhiteSpace(config.CustomerPython) || string.IsNullOrWhiteSpace(config.CustomerModel)))
            throw new InvalidOperationException("Invalid customer configuration: check positive budgets and Python/model settings.");
        return config;
    }
}

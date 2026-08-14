using System.Text.Json;

namespace AgentLoop;

public sealed class LoopConfig
{
    public string GitHubRepo { get; set; } = "aignermax/Lunima";
    public string ClonePath { get; set; } = "";
    public string IntegrationBranch { get; set; } = "dev-ki";
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

    public static LoopConfig Load(string path)
    {
        var json = File.ReadAllText(path);
        var config = JsonSerializer.Deserialize<LoopConfig>(json, new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true,
            ReadCommentHandling = JsonCommentHandling.Skip,
        });
        if (config is null) throw new InvalidOperationException($"Could not parse config: {path}");
        if (string.IsNullOrWhiteSpace(config.ClonePath))
            throw new InvalidOperationException($"clonePath is not set in {path}");
        return config;
    }
}

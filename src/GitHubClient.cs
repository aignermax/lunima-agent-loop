using System.Text.Json;

namespace AgentLoop;

public sealed record IssueSummary(int Number, string Title, string Body, string[] Labels, DateTime CreatedAt);
public sealed record PrSummary(int Number, string Title, string HeadRef);

/// <summary>
/// Thin read-only wrapper around the `gh` CLI (which carries the user's GitHub auth).
/// All writes (comments, labels, PRs, merges) are done by the agent itself inside its kimi run.
/// </summary>
public sealed class GitHubClient
{
    private readonly string _repo;

    public GitHubClient(string repo) => _repo = repo;

    public async Task<List<IssueSummary>> ListOpenIssuesAsync(string label)
    {
        var result = await Proc.RunAsync("gh",
            $"issue list --repo {_repo} --state open --label \"{label}\" --limit 100 --json number,title,body,labels,createdAt");
        ThrowIfFailed(result, "gh issue list");

        var issues = new List<IssueSummary>();
        using var doc = JsonDocument.Parse(result.StdOut);
        foreach (var el in doc.RootElement.EnumerateArray())
        {
            issues.Add(new IssueSummary(
                el.GetProperty("number").GetInt32(),
                el.GetProperty("title").GetString() ?? "",
                el.GetProperty("body").GetString() ?? "",
                el.GetProperty("labels").EnumerateArray().Select(l => l.GetProperty("name").GetString() ?? "").ToArray(),
                el.GetProperty("createdAt").GetDateTime()));
        }
        return issues;
    }

    public async Task<List<PrSummary>> ListOpenPrsAsync(string label)
    {
        var result = await Proc.RunAsync("gh",
            $"pr list --repo {_repo} --state open --label \"{label}\" --limit 100 --json number,title,headRefName");
        ThrowIfFailed(result, "gh pr list");

        var prs = new List<PrSummary>();
        using var doc = JsonDocument.Parse(result.StdOut);
        foreach (var el in doc.RootElement.EnumerateArray())
        {
            prs.Add(new PrSummary(
                el.GetProperty("number").GetInt32(),
                el.GetProperty("title").GetString() ?? "",
                el.GetProperty("headRefName").GetString() ?? ""));
        }
        return prs;
    }

    private static void ThrowIfFailed(ProcResult result, string what)
    {
        if (result.ExitCode != 0)
            throw new InvalidOperationException($"{what} failed ({result.ExitCode}): {result.StdErr}");
    }
}

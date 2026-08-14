using System.Diagnostics;
using System.Text;

namespace AgentLoop;

/// <summary>
/// The loop itself. Stateless between invocations: all durable state lives in GitHub
/// (issues, labels, PRs, branches) plus a tiny JSON file for daily budget counters.
///
/// Commands:
///   init   — clone the target repo, ensure the integration branch exists on origin
///   work   — pick open `agent-task` issues (within the daily cap) and run one kimi worker per issue
///   own    — one Product-Owner kimi pass (review/merge agent PRs, groom + seed the backlog)
///   run    — own (if due today) then work; this is what the scheduler calls
///   status — print config, today's counters and recent runs
/// </summary>
public sealed class LoopOrchestrator
{
    private readonly LoopConfig _config;
    private readonly string _rootDir;
    private readonly GitHubClient _gh;
    private readonly StateStore _state;
    private readonly IAgentRunner _worker;
    private readonly IAgentRunner _owner;

    public LoopOrchestrator(LoopConfig config, string rootDir, GitHubClient gh, StateStore state,
                            IAgentRunner worker, IAgentRunner owner)
    {
        _config = config;
        _rootDir = rootDir;
        _gh = gh;
        _state = state;
        _worker = worker;
        _owner = owner;
    }

    private string LogsDir => Path.Combine(_rootDir, "logs");
    private string PromptsDir => Path.Combine(_rootDir, "prompts");

    public async Task<int> InitAsync()
    {
        if (!Directory.Exists(Path.Combine(_config.ClonePath, ".git")))
        {
            Log($"Cloning {_config.GitHubRepo} → {_config.ClonePath} ...");
            var clone = await Proc.RunAsync("gh", $"repo clone {_config.GitHubRepo} \"{_config.ClonePath}\"",
                timeout: TimeSpan.FromMinutes(30));
            if (clone.ExitCode != 0)
            {
                Console.Error.WriteLine("Clone failed: " + clone.StdErr);
                return 1;
            }
        }

        await Git("fetch origin --prune");

        var ls = await Git($"ls-remote --heads origin {_config.IntegrationBranch}");
        if (string.IsNullOrWhiteSpace(ls.StdOut))
        {
            Log($"Creating integration branch '{_config.IntegrationBranch}' from origin/{_config.BaseBranch} ...");
            var create = await Git($"checkout -b {_config.IntegrationBranch} origin/{_config.BaseBranch}");
            if (create.ExitCode != 0) { Console.Error.WriteLine(create.StdErr); return 1; }
            var push = await Git($"push -u origin {_config.IntegrationBranch}");
            if (push.ExitCode != 0) { Console.Error.WriteLine(push.StdErr); return 1; }
        }
        else
        {
            Log($"Integration branch '{_config.IntegrationBranch}' already exists on origin.");
        }

        // keep our prompt files out of commits: local exclude only, repo stays untouched
        var excludeFile = Path.Combine(_config.ClonePath, ".git", "info", "exclude");
        var exclude = File.Exists(excludeFile) ? File.ReadAllText(excludeFile) : "";
        if (!exclude.Contains(".agent-loop/"))
            File.AppendAllText(excludeFile, "\n.agent-loop/\n");

        Directory.CreateDirectory(LogsDir);
        Directory.CreateDirectory(Path.Combine(_rootDir, "state"));
        Log("init done.");
        return 0;
    }

    public async Task<int> RunOnceAsync()
    {
        if (!_config.Enabled)
        {
            Log("Agent loop is disabled in agent-loop.json (enabled=false). Nothing to do.");
            return 0;
        }
        if (!OwnerDue())
        {
            Log($"Owner pass not due yet (interval {_config.OwnerIntervalMinutes} min, last {_state.LastOwnerRun:HH:mm:ss}).");
        }
        else if (await OwnerHasWorkAsync())
        {
            await OwnAsync();
        }
        else
        {
            Log("Owner pass due, but nothing to do (no open agent PRs, backlog healthy) — skipped, no API cost.");
        }
        await WorkAsync();
        return 0;
    }

    private bool OwnerDue()
    {
        if (_state.LastOwnerRun is null) return true;
        return DateTime.Now - _state.LastOwnerRun.Value >= TimeSpan.FromMinutes(_config.OwnerIntervalMinutes);
    }

    /// <summary>Cheap pre-flight (two gh calls, no LLM): is there anything for the Product Owner to do?</summary>
    private async Task<bool> OwnerHasWorkAsync()
    {
        var openPrs = await _gh.ListOpenPrsAsync(_config.PrLabel);
        if (openPrs.Count > 0) return true;
        var openIssues = await _gh.ListOpenIssuesAsync(_config.TaskLabel);
        return openIssues.Count < 5; // thin backlog needs seeding
    }

    public async Task<int> WorkAsync()
    {
        if (!_config.Enabled) { Log("Disabled (enabled=false)."); return 0; }

        var capacity = _config.MaxTasksPerDay - _state.Today().Tasks;
        if (capacity <= 0)
        {
            Log($"Daily task cap reached ({_config.MaxTasksPerDay}/day). See you tomorrow.");
            return 0;
        }

        var issues = await _gh.ListOpenIssuesAsync(_config.TaskLabel);
        var openPrs = await _gh.ListOpenPrsAsync(_config.PrLabel);
        var candidates = issues
            .Where(i => !i.Labels.Contains(_config.BlockedLabel))
            .Where(i => !i.Labels.Contains(_config.RunningLabel)) // claimed by a worker (maybe on another machine)
            .Where(i => !openPrs.Any(p => p.HeadRef.StartsWith($"agent/issue-{i.Number}-", StringComparison.Ordinal)))
            .OrderBy(i => i.CreatedAt)
            .Take(capacity)
            .ToList();

        if (candidates.Count == 0)
        {
            Log("No open agent-task issues to work on.");
            return 0;
        }

        foreach (var issue in candidates)
            await RunTaskAsync(issue);
        return 0;
    }

    private async Task RunTaskAsync(IssueSummary issue)
    {
        var branch = $"agent/issue-{issue.Number}-{DateTimeOffset.UtcNow.ToUnixTimeSeconds()}";
        Log($"Task: issue #{issue.Number} — {issue.Title}");
        Log($"Branch: {branch}");

        // Claim the issue first: with the claim label visible in GitHub, no second machine starts the same work.
        var claimed = await _gh.AddLabelAsync(issue.Number, _config.RunningLabel);
        if (!claimed)
        {
            Log($"Could not claim issue #{issue.Number} — skipping (another machine may be on it).");
            return;
        }

        try
        {
            await Git("fetch origin");
            var checkout = await Git($"checkout -B {branch} origin/{_config.IntegrationBranch}");
            if (checkout.ExitCode != 0)
            {
                Console.Error.WriteLine("Branch setup failed: " + checkout.StdErr);
                _state.RecordRun(new RunRecord { Kind = "task", Issue = issue.Number, Branch = branch, ExitCode = checkout.ExitCode, Note = "branch setup failed" });
                return;
            }

            var promptFile = Path.Combine(_config.ClonePath, ".agent-loop", $"task-{issue.Number}.md");
            Directory.CreateDirectory(Path.GetDirectoryName(promptFile)!);
            File.WriteAllText(promptFile, RenderWorkerPrompt(issue, branch));

            var logFile = Path.Combine(LogsDir, $"{DateTime.Now:yyyy-MM-dd_HHmmss}_task-{issue.Number}.jsonl");
            var shortPrompt =
                $"Read the file .agent-loop/task-{issue.Number}.md in the current repository and execute it exactly. " +
                "Every rule in that file is binding. Work fully autonomously until the task is done or clearly blocked.";

            var sw = Stopwatch.StartNew();
            var result = await _worker.RunAsync(_config.WorkerModel, shortPrompt, _config.ClonePath, logFile, _config.WorkerTimeoutMinutes);
            sw.Stop();

            _state.Today().Tasks++;
            _state.RecordRun(new RunRecord
            {
                Kind = "task",
                Issue = issue.Number,
                Branch = branch,
                ExitCode = result.ExitCode,
                DurationSec = sw.Elapsed.TotalSeconds,
                Note = result.TimedOut ? "timeout" : null,
            });

            Log($"Task #{issue.Number} finished: exit={result.ExitCode}{(result.TimedOut ? " (TIMEOUT)" : "")}, " +
                $"{sw.Elapsed.TotalMinutes:F1} min, log: {logFile}");
        }
        finally
        {
            await _gh.RemoveLabelAsync(issue.Number, _config.RunningLabel);
        }
    }

    public async Task<int> OwnAsync()
    {
        Log("Product-Owner pass starting.");
        _state.MarkOwnerRun();
        await Git("fetch origin");
        await Git($"checkout {_config.IntegrationBranch}");
        await Git("pull --ff-only"); // best effort; the PO agent resolves/reports from there

        var roadmapFile = Path.Combine(_config.ClonePath, "docs", "ROADMAP.md");
        var roadmap = File.Exists(roadmapFile) ? File.ReadAllText(roadmapFile) : "(docs/ROADMAP.md not found in clone)";

        var openPrs = await _gh.ListOpenPrsAsync(_config.PrLabel);
        var openIssues = await _gh.ListOpenIssuesAsync(_config.TaskLabel);
        var prList = openPrs.Count == 0 ? "(none)" : string.Join("\n", openPrs.Select(p => $"- PR #{p.Number}: {p.Title} (branch {p.HeadRef})"));
        var issueList = openIssues.Count == 0 ? "(none)" : string.Join("\n", openIssues.Select(i => $"- #{i.Number}: {i.Title}"));

        var promptFile = Path.Combine(_config.ClonePath, ".agent-loop", "owner-pass.md");
        Directory.CreateDirectory(Path.GetDirectoryName(promptFile)!);
        File.WriteAllText(promptFile, RenderOwnerPrompt(roadmap, prList, issueList));

        var logFile = Path.Combine(LogsDir, $"{DateTime.Now:yyyy-MM-dd_HHmmss}_owner.jsonl");
        var shortPrompt =
            "Read the file .agent-loop/owner-pass.md in the current repository and execute it exactly. " +
            "Every rule in that file is binding. Work fully autonomously.";

        var sw = Stopwatch.StartNew();
        var result = await _owner.RunAsync(_config.OwnerModel, shortPrompt, _config.ClonePath, logFile, _config.OwnerTimeoutMinutes);
        sw.Stop();

        _state.Today().OwnerRuns++;
        _state.RecordRun(new RunRecord
        {
            Kind = "owner",
            ExitCode = result.ExitCode,
            DurationSec = sw.Elapsed.TotalSeconds,
            Note = result.TimedOut ? "timeout" : null,
        });

        Log($"Owner pass finished: exit={result.ExitCode}{(result.TimedOut ? " (TIMEOUT)" : "")}, " +
            $"{sw.Elapsed.TotalMinutes:F1} min, log: {logFile}");
        return result.ExitCode;
    }

    public int Status()
    {
        var today = _state.Today();
        Console.WriteLine($"Repo:              {_config.GitHubRepo}");
        Console.WriteLine($"Clone:             {_config.ClonePath}");
        Console.WriteLine($"Integration:       {_config.IntegrationBranch} (base: {_config.BaseBranch})");
        Console.WriteLine($"Enabled:           {_config.Enabled}");
        Console.WriteLine($"Models:            worker={_config.WorkerModel}, owner={_config.OwnerModel}");
        Console.WriteLine($"Caps:              {_config.MaxTasksPerDay} tasks/day, owner every {_config.OwnerIntervalMinutes} min (idle passes skipped, no API cost)");
        Console.WriteLine($"Last owner run:    {(_state.LastOwnerRun?.ToString("yyyy-MM-dd HH:mm:ss") ?? "never")}");
        Console.WriteLine($"Today:             {today.Tasks} task(s), {today.OwnerRuns} owner run(s)");
        Console.WriteLine("Recent runs:");
        foreach (var r in _state.RecentRuns(10))
            Console.WriteLine($"  {r.Timestamp}  {r.Kind,-6} issue={r.Issue?.ToString() ?? "-",-5} exit={r.ExitCode,-4} {r.DurationSec / 60:F1} min {(r.Note ?? "")}");
        return 0;
    }

    private string RenderWorkerPrompt(IssueSummary issue, string branch) =>
        File.ReadAllText(Path.Combine(PromptsDir, "worker.md"))
            .Replace("{REPO}", _config.GitHubRepo)
            .Replace("{ISSUE_NUMBER}", issue.Number.ToString())
            .Replace("{ISSUE_TITLE}", issue.Title)
            .Replace("{ISSUE_BODY}", string.IsNullOrWhiteSpace(issue.Body) ? "(no description — use the title and your judgement)" : issue.Body)
            .Replace("{BRANCH}", branch)
            .Replace("{INTEGRATION_BRANCH}", _config.IntegrationBranch)
            .Replace("{BASE_BRANCH}", _config.BaseBranch)
            .Replace("{PR_LABEL}", _config.PrLabel)
            .Replace("{BLOCKED_LABEL}", _config.BlockedLabel)
            .Replace("{DATE}", DateTime.Now.ToString("yyyy-MM-dd"));

    private string RenderOwnerPrompt(string roadmap, string prList, string issueList) =>
        File.ReadAllText(Path.Combine(PromptsDir, "owner.md"))
            .Replace("{REPO}", _config.GitHubRepo)
            .Replace("{INTEGRATION_BRANCH}", _config.IntegrationBranch)
            .Replace("{BASE_BRANCH}", _config.BaseBranch)
            .Replace("{TASK_LABEL}", _config.TaskLabel)
            .Replace("{PR_LABEL}", _config.PrLabel)
            .Replace("{BLOCKED_LABEL}", _config.BlockedLabel)
            .Replace("{DATE}", DateTime.Now.ToString("yyyy-MM-dd HH:mm"))
            .Replace("{ROADMAP}", roadmap)
            .Replace("{OPEN_PRS}", prList)
            .Replace("{OPEN_TASK_ISSUES}", issueList);

    private Task<ProcResult> Git(string args) =>
        Proc.RunAsync("git", args, _config.ClonePath, TimeSpan.FromMinutes(30));

    private static void Log(string message) =>
        Console.WriteLine($"[{DateTime.Now:HH:mm:ss}] {message}");
}

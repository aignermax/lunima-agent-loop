using AgentLoop;

if (args.Length == 0)
{
    PrintUsage();
    return 1;
}

var command = args[0].ToLowerInvariant();
if (command is "--help" or "-h" or "help")
{
    PrintUsage();
    return 0;
}

// locate the tool root: walk up from cwd until agent-loop.json (or the example) appears
var root = FindRoot(Environment.CurrentDirectory);
if (root is null)
{
    Console.Error.WriteLine("agent-loop.json not found (searched upward from current directory).");
    Console.Error.WriteLine("Run lunima-agent-loop from within the agent-loop repository.");
    return 1;
}

var configPath = Path.Combine(root, "agent-loop.json");
if (!File.Exists(configPath))
{
    var example = Path.Combine(root, "agent-loop.example.json");
    if (!File.Exists(example))
    {
        Console.Error.WriteLine($"No config and no example config in {root}.");
        return 1;
    }
    File.Copy(example, configPath);
    Console.WriteLine($"Created {configPath} from the example — review it before real runs.");
}

try
{
    var config = LoopConfig.Load(configPath);
    var state = new StateStore(Path.Combine(root, "state", "state.json"));
    var gh = new GitHubClient(config.GitHubRepo);
    var kimi = await KimiRunner.CreateAsync();
    var loop = new LoopOrchestrator(config, root, gh, state, kimi);

    return command switch
    {
        "init" => await loop.InitAsync(),
        "work" => await loop.WorkAsync(),
        "own" => await loop.OwnAsync(),
        "run" => await loop.RunOnceAsync(),
        "status" => loop.Status(),
        _ => Unknown(command),
    };
}
catch (Exception ex)
{
    Console.Error.WriteLine($"agent-loop failed: {ex.Message}");
    return 1;
}

static int Unknown(string command)
{
    Console.Error.WriteLine($"Unknown command: {command}");
    PrintUsage();
    return 1;
}

static string? FindRoot(string start)
{
    var dir = new DirectoryInfo(start);
    while (dir is not null)
    {
        if (File.Exists(Path.Combine(dir.FullName, "agent-loop.json")) ||
            File.Exists(Path.Combine(dir.FullName, "agent-loop.example.json")))
            return dir.FullName;
        dir = dir.Parent;
    }
    return null;
}

static void PrintUsage()
{
    Console.WriteLine("""
        lunima-agent-loop — autonomous issue → PR loop for the Lunima project

        Usage: lunima-agent-loop <command>

        Commands:
          init     Clone the target repo, ensure the integration branch exists on origin
          run      Product-Owner pass (if due today) + work agent-task issues — what the scheduler calls
          work     Work open 'agent-task' issues (within the daily cap), one kimi run per issue
          own      Single Product-Owner pass (review/merge agent PRs, groom + seed backlog)
          status   Show config, today's counters and recent runs
        """);
}

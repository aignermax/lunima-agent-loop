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

string[] knownCommands = ["init", "work", "own", "run", "customer", "pause", "resume", "status"];
if (!knownCommands.Contains(command))
    return Unknown(command);

try
{
    // locate the tool root: walk up from cwd, then from the executable's folder, until
    // agent-loop.json (or the example) appears. Otherwise (installed binary, e.g. via MSI into
    // read-only Program Files) config, state and logs live in the per-user data folder.
    var root = FindRoot(Environment.CurrentDirectory)
        ?? FindRoot(AppContext.BaseDirectory)
        ?? DefaultDataDir();
    Directory.CreateDirectory(root);

    var configPath = Path.Combine(root, "agent-loop.json");
    if (!File.Exists(configPath))
    {
        // the example ships inside the binary (a copy on disk wins) with "enabled": false,
        // so nothing runs until a human has reviewed it — even if a scheduler keeps firing.
        File.WriteAllText(configPath, EmbeddedFiles.Read(root, "agent-loop.example.json"));
        Console.WriteLine($"Created {configPath} from the example.");
        Console.WriteLine("Review githubRepo, clonePath and models, set \"enabled\": true, then run again.");
        return command == "init" ? 0 : 1;
    }

    var config = LoopConfig.Load(configPath);
    var state = new StateStore(Path.Combine(root, "state", "state.json"));
    if (command == "customer")
    {
        if (!config.Enabled || state.IsPaused)
        {
            Console.WriteLine("Customer pass skipped: loop disabled or paused.");
            return 0;
        }
        if (!config.CustomerEnabled) Console.WriteLine("Customer pass disabled: set customerEnabled=true after desktop setup.");
        return await CustomerReview.RunAsync(config, root);
    }
    var gh = new GitHubClient(config.GitHubRepo);
    IAgentRunner worker = await KimiRunner.CreateAsync();
    IAgentRunner owner = config.OwnerRunner.Equals("claude", StringComparison.OrdinalIgnoreCase)
        ? await ClaudeRunner.CreateAsync()
        : worker;
    var loop = new LoopOrchestrator(config, root, gh, state, worker, owner);

    return command switch
    {
        "init" => await loop.InitAsync(),
        "work" => await loop.WorkAsync(),
        "own" => await loop.OwnAsync(),
        "run" => await loop.RunOnceAsync(),
        "pause" => Pause(loop, args),
        "resume" => loop.Resume(),
        "status" => loop.Status(),
        _ => Unknown(command),
    };
}
catch (Exception ex)
{
    Console.Error.WriteLine($"agent-loop failed: {ex.Message}");
    return 1;
}

/// <summary>
/// `pause [days|yyyy-MM-dd] [reason...]` — no date argument means "pause indefinitely".
/// Examples: `pause`, `pause 14 vacation`, `pause 2026-09-01 vacation`.
/// </summary>
static int Pause(LoopOrchestrator loop, string[] args)
{
    var until = DateTime.MaxValue;
    var reasonStart = 1;

    if (args.Length > 1)
    {
        if (int.TryParse(args[1], out var days) && days > 0)
        {
            until = DateTime.Now.AddDays(days);
            reasonStart = 2;
        }
        else if (DateTime.TryParse(args[1], out var date))
        {
            // a bare date means "paused through the end of that day"
            until = date.TimeOfDay == TimeSpan.Zero ? date.AddDays(1) : date;
            reasonStart = 2;
        }
    }

    var reason = args.Length > reasonStart ? string.Join(' ', args[reasonStart..]) : null;
    return loop.Pause(until, reason);
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

/// <summary>
/// Per-user data folder: %LOCALAPPDATA%\lunima-agent-loop on Windows, ~/.local/share/lunima-agent-loop
/// on Linux, ~/Library/Application Support/lunima-agent-loop on macOS.
/// </summary>
static string DefaultDataDir()
{
    var baseDir = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
    if (string.IsNullOrEmpty(baseDir))
        throw new InvalidOperationException(
            "agent-loop.json not found and no per-user data folder is available (HOME unset?). " +
            "Run from a folder that contains agent-loop.json.");
    return Path.Combine(baseDir, "lunima-agent-loop");
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
          customer Run independent customer UX reviews only (respects pause and enable switches)
          pause    Suspend all passes — 'pause' (indefinitely), 'pause 14 vacation',
                   'pause 2026-09-01 vacation'. Persists in state/state.json, so it
                   survives reboots and Windows updates.
          resume   Lift the pause
          status   Show config, pause state, today's counters and recent runs
        """);
}

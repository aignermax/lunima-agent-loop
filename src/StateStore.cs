using System.Text.Json;

namespace AgentLoop;

public sealed class DayCounters
{
    public int Tasks { get; set; }
    public int OwnerRuns { get; set; }
}

public sealed class RunRecord
{
    public string Timestamp { get; set; } = "";
    public string Kind { get; set; } = "";
    public int? Issue { get; set; }
    public string? Branch { get; set; }
    public int ExitCode { get; set; }
    public double DurationSec { get; set; }
    public string? Note { get; set; }
}

public sealed class LoopState
{
    public Dictionary<string, DayCounters> Days { get; set; } = new();
    public List<RunRecord> LastRuns { get; set; } = new();
    public DateTime? LastOwnerRun { get; set; }
}

/// <summary>
/// JSON-file-backed state: per-day run counters (the budget guardrail) and a short run history.
/// The state file is the only thing that persists between scheduled runs — everything else lives in GitHub.
/// </summary>
public sealed class StateStore
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };
    private readonly string _path;
    private readonly LoopState _state;

    public StateStore(string path)
    {
        _path = path;
        _state = File.Exists(path)
            ? JsonSerializer.Deserialize<LoopState>(File.ReadAllText(path)) ?? new LoopState()
            : new LoopState();
    }

    public DayCounters Today()
    {
        var key = DateTime.Now.ToString("yyyy-MM-dd");
        if (!_state.Days.TryGetValue(key, out var counters))
        {
            counters = new DayCounters();
            _state.Days[key] = counters;
        }
        return counters;
    }

    public IReadOnlyList<RunRecord> RecentRuns(int count) =>
        _state.LastRuns.TakeLast(count).ToList();

    public DateTime? LastOwnerRun => _state.LastOwnerRun;

    public void MarkOwnerRun()
    {
        _state.LastOwnerRun = DateTime.Now;
        Save();
    }

    public void RecordRun(RunRecord record)
    {
        record.Timestamp = DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss");
        _state.LastRuns.Add(record);
        if (_state.LastRuns.Count > 100)
            _state.LastRuns.RemoveRange(0, _state.LastRuns.Count - 100);

        // prune day counters older than 14 days
        var cutoff = DateTime.Now.AddDays(-14).ToString("yyyy-MM-dd");
        foreach (var key in _state.Days.Keys.Where(k => string.CompareOrdinal(k, cutoff) < 0).ToList())
            _state.Days.Remove(key);

        Save();
    }

    public void Save()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
        File.WriteAllText(_path, JsonSerializer.Serialize(_state, JsonOptions));
    }
}

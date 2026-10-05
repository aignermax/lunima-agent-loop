using AgentLoop;
using System.Text.Json;

var root = Path.Combine(Path.GetTempPath(), "customer-loop-tests-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(root);
var config = new LoopConfig { ClonePath = root };
void Assert(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
    Console.WriteLine("PASS: " + message);
}

Assert(await CustomerReview.RunAsync(config, root) == 0 && !Directory.Exists(Path.Combine(root, "state")),
    "disabled customer role does not launch a process or write acceptance");
Assert(CustomerReview.OwnerContext(config, root).Contains("disabled"), "disabled role disclosed to PO");
config.CustomerEnabled = true;
Assert(CustomerReview.OwnerContext(config, root).Contains(Path.Combine(root, "state", "customer", "strategy.json")),
    "PO receives absolute strategy snapshot path outside its working clone");
config.CustomerPython = Path.Combine(root, "missing-python");
Assert(await CustomerReview.RunAsync(config, root) == 1, "missing Python is a blocked review");
Assert(File.ReadAllText(CustomerReview.FeedbackPath(root)).Contains("BLOCKED"), "failure replaces stale feedback");

var script = Path.Combine(root, "tools", "ux-tester", "customer_cycle.py");
Directory.CreateDirectory(Path.GetDirectoryName(script)!);
File.WriteAllText(script, """
import json, pathlib, sys
values = dict(zip(sys.argv[1::2], sys.argv[2::2]))
destination = pathlib.Path(values['--feedback'])
destination.write_text('# Independent customer feedback\nIntegration baseline: NEEDS_CHANGES', encoding='utf-8')
destination.with_suffix('.json').write_text(json.dumps(values), encoding='utf-8')
""");
config.CustomerPython = Environment.GetEnvironmentVariable("CUSTOMER_TEST_PYTHON") ?? "python";
config.CustomerModel = "model with spaces";
Assert(await CustomerReview.RunAsync(config, root) == 0, "C# launches Python and receives customer handoff");
using var capturedArgs = JsonDocument.Parse(File.ReadAllText(Path.ChangeExtension(CustomerReview.FeedbackPath(root), ".json")));
Assert(capturedArgs.RootElement.GetProperty("--model").GetString() == config.CustomerModel, "model remains one argument");
Assert(capturedArgs.RootElement.GetProperty("--state-dir").GetString() == Path.Combine(root, "state", "customer"),
    "customer workspace is separate from worker clone");
var context = CustomerReview.OwnerContext(config, root);
Assert(context.Contains("--match-head-commit") && context.Contains("needs_changes") && context.Contains("baseline"),
    "PO receives commit-specific acceptance rules");

var configPath = Path.Combine(root, "agent-loop.json");
File.WriteAllText(configPath, """{"clonePath":"test","customerEnabled":true,"customerMaxSteps":0}""");
try { LoopConfig.Load(configPath); throw new Exception("Invalid budgets accepted"); }
catch (InvalidOperationException) { Console.WriteLine("PASS: invalid customer budgets rejected"); }
Console.WriteLine("Customer integration checks completed.");

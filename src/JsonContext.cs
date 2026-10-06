using System.Text.Json;
using System.Text.Json.Serialization;

namespace AgentLoop;

/// <summary>
/// Source-generated JSON metadata for the config and state files. Reflection-based
/// serialization is not available under NativeAOT, so every (de)serialized type is listed here.
/// </summary>
[JsonSourceGenerationOptions(
    PropertyNameCaseInsensitive = true,
    ReadCommentHandling = JsonCommentHandling.Skip,
    WriteIndented = true)]
[JsonSerializable(typeof(LoopConfig))]
[JsonSerializable(typeof(LoopState))]
internal sealed partial class JsonContext : JsonSerializerContext;

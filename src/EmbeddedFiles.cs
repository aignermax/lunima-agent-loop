using System.Reflection;

namespace AgentLoop;

/// <summary>
/// Files compiled into the executable (prompts, example config), so a published single-file
/// binary runs without any files next to it. A file on disk under the tool root wins, which
/// keeps prompts editable without a rebuild.
/// </summary>
public static class EmbeddedFiles
{
    /// <summary>Reads <paramref name="relativePath"/> (e.g. "prompts/owner.md") from disk if present, else from the binary.</summary>
    public static string Read(string rootDir, string relativePath)
    {
        var onDisk = Path.Combine(rootDir, relativePath);
        if (File.Exists(onDisk)) return File.ReadAllText(onDisk);

        using var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream(relativePath)
            ?? throw new FileNotFoundException($"'{relativePath}' is neither on disk nor embedded.", onDisk);
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }
}

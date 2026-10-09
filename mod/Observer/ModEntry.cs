using StardewModdingAPI;
using System.Collections.Concurrent;
using SvOptimizer.Core;

namespace SvOptimizer.Observer;

public sealed class ModConfig
{
    public string DataDirectory { get; set; } = BridgeClient.DefaultDirectory();
    public int MaxCandidatePlots { get; set; } = 2500;
}

public sealed class ModEntry : Mod
{
    private BridgeClient? bridge;
    private SnapshotReader? reader;
    private readonly ConcurrentQueue<string> refreshRequests = new();
    public override void Entry(IModHelper helper)
    {
        var config = helper.ReadConfig<ModConfig>();
        helper.Events.GameLoop.GameLaunched += (_, _) =>
        {
            reader = new SnapshotReader(config.MaxCandidatePlots);
            bridge = new BridgeClient(config.DataDirectory, id => refreshRequests.Enqueue(id),
                text => Monitor.Log(text, LogLevel.Trace));
            bridge.Start();
        };
        helper.Events.GameLoop.SaveLoaded += (_, _) => Capture();
        helper.Events.GameLoop.DayStarted += (_, _) => Capture();
        helper.Events.GameLoop.ReturnedToTitle += (_, _) => Capture();
        helper.Events.GameLoop.UpdateTicked += (_, _) =>
        {
            if (refreshRequests.TryDequeue(out string? id)) Capture(id);
        };
        helper.ConsoleCommands.Add("sv_snapshot", "Capture a read-only farm snapshot for the local optimizer.", (_, _) => Capture());
        Monitor.Log($"Read-only observer enabled. Bridge data directory: {config.DataDirectory}", LogLevel.Info);
    }
    private void Capture(string requestId = "")
    {
        if (reader == null || bridge == null) return;
        try
        {
            var snapshot=reader.Capture(Context.IsWorldReady, !Context.IsMultiplayer,
                GameAccess.Get<string>(GameAccess.Type("StardewValley.Game1"), "version"), Constants.ApiVersion.ToString());
            snapshot.RefreshRequestId=requestId;
            bridge.Publish(snapshot);
        }
        catch (Exception ex)
        {
            Monitor.Log($"Observation failed; no game state was changed. {ex}", LogLevel.Error);
        }
    }
}

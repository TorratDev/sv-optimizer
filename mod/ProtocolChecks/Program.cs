using Google.Protobuf;
using Grpc.Core;
using Grpc.Net.Client;
using SvOptimizer.Core;
using SvOptimizer.Protocol;

static void Check(bool condition, string message) { if (!condition) throw new Exception(message); }
Check(SnapshotMath.DaysToHarvest(new[] { 1, 2, 3, 99999 }, 0, 0, false) == 6, "New crop phase timing");
Check(SnapshotMath.DaysToHarvest(new[] { 1, 2, 3, 99999 }, 1, 1, false) == 4, "Growing crop timing");
Check(SnapshotMath.DaysToHarvest(new[] { 1, 2, 3, 99999 }, 3, 0, false) == 0, "Ready crop timing");
Check(SnapshotMath.DaysToHarvest(new[] { 1, 2, 3, 99999 }, 3, 2, true) == 2, "Regrowing crop timing");
Check(Math.Abs(SnapshotMath.ExpectedYield(1, 1, 0.2) - 1.25) < 0.0001, "Geometric bonus yield");
Check(SnapshotMath.DateCondition("DAY_OF_WEEK Fri", 5, _ => throw new Exception("Date must not use current game state")), "Future Friday");
Check(!SnapshotMath.DateCondition("DAY_OF_WEEK Fri", 6, _ => true), "Future non-Friday");
Check(SnapshotMath.DateCondition("DAY_OF_MONTH even", 6, _ => false), "Even days");
Check(SnapshotMath.DateCondition("!DAY_OF_WEEK Wed", 5, _ => false), "Negated date");
Check(!SnapshotMath.DateCondition("RANDOM 0.5", 1, _ => true), "No random prediction");
Check(!SnapshotMath.DateCondition("TIME 900 1700", 1, _ => true), "No future time prediction");
Check(GameAccess.Get<int>(new FixtureField(), "Value") == 42, "Game field access");
try { GameAccess.Get<int>(new FixtureField(), "Missing"); throw new Exception("Missing member must fail"); }
catch (MissingMemberException) { }
Console.WriteLine("C# snapshot math, date conditions, and strict game-member reads passed.");

if (args.Length == 3 && args[0] == "--bridge")
{
    var config = BridgeClient.ReadConfiguration(args[1]);
    var snapshot = JsonParser.Default.Parse<FarmSnapshot>(File.ReadAllText(args[2]));
    using var channel = GrpcChannel.ForAddress(config.Address, new GrpcChannelOptions { HttpHandler = new SocketsHttpHandler { UseProxy = false } });
    using var cancel = new CancellationTokenSource(TimeSpan.FromSeconds(40));
    using var call = new FarmBridge.FarmBridgeClient(channel).Observe(new Metadata { { "authorization", $"Bearer {config.Token}" } }, cancellationToken: cancel.Token);
    snapshot.CapturedAtUnixMs = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
    await call.RequestStream.WriteAsync(snapshot);
    Console.WriteLine("C# observer connected");
    await foreach (var request in call.ResponseStream.ReadAllAsync(cancel.Token))
    {
        snapshot.CapturedAtUnixMs = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        snapshot.RefreshRequestId = request.RequestId;
        await call.RequestStream.WriteAsync(snapshot);
        Console.WriteLine($"Refreshed: {request.RequestId}");
    }
}
public sealed class FixtureField { public int Value = 42; }

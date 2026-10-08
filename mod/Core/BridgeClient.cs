using System.Net;
using System.Text.Json;
using System.Threading.Channels;
using Grpc.Core;
using Grpc.Net.Client;
using SvOptimizer.Protocol;

namespace SvOptimizer.Core;

/// <summary>Only detached Protobuf messages cross the background-thread boundary.</summary>
public sealed class BridgeClient : IDisposable
{
    private readonly Channel<FarmSnapshot> snapshots = Channel.CreateBounded<FarmSnapshot>(new BoundedChannelOptions(1) { FullMode = BoundedChannelFullMode.DropOldest });
    private readonly CancellationTokenSource stop = new();
    private readonly string directory;
    private readonly Action<string> requestCapture;
    private readonly Action<string> log;
    private Task? worker;
    public BridgeClient(string directory, Action<string> requestCapture, Action<string> log)
    { this.directory = directory; this.requestCapture = requestCapture; this.log = log; }

    public void Start() => worker ??= Task.Run(Run);
    public void Publish(FarmSnapshot snapshot) => snapshots.Writer.TryWrite(snapshot);
    public void Dispose() { stop.Cancel(); snapshots.Writer.TryComplete(); }

    public static string DefaultDirectory() => Environment.GetEnvironmentVariable("SV_OPTIMIZER_DATA_DIR")
        ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "sv-optimizer");

    public static (Uri Address, string Token) ReadConfiguration(string directory)
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(directory, "bridge.json")));
        var root = doc.RootElement;
        if (root.GetProperty("schema_version").GetInt32() != 1) throw new InvalidDataException("Unsupported bridge schema");
        var address = new Uri(root.GetProperty("address").GetString()!);
        if (address.Scheme != "http" || !IPAddress.TryParse(address.Host.Trim('[', ']'), out var ip) || !IPAddress.IsLoopback(ip)
            || address.AbsolutePath != "/" || !string.IsNullOrEmpty(address.Query) || !string.IsNullOrEmpty(address.UserInfo))
            throw new InvalidDataException("Bridge address must be loopback HTTP without credentials, path, or query");
        var token = root.GetProperty("token").GetString()!;
        if (!Guid.TryParse(token, out _)) throw new InvalidDataException("Invalid bridge token");
        return (address, token);
    }

    private async Task Run()
    {
        bool reported = false;
        while (!stop.IsCancellationRequested)
        {
            try
            {
                var config = ReadConfiguration(directory);
                using var channel = GrpcChannel.ForAddress(config.Address, new GrpcChannelOptions
                {
                    HttpHandler = new SocketsHttpHandler { UseProxy = false, EnableMultipleHttp2Connections = true },
                    MaxSendMessageSize = 8 * 1024 * 1024, MaxReceiveMessageSize = 1024 * 1024
                });
                var client = new FarmBridge.FarmBridgeClient(channel);
                using var connection = CancellationTokenSource.CreateLinkedTokenSource(stop.Token);
                using var call = client.Observe(new Metadata { { "authorization", $"Bearer {config.Token}" } }, cancellationToken: connection.Token);
                requestCapture("");
                var sending = Send(call.RequestStream, connection.Token);
                var receiving = Receive(call.ResponseStream, connection.Token);
                try { await await Task.WhenAny(sending, receiving); }
                finally
                {
                    connection.Cancel();
                    try { await Task.WhenAll(sending, receiving); }
                    catch (OperationCanceledException) { }
                    catch (RpcException) when (connection.IsCancellationRequested) { }
                }
                reported = false;
            }
            catch (Exception ex) when (!stop.IsCancellationRequested)
            {
                if (!reported) { log($"Optimizer bridge waiting/reconnecting: {ex.Message}"); reported = true; }
            }
            try { await Task.Delay(2000, stop.Token); }
            catch (OperationCanceledException) { break; }
        }
    }
    private async Task Send(IClientStreamWriter<FarmSnapshot> output, CancellationToken cancel)
    {
        await foreach (var snapshot in snapshots.Reader.ReadAllAsync(cancel))
            await output.WriteAsync(snapshot, cancel);
    }
    private async Task Receive(IAsyncStreamReader<RefreshRequest> input, CancellationToken cancel)
    {
        await foreach (var request in input.ReadAllAsync(cancel)) requestCapture(request.RequestId);
    }
}

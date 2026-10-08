"""Real C# client -> Rust gRPC -> JSON-RPC MCP integration; requires .NET 8."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "target/debug/sv-optimizer"
dotnet = os.environ.get("DOTNET_COMMAND", "dotnet")
with tempfile.TemporaryDirectory() as directory:
    server = subprocess.Popen([str(binary), "--data-dir", directory, "--port", "52769", "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    observer = None
    try:
        config = Path(directory) / "bridge.json"
        for _ in range(100):
            if config.exists(): break
            if server.poll() is not None: raise RuntimeError("Rust bridge failed to start")
            time.sleep(0.05)
        observer = subprocess.Popen([dotnet, "run", "--project", str(root / "mod/ProtocolChecks"), "--no-build", "--", "--bridge", directory, str(root / "fixtures/spring-demo.json")], stdout=subprocess.PIPE, text=True)
        while True:
            line = observer.stdout.readline()
            if not line: raise RuntimeError("C# observer failed to connect")
            if "C# observer connected" in line: break
        def rpc(i, method, params):
            server.stdin.write(json.dumps(dict(jsonrpc="2.0", id=i, method=method, params=params)) + "\n")
            server.stdin.flush()
            response = json.loads(server.stdout.readline())
            assert response["id"] == i, response
            assert "error" not in response, response
            return response["result"]
        init = rpc(1, "initialize", dict(protocolVersion="2025-06-18", capabilities={}, clientInfo=dict(name="integration", version="1")))
        assert init["serverInfo"]["name"] == "sv-optimizer"
        snapshot = rpc(2, "tools/call", dict(name="get_farm_snapshot", arguments={}))
        assert snapshot["isError"] is False, snapshot
        assert snapshot["structuredContent"]["snapshot"]["money"] == 500
        plan = rpc(3, "tools/call", dict(name="plan_earnings", arguments=dict(horizon_days=5)))
        assert plan["isError"] is False, plan
        assert plan["structuredContent"]["plan"]["optimality"] == "feasible_estimate"
        progress = rpc(4, "tools/call", dict(name="check_plan_progress", arguments={}))
        assert progress["isError"] is False, progress
        assert progress["structuredContent"]["plan_id"] == plan["structuredContent"]["plan"]["id"]
        print("C# → gRPC → Rust → MCP live snapshot, refresh, planning, and progress passed.")
    finally:
        if observer is not None:
            observer.terminate()
            observer.wait(timeout=10)
        server.terminate()
        server.wait(timeout=10)

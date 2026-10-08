"""Exercise terminal -> MCP subprocess -> tool loop using a local Ollama mock."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "target/debug/sv-optimizer"
tool_seen = []
class Ollama(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_GET(self):
        self.reply(dict(models=[dict(name="qwen3:4b")]))
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if request["messages"][-1]["role"] == "tool":
            result = json.loads(request["messages"][-1]["content"])
            assert result["isError"] is False, result
            assert result["structuredContent"]["plan"]["horizon_days"] == 5
            tool_seen.append(result)
            self.reply(dict(message=dict(role="assistant", content="The calculated plan is a feasible estimate. Use /details for assumptions.")))
        else:
            self.reply(dict(message=dict(role="assistant", content="", tool_calls=[dict(function=dict(name="plan_earnings", arguments=dict(horizon_days=5)))])))
    def reply(self, value):
        data = json.dumps(value).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
server = ThreadingHTTPServer(("127.0.0.1", 0), Ollama)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory() as directory:
        result = subprocess.run([str(binary), "--data-dir", directory, "--fixture", str(root / "fixtures/spring-demo.json"), "chat", "--model", "qwen3:4b", "--ollama-url", f"http://127.0.0.1:{server.server_port}"], input="Create a five day earnings plan\n/details\n/quit\n", capture_output=True, text=True, timeout=40)
        assert result.returncode == 0, result.stderr
        assert "SIMULATION" in result.stdout
        assert "feasible estimate" in result.stdout
        assert "Assumption:" in result.stdout
        assert len(tool_seen) == 1
        print("Terminal → local Ollama API mock → MCP subprocess → calculated plan and details passed.")
finally:
    server.shutdown()

"""Exercise HTTP, persistence, auth and graceful stop without network probes."""
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.request import Request, urlopen
from urllib.error import HTTPError, URLError

binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="aujitter-smoke-") as directory:
    base = Path(directory)
    config = base / "settings.json"
    config.write_text(json.dumps({"paused": True}))
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    token = "ci-only-unique-test-token-32-characters"
    process = subprocess.Popen([str(binary), "--config", str(config), "--database", str(base / "history.sqlite3"), "--token", token, "run", "--bind", f"127.0.0.1:{port}"])

    def request(path, method="GET", payload=None, auth=True, extra=None):
        headers = {"Content-Type": "application/json"}
        if auth:
            headers["Authorization"] = "Bearer " + token
        if extra:
            headers.update(extra)
        data = json.dumps(payload).encode() if payload is not None else None
        return urlopen(Request(url + path, data, headers, method=method), timeout=3)

    try:
        for _ in range(100):
            try:
                with request("/api/health", auth=False) as response:
                    assert json.load(response)["app"] == "aujitter"
                break
            except (URLError, OSError):
                if process.poll() is not None:
                    raise RuntimeError("Service stopped during startup")
                time.sleep(0.1)
        else:
            raise RuntimeError("Service did not start")
        with request("/api/dashboard") as response:
            view = json.load(response)
            assert view["paused"] and view["latest"] is None
            assert "frame-ancestors 'none'" in response.headers["Content-Security-Policy"]
        for options, code in [({"auth": False}, 401), ({"extra": {"Host": f"evil.example:{port}"}}, 403), ({"extra": {"Origin": "https://evil.example"}}, 403)]:
            try:
                request("/api/dashboard", **options)
                raise AssertionError("Unsafe request was accepted")
            except HTTPError as error:
                assert error.code == code
        with request("/api/control", "POST", {"gaming": True}) as response:
            assert json.load(response)["saved"]
        assert json.loads(config.read_text())["gaming"] is True
        with request("/api/presets") as response:
            presets = json.load(response)
            assert len(presets) >= 11
        with request("/api/preset", "POST", {"preset": "gaming"}) as response:
            assert json.load(response)["preset"] == "gaming"
        saved = json.loads(config.read_text())
        assert saved["paused"] and saved["gaming"] and saved["gaming_interval_ms"] == 15000
        try:
            with request("/api/settings", "POST", {"interval_ms": 10}) as response:
                raise AssertionError("Invalid settings accepted")
        except HTTPError as error:
            assert error.code == 400
        with request("/api/export") as response:
            assert "scope" in json.load(response)
        with request("/api/shutdown", "POST", {}) as response:
            assert json.load(response)["stopping"]
        assert process.wait(timeout=10) == 0
        print("Service smoke checks passed")
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)

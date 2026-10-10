"""Port of tests/registry.rs: registry dependencies and `ryn publish` against an in-process mock
of the pods HTTP API, so the test needs neither the network nor the real registry."""

import hashlib
import json
import os
import shutil
import subprocess
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from ryn_harness import IS_WINDOWS, TempDir, run_ryn


def tar_command() -> str:
    if IS_WINDOWS:
        system_root = os.environ.get("SystemRoot", r"C:\Windows")
        system_tar = Path(system_root) / "System32" / "tar.exe"
        if system_tar.is_file():
            return str(system_tar)
    return "tar"


def package_archive(root: Path) -> bytes:
    stage = root / "stage"
    package = stage / "mathlib"
    (package / "src").mkdir(parents=True)
    (package / "ryn.yaml").write_text(
        "name: mathlib\nversion: 1.2.0\nowner: tester\ndescription: Test math\ndependencies: {}\n",
        encoding="utf-8",
    )
    (package / "src" / "ops.ryn").write_text(
        "pub fun triple(x: i32) -> i32 => x * 3\n", encoding="utf-8"
    )
    archive = root / "mathlib.tar.gz"
    completed = subprocess.run(
        [tar_command(), "-czf", str(archive), "-C", str(stage), "mathlib"]
    )
    assert completed.returncode == 0, "tar failed"
    return archive.read_bytes()


class MockRegistry:
    """Serves resolve, download and publish for one package until the test ends."""

    def __init__(self, archive: bytes, checksum: str):
        self.archive = archive
        self.checksum = checksum
        self.uploads: list[tuple[str, bytes]] = []
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self.server.registry = self
        self.address = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def start(self) -> str:
        self.thread.start()
        return self.address

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()


class _Handler(BaseHTTPRequestHandler):
    # HTTP/1.1 so that the base class answers `Expect: 100-continue` before the body is read.
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):  # keep test output quiet
        pass

    def do_GET(self):
        self._respond()

    def do_PUT(self):
        self._respond()

    def _respond(self):
        registry = self.server.registry
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        method = self.command
        path = self.path

        if method == "GET" and path.startswith("/api/v1/packages/mathlib/resolve"):
            status, content_type, payload = (
                200,
                "application/json",
                json.dumps({"version": "1.2.0", "checksum": registry.checksum}).encode(),
            )
        elif method == "GET" and path == "/api/v1/packages/mathlib/1.2.0/download":
            status, content_type, payload = 200, "application/gzip", registry.archive
        elif method == "PUT" and path == "/api/v1/packages/new":
            auth = self.headers.get("Authorization") or ""
            registry.uploads.append((auth, body))
            status, content_type = 200, "application/json"
            payload = (
                '{"ok": true, "name": "usesmath", "version": "0.1.0", "url": "'
                + registry.address
                + '/packages/usesmath/0.1.0"}'
            ).encode()
        else:
            status, content_type = 404, "application/json"
            payload = b'{"errors": [{"detail": "not found"}]}'

        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)


class RegistryTests(unittest.TestCase):
    def test_registry_dependencies_lock_fetch_verify_and_publish(self):
        with TempDir("registry-e2e") as root:
            archive = package_archive(root)
            checksum = hashlib.sha256(archive).hexdigest()
            registry = MockRegistry(archive, checksum)
            self.addCleanup(registry.stop)
            registry_address = registry.start()

            app = root / "app"
            (app / "src").mkdir(parents=True)
            (app / "ryn.yaml").write_text(
                "name: usesmath\nversion: 0.1.0\nowner: tester\ndescription: Uses mathlib\nlicense: MIT\nkeywords: [test]\ndependencies:\n  mathlib: ^1.0.0\n",
                encoding="utf-8",
            )
            (app / "src" / "main.ryn").write_text(
                "use mathlib::ops\n\nfun main() -> i32 {\n    echo ops::triple(14)\n    return 0\n}\n",
                encoding="utf-8",
            )

            def ryn(*args):
                return run_ryn(
                    *args,
                    env={
                        "RYN_REGISTRY": registry_address,
                        "RYN_TOKEN": "pods_test_token",
                    },
                )

            app_arg = str(app)

            # The first run creates ryn.lock, downloads, verifies and builds.
            run = ryn("run", app_arg)
            self.assertEqual(run.code, 0, f"stderr: {run.stderr}")
            self.assertEqual(run.stdout, "42\n")
            lock = (app / "ryn.lock").read_text(encoding="utf-8")
            self.assertIn("version: 1.2.0", lock, lock)
            self.assertIn(f"checksum: {checksum}", lock, lock)
            self.assertIn(f"source: registry:{registry_address}", lock, lock)

            # A tampered pin is refused before anything is unpacked.
            shutil.rmtree(app / ".ryn")
            (app / "ryn.lock").write_text(
                lock.replace(checksum, "0" * 64), encoding="utf-8"
            )
            tampered = ryn("run", app_arg)
            self.assertNotEqual(tampered.code, 0)
            self.assertIn("checksum mismatch", tampered.stderr)
            (app / "ryn.lock").write_text(lock, encoding="utf-8")

            # Publishing is rejected for path dependencies and sends the token otherwise.
            publish = ryn("publish", app_arg)
            self.assertEqual(publish.code, 0, f"stderr: {publish.stderr}")
            self.assertIn("/packages/usesmath/0.1.0", publish.stdout)
            uploads = registry.uploads
            self.assertEqual(len(uploads), 1)
            self.assertEqual(uploads[0][0], "Bearer pods_test_token")
            self.assertEqual(uploads[0][1][:2], b"\x1f\x8b", "upload is gzip")

            (app / "ryn.yaml").write_text(
                "name: usesmath\nversion: 0.1.1\nowner: tester\ndependencies:\n  local:\n    path: ../local\n",
                encoding="utf-8",
            )
            rejected = ryn("publish", app_arg)
            self.assertNotEqual(rejected.code, 0)


if __name__ == "__main__":
    unittest.main()

"""Exercise the compiled TUI through a real Unix pseudo-terminal.

Usage: /path/to/venv/bin/python tests/test_tui.py [binary]
The Python interpreter must have jupyter-client and ipykernel installed.
Pass --mock to exercise the TUI with a protocol fixture when sockets are unavailable;
this mode does not verify Jupyter or Python execution.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

mock = "--mock" in sys.argv
arguments = [arg for arg in sys.argv[1:] if arg != "--mock"]
binary = Path(arguments[0] if arguments else "target/debug/notebook-tui").resolve()
with tempfile.TemporaryDirectory(prefix="notebook-tui-e2e-") as temp:
    path = Path(temp) / "test.ipynb"
    codes = ["x = 40\nprint('first cell')", "x + 2", "1 / 0", "print('must not execute')"]
    cells = [dict(cell_type="code", id=f"cell-{i}", metadata={}, source=code, outputs=[], execution_count=None) for i, code in enumerate(codes)]
    path.write_text(json.dumps(dict(nbformat=4, nbformat_minor=5, metadata={"custom": "preserve"}, cells=cells)))
    python = sys.executable
    if mock:
        stub = Path(temp) / "python-stub"
        stub.write_text((Path(__file__).parent / "fixtures" / "kernel_stub.py").read_text())
        stub.chmod(0o700)
        python = str(stub)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))
    proc = subprocess.Popen([str(binary), str(path), "--python", python], stdin=slave, stdout=slave, stderr=slave, env=dict(os.environ, TERM="xterm-256color"))
    os.close(slave)
    screen = bytearray()

    def pump(seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], min(0.1, max(0, deadline - time.monotonic())))[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                screen.extend(data)
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")

    def mouse_click(x, y):
        os.write(master, f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m".encode())

    try:
        pump(0.4)
        assert proc.poll() is None, bytes(screen).decode(errors="replace")
        assert b"Quit" in screen
        if mock:
            # Exercise the real mouse input path: Run and Stop, then restart.
            mouse_click(4, 3)
            pump(0.3)
            assert b"Running" in screen
            mouse_click(28, 3)
            pump(0.4)
            assert b"Interrupted" in screen
            os.write(master, b"R")
            pump(0.15)
            os.write(master, b"y")
            pump(0.3)
        os.write(master, b"\x1b[15;2~")  # Shift+F5: run all.
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            pump(0.15)
            # Ratatui sends screen diffs, so words can span multiple updates.
            # Read persisted outputs rather than searching the raw escape stream.
            os.write(master, b"\x13")
            pump(0.1)
            current = json.loads(path.read_text())
            if any(o.get("ename") == "ZeroDivisionError" for o in current["cells"][2]["outputs"]):
                break
        else:
            import re
            text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", bytes(screen).decode(errors="replace"))
            raise AssertionError("Execution did not finish: " + text[-6000:])
        assert "✓ Completed".encode() in screen
        os.write(master, b"\x1b[B\x1b[B")
        pump(0.2)
        if mock:
            assert "× Error".encode() in screen
        os.write(master, b"\x13")  # Ctrl+S: save outputs.
        pump(0.5)
        saved = json.loads(path.read_text())
        assert saved["metadata"] == {"custom": "preserve"}
        assert saved["cells"][0]["execution_count"] == 1, saved
        assert saved["cells"][1]["outputs"][0]["data"]["text/plain"] == "42", saved
        assert saved["cells"][2]["outputs"][0]["ename"] == "ZeroDivisionError", saved
        assert saved["cells"][3]["execution_count"] is None, saved
        assert saved["cells"][3]["outputs"] == [], saved
        # Edit, undo and redo using actual terminal keys, then click Quit.
        os.write(master, b"\r\x1b[H#")
        pump(0.15)
        os.write(master, b"\x1a")  # Ctrl+Z
        pump(0.1)
        os.write(master, b"\x19")  # Ctrl+Y
        pump(0.1)
        os.write(master, b"\x1b")
        pump(0.15)
        mouse_click(115, 3)
        pump(0.15)
        assert b"CONFIRMATION" in screen
        os.write(master, b"s")  # Save and quit.

        pump(0.5)
        assert proc.wait(timeout=5) == 0
        final = json.loads(path.read_text())
        assert "".join(final["cells"][2]["source"]).startswith("#"), final
        mode = "protocol fixture" if mock else "real Jupyter kernel"
        print(f"OK: PTY + {mode} → run → status → stop on error → edit → save → mouse Quit")
    finally:
        if proc.poll() is None:
            os.write(master, b"\x11")
            pump(0.2)
            os.write(master, b"y")
            pump(0.3)
            if proc.poll() is None:
                proc.terminate()
            proc.wait(timeout=5)
        os.close(master)

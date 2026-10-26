"""Integration tests: a real ipykernel, without external network access or course execution."""
import json
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest

BRIDGE = Path(__file__).resolve().parents[1] / "python" / "bridge.py"


class BridgeTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.proc = subprocess.Popen(
            [sys.executable, "-u", str(BRIDGE), self.temp.name],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True,
        )
        self.events = queue.Queue()
        def read():
            for line in self.proc.stdout:
                self.events.put(json.loads(line))
        threading.Thread(target=read, daemon=True).start()
        self.addCleanup(self.cleanup)
        self.until("ready")

    def cleanup(self):
        if self.proc.poll() is None:
            self.send(cmd="shutdown")
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        self.proc.stdin.close()
        self.proc.stdout.close()
        self.proc.stderr.close()
        self.temp.cleanup()

    def send(self, **command):
        self.proc.stdin.write(json.dumps(command) + "\n")
        self.proc.stdin.flush()

    def until(self, target, timeout=45):
        deadline = time.monotonic() + timeout
        seen = []
        while time.monotonic() < deadline:
            event = self.events.get(timeout=max(0.01, deadline - time.monotonic()))
            seen.append(event)
            if event["event"] == "fatal":
                self.fail(event["message"])
            if event["event"] == target:
                return seen
        self.fail(f"Missing {target}: {seen}")

    def execute(self, code, cell=0):
        self.send(cmd="execute", code=code, cell=cell)
        return self.until("done")

    def test_state_magics_errors_rich_output_and_cwd(self):
        events = self.execute("x = 40\nprint('café 世界')\nx + 2")
        outputs = [e["output"] for e in events if e["event"] == "output"]
        self.assertIn("café 世界", outputs[0]["text"])
        self.assertEqual(outputs[-1]["data"]["text/plain"], "42")
        events = self.execute("%pwd\nprint(x)", cell=1)
        self.assertTrue(any("40" in e.get("output", {}).get("text", "") for e in events))
        events = self.execute("import os\nprint(os.getcwd())")
        self.assertTrue(any(self.temp.name in e.get("output", {}).get("text", "") for e in events))
        events = self.execute("from IPython.display import display, HTML\ndisplay(HTML('<b>ok</b>'))")
        self.assertTrue(any("text/html" in e.get("output", {}).get("data", {}) for e in events))
        events = self.execute("1/0")
        self.assertTrue(any(e.get("output", {}).get("ename") == "ZeroDivisionError" for e in events))
        self.assertTrue(any(e["event"] == "count" for e in events))

    def test_input_interrupt_restart(self):
        self.send(cmd="execute", code="name = input('Name? ')\nprint('Hello', name)", cell=0)
        events = self.until("input")
        self.assertEqual(events[-1]["prompt"], "Name? ")
        self.send(cmd="input", value="Charly")
        self.assertTrue(any("Charly" in e.get("output", {}).get("text", "") for e in self.until("done")))
        self.send(cmd="execute", code="import time\nprint('started', flush=True)\ntime.sleep(60)", cell=1)
        self.until("output")
        self.send(cmd="interrupt")
        events = self.until("done", timeout=10)
        self.assertTrue(any(e.get("output", {}).get("ename") == "KeyboardInterrupt" for e in events))
        self.send(cmd="restart")
        self.until("ready")
        events = self.execute("print('name' in globals())")
        self.assertTrue(any("False" in e.get("output", {}).get("text", "") for e in events))

    def test_demo_notebook(self):
        demo = json.loads((BRIDGE.parents[1] / "examples" / "demo.ipynb").read_text())
        outputs = []
        for i, cell in enumerate(demo["cells"]):
            if cell["cell_type"] != "code":
                continue
            code = cell["source"] if isinstance(cell["source"], str) else "".join(cell["source"])
            events = self.execute(code, cell=i)
            outputs.extend(e["output"] for e in events if e["event"] == "output")
        self.assertFalse(any(o["output_type"] == "error" for o in outputs), outputs)
        self.assertTrue(any("image/png" in o.get("data", {}) for o in outputs))
        self.assertTrue(any("text/html" in o.get("data", {}) for o in outputs))

    def test_display_update_and_deferred_clear(self):
        events = self.execute("from IPython.display import display, clear_output\nh=display('before', display_id=True)\nh.update('after')\nclear_output(wait=True)\nprint('end')")
        self.assertTrue(any(e["event"] == "update" for e in events))
        self.assertTrue(any(e["event"] == "clear" for e in events))


if __name__ == "__main__":
    unittest.main(verbosity=2)

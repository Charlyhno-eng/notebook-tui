"""Private JSON-lines transport between the Rust UI and a real Jupyter kernel."""
import json
import os
import queue
import sys
import threading
import time


def emit(event, **fields):
    print(json.dumps(dict(event=event, **fields)), flush=True)


def main():
    from jupyter_client import KernelManager

    commands = queue.Queue()

    def reader():
        for line in sys.stdin:
            try:
                commands.put(json.loads(line))
            except ValueError:
                pass
        commands.put({"cmd": "shutdown"})

    threading.Thread(target=reader, daemon=True).start()
    km = KernelManager(kernel_name="python3")
    km.kernel_spec.argv = [sys.executable, "-m", "ipykernel_launcher", "-f", "{connection_file}"]
    # Keep connection files inside the writable temporary directory.
    import tempfile
    runtime = tempfile.TemporaryDirectory(prefix="notebook-tui-kernel-")
    km.connection_file = os.path.join(runtime.name, "connection.json")
    client = None
    active = None
    deferred_clear = False
    displays = {}
    try:
        km.start_kernel(cwd=sys.argv[1])
        client = km.blocking_client()
        client.start_channels()
        client.wait_for_ready(timeout=40)
        emit("ready", python=sys.executable)
        last_alive = time.monotonic()
        while True:
            try:
                command = commands.get(timeout=0.01)
            except queue.Empty:
                command = None
            if command:
                op = command["cmd"]
                if op == "shutdown":
                    break
                if op == "interrupt":
                    km.interrupt_kernel()
                elif op == "restart":
                    km.restart_kernel(now=True)
                    client.wait_for_ready(timeout=40)
                    active = None
                    displays.clear()
                    emit("ready", python=sys.executable)
                elif op == "input":
                    client.input(command["value"])
                elif op == "execute" and active is None:
                    deferred_clear = False
                    msg_id = client.execute(command["code"], allow_stdin=True, stop_on_error=True)
                    active = (msg_id, command["cell"])
                    emit("started", cell=active[1])
            if time.monotonic() - last_alive > 1:
                last_alive = time.monotonic()
                if not km.is_alive():
                    raise RuntimeError("The Python kernel stopped. Restart the kernel.")
            try:
                request = client.get_stdin_msg(timeout=0)
                if active and request["parent_header"].get("msg_id") == active[0]:
                    emit("input", prompt=request["content"]["prompt"], password=request["content"].get("password", False))
            except queue.Empty:
                pass
            try:
                msg = client.get_iopub_msg(timeout=0.02)
            except queue.Empty:
                continue
            kind = msg["header"]["msg_type"]
            content = msg["content"]
            if kind == "update_display_data":
                display_id = content.get("transient", {}).get("display_id")
                for cell in displays.get(display_id, []):
                    emit("update", cell=cell, display_id=display_id, data=content["data"], metadata=content.get("metadata", {}))
                continue
            if not active or msg["parent_header"].get("msg_id") != active[0]:
                continue
            cell = active[1]
            if kind == "status" and content["execution_state"] == "idle":
                emit("done", cell=cell)
                active = None
            elif kind == "execute_input":
                emit("count", cell=cell, count=content["execution_count"])
            elif kind == "clear_output":
                if content.get("wait"):
                    deferred_clear = True
                else:
                    emit("clear", cell=cell)
            elif kind in ("stream", "display_data", "execute_result", "error"):
                if deferred_clear:
                    emit("clear", cell=cell)
                    deferred_clear = False
                output = dict(content, output_type=kind)
                transient = output.pop("transient", {})
                display_id = transient.get("display_id")
                if display_id:
                    displays.setdefault(display_id, []).append(cell)
                emit("output", cell=cell, output=output, display_id=display_id)
    finally:
        if client:
            client.stop_channels()
        if km.has_kernel:
            km.shutdown_kernel(now=True)
        runtime.cleanup()


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        emit("fatal", message=str(exc))
        sys.exit(1)

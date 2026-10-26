#!/usr/bin/env python3
"""Deterministic JSON transport fixture for TUI tests. Does not execute Python.

Ignore the interpreter arguments passed by Kernel::start and speak the bridge
protocol on stdin/stdout. Use tests/test_bridge.py to test a real Jupyter kernel.
"""
import json
import sys
import threading

cancelled = threading.Event()
write_lock = threading.Lock()
counter = 0
worker = None


def emit(event, **fields):
    with write_lock:
        print(json.dumps(dict(event=event, **fields)), flush=True)


def execute(cell, count):
    emit("started", cell=cell)
    emit("count", cell=cell, count=count)
    if cancelled.wait(0.8):
        emit("output", cell=cell, output=dict(output_type="error", ename="KeyboardInterrupt", evalue="", traceback=["KeyboardInterrupt"]))
    elif cell == 0:
        emit("output", cell=cell, output=dict(output_type="stream", name="stdout", text="first cell\n"))
    elif cell == 1:
        emit("output", cell=cell, output=dict(output_type="execute_result", execution_count=count, metadata={}, data={"text/plain": "42"}))
    elif cell == 2:
        emit("output", cell=cell, output=dict(output_type="error", ename="ZeroDivisionError", evalue="division by zero", traceback=["ZeroDivisionError: division by zero"]))
    emit("done", cell=cell)


emit("ready", python="protocol fixture (not a Python kernel)")
for line in sys.stdin:
    command = json.loads(line)
    op = command["cmd"]
    if op == "shutdown":
        cancelled.set()
        if worker:
            worker.join()
        break
    if op == "execute":
        if worker:
            worker.join()
        counter += 1
        cancelled.clear()
        worker = threading.Thread(target=execute, args=(command["cell"], counter))
        worker.start()
    elif op == "interrupt":
        cancelled.set()
    elif op == "restart":
        cancelled.set()
        if worker:
            worker.join()
        counter = 0
        emit("ready", python="protocol fixture (not a Python kernel)")

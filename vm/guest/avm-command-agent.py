#!/usr/bin/env python3
import base64
import fcntl
import hashlib
import json
import os
import pwd
import selectors
import signal
import subprocess
import sys
import time
import uuid
from pathlib import Path

ROOT = Path(os.environ.get("AVM_COMMAND_ROOT", "/var/lib/avm/commands"))
MAX_REQUEST = 1024 * 1024
MAX_OUTPUT = 16 * 1024 * 1024
TERMINAL = {"exited", "cancelled", "failed_to_start", "lost"}


def now_ns():
    return time.time_ns()


def boot_id():
    return Path("/proc/sys/kernel/random/boot_id").read_text().strip()


def state_path(command_id):
    return ROOT / f"{command_id}.json"


def atomic_write(path, value):
    temporary = ROOT / f".tmp-{uuid.uuid4()}"
    with open(temporary, "x", encoding="utf-8") as output:
        json.dump(value, output, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)


def load(command_id):
    with open(state_path(command_id), encoding="utf-8") as source:
        return json.load(source)


def response(value):
    sys.stdout.write(json.dumps(value, sort_keys=True) + "\n")
    sys.stdout.flush()


def request_digest(cwd, argv):
    encoded = json.dumps({"cwd": cwd, "argv": argv}, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
    return hashlib.sha256(encoded.encode()).hexdigest()


def validate_start(request):
    command_id = str(uuid.UUID(request["commandId"]))
    argv = request.get("argv")
    if not isinstance(argv, list) or not argv or not all(isinstance(value, str) and "\0" not in value for value in argv):
        raise ValueError("argv must be a non-empty string array")
    cwd = os.path.realpath(request.get("cwd", "/workspace"))
    if cwd != "/workspace" and not cwd.startswith("/workspace/"):
        raise ValueError("cwd must be inside /workspace")
    if not os.path.isdir(cwd):
        raise ValueError("cwd does not exist")
    key = request.get("idempotencyKey")
    if key is not None and (not isinstance(key, str) or not key or len(key) > 256):
        raise ValueError("invalid idempotency key")
    return command_id, cwd, argv, key, request_digest(cwd, argv)


def drop_to_avm():
    user = pwd.getpwnam("avm")
    os.initgroups(user.pw_name, user.pw_gid)
    os.setgid(user.pw_gid)
    os.setuid(user.pw_uid)


def monitor(command_id, cwd, argv):
    path = state_path(command_id)
    try:
        process = subprocess.Popen(
            argv,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            preexec_fn=drop_to_avm,
        )
        state = load(command_id)
        state.update({
            "state": "running",
            "startedAtNs": now_ns(),
            "pid": process.pid,
            "processGroup": process.pid,
            "guestBootId": boot_id(),
        })
        atomic_write(path, state)
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ, "stdout")
        selector.register(process.stderr, selectors.EVENT_READ, "stderr")
        retained = {"stdout": bytearray(), "stderr": bytearray()}
        dropped = {"stdout": 0, "stderr": 0}
        outputs = {
            stream: open(ROOT / f"{command_id}.{stream}", "wb")
            for stream in ("stdout", "stderr")
        }
        try:
            while selector.get_map():
                for key, _ in selector.select(timeout=0.25):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    available = max(0, MAX_OUTPUT - len(retained[key.data]))
                    kept = chunk[:available]
                    retained[key.data].extend(kept)
                    outputs[key.data].write(kept)
                    outputs[key.data].flush()
                    dropped[key.data] += len(chunk) - available
        finally:
            for output in outputs.values():
                output.flush()
                os.fsync(output.fileno())
                output.close()
        return_code = process.wait()
        state = load(command_id)
        cancelled = bool(state.get("cancelRequestedAtNs")) and return_code < 0
        state.update({
            "state": "cancelled" if cancelled else "exited",
            "exitedAtNs": now_ns(),
            "exitCode": 128 + (-return_code) if return_code < 0 else return_code,
            "signal": -return_code if return_code < 0 else None,
            "stdoutRetainedBytes": len(retained["stdout"]),
            "stderrRetainedBytes": len(retained["stderr"]),
            "stdoutDroppedBytes": dropped["stdout"],
            "stderrDroppedBytes": dropped["stderr"],
        })
        atomic_write(path, state)
    except BaseException as error:
        try:
            state = load(command_id)
            state.update({"state": "failed_to_start", "exitedAtNs": now_ns(), "error": str(error)})
            atomic_write(path, state)
        finally:
            os._exit(1)


def start(request):
    command_id, cwd, argv, key, digest = validate_start(request)
    ROOT.mkdir(parents=True, exist_ok=True, mode=0o700)
    with open(ROOT / ".lock", "a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        key_hash = hashlib.sha256(key.encode()).hexdigest() if key else None
        for path in ROOT.glob("*.json"):
            existing = json.loads(path.read_text())
            if key_hash and existing.get("idempotencyKeyHash") == key_hash:
                if existing.get("requestDigest") != digest:
                    raise ValueError("idempotency key already belongs to a different command")
                return existing
        path = state_path(command_id)
        if path.exists():
            existing = load(command_id)
            if existing.get("requestDigest") != digest:
                raise ValueError("command id already belongs to a different command")
            return existing
        state = {
            "commandId": command_id,
            "state": "accepted",
            "cwd": cwd,
            "argv": argv,
            "requestDigest": digest,
            "idempotencyKeyHash": key_hash,
            "acceptedAtNs": now_ns(),
            "guestBootId": boot_id(),
        }
        atomic_write(path, state)
        child = os.fork()
        if child == 0:
            os.setsid()
            null = os.open("/dev/null", os.O_RDWR)
            for descriptor in (0, 1, 2):
                os.dup2(null, descriptor)
            monitor(command_id, cwd, argv)
            os._exit(0)
        return state


def attach(state):
    result = dict(state)
    for stream in ("stdout", "stderr"):
        path = ROOT / f"{state['commandId']}.{stream}"
        data = path.read_bytes() if path.exists() else b""
        result[f"{stream}Base64"] = base64.b64encode(data).decode()
    return result


def wait_for(command_id, timeout_ms):
    deadline = time.monotonic() + timeout_ms / 1000 if timeout_ms else None
    while True:
        state = load(command_id)
        if state["state"] in TERMINAL:
            return state
        if deadline is not None and time.monotonic() >= deadline:
            result = dict(state)
            result["waitTimedOut"] = True
            return result
        time.sleep(0.1)


def cancel(command_id, grace_ms):
    path = state_path(command_id)
    state = load(command_id)
    if state["state"] in TERMINAL:
        return state
    state["cancelRequestedAtNs"] = now_ns()
    atomic_write(path, state)
    deadline = time.monotonic() + grace_ms / 1000
    group = state.get("processGroup")
    while not group and state["state"] not in TERMINAL and time.monotonic() < deadline:
        time.sleep(0.05)
        state = load(command_id)
        group = state.get("processGroup")
    if group and state.get("guestBootId") == boot_id():
        try:
            os.killpg(group, signal.SIGTERM)
        except ProcessLookupError:
            pass
        remaining_ms = max(0, int((deadline - time.monotonic()) * 1000))
        result = wait_for(command_id, remaining_ms)
        if result["state"] not in TERMINAL:
            try:
                os.killpg(group, signal.SIGKILL)
            except ProcessLookupError:
                pass
            result = wait_for(command_id, 5000)
        return result
    state.update({"state": "lost", "exitedAtNs": now_ns(), "error": "guest process identity is unavailable"})
    atomic_write(path, state)
    return state


def main():
    raw = sys.stdin.buffer.read(MAX_REQUEST + 1)
    if len(raw) > MAX_REQUEST:
        raise ValueError("request too large")
    request = json.loads(raw)
    operation = request.get("operation")
    ROOT.mkdir(parents=True, exist_ok=True, mode=0o700)
    if operation == "start":
        result = start(request)
    elif operation == "list":
        result = {"commands": [json.loads(path.read_text()) for path in sorted(ROOT.glob("*.json"))]}
    elif operation == "status":
        result = load(str(uuid.UUID(request["commandId"])))
    elif operation == "wait":
        result = wait_for(str(uuid.UUID(request["commandId"])), int(request.get("timeoutMs", 0)))
    elif operation == "attach":
        result = attach(load(str(uuid.UUID(request["commandId"]))))
    elif operation == "cancel":
        result = cancel(str(uuid.UUID(request["commandId"])), int(request.get("graceMs", 5000)))
    elif operation == "health":
        result = {"guestBootId": boot_id(), "user": "avm"}
    else:
        raise ValueError("unsupported operation")
    response(result)


if __name__ == "__main__":
    try:
        main()
    except BaseException as error:
        response({"error": str(error)})
        raise SystemExit(1)

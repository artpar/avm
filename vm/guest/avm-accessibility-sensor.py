#!/usr/bin/python3
import json
import os
import time

import pyatspi
from gi.repository import GLib

DEVICE = "/dev/virtio-ports/org.avm.accessibility"
BUS_READY_MARKER = "/run/user/1000/avm-accessibility-bus-ready"
MAX_TREE_NODES = 500
MAX_TREE_DEPTH = 10
MAX_TEXT_CHARACTERS = 4096
MAX_LINE_BYTES = 900_000
COMMAND_RETRY_MS = 1_000
COMMAND_IO_CONDITIONS = GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR | GLib.IO_NVAL

output = None
command_buffer = b""
connected = False
command_watch_id = None
command_retry_id = None


def source_timestamp():
    return {
        "guestMonotonicNs": time.monotonic_ns(),
        "guestWallClockTime": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def emit(kind, payload):
    global connected
    envelope = {
        "kind": kind,
        "sourceTimestamp": source_timestamp(),
        "payload": payload,
    }
    encoded = json.dumps(envelope, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if len(encoded) > MAX_LINE_BYTES:
        envelope["payload"] = {
            "error": "accessibility event exceeded guest size limit",
            "originalKind": kind,
        }
        envelope["kind"] = "accessibility.sensor.error"
        encoded = json.dumps(envelope, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    if output is None or not connected:
        return False
    try:
        remaining = memoryview(encoded + b"\n")
        while remaining:
            written = output.write(remaining)
            if not written:
                raise OSError("guest accessibility device accepted no data")
            remaining = remaining[written:]
        return True
    except (BrokenPipeError, OSError):
        connected = False
        return False


def safely(default, operation):
    try:
        return operation()
    except Exception:
        return default


def enum_name(value):
    nick = getattr(value, "value_nick", None)
    if nick:
        return nick
    text = str(value)
    return text.rsplit("_", 1)[-1].lower()


def application_name(accessible):
    current = accessible
    for _ in range(64):
        if current is None:
            break
        role = safely("", current.getRoleName)
        if role == "application":
            return safely("", lambda: current.name)
        current = safely(None, lambda: current.parent)
    return None


def interfaces(accessible):
    return safely([], lambda: list(accessible.get_interfaces()))


def geometry(accessible, available):
    if "Component" not in available:
        return None
    extents = safely(None, lambda: accessible.queryComponent().getExtents(pyatspi.DESKTOP_COORDS))
    if extents is None:
        return None
    return {
        "x": extents.x,
        "y": extents.y,
        "width": extents.width,
        "height": extents.height,
    }


def actions(accessible, available):
    if "Action" not in available:
        return []
    action = safely(None, accessible.queryAction)
    if action is None:
        return []
    count = safely(0, lambda: action.nActions)
    return [
        {
            "name": safely("", lambda index=index: action.getName(index)),
            "description": safely("", lambda index=index: action.getDescription(index)),
            "keyBinding": safely("", lambda index=index: action.getKeyBinding(index)),
        }
        for index in range(min(count, 64))
    ]


def text_value(accessible, available):
    result = {}
    if "Text" in available:
        text = safely(None, accessible.queryText)
        if text is not None:
            count = safely(0, lambda: text.characterCount)
            result["text"] = safely("", lambda: text.getText(0, min(count, MAX_TEXT_CHARACTERS)))
            result["textCharacterCount"] = count
            result["textTruncated"] = count > MAX_TEXT_CHARACTERS
    if "Value" in available:
        value = safely(None, accessible.queryValue)
        if value is not None:
            result["value"] = {
                "current": safely(None, lambda: value.currentValue),
                "minimum": safely(None, lambda: value.minimumValue),
                "maximum": safely(None, lambda: value.maximumValue),
                "increment": safely(None, lambda: value.minimumIncrement),
            }
    return result


def relations(accessible):
    result = []
    for relation in safely([], accessible.getRelationSet):
        targets = []
        for index in range(min(safely(0, lambda: relation.nTargets), 32)):
            target = safely(None, lambda index=index: relation.getTarget(index))
            if target is not None:
                targets.append(
                    {
                        "role": safely("", target.getRoleName),
                        "name": safely("", lambda: target.name),
                    }
                )
        result.append(
            {
                "type": enum_name(safely("unknown", lambda: relation.relationType)),
                "targets": targets,
            }
        )
    return result


def describe(accessible, include_children=False, budget=None, depth=0):
    available = interfaces(accessible)
    state_values = safely([], lambda: accessible.getState().getStates())
    description = {
        "application": application_name(accessible),
        "role": safely("unknown", accessible.getRoleName),
        "name": safely("", lambda: accessible.name),
        "description": safely("", lambda: accessible.description),
        "states": [enum_name(state) for state in state_values],
        "relations": relations(accessible),
        "geometry": geometry(accessible, available),
        "actions": actions(accessible, available),
        "interfaces": available,
        "childCount": safely(0, lambda: accessible.childCount),
    }
    description.update(text_value(accessible, available))
    if include_children and budget is not None and depth < MAX_TREE_DEPTH:
        children = []
        child_count = description["childCount"]
        for index in range(child_count):
            if budget[0] >= MAX_TREE_NODES:
                break
            child = safely(None, lambda index=index: accessible.getChildAtIndex(index))
            if child is None:
                continue
            budget[0] += 1
            children.append(describe(child, True, budget, depth + 1))
        description["children"] = children
        description["childrenTruncated"] = len(children) < child_count
    return description


def initial_snapshot():
    desktop = pyatspi.Registry.getDesktop(0)
    budget = [1]
    tree = describe(desktop, True, budget)
    emit(
        "accessibility.tree.snapshot",
        {
            "tree": tree,
            "nodeCount": budget[0],
            "nodeLimit": MAX_TREE_NODES,
            "depthLimit": MAX_TREE_DEPTH,
            "truncated": budget[0] >= MAX_TREE_NODES,
        },
    )


def emit_ready(heartbeat):
    desktop = pyatspi.Registry.getDesktop(0)
    emit(
        "accessibility.sensor.ready",
        {
            "observerVersion": 2,
            "desktopCount": pyatspi.Registry.getDesktopCount(),
            "desktopName": safely("", lambda: desktop.name),
            "pid": os.getpid(),
            "heartbeat": heartbeat,
        },
    )


def readiness_heartbeat():
    if connected:
        emit_ready(True)
    return True


def arm_command_watch():
    global command_retry_id, command_watch_id
    command_retry_id = None
    if command_watch_id is None:
        command_watch_id = GLib.io_add_watch(
            output.fileno(), COMMAND_IO_CONDITIONS, on_command
        )
    return False


def schedule_command_watch():
    global command_retry_id
    if command_retry_id is None:
        command_retry_id = GLib.timeout_add(COMMAND_RETRY_MS, arm_command_watch)


def disconnect_command_source():
    global command_buffer, connected, command_watch_id
    connected = False
    command_buffer = b""
    command_watch_id = None
    schedule_command_watch()
    return False


def on_command(source, condition):
    global command_buffer, connected
    if not condition & GLib.IO_IN:
        return disconnect_command_source()
    try:
        incoming = os.read(source, 65536)
    except BlockingIOError:
        if condition & (GLib.IO_HUP | GLib.IO_ERR | GLib.IO_NVAL):
            return disconnect_command_source()
        return True
    except OSError:
        return disconnect_command_source()
    if not incoming:
        return disconnect_command_source()
    command_buffer += incoming
    while b"\n" in command_buffer:
        line, command_buffer = command_buffer.split(b"\n", 1)
        command = safely({}, lambda: json.loads(line.decode("utf-8")))
        if command.get("command") == "observe":
            connected = True
            emit_ready(False)
            initial_snapshot()
    if condition & (GLib.IO_HUP | GLib.IO_ERR | GLib.IO_NVAL):
        return disconnect_command_source()
    return True


def on_event(event):
    emit(
        "accessibility.object.event",
        {
            "eventType": event.type,
            "detail1": event.detail1,
            "detail2": event.detail2,
            "anyData": safely(None, lambda: str(event.any_data)),
            "object": describe(event.source),
        },
    )


def main():
    global output
    desktop = pyatspi.Registry.getDesktop(0)
    with open(BUS_READY_MARKER, "w", encoding="utf-8") as marker:
        marker.write("ready\n")
    output = open(DEVICE, "r+b", buffering=0)
    for event_type in (
        "object:property-change",
        "object:state-changed",
        "object:children-changed",
        "object:text-changed",
        "object:bounds-changed",
        "window:",
        "focus:",
    ):
        pyatspi.Registry.registerEventListener(on_event, event_type)
    arm_command_watch()
    GLib.timeout_add_seconds(5, readiness_heartbeat)
    pyatspi.Registry.start()


if __name__ == "__main__":
    main()

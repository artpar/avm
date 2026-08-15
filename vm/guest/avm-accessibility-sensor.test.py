#!/usr/bin/env python3
import importlib.util
import pathlib
import sys
import types
import unittest
from unittest import mock

sys.dont_write_bytecode = True


class FakeGLib:
    IO_IN = 1
    IO_ERR = 8
    IO_HUP = 16
    IO_NVAL = 32

    def __init__(self):
        self.next_source_id = 1
        self.watches = []
        self.timeouts = []

    def io_add_watch(self, source, condition, callback):
        source_id = self.next_source_id
        self.next_source_id += 1
        self.watches.append((source_id, source, condition, callback))
        return source_id

    def timeout_add(self, interval_ms, callback):
        source_id = self.next_source_id
        self.next_source_id += 1
        self.timeouts.append((source_id, interval_ms, callback))
        return source_id


class FakeOutput:
    def fileno(self):
        return 6


def load_sensor(fake_glib):
    pyatspi = types.ModuleType("pyatspi")
    pyatspi.DESKTOP_COORDS = 0
    pyatspi.Registry = types.SimpleNamespace()
    repository = types.ModuleType("gi.repository")
    repository.GLib = fake_glib
    gi = types.ModuleType("gi")
    gi.repository = repository
    with mock.patch.dict(
        sys.modules,
        {"pyatspi": pyatspi, "gi": gi, "gi.repository": repository},
    ):
        path = pathlib.Path(__file__).with_name("avm-accessibility-sensor.py")
        spec = importlib.util.spec_from_file_location("avm_accessibility_sensor", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    module.output = FakeOutput()
    return module


class CommandWatchTests(unittest.TestCase):
    def setUp(self):
        self.glib = FakeGLib()
        self.sensor = load_sensor(self.glib)

    def test_eof_removes_watch_and_rearms_once_with_backoff(self):
        self.sensor.connected = True
        self.sensor.command_buffer = b"partial"
        self.sensor.command_watch_id = 41
        with mock.patch.object(self.sensor.os, "read", return_value=b""):
            self.assertFalse(self.sensor.on_command(6, self.glib.IO_IN))
            self.assertFalse(self.sensor.on_command(6, self.glib.IO_IN))

        self.assertFalse(self.sensor.connected)
        self.assertEqual(self.sensor.command_buffer, b"")
        self.assertIsNone(self.sensor.command_watch_id)
        self.assertEqual(len(self.glib.timeouts), 1)
        _, interval_ms, retry = self.glib.timeouts[0]
        self.assertEqual(interval_ms, 1_000)

        self.assertFalse(retry())
        self.assertEqual(len(self.glib.watches), 1)
        _, source, conditions, callback = self.glib.watches[0]
        self.assertEqual(source, 6)
        self.assertEqual(
            conditions,
            self.glib.IO_IN | self.glib.IO_HUP | self.glib.IO_ERR | self.glib.IO_NVAL,
        )
        self.assertIs(callback, self.sensor.on_command)

    def test_hup_without_input_does_not_read_or_keep_source(self):
        self.sensor.command_watch_id = 42
        with mock.patch.object(self.sensor.os, "read") as read:
            self.assertFalse(self.sensor.on_command(6, self.glib.IO_HUP))
        read.assert_not_called()
        self.assertEqual(len(self.glib.timeouts), 1)

    def test_observe_command_remains_connected_on_live_source(self):
        with (
            mock.patch.object(
                self.sensor.os,
                "read",
                return_value=b'{"command":"observe","protocolVersion":1}\n',
            ),
            mock.patch.object(self.sensor, "emit_ready") as emit_ready,
            mock.patch.object(self.sensor, "initial_snapshot") as initial_snapshot,
        ):
            self.assertTrue(self.sensor.on_command(6, self.glib.IO_IN))

        self.assertTrue(self.sensor.connected)
        self.assertEqual(self.sensor.command_buffer, b"")
        emit_ready.assert_called_once_with(False)
        initial_snapshot.assert_called_once_with()
        self.assertEqual(self.glib.timeouts, [])

    def test_hup_after_input_processes_complete_command_then_disconnects(self):
        with (
            mock.patch.object(
                self.sensor.os, "read", return_value=b'{"command":"observe"}\n'
            ),
            mock.patch.object(self.sensor, "emit_ready"),
            mock.patch.object(self.sensor, "initial_snapshot"),
        ):
            self.assertFalse(
                self.sensor.on_command(6, self.glib.IO_IN | self.glib.IO_HUP)
            )

        self.assertFalse(self.sensor.connected)
        self.assertEqual(len(self.glib.timeouts), 1)


if __name__ == "__main__":
    unittest.main()

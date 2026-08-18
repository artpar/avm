import importlib.util
import json
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("avm-command-agent.py")
SPEC = importlib.util.spec_from_file_location("avm_command_agent", SCRIPT)
AGENT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AGENT)


class CommandAgentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        AGENT.ROOT = pathlib.Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def test_atomic_state_round_trip(self):
        AGENT.atomic_write(AGENT.state_path("test"), {"state": "accepted"})
        self.assertEqual(AGENT.load("test")["state"], "accepted")

    def test_digest_is_stable_and_argv_sensitive(self):
        first = AGENT.request_digest("/workspace", ["printf", "%s", "a b"])
        second = AGENT.request_digest("/workspace", ["printf", "%s", "a b"])
        changed = AGENT.request_digest("/workspace", ["printf", "%s", "a*b"])
        self.assertEqual(first, second)
        self.assertNotEqual(first, changed)

    def test_rejects_shellless_invalid_argv_and_outside_cwd(self):
        with self.assertRaisesRegex(ValueError, "argv"):
            AGENT.validate_start({"commandId": "00000000-0000-0000-0000-000000000001", "argv": []})
        with self.assertRaisesRegex(ValueError, "inside /workspace"):
            AGENT.validate_start({"commandId": "00000000-0000-0000-0000-000000000001", "cwd": "/tmp", "argv": ["true"]})

    def test_list_state_files_are_json(self):
        AGENT.atomic_write(AGENT.state_path("00000000-0000-0000-0000-000000000001"), {"commandId": "00000000-0000-0000-0000-000000000001"})
        values = [json.loads(path.read_text()) for path in AGENT.ROOT.glob("*.json")]
        self.assertEqual(len(values), 1)


if __name__ == "__main__":
    unittest.main()

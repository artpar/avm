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
        self.workspace_root = pathlib.Path(self.temp.name) / "avm-workspace" / "generations" / "current"
        self.workspace_root.mkdir(parents=True)
        self.workspace_link = pathlib.Path(self.temp.name) / "workspace"
        self.workspace_link.symlink_to(self.workspace_root, target_is_directory=True)
        AGENT.WORKSPACE = self.workspace_link

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

    def test_accepts_workspace_symlink_and_descendant(self):
        descendant = self.workspace_root / "src"
        descendant.mkdir()
        root = AGENT.validate_start({"commandId": "00000000-0000-0000-0000-000000000001", "cwd": str(self.workspace_link), "argv": ["pwd"]})
        child = AGENT.validate_start({"commandId": "00000000-0000-0000-0000-000000000002", "cwd": str(self.workspace_link / "src"), "argv": ["pwd"]})
        self.assertEqual(pathlib.Path(root[1]), self.workspace_root.resolve())
        self.assertEqual(pathlib.Path(child[1]), descendant.resolve())

    def test_rejects_workspace_symlink_escape(self):
        outside = pathlib.Path(self.temp.name) / "outside"
        outside.mkdir()
        (self.workspace_root / "escape").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "inside /workspace"):
            AGENT.validate_start({"commandId": "00000000-0000-0000-0000-000000000003", "cwd": str(self.workspace_link / "escape"), "argv": ["pwd"]})

    def test_list_state_files_are_json(self):
        AGENT.atomic_write(AGENT.state_path("00000000-0000-0000-0000-000000000001"), {"commandId": "00000000-0000-0000-0000-000000000001"})
        values = [json.loads(path.read_text()) for path in AGENT.ROOT.glob("*.json")]
        self.assertEqual(len(values), 1)


if __name__ == "__main__":
    unittest.main()

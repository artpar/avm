#!/usr/bin/env python3
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class GuestGraphicsConfigTests(unittest.TestCase):
    def test_chromium_explicitly_uses_software_webgl(self):
        config = (ROOT / "vm/image/user-data.yaml").read_text(encoding="utf-8")
        self.assertIn("--use-gl=angle", config)
        self.assertIn("--use-angle=swiftshader-webgl", config)
        self.assertIn("--enable-unsafe-swiftshader", config)

    def test_authoritative_capture_stays_on_shared_memory_scanouts(self):
        vm = (ROOT / "src/vm.rs").read_text(encoding="utf-8")
        display = (ROOT / "src/display.rs").read_text(encoding="utf-8")
        self.assertIn('"virtio-vga".into()', vm)
        self.assertIn("gl=off,audiodev=avm-audio", vm)
        self.assertIn("display.scanout_dmabuf_unsupported", display)

    def test_command_agent_and_pinned_host_key_are_built_into_contract(self):
        config = (ROOT / "vm/image/user-data.yaml").read_text(encoding="utf-8")
        builder = (ROOT / "vm/image/build-base.sh").read_text(encoding="utf-8")
        self.assertIn("/usr/local/bin/avm-command-agent", config)
        self.assertIn("@@COMMAND_AGENT_B64@@", config)
        self.assertIn("ssh_host_ed25519_key.pub", builder)
        self.assertIn("avm_ssh_host_ed25519_key.pub", builder)

    def test_workspace_mount_has_stable_root_and_current_link(self):
        config = (ROOT / "vm/image/user-data.yaml").read_text(encoding="utf-8")
        self.assertIn("Where=/avm-workspace", config)
        self.assertIn("L /workspace - - - - /avm-workspace/current", config)


if __name__ == "__main__":
    unittest.main()

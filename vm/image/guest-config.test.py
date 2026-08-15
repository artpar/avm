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


if __name__ == "__main__":
    unittest.main()

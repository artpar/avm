#!/usr/bin/env python3
import importlib.util
import pathlib
import struct
import sys
import tempfile
import unittest
import zlib

sys.dont_write_bytecode = True

SCRIPT = pathlib.Path(__file__).with_name("png-region-check.py")
SPEC = importlib.util.spec_from_file_location("png_region_check", SCRIPT)
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


def write_rgb_png(path, width, height, color):
    def chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    scanline = b"\x00" + bytes(color) * width
    payload = scanline * height
    path.write_bytes(
        CHECKER.PNG_SIGNATURE
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(payload))
        + chunk(b"IEND", b"")
    )


class PngRegionCheckTests(unittest.TestCase):
    def test_accepts_expected_central_color(self):
        with tempfile.TemporaryDirectory() as directory:
            image = pathlib.Path(directory) / "frame.png"
            write_rgb_png(image, 8, 8, (32, 191, 64))
            ratio, matched, total, width, height = CHECKER.matching_ratio(
                image, (32, 191, 64), 0
            )
        self.assertEqual((width, height), (8, 8))
        self.assertEqual(matched, total)
        self.assertEqual(ratio, 1)

    def test_rejects_a_different_color(self):
        with tempfile.TemporaryDirectory() as directory:
            image = pathlib.Path(directory) / "frame.png"
            write_rgb_png(image, 8, 8, (230, 51, 204))
            ratio, _, _, _, _ = CHECKER.matching_ratio(image, (32, 191, 64), 4)
        self.assertEqual(ratio, 0)


if __name__ == "__main__":
    unittest.main()

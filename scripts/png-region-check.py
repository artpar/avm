#!/usr/bin/env python3
import argparse
import json
import struct
import sys
import zlib


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def paeth(left, above, upper_left):
    prediction = left + above - upper_left
    left_distance = abs(prediction - left)
    above_distance = abs(prediction - above)
    upper_left_distance = abs(prediction - upper_left)
    if left_distance <= above_distance and left_distance <= upper_left_distance:
        return left
    if above_distance <= upper_left_distance:
        return above
    return upper_left


def decode_png(path):
    data = path.read_bytes()
    if not data.startswith(PNG_SIGNATURE):
        raise ValueError("not a PNG file")
    offset = len(PNG_SIGNATURE)
    compressed = bytearray()
    header = None
    while offset < len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        offset += length + 12
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", payload)
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    if header is None:
        raise ValueError("PNG has no IHDR chunk")
    width, height, bit_depth, color_type, compression, filtering, interlace = header
    if bit_depth != 8 or color_type not in (2, 6):
        raise ValueError("only 8-bit RGB and RGBA PNGs are supported")
    if compression != 0 or filtering != 0 or interlace != 0:
        raise ValueError("unsupported PNG encoding")
    channels = 3 if color_type == 2 else 4
    stride = width * channels
    encoded = zlib.decompress(bytes(compressed))
    if len(encoded) != height * (stride + 1):
        raise ValueError("PNG scanline data has an unexpected length")
    rows = []
    previous = bytearray(stride)
    cursor = 0
    for _ in range(height):
        filter_type = encoded[cursor]
        cursor += 1
        filtered = encoded[cursor : cursor + stride]
        cursor += stride
        row = bytearray(stride)
        for index, value in enumerate(filtered):
            left = row[index - channels] if index >= channels else 0
            above = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_type == 0:
                predictor = 0
            elif filter_type == 1:
                predictor = left
            elif filter_type == 2:
                predictor = above
            elif filter_type == 3:
                predictor = (left + above) // 2
            elif filter_type == 4:
                predictor = paeth(left, above, upper_left)
            else:
                raise ValueError(f"unsupported PNG filter {filter_type}")
            row[index] = (value + predictor) & 0xFF
        rows.append(row)
        previous = row
    return width, height, channels, rows


def matching_ratio(path, target, tolerance):
    width, height, channels, rows = decode_png(path)
    x_start, x_end = width // 4, width * 3 // 4
    y_start, y_end = height // 4, height * 3 // 4
    matched = 0
    total = 0
    for row in rows[y_start:y_end]:
        for x in range(x_start, x_end):
            pixel = row[x * channels : x * channels + 3]
            total += 1
            if all(abs(actual - expected) <= tolerance for actual, expected in zip(pixel, target)):
                matched += 1
    return matched / total if total else 0, matched, total, width, height


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("image", type=__import__("pathlib").Path)
    parser.add_argument("red", type=int)
    parser.add_argument("green", type=int)
    parser.add_argument("blue", type=int)
    parser.add_argument("--tolerance", type=int, default=4)
    parser.add_argument("--minimum-ratio", type=float, default=0.8)
    arguments = parser.parse_args()
    target = (arguments.red, arguments.green, arguments.blue)
    ratio, matched, total, width, height = matching_ratio(
        arguments.image, target, arguments.tolerance
    )
    result = {
        "image": str(arguments.image),
        "width": width,
        "height": height,
        "targetRgb": target,
        "tolerance": arguments.tolerance,
        "matchedPixels": matched,
        "sampledPixels": total,
        "matchingRatio": ratio,
        "accepted": ratio >= arguments.minimum_ratio,
    }
    print(json.dumps(result, separators=(",", ":")))
    return 0 if result["accepted"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, zlib.error) as error:
        print(f"PNG check failed: {error}", file=sys.stderr)
        sys.exit(2)

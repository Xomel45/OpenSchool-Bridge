#!/usr/bin/env python3
"""Generates the shared test files of the settings format (see format.md). Run: python3 make_fixtures.py"""
import hashlib, json, os, zipfile

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")
PNG = bytes.fromhex("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082")
MP4 = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom"  # not a playable video: the readers only look at the name and the size

def build(name, entries):
    path = os.path.join(OUT, name)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as z:
        for n, data in entries:
            z.writestr(zipfile.ZipInfo(n, date_time=(2026, 10, 11, 0, 0, 0)), data)
    return path

def cfg(**kw):
    return json.dumps({"format": "openschool-config", "version": 1, **kw}, ensure_ascii=False, indent=2).encode()

os.makedirs(OUT, exist_ok=True)
FULL = dict(
    style="sunrise", scheme="dark", accent="#B04199", zoom=1.15,
    bg={"mode": "gradient", "kind": "image", "color": "#E2E8F0", "a": "#FF9966", "b": "#5B86E5", "angle": 120, "dim": 40, "blur": 3},
    notifications={"marks": True, "homework": False, "absences": True, "interval_min": 5, "detail": False},
    network={"mode": "direct"},
)
build("minimal.cfg", [("config.json", cfg())])
build("full.cfg", [("config.json", cfg(**FULL))])
build("with-image.cfg", [("config.json", cfg(**{**FULL, "bg": {**FULL["bg"], "mode": "media", "kind": "image"}})), ("background.png", PNG)])
build("with-video.cfg", [("config.json", cfg(**{**FULL, "bg": {**FULL["bg"], "mode": "media", "kind": "video"}})), ("background.mp4", MP4)])
# values that must be skipped, entries that must be ignored; what is valid next to them still applies
HOSTILE = dict(style="nope", scheme=7, accent="red", zoom=9, bg={"mode": "x", "color": "red", "dim": 500, "blur": -4},
               notifications={"marks": "yes", "interval_min": 7, "detail": False}, network={"mode": "teleport"}, surprise={"a": 1})
build("hostile.cfg", [("../evil.sh", b"x"), ("config.json", cfg(**HOSTILE)), ("sub/config.json", b'{"format":"other"}'), ("background.exe", b"x"), ("notes.txt", b"hi")])

sums = []
for f in sorted(os.listdir(OUT)):
    if f.endswith(".cfg"):
        sums.append(f"{hashlib.sha256(open(os.path.join(OUT, f), 'rb').read()).hexdigest()}  {f}")
open(os.path.join(OUT, "SHA256SUMS"), "w").write("\n".join(sums) + "\n")
print("\n".join(sums))

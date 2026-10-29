#!/usr/bin/env python3
"""Zip a staging folder into the payload that the launcher carries. Usage: make_zip.py DIR OUT.zip"""
import os
import sys
import zipfile

src, out = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for name in sorted(os.listdir(src)):
        path = os.path.join(src, name)
        if not os.path.isfile(path):
            continue
        info = zipfile.ZipInfo(name)
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = (0o755 if os.access(path, os.X_OK) else 0o644) << 16
        with open(path, "rb") as f:
            z.writestr(info, f.read(), compresslevel=9)
print(f"{out}: {os.path.getsize(out) / 1e6:.1f} MB")

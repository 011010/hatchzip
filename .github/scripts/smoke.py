"""Release smoke test: extract a freshly made archive with the built binary.

    python smoke.py path/to/hatchzip

Builds a ZIP with a few files in a temporary folder, extracts it in place,
and checks that every file came out byte for byte and the archive is gone.
Runs on every platform a release is built for, before anything is published.
"""

import os
import subprocess
import sys
import tempfile
import zipfile

binary = os.path.abspath(sys.argv[1])
files = {
    "game/data/big.bin": os.urandom(3_000_000),
    "game/config.ini": b"volume=7\n" * 20_000,
    "game/empty.txt": b"",
    "readme.txt": b"smoke test\n",
}

with tempfile.TemporaryDirectory() as tmp:
    archive = os.path.join(tmp, "smoke.zip")
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            z.writestr(name, data)

    run = lambda *args: subprocess.run([binary, *args], check=True, capture_output=True, text=True)
    print(run("--version").stdout.strip())
    print(run("check", archive, "--keep-free", "0").stdout)
    run("extract", archive, "--keep-free", "0", "--yes")

    out = os.path.join(tmp, "smoke")
    for name, data in files.items():
        with open(os.path.join(out, name), "rb") as f:
            assert f.read() == data, f"{name} differs"
    assert not os.path.exists(archive), "the archive should be gone"
    assert not os.path.exists(archive + ".hatchzip"), "the journal should be gone"
    print(f"ok: {len(files)} files extracted in place")

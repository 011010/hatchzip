"""Release smoke test: extract freshly made and stored archives with the
built binary.

    python smoke.py [runner ...] path/to/hatchzip

Builds a ZIP with a few files in a temporary folder, extracts it in place,
and checks that every file came out byte for byte and the archive is gone.
Then extracts a RAR set from the test fixtures, which every release build
must read. Runs on every platform a release is built for, before anything
is published. Anything before the binary runs it, such as
`qemu-aarch64-static` for a binary of another architecture.
"""

import os
import shutil
import subprocess
import sys
import tempfile
import zipfile

binary = os.path.abspath(sys.argv[-1])
runner = sys.argv[1:-1]
root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
files = {
    "game/data/big.bin": os.urandom(3_000_000),
    "game/config.ini": b"volume=7\n" * 20_000,
    "game/empty.txt": b"",
    "readme.txt": b"smoke test\n",
}


def run(*args):
    return subprocess.run([*runner, binary, *args], check=True, capture_output=True, text=True)


with tempfile.TemporaryDirectory() as tmp:
    archive = os.path.join(tmp, "smoke.zip")
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            z.writestr(name, data)

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

# RAR goes through UnRAR, C++ compiled into the binary: this catches a build
# without it, or one whose C++ runtime is missing or broken.
with tempfile.TemporaryDirectory() as tmp:
    fixture = os.path.join(root, "crates", "cli", "tests", "fixtures", "rar", "rar5")
    for name in os.listdir(fixture):
        shutil.copy(os.path.join(fixture, name), tmp)
    first = os.path.join(tmp, "set.part1.rar")
    out = os.path.join(tmp, "out")
    run("extract", first, "-o", out, "--keep-free", "0", "--yes")
    with open(os.path.join(out, "game", "config", "settings.ini"), "rb") as f:
        assert f.read() == b"volume=7\n" * 2_000, "settings.ini differs"
    sizes = {"a.pak": 30_000, "b.pak": 20_000, "c.pak": 36_000, "d.pak": 6_000}
    for name, size in sizes.items():
        assert os.path.getsize(os.path.join(out, "game", name)) == size, f"{name} differs"
    left = [n for n in os.listdir(tmp) if n != "out"]
    assert not left, f"left behind: {left}"
    print("ok: a RAR set extracted in place")

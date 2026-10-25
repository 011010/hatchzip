"""Release check: the built programs have no way to reach the network.

    python no-network.py path/to/program ...

HatchZip promises no network connections (SECURITY.md), and cargo-deny keeps
networking crates out of the dependencies. This checks the result: the
Windows and macOS programs the release ships.

- Windows: no networking DLL in the import table or the delay-load table,
  and no networking DLL named anywhere in the file, so none can be loaded
  by name at run time either.
- macOS: no socket or name lookup function among the imported symbols, and
  no networking framework among the linked libraries (`nm` and `otool`,
  which every macOS runner has).

Runs before anything is published. Only the standard library is used.
"""

import struct
import subprocess
import sys

WINDOWS_DLLS = [
    "ws2_32", "wsock32", "mswsock", "wininet", "winhttp", "urlmon",
    "dnsapi", "iphlpapi", "webio", "httpapi", "winsock",
]
MACOS_SYMBOLS = {
    "_socket", "_connect", "_getaddrinfo", "_gethostbyname", "_sendto",
}
MACOS_LIBRARIES = ["CFNetwork.framework", "Network.framework"]


def pe_imports(data):
    """The DLL names in a PE file's import and delay-load tables."""
    (pe,) = struct.unpack_from("<I", data, 0x3C)
    if data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("not a PE file")
    sections, opt_size = struct.unpack_from("<H12xH", data, pe + 6)
    opt = pe + 24
    (magic,) = struct.unpack_from("<H", data, opt)
    dirs = opt + (112 if magic == 0x20B else 96)
    table = opt + opt_size
    spans = []
    for i in range(sections):
        vsize, vaddr, raw_size, raw_ptr = struct.unpack_from("<4I", data, table + 40 * i + 8)
        spans.append((vaddr, max(vsize, raw_size), raw_ptr))

    def offset(rva):
        for vaddr, size, raw in spans:
            if vaddr <= rva < vaddr + size:
                return rva - vaddr + raw
        raise ValueError(f"address {rva:#x} is in no section")

    def name(rva):
        at = offset(rva)
        return data[at:data.index(b"\0", at)].decode("ascii").lower()

    names = []
    # The import table (entry 1) has 20-byte records with the name at 12;
    # the delay-load table (entry 13) has 32-byte records with it at 4.
    for entry, size, at in [(1, 20, 12), (13, 32, 4)]:
        (rva,) = struct.unpack_from("<I", data, dirs + 8 * entry)
        if rva == 0:
            continue
        record = offset(rva)
        while any(data[record:record + size]):
            (dll,) = struct.unpack_from("<I", data, record + at)
            names.append(name(dll))
            record += size
    return names


def check_windows(path, data):
    problems = []
    imports = pe_imports(data)
    if not imports:
        problems.append("no imports found, so the check could not run")
    lowered = data.lower()
    for dll in WINDOWS_DLLS:
        if any(i.startswith(dll) for i in imports):
            problems.append(f"imports {dll}.dll")
        elif dll.encode() in lowered or dll.encode("utf-16-le") in lowered:
            problems.append(f"names {dll} in its data")
    print(f"{path}: imports {', '.join(sorted(set(imports)))}")
    return problems


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def check_macos(path):
    problems = []
    symbols = {line.strip() for line in run("nm", "-u", path).splitlines()}
    if not symbols:
        problems.append("no imported symbols found, so the check could not run")
    problems += [f"imports {s}" for s in sorted(MACOS_SYMBOLS & symbols)]
    libraries = run("otool", "-L", path)
    problems += [f"links {lib}" for lib in MACOS_LIBRARIES if lib in libraries]
    return problems


failed = False
for path in sys.argv[1:]:
    with open(path, "rb") as f:
        data = f.read()
    if data[:2] == b"MZ":
        problems = check_windows(path, data)
    elif data[:4] in (b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe"):
        problems = check_macos(path)
    else:
        problems = ["is neither a Windows nor a macOS program"]
    for p in problems:
        print(f"{path}: {p}")
    failed |= bool(problems)
    if not problems:
        print(f"{path}: no networking")
sys.exit(1 if failed else 0)

#!/usr/bin/env python3
"""List Rusk PIDs using this demo's exact state archive, without printing argv."""
import argparse
import ctypes
import os
from pathlib import Path
import subprocess
import sys


def darwin_argv(data):
    """Decode KERN_PROCARGS2: argc, executable path, padding, then argv."""
    if len(data) < 4:
        raise ValueError("missing argc")
    count = int.from_bytes(data[:4], sys.byteorder, signed=True)
    if not 0 < count <= 16384:
        raise ValueError("invalid argc")
    start = data.index(b"\0", 4) + 1
    while start < len(data) and data[start] == 0:
        start += 1
    parts = data[start:].split(b"\0")
    if len(parts) <= count:
        raise ValueError("truncated argv")
    return parts[:count]


def process_argv(pid):
    if sys.platform.startswith("linux"):
        return Path(f"/proc/{pid}/cmdline").read_bytes().rstrip(b"\0").split(b"\0")
    if sys.platform != "darwin":
        raise RuntimeError(f"unsupported process platform: {sys.platform}")
    libc = ctypes.CDLL(None, use_errno=True)
    sysctl = libc.sysctl
    sysctl.argtypes = [ctypes.POINTER(ctypes.c_int), ctypes.c_uint, ctypes.c_void_p,
                      ctypes.POINTER(ctypes.c_size_t), ctypes.c_void_p, ctypes.c_size_t]
    sysctl.restype = ctypes.c_int
    # KERN_ARGMAX bounds the kernel-provided argument buffer.
    argmax = ctypes.c_int()
    size = ctypes.c_size_t(ctypes.sizeof(argmax))
    if sysctl((ctypes.c_int * 2)(1, 8), 2, ctypes.byref(argmax), ctypes.byref(size), None, 0):
        raise OSError(ctypes.get_errno(), "cannot read argument limit")
    if not 0 < argmax.value <= 2 * 1024 * 1024:
        raise ValueError("invalid argument limit")
    buffer = ctypes.create_string_buffer(argmax.value)
    size = ctypes.c_size_t(len(buffer))
    if sysctl((ctypes.c_int * 3)(1, 49, pid), 3, buffer, ctypes.byref(size), None, 0):
        raise OSError(ctypes.get_errno(), "cannot read process arguments")
    return darwin_argv(buffer.raw[:size.value])


def uses_state(argv, state):
    if not argv or os.path.basename(argv[0]) != b"rusk":
        return False
    return any(
        arg == b"--state=" + state
        or (arg in (b"-s", b"--state") and index + 1 < len(argv) and argv[index + 1] == state)
        for index, arg in enumerate(argv[1:], 1)
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", required=True)
    args = parser.parse_args()
    result = subprocess.run(["pgrep", "-x", "rusk"], capture_output=True, text=True)
    if result.returncode not in (0, 1):
        raise RuntimeError("cannot list Rusk processes")
    for value in result.stdout.split():
        pid = int(value)
        try:
            matches = uses_state(process_argv(pid), os.fsencode(args.state))
        except (OSError, ValueError):
            continue  # Exited or inaccessible processes are never selected.
        if matches:
            print(pid)


if __name__ == "__main__":
    main()

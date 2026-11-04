"""Read only proxy settings from an agent process on Linux, macOS or Windows.

The full environment stays in this process; only the explicit allowlist is emitted.
"""
import ctypes
import json
import os
from pathlib import Path
import struct
import sys

NAMES = {'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'}


def selected(entries):
    out = {}
    for entry in entries:
        key, sep, value = entry.partition('=')
        if os.name == 'nt':
            key = key.lower()  # Windows environment names are case-insensitive.
        if sep and key.lower() in NAMES and key in (key.lower(), key.upper()):
            out[key] = value
    return out


def darwin_environment(pid):
    # KERN_PROCARGS2 gives argv and the environment as separate NUL-terminated strings.
    libc = ctypes.CDLL(None, use_errno=True)
    mib = (ctypes.c_int * 3)(1, 49, pid)
    size = ctypes.c_size_t(1024 * 1024)
    buf = ctypes.create_string_buffer(size.value)
    if libc.sysctl(mib, 3, buf, ctypes.byref(size), None, 0) != 0:
        raise OSError(ctypes.get_errno(), 'Cannot read process arguments')
    data = buf.raw[:size.value]
    argc = struct.unpack_from('i', data)[0]
    pos = data.index(b'\0', 4) + 1  # executable path, then padding
    while pos < len(data) and data[pos] == 0:
        pos += 1
    for _ in range(argc):
        pos = data.index(b'\0', pos) + 1
    return data[pos:].decode('utf-8', errors='replace').split('\0')


def windows_environment(pid):
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    nt = ctypes.WinDLL('ntdll')
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.ReadProcessMemory.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
    nt.NtQueryInformationProcess.argtypes = [wintypes.HANDLE, wintypes.ULONG, ctypes.c_void_p, wintypes.ULONG, ctypes.c_void_p]
    nt.NtQueryInformationProcess.restype = wintypes.LONG
    handle = kernel.OpenProcess(0x410, False, pid)  # query information + read memory
    if not handle:
        raise OSError('Cannot open process')
    try:
        def read(address, count):
            buf = ctypes.create_string_buffer(count)
            done = ctypes.c_size_t()
            kernel.ReadProcessMemory(handle, address, buf, count, ctypes.byref(done))
            if not done.value:
                raise OSError('Cannot read process memory')
            return buf.raw[:done.value]
        info = (ctypes.c_void_p * 6)()
        if nt.NtQueryInformationProcess(handle, 0, info, ctypes.sizeof(info), None) != 0:
            raise OSError('Cannot read process information')
        pointer_size = ctypes.sizeof(ctypes.c_void_p)
        peb = info[1]
        if pointer_size == 8:
            wow = ctypes.c_void_p()
            if nt.NtQueryInformationProcess(handle, 26, ctypes.byref(wow), ctypes.sizeof(wow), None) == 0 and wow.value:
                peb, pointer_size = wow.value, 4
        pointer = lambda address: int.from_bytes(read(address, pointer_size), 'little')
        params = pointer(peb + (0x20 if pointer_size == 8 else 0x10))
        environment = pointer(params + (0x80 if pointer_size == 8 else 0x48))
        data = b''
        while len(data) < 1024 * 1024:
            address = environment + len(data)
            data += read(address, min(256, 4096 - address % 4096))
            end = next((i for i in range(max(0, len(data) - 260) // 2 * 2, len(data) - 3, 2) if data[i:i + 4] == b'\0\0\0\0'), None)
            if end is not None:
                return data[:end].decode('utf-16-le', errors='replace').split('\0')
        raise OSError('Environment too large')
    finally:
        kernel.CloseHandle(handle)


def process_proxies(pid):
    if sys.platform == 'darwin':
        entries = darwin_environment(pid)
    elif os.name == 'nt':
        entries = windows_environment(pid)
    else:
        entries = Path(f'/proc/{pid}/environ').read_bytes().decode('utf-8', errors='replace').split('\0')
    return selected(entries)


if __name__ == '__main__':
    try:
        print(json.dumps(process_proxies(int(sys.argv[1]))))
    except (OSError, ValueError, IndexError):
        print('{}')

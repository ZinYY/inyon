"""Windows SSH bridge for existing Herdr panes. No listener or agent hooks are installed.

The hub sends this source and a command as a length-prefixed JSON frame on stdin.
Only the remaining stdin belongs to the command (including pasted text). Herdr's
local named pipe supplies exact session identities, terminal screens and input.
"""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
import threading
import time


def emit(value):
    sys.stdout.buffer.write((str(value) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()


def rpc(address, method, params=None):
    """One bounded NDJSON request; never launch a server or change its access policy."""
    import msvcrt
    from ctypes import wintypes
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.PeekNamedPipe.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                                    ctypes.c_void_p, ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    kernel.PeekNamedPipe.restype = wintypes.BOOL
    deadline = time.monotonic() + 5
    with open("\\\\.\\pipe\\" + address, "r+b", buffering=0) as pipe:
        request = json.dumps({"id": "inyon", "method": method, "params": params or {}}, ensure_ascii=False).encode("utf-8") + b"\n"
        pipe.write(request)
        data = bytearray()
        handle = msvcrt.get_osfhandle(pipe.fileno())
        while time.monotonic() < deadline:
            available = wintypes.DWORD()
            if not kernel.PeekNamedPipe(handle, None, 0, None, ctypes.byref(available), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if available.value:
                chunk = pipe.read(min(available.value, 65536))
                if not chunk:
                    raise RuntimeError("Herdr closed the API connection")
                data.extend(chunk)
                if len(data) > 64 * 1024 * 1024:
                    raise RuntimeError("Herdr response exceeds 64 MiB")
                if b"\n" in data:
                    answer = json.loads(data.split(b"\n", 1)[0])
                    if "error" in answer:
                        raise RuntimeError(answer["error"].get("message", "Herdr API error"))
                    return answer["result"]
            else:
                time.sleep(0.005)
    raise TimeoutError("Herdr API did not answer within 5 seconds")


def sessions():
    r = subprocess.run(["herdr", "session", "list", "--json"], capture_output=True, timeout=8,
                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    if r.returncode:
        raise RuntimeError(r.stderr.decode("utf-8", errors="replace").strip() or "Could not list Herdr sessions")
    return [s for s in json.loads(r.stdout)["sessions"] if s.get("running")]


def number(value):
    return int(hashlib.sha256(value.encode("utf-8")).hexdigest()[:8], 16)


def mapped(session, snap):
    workspaces = {w["workspace_id"]: w for w in snap["workspaces"]}
    tabs = {t["tab_id"]: t for t in snap["tabs"]}
    layouts = {p["pane_id"]: p["rect"] for l in snap["layouts"] for p in l["panes"]}
    out = []
    for p in snap["panes"]:
        workspace = workspaces[p["workspace_id"]]
        tab = tabs[p["tab_id"]]
        sid = number(session["name"] + "/" + p["workspace_id"])
        pid = number(p["pane_id"])
        wid = number(p["tab_id"])
        out.append(dict(p, address=session["socket_path"], session_name=session["name"],
                        sid=f"${sid}", id=f"%{sid}.{pid}", wid=f"@{sid}.{wid}",
                        workspace_label=workspace.get("label") or session["name"],
                        tab_label=tab.get("label") or p["tab_id"], tab_number=tab.get("number", 0),
                        rect=layouts.get(p["pane_id"], {"width": 80, "height": 24})))
    return out


def panes():
    out = []
    for session in sessions():
        snap = rpc(session["socket_path"], "session.snapshot")["snapshot"]
        out.extend(mapped(session, snap))
    if len({p["id"] for p in out}) != len(out):
        raise RuntimeError("Herdr pane ID collision; refusing to route input")
    return out


def selected(rows, target):
    found = [p for p in rows if target in (p["id"], p["wid"], p["sid"])]
    if len(found) != 1:
        raise RuntimeError("Herdr target is missing or ambiguous: " + target)
    return found[0]


def process_started(pid):
    from ctypes import wintypes
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        return 0
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
            return 0
        return ((times[0].dwHighDateTime << 32) + times[0].dwLowDateTime) / 10000000 - 11644473600
    finally:
        kernel.CloseHandle(handle)


def transcript(p, home):
    identity = p.get("agent_session") or {}
    if p.get("agent") != "claude" or identity.get("kind") != "id":
        return None
    value = identity.get("value", "")
    if not re.fullmatch(r"[0-9a-fA-F-]{36}", value):
        return None
    # Match the exact identity supplied by Herdr's existing Claude integration.
    hits = list((home / ".claude" / "projects").glob("*/" + value + ".jsonl"))
    return (value, hits[0].as_posix()) if len(hits) == 1 else None


def clean(value):
    return str(value).replace("\x1f", " ").replace("\r", " ").replace("\n", " ")


def fields(p):
    return {
        "session_id": p["sid"], "session_name": p["workspace_label"], "session_attached": "1",
        "window_id": p["wid"], "window_index": p["tab_number"], "window_name": p["tab_label"],
        "window_active": int(p.get("focused", False)), "window_activity": p.get("activity", 0),
        "pane_id": p["id"], "pane_index": 0, "pane_active": 1, "pane_pid": p.get("shell_pid", 0),
        "pane_current_command": p.get("agent") or "powershell", "pane_current_path": p.get("cwd", "").replace("\\", "/"),
        "pane_title": p.get("terminal_title_stripped") or p.get("name") or p["tab_label"],
        "pane_width": p["rect"]["width"], "pane_height": p["rect"]["height"], "pane_dead": 0,
        "window_width": p["rect"]["width"], "window_height": p["rect"]["height"],
        # The public Herdr read API does not expose its terminal cursor.
        "cursor_x": 0, "cursor_y": 0, "cursor_flag": 0, "alternate_on": 0,
    }


def probe():
    rows = panes()
    home = Path.home()
    now = int(time.time())
    processes, hooks = [], []
    for p in rows:
        info = rpc(p["address"], "pane.process_info", {"pane_id": p["pane_id"]})["process_info"]
        p["shell_pid"] = info.get("shell_pid", 0)
        processes.append(f'{p["shell_pid"]} 0 0 powershell powershell')
        for proc in info.get("foreground_processes", []):
            name = proc.get("name", "").removesuffix(".exe")
            elapsed = max(0, now - int(process_started(proc["pid"])))
            processes.append(f'{proc["pid"]} {p["shell_pid"]} {elapsed} {name} {clean(proc.get("cmdline", name))}')
        match = transcript(p, home)
        if match:
            p["activity"] = int(Path(match[1]).stat().st_mtime)
            hooks.append({"pane": p["id"], "ts": now, "data": {
                "session_id": match[0], "transcript_path": match[1], "cwd": p.get("cwd", ""),
                # Identity only: do not invent Stop/UserPromptSubmit events every poll.
                "hook_event_name": "HerdrSessionIdentity",
            }})
    emit("@@HOST " + socket.gethostname())
    emit("@@HOME " + home.as_posix())
    emit("@@OS Windows")
    emit("@@NOW " + str(now))
    emit("@@KIND Windows|3|||1|0||1")
    emit("@@BINS")
    for name in sorted({p.get("agent") for p in rows} - {None}):
        emit(name)
    emit("@@TMUX")
    order = ["session_id", "session_name", "session_attached", "window_id", "window_index", "window_name",
             "window_active", "window_activity", "pane_id", "pane_index", "pane_active", "pane_pid",
             "pane_current_command", "pane_current_path", "pane_title", "pane_width", "pane_height", "pane_dead",
             "window_width", "window_height"]
    for p in rows:
        f = fields(p)
        emit("\x1f".join(clean(f[k]) for k in order))
    emit("@@CLIENTS")
    for sid in sorted({p["sid"] for p in rows}):
        emit(sid + " 0")
    emit("@@PS")
    for proc in processes:
        emit(proc)
    emit("@@HOOK")
    emit("missing")  # Herdr hooks supply identity; Inyon's usage integration is not installed.
    emit("@@PANES")
    for hook in hooks:
        emit(json.dumps(hook, ensure_ascii=False))
    emit("@@END")


def option(args, key, default=None):
    return args[args.index(key) + 1] if key in args and args.index(key) + 1 < len(args) else default


def key_name(key):
    keys = {"Enter": "enter", "Escape": "esc", "BSpace": "backspace", "Space": "space", "BTab": "shift+tab",
            "PPage": "pageup", "NPage": "pagedown", "PageUp": "pageup", "PageDown": "pagedown", "IC": "insert", "DC": "delete"}
    for prefix, name in [("C-", "ctrl+"), ("M-", "alt+"), ("S-", "shift+")]:
        if key.startswith(prefix):
            return name + key_name(key[len(prefix):])
    return keys.get(key, key.lower() if len(key) > 1 else key)


def tmux(args):
    # Global -t is used by Inyon's paste-buffer path for Windows multiplexers.
    if args[:1] == ["-t"]:
        args = args[2:]
    if not args:
        raise RuntimeError("Missing terminal operation")
    op, args = args[0], args[1:]
    if op in ("resize-window", "set-option", "set-window-option"):
        # Herdr owns the desktop layout. Keep its actual dimensions when mirroring.
        return
    if op in ("load-buffer", "delete-buffer", "paste-buffer"):
        name = option(args, "-b", "")
        if not re.fullmatch(r"inyon-[A-Za-z0-9-]+", name):
            raise RuntimeError("Invalid paste buffer")
        folder = Path.home() / ".inyon" / "herdr-buffers"
        folder.mkdir(parents=True, exist_ok=True)
        path = folder / name
        if op == "load-buffer":
            path.write_bytes(Path(args[-1]).read_bytes())
            return
        if op == "delete-buffer":
            path.unlink(missing_ok=True)
            return
        try:
            p = selected(panes(), option(args, "-t"))
            text = path.read_text(encoding="utf-8")
            rpc(p["address"], "pane.send_text", {"pane_id": p["pane_id"], "text": text})
        finally:
            path.unlink(missing_ok=True)
        return
    rows = panes()
    if op in ("list-panes", "list-sessions", "list-clients"):
        if op == "list-clients":
            for sid in sorted({p["sid"] for p in rows}):
                emit(sid + " 0")
            return
        fmt = option(args, "-F", "#{pane_id}")
        seen = set()
        for p in rows:
            if op == "list-sessions" and p["sid"] in seen:
                continue
            seen.add(p["sid"])
            f = fields(p)
            emit(re.sub(r"#\{([^}]+)\}", lambda m: clean(f.get(m[1], "")), fmt))
        return
    p = selected(rows, option(args, "-t"))
    if op in ("display", "display-message"):
        f = fields(p)
        emit(re.sub(r"#\{([^}]+)\}", lambda m: clean(f.get(m[1], "")), args[-1]))
    elif op == "capture-pane":
        result = rpc(p["address"], "pane.read", {"pane_id": p["pane_id"], "source": "visible",
                     "format": "ansi" if "-e" in args else "text", "strip_ansi": "-e" not in args})
        emit(result["read"]["text"].rstrip("\n"))
    elif op == "send-keys":
        values = args[:]
        at = values.index("-t")
        del values[at:at + 2]
        if "-H" in values:
            values.remove("-H")
            text = bytes.fromhex("".join(values)).decode("utf-8")
            rpc(p["address"], "pane.send_text", {"pane_id": p["pane_id"], "text": text})
        elif "-l" in values:
            values.remove("-l")
            rpc(p["address"], "pane.send_text", {"pane_id": p["pane_id"], "text": "".join(values)})
        else:
            rpc(p["address"], "pane.send_keys", {"pane_id": p["pane_id"], "keys": [key_name(k) for k in values]})
    else:
        raise RuntimeError("Herdr bridge does not support terminal operation: " + op)


def control(command):
    target = re.search(r"-t '([^']+)'", command)
    sid = target[1] if target else None
    closed = threading.Event()
    def watch_stdin():
        while os.read(0, 4096):
            pass
        closed.set()
    threading.Thread(target=watch_stdin, daemon=True).start()
    previous = {}
    while not closed.is_set():
        rows = [p for p in panes() if sid is None or p["sid"] == sid]
        current = {}
        for p in rows:
            result = rpc(p["address"], "pane.read", {"pane_id": p["pane_id"], "source": "visible", "format": "text", "strip_ansi": True})
            current[p["id"]] = result["read"]["text"]
            if previous.get(p["id"]) != current[p["id"]]:
                emit("%output " + p["id"] + " changed")
        if set(previous) != set(current):
            emit("%sessions-changed")
        previous = current
        closed.wait(0.75)


def execute(request):
    home = Path.home()
    busybox = home / ".inyon" / "bin" / "busybox.exe"
    if not busybox.is_file():
        raise RuntimeError("Windows Herdr bridge needs BusyBox; run scripts/setup-windows-herdr.ps1 from the Inyon repository")
    if request["mode"] == "probe":
        return probe()
    if request["mode"] == "control":
        return control(request["command"])
    source = request["source"].encode("utf-8")
    path = home / ".inyon" / "bridges" / ("herdr-" + hashlib.sha256(source).hexdigest()[:16] + ".py")
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        temporary = path.with_suffix(f".{os.getpid()}.tmp")
        temporary.write_bytes(source)
        os.replace(temporary, path)
    env = os.environ.copy()
    env.update(HOME=home.as_posix(), PYTHONIOENCODING="utf-8", PYTHONUTF8="1")
    env["PATH"] = str(busybox.parent) + os.pathsep + str(Path(sys.executable).parent) + os.pathsep + env.get("PATH", "")
    prelude = ("python3() { command python.exe \"$@\"; }\n"
               "tmux() { command python.exe -u " + shlex.quote(path.as_posix()) + " tmux \"$@\"; }\n")
    return subprocess.call([str(busybox), "sh", "-c", prelude + request["command"]], env=env)


if __name__ == "__main__":
    try:
        if "INYON_REQUEST" in globals():
            result = execute(INYON_REQUEST)
        elif sys.argv[1:2] == ["tmux"]:
            result = tmux(sys.argv[2:])
        else:
            raise RuntimeError("This bridge is started by Inyon over SSH")
        sys.exit(result or 0)
    except Exception as error:
        print("Inyon Windows/Herdr: " + str(error), file=sys.stderr)
        sys.exit(1)

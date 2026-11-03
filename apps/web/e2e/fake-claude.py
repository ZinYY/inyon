#!/usr/bin/python3
# A stand-in for Claude Code in a tmux pane: draws its input box, takes typed text, and writes
# each message to a transcript the way Claude does, plus a log of everything it received.
import json, os, sys, tty, uuid, datetime, shutil

home = os.environ["HOME"]
cwd = os.getcwd()
sid = str(uuid.uuid4())
proj = os.path.join(home, ".claude", "projects", "".join(c if c.isalnum() else "-" for c in cwd))
os.makedirs(proj, exist_ok=True)
path = os.path.join(proj, sid + ".jsonl")
log = os.path.join(home, "fake-claude.log")
shown = []


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z")


def write(entry):
    entry.update({"uuid": str(uuid.uuid4()), "timestamp": now(), "sessionId": sid, "cwd": cwd})
    with open(path, "a") as f:
        f.write(json.dumps(entry) + "\n")


def user(text):
    write({"type": "user", "message": {"role": "user", "content": text}})


def reply(text):
    write({"type": "assistant", "message": {"id": str(uuid.uuid4()), "role": "assistant", "model": "claude-fake-1", "content": [{"type": "text", "text": text}]}})


user("hello")
reply("Hi. I'm a stand-in for Claude.")
shown += ["> hello", "  Hi. I'm a stand-in for Claude."]

buf = ""


def draw():
    w = shutil.get_terminal_size((100, 30)).columns
    rule = "─" * (w - 2)
    out = "\x1b[2J\x1b[H" + "\r\n".join(shown[-10:]) + "\r\n\r\n"
    rows = buf.split("\n")
    out += rule + "\r\n" + "❯ " + rows[0] + "".join("\r\n  " + r for r in rows[1:]) + "\r\n" + rule + "\r\n  ? for shortcuts\r\n"
    sys.stdout.write(out)
    sys.stdout.flush()


def submit(text):
    with open(log, "a") as f:
        f.write(json.dumps(text) + "\n")
    if text.startswith("/"):
        name, _, args = text.partition(" ")
        user(f"<command-name>{name}</command-name>\n<command-message>{name[1:]}</command-message>\n<command-args>{args}</command-args>")
        shown.append(f"> {text}")
    else:
        user(text)
        reply(f"Got it: {text}")
        shown.extend([f"> {text}", f"  Got it: {text}"])


tty.setraw(0)
# ask for bracketed paste, as Claude does
sys.stdout.write("\x1b[?2004h")
draw()
pending = ""
# inside a bracketed paste a line break is part of the text, not Enter
pasting = False
while True:
    data = os.read(0, 4096)
    if not data:
        break
    pending += data.decode("utf-8", "replace")
    i = 0
    while i < len(pending):
        ch = pending[i]
        if ch == "\x1b":
            # escape sequences: bracketed paste markers, arrows (ignored)
            j = i + 1
            if j < len(pending) and pending[j] == "[":
                j += 1
                while j < len(pending) and not (pending[j].isalpha() or pending[j] == "~"):
                    j += 1
            seq = pending[i:j + 1]
            if seq == "\x1b[200~":
                pasting = True
            elif seq == "\x1b[201~":
                pasting = False
            i = j + 1
            continue
        if ch == "\r" and pasting:
            buf += "\n"
        elif ch == "\r":
            text = buf.strip()
            buf = ""
            if text:
                submit(text)
        elif ch == "\n":
            buf += "\n"
        elif ch in ("\x7f", "\x08"):
            buf = buf[:-1]
        elif ch in ("\x03", "\x15"):
            buf = ""
        elif ch >= " ":
            buf += ch
        i += 1
    pending = ""
    draw()

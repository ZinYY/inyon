"""Devin Desktop history and explicitly visible tmux handoffs.

The native store is always read-only. Only Devin writes its conversations.
Busy/unknown owners are never force-opened. Our files contain dispatch receipts
and tmux identities, never credentials or copies of the native database.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
import time

BUSY = 'Devin can only receive a new message after the previous response has finished. Finish any pending question or approval in Devin first.'
UNKNOWN = 'Devin can only resume when its previous turn is confirmed finished. Open the session in Devin and finish the turn first.'
HOME = Path.home()
ROOT = HOME / '.inyon/devin'
DATA = Path(os.environ.get('DEVIN_SESSIONS_DIR') or Path(os.environ.get('XDG_DATA_HOME', str(HOME / '.local/share'))) / 'devin/cli')
_LOG_CACHE = {}


class Refused(Exception):
    pass


def valid_id(value):
    return isinstance(value, str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,127}', value) is not None


def connect():
    db = sqlite3.connect((DATA / 'sessions.db').as_uri() + '?mode=ro', uri=True, timeout=2)
    db.execute('PRAGMA query_only=ON')
    return db


def object_json(raw):
    value = json.loads(raw or 'null')
    return value if isinstance(value, dict) else {}


def selected_nodes(rows, head):
    """Iterative postorder: selected ancestry and explicit compaction links only."""
    nodes = {r[0]: r for r in rows}
    seen, visiting, result = set(), set(), []
    stack = [(head, False)]
    while stack:
        node, exit_node = stack.pop()
        if node is None or node in seen:
            continue
        if node not in nodes:
            raise Refused('Devin history is incomplete; reload after Devin has saved it.')
        row = nodes[node]
        if exit_node:
            visiting.remove(node)
            seen.add(node)
            result.append(row)
            continue
        if node in visiting:
            raise Refused('Devin history contains a cyclic branch.')
        visiting.add(node)
        meta = object_json(row[3])
        prior = (meta.get('extensions') or {}).get('compact/prior_node_ids') or []
        prior = [p for p in prior if isinstance(p, int) and p != node and p in nodes]
        stack.append((node, True))
        stack.extend((p, False) for p in reversed([row[1]] + prior))
    return result


def text_content(value):
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return '\n'.join(p['text'] for p in value if isinstance(p, dict) and isinstance(p.get('text'), str))
    return ''


def session_info(db, sid):
    row = db.execute('SELECT id,title,working_directory,model,agent_mode,last_activity_at,main_chain_id,workspace_dirs FROM sessions WHERE id=? AND hidden=0', (sid,)).fetchone()
    if row is None:
        raise Refused('Devin session not found.')
    return dict(zip(('id', 'title', 'cwd', 'model', 'mode', 'updated', 'head', 'workspaces'), row))


def history(db, sid):
    deadline = time.monotonic() + 5
    db.set_progress_handler(lambda: int(time.monotonic() > deadline), 10000)
    db.execute('BEGIN')
    try:
        info = session_info(db, sid)
        rows = db.execute('SELECT node_id,parent_node_id,chat_message,metadata FROM message_nodes WHERE session_id=?', (sid,)).fetchall()
        messages = {}
        for node, parent, raw, _ in selected_nodes(rows, info['head']):
            m = object_json(raw)
            role, meta = m.get('role'), m.get('metadata') or {}
            if role not in ('user', 'assistant', 'tool') or (role == 'user' and meta.get('is_user_input') is not True):
                continue
            mid = m.get('message_id')
            if not isinstance(mid, str):
                continue
            text = text_content(m.get('content'))
            tools = [{'name': t.get('name', 'Tool'), 'input': t.get('arguments')} for t in (m.get('tool_calls') or []) if isinstance(t, dict)]
            thinking = (m.get('thinking') or {}).get('thinking')
            if not text and not tools and not thinking:
                continue
            # No signatures, system context, environment or other metadata.
            messages[mid] = {'id': mid, 'role': role, 'text': text, 'tools': tools, 'thinking': thinking,
                             'time': meta.get('created_at'), 'finish': meta.get('finish_reason')}
        return info, messages
    finally:
        db.rollback()
        db.set_progress_handler(None, 0)


def process(pid):
    if not isinstance(pid, int) or pid <= 1:
        return None
    r = subprocess.run(['ps', '-p', str(pid), '-o', 'comm='], capture_output=True, text=True, timeout=3)
    return r.stdout.strip() if r.returncode == 0 else None


def lock_owner(sid):
    path = DATA / 'session_locks' / (sid + '.lock')
    try:
        with path.open('r') as f:
            try:
                fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                pid = int(f.read().strip())
                comm = process(pid)
                if comm and Path(comm).name == 'devin':
                    return pid
                raise Refused(UNKNOWN)
            else:
                fcntl.flock(f, fcntl.LOCK_UN)
                return None
    except FileNotFoundError:
        return None


def log_idle(pid):
    """Prompt spans close after response persistence. Missing evidence fails closed."""
    logs = sorted((DATA / 'logs').glob('*_' + str(pid) + '.log'), key=lambda p: p.stat().st_mtime)
    if not logs:
        return None
    p = logs[-1]
    stat = p.stat()
    cached = _LOG_CACHE.get(pid)
    if cached and cached['path'] == str(p) and cached['size'] == stat.st_size and cached['mtime'] == stat.st_mtime_ns:
        return cached['idle']
    incremental = cached and cached['path'] == str(p) and cached['size'] < stat.st_size
    with p.open('rb') as f:
        start = cached['size'] if incremental else max(0, stat.st_size - 4 * 1024 * 1024)
        truncated = start > 0
        f.seek(start)
        lines = f.read().decode('utf-8', 'replace').splitlines()
        end = f.tell()
    prompts = [line for line in lines if 'acp_bridge_dispatch{method="session/prompt"' in line]
    idle = cached['idle'] if incremental else None
    complete = cached.get('complete', False) if incremental else not truncated
    if prompts:
        idle = bool(re.search(r'\}: (?:chisel_agent::acp_server::manager|chisel_agent::acp_server): close time\.', prompts[-1]))
    # A process which has only loaded a saved session has not started a turn.
    # Startup may finish in a later incremental read. Only the root load span
    # counts; a nested database read does not mean the Agent is ready.
    elif idle is None and complete and any(re.search(r':load_session: chisel_agent::acp_server::agent_impl: close time\.', line) for line in lines):
        idle = True
    _LOG_CACHE[pid] = {'path': str(p), 'size': end, 'mtime': stat.st_mtime_ns, 'idle': idle, 'complete': complete}
    return idle


def completed_head(db, sid, native_idle=False):
    row = db.execute('SELECT main_chain_id FROM sessions WHERE id=?', (sid,)).fetchone()
    node, seen = row[0] if row else None, set()
    while node is not None and node not in seen and len(seen) < 200:
        seen.add(node)
        row = db.execute('SELECT chat_message,parent_node_id FROM message_nodes WHERE session_id=? AND node_id=?', (sid, node)).fetchone()
        if not row:
            return False
        m = object_json(row[0])
        meta = m.get('metadata') or {}
        if m.get('role') == 'assistant':
            # Desktop-imported and compacted replies can omit finish_reason.
            # Accept those only after independent runtime evidence says idle
            # (or no native writer exists), never merely after a quiet interval.
            finished = meta.get('finish_reason') in ('stop', 'end_turn')
            imported = native_idle and meta.get('finish_reason') is None and bool(text_content(m.get('content')))
            return (finished or imported) and not m.get('tool_calls')
        if m.get('role') == 'tool' or (m.get('role') == 'user' and meta.get('is_user_input') is True):
            return False
        node = row[1]
    return False


def owner_ready(db, sid):
    pid = lock_owner(sid)
    if pid is None:
        # An interrupted saved turn is not silently continued either.
        if not completed_head(db, sid, native_idle=True):
            raise Refused(UNKNOWN)
        return None
    idle = log_idle(pid)
    if idle is not True:
        raise Refused(BUSY if idle is False else UNKNOWN)
    # Force open terminates the lock-holder process. Check every session it owns.
    for p in (DATA / 'session_locks').glob('*.lock'):
        try:
            if p.read_text().strip() == str(pid) and valid_id(p.stem) and lock_owner(p.stem) == pid and not completed_head(db, p.stem, native_idle=True):
                raise Refused(BUSY if p.stem == sid else 'Devin can only resume after every conversation in the same Agent process has finished. Another conversation is still running or waiting for input.')
        except FileNotFoundError:
            continue
    return pid


def startup_ready(db, sid, pane_pid, current):
    # The TUI can render its empty composer before the resumed Agent finishes
    # loading. Keep waiting for the same independent idle evidence used by sends.
    owner = lock_owner(sid)
    if not owner or not descendant(owner, pane_pid) or not composer_ready(current):
        return False
    try:
        return owner_ready(db, sid) == owner
    except Refused:
        return False


def executable(name):
    choices = [HOME / '.local/bin' / name, Path('/opt/homebrew/bin') / name, Path('/usr/local/bin') / name]
    found = shutil.which(name)
    if found:
        choices.insert(0, Path(found))
    for p in choices:
        if p.is_file() and os.access(p, os.X_OK):
            return str(p)
    raise Refused(name + ' is not installed on this machine.')


def tmux_name(title, sid):
    # tmux reserves dots and colons in session names. Keep Unicode, spaces and
    # parentheses; the stable native ID, rather than this label, proves identity.
    title = ''.join(c for c in (title or sid) if ord(c) >= 32 and ord(c) != 127).strip()
    return 'devin-' + re.sub(r'[:.]', '_', title or sid)[:140]


def tmux(*args, input=None, check=True):
    r = subprocess.run([executable('tmux'), *args], input=input, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=8)
    if check and r.returncode:
        raise Refused('Cannot access the Devin tmux session. Open Terminal to inspect it.')
    return r.stdout.decode('utf-8', 'replace') if check else r


def record_path(sid):
    return ROOT / (sid + '.json')


def read_record(sid):
    try:
        r = object_json(record_path(sid).read_text())
        return r if r.get('id') == sid else {}
    except (OSError, ValueError):
        return {}


def write_record(sid, record):
    p = record_path(sid)
    temp = p.with_suffix('.tmp')
    temp.write_text(json.dumps(record))
    temp.chmod(0o600)
    temp.replace(p)


def managed_pane(sid, record=None):
    r = record if record is not None else read_record(sid)
    pane, name = r.get('pane'), r.get('tmux')
    if not pane or not name:
        return None
    # A pane may also be linked into an Inyon terminal-view session. A bare
    # display-message target can choose that alias; check membership explicitly.
    result = tmux('list-panes', '-a', '-F', '#{session_name}\t#{pane_id}\t#{pane_pid}', check=False)
    rows = [line.split('\t') for line in result.stdout.decode().splitlines()]
    if result.returncode == 0 and [name, pane, str(r.get('panePid'))] in rows:
        return pane
    return None


def screen(pane):
    return tmux('capture-pane', '-p', '-t', pane)


def composer_ready(text):
    lines = text.splitlines()
    rules = [i for i, line in enumerate(lines) if line.startswith('──')]
    if len(rules) < 2:
        return False
    content = [line.strip() for line in lines[rules[-2] + 1:rules[-1]] if line.strip()]
    return len(content) == 1 and content[0].startswith('❭ Ask Devin to ') and not any(s in text for s in ('esc twice to interrupt', 'Starting session…', 'Force open', '↵ confirm'))


def descendant(child, parent):
    for _ in range(8):
        if child == parent:
            return True
        r = subprocess.run(['ps', '-p', str(child), '-o', 'ppid='], capture_output=True, text=True, timeout=3)
        try:
            child = int(r.stdout.strip())
        except ValueError:
            return False
        if child <= 1:
            return False
    return False


def owner_pane(pid):
    """Discover existing CLIs, including ones started manually or before a hub restart.

    A cwd, title or session name is not identity. The live native writer PID must
    descend from this exact pane's shell. Linked windows share a pane ID.
    """
    if not pid:
        return None
    result = tmux('list-panes', '-a', '-F', '#{session_name}\t#{pane_id}\t#{pane_pid}', check=False)
    if result.returncode:
        return None
    rows = [line.split('\t') for line in result.stdout.decode().splitlines()]
    rows = [row for row in rows if len(row) == 3 and row[2].isdigit()]
    tree = subprocess.run(['ps', '-eo', 'pid=,ppid='], capture_output=True, text=True, timeout=3).stdout
    parents = {int(parts[0]): int(parts[1]) for line in tree.splitlines() if len(parts := line.split()) == 2}
    ancestors = set()
    while pid > 1 and pid not in ancestors:
        ancestors.add(pid)
        pid = parents.get(pid, 0)
    found = [row for row in rows if int(row[2]) in ancestors]
    if len({row[1] for row in found}) != 1:
        return None
    found.sort(key=lambda row: row[0].startswith('inyon-view-'))
    name, pane, parent = found[0]
    return {'tmux': name, 'pane': pane, 'panePid': int(parent)}


def resolve_pane(pane):
    if not re.fullmatch(r'%[0-9]+', pane):
        raise Refused('Invalid Devin pane.')
    found = []
    with connect() as db:
        for path in (DATA / 'session_locks').glob('*.lock'):
            if not valid_id(path.stem):
                continue
            try:
                owner = lock_owner(path.stem)
                located = owner_pane(owner)
                if located and located['pane'] == pane:
                    found.append(session_info(db, path.stem)['id'])
            except Refused:
                continue
    if len(found) != 1:
        raise Refused('Choose the saved Devin conversation. This terminal does not have one unambiguous active conversation.')
    return {'session': found[0]}


def send_message(data):
    sid, text, rid = data.get('id'), data.get('text'), data.get('requestId')
    if not valid_id(sid) or not valid_id(rid) or not isinstance(text, str) or not text.strip():
        raise Refused('A session, message and request ID are required.')
    if len(text.encode()) > 512 * 1024 or any(ord(c) < 32 and c not in '\n\t' for c in text):
        raise Refused('Use plain text up to 512 KB.')
    if text.lstrip().startswith(('/', '!')):
        raise Refused('Use the terminal for Devin commands. This chat sends messages only.')
    ROOT.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (ROOT / 'dispatch.lock').open('a') as guard:
        try:
            fcntl.flock(guard, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise Refused('Another Devin message is being delivered. Try again when it finishes.')
        r = read_record(sid)
        digest = hashlib.sha256(text.encode()).hexdigest()
        receipts = r.setdefault('receipts', {})
        if rid in receipts:
            receipt = receipts[rid]
            if receipt.get('digest') != digest:
                raise Refused('This request ID was already used for another message.')
            if receipt.get('status') != 'sent':
                raise Refused('Delivery is uncertain. Check the Devin terminal before sending again.')
            return {**receipt, 'tmux': r.get('tmux'), 'pane': r.get('pane')}
        with connect() as db:
            info = session_info(db, sid)
            owner = owner_ready(db, sid)
            existing = owner_pane(owner)
            if existing:
                r.update(id=sid, **existing)
                write_record(sid, r)
            pane = managed_pane(sid, r)
            if existing and not pane:
                raise Refused('The Devin terminal changed during discovery. Retry after its terminal has settled.')
            retired_pane = pane if pane and not descendant(owner or 0, int(r['panePid'])) else None
            if retired_pane:
                pane = None
            if not pane:
                name = r['tmux'] if retired_pane else tmux_name(info.get('title'), sid)
                exists = tmux('has-session', '-t', '=' + name, check=False).returncode == 0
                if exists and not retired_pane:
                    name += ' [' + hashlib.sha256(sid.encode()).hexdigest()[:8] + ']'
                    exists = tmux('has-session', '-t', '=' + name, check=False).returncode == 0
                    if exists:
                        raise Refused('A tmux session with this name already exists. Inspect it before retrying.')
                args = [executable('devin'), '--resume', sid]
                # Resume restores the native session. Positional PATH arguments
                # open Desktop windows in this CLI; never use them for roots.
                shell = '/bin/zsh' if Path('/bin/zsh').exists() else '/bin/bash'
                command = shlex.join(args) + '; exec ' + shlex.quote(shell) + ' -l'
                target = ['new-window', '-t', '=' + name + ':'] if exists else ['new-session', '-s', name.replace('#', '##'), '-x', '140', '-y', '45']
                out = tmux(*target, '-d', '-P', '-F', '#{pane_id}\t#{pane_pid}', '-n', 'Devin', '-c', info['cwd'].replace('#', '##'), shlex.join([shell, '-lic', command]))
                pane, pid = out.strip().split('\t')
                r.update(id=sid, tmux=name, pane=pane, panePid=int(pid), receipts=receipts)
                write_record(sid, r)
                deadline = time.monotonic() + 40
                forced = False
                while time.monotonic() < deadline:
                    current = screen(pane)
                    if not forced and sid + ' is already in use' in current and '2 Force open' in current and re.search(r'^❭ 1 Cancel\s*$', current, re.M):
                        checked = owner_ready(db, sid)
                        if not checked or ('PID ' + str(checked) + ')') not in current:
                            raise Refused('Devin ownership changed. Inspect the terminal before retrying.')
                        tmux('send-keys', '-t', pane, 'Down', 'Enter')
                        forced = True
                    if startup_ready(db, sid, int(pid), current):
                        break
                    time.sleep(.25)
                else:
                    raise Refused('Devin is not ready. Open its tmux terminal to finish startup or sign-in.')
            current = screen(pane)
            owner_ready(db, sid)
            if not composer_ready(current):
                raise Refused('Devin can only receive a message at an empty, idle prompt. Finish the current terminal interaction first.')
            receipt = {'requestId': rid, 'digest': digest, 'status': 'sending', 'at': time.time()}
            baseline = db.execute('SELECT COALESCE(MAX(id),0) FROM prompt_history WHERE session_id=?', (sid,)).fetchone()[0]
            receipts[rid] = receipt
            r['receipts'] = dict(list(receipts.items())[-40:])
            write_record(sid, r)
            buffer = 'inyon-devin-' + rid
            try:
                tmux('load-buffer', '-b', buffer, '-', input=text.encode())
                tmux('paste-buffer', '-p', '-d', '-b', buffer, '-t', pane)
                time.sleep(.35)
                if composer_ready(screen(pane)):
                    raise Refused('Devin did not accept the paste. Inspect its terminal before retrying.')
                tmux('send-keys', '-t', pane, 'Enter')
                deadline = time.monotonic() + 12
                while time.monotonic() < deadline:
                    rows = db.execute('SELECT content FROM prompt_history WHERE session_id=? AND id>? ORDER BY id DESC LIMIT 8', (sid, baseline)).fetchall()
                    if any(row[0] == text for row in rows):
                        receipt['status'] = 'sent'
                        write_record(sid, r)
                        return {**receipt, 'tmux': r['tmux'], 'pane': pane}
                    time.sleep(.2)
                raise Refused('Devin has not confirmed delivery. Check its terminal; this request will not be sent twice.')
            finally:
                tmux('delete-buffer', '-b', buffer, check=False)


def state(db, sid):
    r = read_record(sid)
    owner = lock_owner(sid)
    existing = owner_pane(owner)
    pane = existing['pane'] if existing else None
    if existing:
        r.update(existing)
    try:
        owner_ready(db, sid)
        status, reason = 'idle', None
    except Refused as e:
        status, reason = 'busy', str(e)
    preview = None
    if pane:
        current = screen(pane)
        if not composer_ready(current):
            status = 'busy'
            reason = BUSY
            # A bounded, explicitly labelled terminal preview; saved history remains canonical.
            preview = current[-18000:]
    return {'status': status, 'reason': reason, 'source': 'cli' if pane else 'app', 'tmux': r.get('tmux') if pane else None, 'pane': pane, 'live': preview}


def emit(value):
    print(json.dumps(value, ensure_ascii=False, separators=(',', ':')), flush=True)


def watch(sid):
    previous, previous_state, version = {}, None, None
    with connect() as db:
        info = None
        while True:
            current = db.execute('PRAGMA data_version').fetchone()[0]
            if current != version:
                next_info = session_info(db, sid)
                if info is None or next_info != info:
                    info, messages = history(db, sid)
                    emit({'session': info, 'order': list(messages), 'messages': [m for k, m in messages.items() if previous.get(k) != m]})
                    previous = messages
                version = current
            current_state = state(db, sid)
            if current_state != previous_state:
                emit({'state': current_state})
                previous_state = current_state
            time.sleep(.6)


def main(args):
    if args[0] == 'list':
        if not (DATA / 'sessions.db').exists():
            emit({'sessions': []})
            return
        with connect() as db:
            emit({'sessions': [dict(zip(('id', 'title', 'cwd', 'model', 'updated'), row)) for row in db.execute('SELECT id,title,working_directory,model,last_activity_at FROM sessions WHERE hidden=0 ORDER BY last_activity_at DESC LIMIT 200')]})
    elif args[0] == 'send':
        emit(send_message(json.load(sys.stdin)))
    elif args[0] == 'resolve':
        emit(resolve_pane(args[1]))
    elif args[0] == 'watch' and valid_id(args[1]):
        watch(args[1])
    else:
        raise Refused('Invalid Devin request.')


if __name__ == '__main__':
    try:
        main(sys.argv[1:])
    except (Refused, OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as e:
        emit({'error': str(e) if isinstance(e, Refused) else 'Could not access Devin on this machine. Check its terminal and connection.'})
        sys.exit(1)

"""Replace inherited launcher names on the first prompt in QYY and LYR.
Native automatic titles and deliberate user names remain under Claude's control.
"""
import json, os, re, sys, time
from pathlib import Path


def read_event(stream):
    """Read one complete hook object without waiting for its pipe to close."""
    data = bytearray()
    while len(data) < 2 * 1024 * 1024:
        chunk = stream.read1(min(65536, 2 * 1024 * 1024 - len(data)))
        if not chunk:
            return None
        data.extend(chunk)
        try:
            value, _ = json.JSONDecoder().raw_decode(data.decode('utf-8').lstrip())
            return value if isinstance(value, dict) else None
        except (ValueError, UnicodeDecodeError):
            pass
    return None


def plain_prompt(text):
    text = re.sub(r'<(?:system-reminder|local-command-caveat)>[\s\S]*?</(?:system-reminder|local-command-caveat)>', '', text)
    return re.sub(r'\s+', ' ', re.sub(r'<[^>]+>', '', text.split('Attached files:')[0])).strip()


def content_title(prompt):
    text = plain_prompt(prompt)
    text = re.sub(r'^你好[，,!！。\s]*', '', text)
    text = re.sub(r'^(?:请帮我|帮我|请|告诉我|介绍一下|解释一下)\s*', '', text)
    files = re.findall(r'(?m)^\s*(?:[A-Za-z]:[/\\]|/)[^\n]+', prompt.split('Attached files:', 1)[-1]) if 'Attached files:' in prompt else []
    if files:
        name = re.split(r'[/\\]', files[0].strip())[-1]
        name = re.sub(r'^\d{6}-[0-9a-f]{6}-', '', name)
        if re.search(r'[A-Za-z\u4e00-\u9fff]', name.rsplit('.', 1)[0]):
            text = text + ' · ' + name
    text = text.strip('，。！？,.!? ')
    return text[:44] + ('…' if len(text) > 44 else '')


def real_user_text(record):
    if record.get('type') != 'user' or record.get('isMeta') or record.get('isSidechain'):
        return None
    content = record.get('message', {}).get('content', '')
    text = content if isinstance(content, str) else ' '.join(c.get('text', '') for c in content if c.get('type') == 'text')
    if re.search(r'<(?:command-name|local-command-stdout|local-command-stderr|task-notification)>|^Caveat:', text):
        return None
    return plain_prompt(text) or None


def choose_title(project, session, prompt, custom, prior_users, state, large=False):
    if not prompt.strip() or prompt.lstrip().startswith('/'):
        return None
    managed = set(state.get('managed_titles', {}).get(project, [])) | {project}
    # A genuine unnamed session already gets Claude's native AI-generated title.
    if custom not in managed:
        return None
    previous = state.get('sessions', {}).get(session, {})
    if previous.get('project') == project and previous.get('title'):
        # A timeout may have discarded the prior hook's output after it saved its
        # state. Retry that same title while the old managed title is still there.
        return None if custom == previous['title'] else previous['title']
    clean = plain_prompt(prompt)
    first = not large and (not prior_users or (len(prior_users) == 1 and prior_users[0] == clean))
    if not first and custom != project:
        return None
    title = content_title(prior_users[0] if custom == project and prior_users else prompt)
    return title or None


def main():
    event = read_event(sys.stdin.buffer)
    if not event or event.get('hook_event_name') != 'UserPromptSubmit': return
    home = Path.home()
    cwd = Path(event.get('cwd', '')).resolve()
    project = next((name for name, root in [('QYY', home/'QYY'), ('LYR', home/'LYR'), ('input-verification', home/'.inyon/input-verification')]
                    if cwd == root.resolve() or root.resolve() in cwd.parents), None)
    if project is None: return
    session = event.get('session_id', '')
    if not re.fullmatch(r'[0-9a-fA-F-]{36}', session): return
    storage = home/'.inyon'; storage.mkdir(exist_ok=True)
    state_path = storage/'auto-title-state.json'
    state = json.loads(state_path.read_text(encoding='utf-8')) if state_path.exists() else {}
    path = Path(event.get('transcript_path', ''))
    if path.stem != session: return
    users, custom = [], None
    size = path.stat().st_size if path.exists() else 0
    if size:
        with path.open('rb') as f:
            # Most submissions belong to a session already named. Its latest
            # title near the end is enough; no prompt/history scan is needed.
            if session in state.get('sessions', {}):
                f.seek(max(0, size-65536))
                for line in f.read(65536).decode('utf-8', errors='replace').splitlines():
                    try: r = json.loads(line)
                    except ValueError: continue
                    if r.get('type') == 'custom-title': custom = r.get('customTitle')
                previous = state['sessions'][session].get('title')
                managed = set(state.get('managed_titles', {}).get(project, [])) | {project}
                if custom and (custom == previous or custom not in managed): return
                f.seek(0)
            head = f.read(262144)
            f.seek(max(0, size-262144)); tail = f.read(262144)
        for line in head.decode('utf-8', errors='replace').splitlines():
            try: r = json.loads(line)
            except ValueError: continue
            user = real_user_text(r)
            if user: users.append(user)
        for line in (head+b'\n'+tail).decode('utf-8', errors='replace').splitlines():
            try: r = json.loads(line)
            except ValueError: continue
            if r.get('type') == 'custom-title': custom = r.get('customTitle')
    lock = open(storage/'auto-title-state.lock', 'a+b')
    lock.seek(0)
    if not lock.read(1): lock.write(b'0'); lock.flush()
    lock.seek(0)
    import msvcrt
    try: msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
    except OSError: lock.close(); return
    try:
        state = json.loads(state_path.read_text(encoding='utf-8')) if state_path.exists() else {}
        title = choose_title(project, session, event.get('prompt', ''), custom, users, state, size > 262144)
        if title is None: return
        state.setdefault('sessions', {})[session] = {'project': project, 'title': title}
        names = state.setdefault('managed_titles', {}).setdefault(project, [])
        if title not in names: names.append(title)
        tmp = state_path.with_suffix('.tmp')
        tmp.write_text(json.dumps(state, ensure_ascii=False, indent=2)+'\n', encoding='utf-8');tmp.replace(state_path)
        print(json.dumps({'hookSpecificOutput': {'hookEventName': 'UserPromptSubmit', 'sessionTitle': title}}, ensure_ascii=False))
    finally: lock.close()


if __name__ == '__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    try: main()
    except Exception:
        # A naming failure must never block sending or alter the user's prompt.
        pass

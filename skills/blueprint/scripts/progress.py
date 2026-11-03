#!/usr/bin/env python3
"""Update a Blueprint site's progress.json, which the plan page polls every 20 seconds.

  progress.py [--file site/progress.json] task <id> <todo|doing|done|blocked> [note]
  progress.py [--file ...] add <release-r> <id> "<title>"     add a task to a release (r is "R1" etc.)
  progress.py [--file ...] link <id> <url>                     attach a PR or link to a task
  progress.py [--file ...] log "<text>"                        add a line to the activity log
  progress.py [--file ...] now "<text>"                        set the "working on" line

--file defaults to $BLUEPRINT_PROGRESS, then ./progress.json. Several workers can run it at once: each
takes progress.json.lock, writes progress.json.tmp and renames it over progress.json, so a reader (the
page, or Inyon) never sees half a file. A lock older than 10 seconds was left by a crash and is taken over.
"""
import datetime
import json
import os
import random
import sys
import time

LOCK_STALE_S = 10
LOCK_WAIT_S = 30


def lock(path):
    """Create <path>.lock exclusively, waiting while another writer holds it."""
    name = path + '.lock'
    until = time.monotonic() + LOCK_WAIT_S
    while True:
        try:
            os.close(os.open(name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644))
            return name
        except FileExistsError:
            try:
                if time.time() - os.stat(name).st_mtime > LOCK_STALE_S:
                    os.unlink(name)
                    continue
            except FileNotFoundError:
                continue
            if time.monotonic() > until:
                sys.exit(f'{name} is held by another writer; try again, or delete it if nothing is running')
            time.sleep(0.02 + random.random() * 0.04)


def write(path, d):
    """Write d to <path>.tmp, flush it to disk, then rename it over path."""
    tmp = path + '.tmp'
    try:
        mode = os.stat(path).st_mode & 0o777
    except FileNotFoundError:
        mode = 0o644
    with open(os.open(tmp, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, mode), 'w', encoding='utf-8') as f:
        json.dump(d, f, indent=1, ensure_ascii=False)
        f.flush()
        os.fsync(f.fileno())
    os.chmod(tmp, mode)
    os.replace(tmp, path)


def rollup(r):
    st = [x['status'] for x in r['tasks']]
    r['status'] = 'done' if st and all(s == 'done' for s in st) else 'blocked' if 'blocked' in st else 'doing' if any(s in ('doing', 'done') for s in st) else 'todo'


def find(d, tid):
    for r in d['releases']:
        for t in r['tasks']:
            if t['id'] == tid:
                return r, t
    sys.exit(f'no task {tid}')


def apply(d, args, now):
    cmd, a = args[0], args[1:]
    if cmd == 'task':
        if len(a) < 2 or a[1] not in ('todo', 'doing', 'done', 'blocked'):
            sys.exit(__doc__)
        r, t = find(d, a[0])
        t['status'] = a[1]
        if len(a) > 2:
            t['note'] = a[2]
        t['updated'] = now
        rollup(r)
        d['log'].insert(0, {'t': now, 'text': f"{r['v']} · {t['title']}: {a[1]}" + (f" ({a[2]})" if len(a) > 2 else '')})
    elif cmd == 'add' and len(a) >= 3:
        r = next((x for x in d['releases'] if x['r'] == a[0]), None) or sys.exit(f'no release {a[0]}')
        r['tasks'].append({'id': a[1], 'title': a[2], 'status': 'todo'})
        rollup(r)
    elif cmd in ('link', 'pr') and len(a) >= 2:
        find(d, a[0])[1]['link'] = a[1]
    elif cmd == 'log' and len(a) >= 1:
        d['log'].insert(0, {'t': now, 'text': a[0]})
    elif cmd == 'now' and len(a) >= 1:
        d['now'] = a[0]
    else:
        sys.exit(__doc__)
    d['log'] = d['log'][:200]
    d['updated'] = now


def main():
    args = sys.argv[1:]
    path = os.environ.get('BLUEPRINT_PROGRESS', 'progress.json')
    if args[:1] == ['--file']:
        path, args = args[1], args[2:]
    if not args:
        sys.exit(__doc__)
    now = datetime.datetime.now().astimezone().isoformat(timespec='minutes')
    held = lock(path)
    try:
        with open(path, encoding='utf-8') as f:
            d = json.load(f)
        apply(d, args, now)
        write(path, d)
    finally:
        try:
            os.unlink(held)
        except FileNotFoundError:
            pass


if __name__ == '__main__':
    main()

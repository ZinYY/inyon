"""Index Claude's current parent-UUID chain without copying conversation content."""
import json
import sys


def index(lines, before=None, since=0, windows_psmux=False):
    parents = {}
    tool_gaps = {}
    pending_tool_result = False
    leaf = None
    size = 0
    new_turn = False
    for raw in lines:
        offset = size
        size += len(raw)
        try:
            row = json.loads(raw)
        except (ValueError, UnicodeError):
            continue
        uid = row.get('uuid')
        if not isinstance(uid, str) or 'parentUuid' not in row or row.get('isSidechain'):
            continue
        content = row.get('message', {}).get('content')
        if before and offset >= since and row.get('type') == 'user' and not row.get('isMeta'):
            if isinstance(content, str) and not content.lstrip().startswith('<command-'):
                new_turn = True
            elif isinstance(content, list) and content and all(b.get('type') != 'tool_result' for b in content):
                new_turn = True
        parent = row.get('parentUuid')
        # Windows Claude 2.1.317 can reference an unpersisted runtime attachment
        # between a tool result and the assistant's continuation. A missing UUID
        # is not evidence of a rewind. Bridge only that tool continuation, never
        # an arbitrary human turn, an explicit root, or a parent present on disk.
        if windows_psmux and row.get('type') == 'assistant' and pending_tool_result and isinstance(parent, str) and leaf:
            tool_gaps[uid] = (parent, leaf)
        if row.get('type') == 'user':
            pending_tool_result = isinstance(content, list) and any(b.get('type') == 'tool_result' for b in content if isinstance(b, dict))
        elif row.get('type') == 'assistant':
            pending_tool_result = False
        # Compaction starts a new context, but its earlier conversation is still history.
        if row.get('subtype') == 'compact_boundary' and parent is None:
            parent = row.get('logicalParentUuid') or leaf
        if uid not in parents:
            parents[uid] = parent if isinstance(parent, str) else None
        leaf = uid
    for uid, (parent, previous) in tool_gaps.items():
        if parent not in parents:
            parents[uid] = previous
    if before in parents and not new_turn:
        leaf = parents[before]
    active = set()
    at = leaf
    while at in parents and at not in active:
        active.add(at)
        at = parents[at]
    return {'known': list(active), 'discarded': list(set(parents) - active), 'leaf': leaf, 'size': size,
            'bridged': any(parent not in parents for parent, _ in tool_gaps.values())}


if __name__ == '__main__':
    windows_psmux = '--windows-psmux' in sys.argv
    sys.argv = [arg for arg in sys.argv if arg != '--windows-psmux']
    with open(sys.argv[1], 'rb') as stream:
        before = sys.argv[2] if len(sys.argv) > 2 else None
        since = int(sys.argv[3]) if len(sys.argv) > 3 else 0
        print(json.dumps(index(stream, before, since, windows_psmux), separators=(',', ':')))

"""Index Claude's current parent-UUID chain without copying conversation content."""
import json
import re
import sys


def prompt_text(value):
    if not isinstance(value, str):
        return ''
    # Claude's paste wrapper is transport markup; the body is the exact input.
    return re.sub(r'</?pasted_content\b[^>]*>', '', value).strip()


def prompt_key(value):
    return ' '.join(prompt_text(value).split())


def index(lines, before=None, since=0, windows_psmux=False):
    parents = {}
    tool_gaps = {}
    pending_tool_result = False
    leaf = None
    size = 0
    new_turn = False
    kinds, times, human, last_prompts, prompt_bodies = {}, {}, set(), [], {}
    for raw in lines:
        offset = size
        size += len(raw)
        try:
            row = json.loads(raw)
        except (ValueError, UnicodeError):
            continue
        if windows_psmux:
            if row.get('type') == 'last-prompt' and isinstance(row.get('lastPrompt'), str):
                prompt = row['lastPrompt']
                last_prompts.append((row.get('leafUuid'), prompt, prompt_bodies.get(prompt_key(prompt), prompt)))
            # Some Windows releases omit the user row entirely, but preserve the
            # exact pasted text in an API snapshot and last-prompt's leaf link.
            # Retain text only, never document/image payloads or system context.
            if row.get('type') == 'api-request-blob' and row.get('message', {}).get('role') == 'user':
                blocks = row['message'].get('content')
                for block in blocks if isinstance(blocks, list) else []:
                    text = block.get('text') if isinstance(block, dict) and block.get('type') == 'text' else None
                    if isinstance(text, str) and len(text) <= 200000 and not text.lstrip().startswith(('<system-', '<local-command-', '<command-')):
                        prompt_bodies[prompt_key(text)] = prompt_text(text)
        uid = row.get('uuid')
        if not isinstance(uid, str) or 'parentUuid' not in row or row.get('isSidechain'):
            continue
        content = row.get('message', {}).get('content')
        kinds[uid] = row.get('type')
        times[uid] = row.get('timestamp', '')
        if row.get('type') == 'user' and not row.get('isMeta'):
            if isinstance(content, str) and not content.lstrip().startswith(('<command-', '<local-command-')):
                human.add(uid)
            elif isinstance(content, list) and content and all(isinstance(b, dict) and b.get('type') != 'tool_result' for b in content):
                human.add(uid)
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
            # The logical parent can be an attachment that Claude never persisted.
            # Its preserved segment names the last actual message on the active
            # branch, including when compaction follows a rewind.
            metadata = row.get('compactMetadata') or {}
            segment = metadata.get('preservedSegment') or {}
            preserved = metadata.get('preservedMessages') or {}
            candidates = [row.get('logicalParentUuid'), segment.get('tailUuid'),
                          *reversed(preserved.get('allUuids') or []), leaf]
            parent = next((p for p in candidates if isinstance(p, str) and p in parents), None)
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
    prefixes, ambiguous = {}, set()
    if windows_psmux:
        for tip, prompt, text in last_prompts:
            if not isinstance(tip, str):
                continue
            at, seen, first_assistant = tip, set(), None
            while at in parents and at not in seen and at not in human:
                seen.add(at)
                if kinds.get(at) == 'assistant':
                    first_assistant = at
                at = parents[at]
            # A linked turn with an absent parent, not an explicit root or a
            # rewind. Never create a second prompt for a persisted human turn.
            if at in human or not isinstance(at, str) or at in parents or first_assistant not in active or not prompt.strip():
                continue
            recovered = {'type': 'user', 'uuid': at, 'timestamp': times.get(first_assistant, ''),
                         'message': {'role': 'user', 'content': text}}
            if first_assistant in prefixes and prompt_key(prefixes[first_assistant]['message']['content']) != prompt_key(text):
                ambiguous.add(first_assistant)
            prefixes[first_assistant] = recovered
        for uid in ambiguous:
            prefixes.pop(uid, None)
    return {'known': list(active), 'discarded': list(set(parents) - active), 'leaf': leaf, 'size': size,
            'bridged': any(parent not in parents for parent, _ in tool_gaps.values()), 'prefixes': prefixes, 'repair_enabled': windows_psmux,
            'repair_pending': windows_psmux and any(kinds.get(uid) == 'assistant' and isinstance(parents[uid], str)
                and parents[uid] not in parents and uid not in prefixes for uid in active)}


if __name__ == '__main__':
    windows_psmux = '--windows-psmux' in sys.argv
    sys.argv = [arg for arg in sys.argv if arg != '--windows-psmux']
    with open(sys.argv[1], 'rb') as stream:
        before = sys.argv[2] if len(sys.argv) > 2 else None
        since = int(sys.argv[3]) if len(sys.argv) > 3 else 0
        print(json.dumps(index(stream, before, since, windows_psmux), separators=(',', ':')))

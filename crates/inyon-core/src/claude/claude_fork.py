"""INYON_CLAUDE_SDK_FORK: call Anthropic's public API; never rewrite transcripts."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

SDK_VERSION = '0.3.318'
SDK_JS = r'''
const {createRequire} = require('node:module');
const {pathToFileURL} = require('node:url');
const fs = require('node:fs');
const path = require('node:path');
(async () => {
  const input = JSON.parse(process.argv[1]);
  const root = process.argv[2];
  const pkg = path.join(root, 'node_modules', '@anthropic-ai', 'claude-agent-sdk');
  const version = JSON.parse(fs.readFileSync(path.join(pkg, 'package.json'), 'utf8')).version;
  if (version !== process.argv[3]) throw new Error('Install the pinned Claude Agent SDK for this Inyon version');
  const req = createRequire(path.join(root, 'package.json'));
  const {forkSession} = await import(pathToFileURL(req.resolve('@anthropic-ai/claude-agent-sdk')).href);
  const options = {dir: input.dir, upToMessageId: input.upToMessageId};
  if (input.title) options.title = input.title;
  const result = await forkSession(input.sessionId, options);
  console.log(JSON.stringify({sessionId: result.sessionId}));
})().catch(error => { console.error(error.message); process.exitCode = 1; });
'''


def node_binary():
    home = Path.home()
    candidates = [shutil.which('node'), '/opt/homebrew/bin/node', '/usr/local/bin/node']
    candidates += [str(p) for p in sorted((home / '.nvm/versions/node').glob('*/bin/node'), reverse=True)]
    candidates += [str(p) for p in sorted((home / '.inyon/runtimes').glob('node-*/node.exe'), reverse=True)]
    candidates += [str(Path(os.environ.get('ProgramFiles', 'C:/Program Files')) / 'nodejs/node.exe')]
    return next((p for p in candidates if p and Path(p).is_file()), None)


def main():
    node = node_binary()
    root = Path.home() / '.inyon/claude-sdk'
    package = root / 'node_modules/@anthropic-ai/claude-agent-sdk/package.json'
    if not node or not package.is_file():
        raise ValueError('Rewound forks need Node.js and the official Claude Agent SDK on this machine. Run scripts/install-claude-sdk.py from the Inyon repository.')
    data = json.loads(sys.argv[1])
    transcript = Path(data['transcript'])
    if transcript.stem != data['sessionId'] or transcript.parent.parent.name != 'projects':
        raise ValueError('The current Claude transcript does not match its session ID')
    # Use the config home that owns the verified source record, including custom
    # CLAUDE_CONFIG_DIR installations. No API credentials are needed for this API.
    env = os.environ.copy()
    env['CLAUDE_CONFIG_DIR'] = str(transcript.parent.parent.parent)
    result = subprocess.run([node, '-e', SDK_JS, json.dumps(data), str(root), SDK_VERSION],
                            env=env, capture_output=True, text=True, encoding='utf-8', timeout=30)
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    return result.returncode


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)

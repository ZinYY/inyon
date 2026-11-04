"""Install the pinned official SDK privately for forks after a Claude rewind.

Requires Node.js >= 18.14 and npm. The SDK's optional CLI binaries are not needed:
forkSession is a local session-management API, not an agent/model invocation.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'crates/inyon-core/src/claude'))
from claude_fork import node_binary, SDK_VERSION

node = node_binary()
if not node:
    sys.exit('Install Node.js >= 18.14 with npm, then run this script again.')
node_dir = Path(node).resolve().parent
cli = node_dir / 'node_modules/npm/bin/npm-cli.js'
npm = shutil.which('npm') or shutil.which('npm.cmd')
if not cli.exists() and npm:
    resolved = Path(npm).resolve()
    if resolved.suffix == '.js':
        cli = resolved
    else:
        cli = resolved.parent / 'node_modules/npm/bin/npm-cli.js'
if not cli.exists():
    # nvm and Homebrew keep npm beside node as a symlink to its JS entry point.
    linked = node_dir / 'npm'
    if linked.exists():
        cli = linked.resolve()
if not cli.exists():
    sys.exit('npm was not found beside Node.js; install npm and retry.')
env = os.environ.copy()
env['PATH'] = str(node_dir) + os.pathsep + env.get('PATH', '')
subprocess.run([node, str(cli), 'install', '--prefix', str(Path.home() / '.inyon/claude-sdk'),
                '--save-exact', '--ignore-scripts', '--omit=optional', '--no-audit', '--no-fund',
                '--registry=https://registry.npmjs.org', '@anthropic-ai/claude-agent-sdk@' + SDK_VERSION],
               env=env, check=True)

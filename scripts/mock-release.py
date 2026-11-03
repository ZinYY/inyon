#!/usr/bin/env python3
"""A stand-in for the releases, for the container tests.

Serves a folder both ways the installers and updaters read a release. As the website does: each
file under /download/<name>, with /download/latest.json naming the version and listing the files,
which is where `inyon update` looks for the newest version by default. As GitHub does: each file
under /releases/latest/download/<name>, and /releases/latest answering with the redirect to
/releases/tag/v<version>, which installs from before the website read the version from. get.sh is
served at the root too, as the website does. The folder carries SHA256SUMS.minisig next to
SHA256SUMS (container-tests.sh signs it with apps/cli/tests/keys/test.key), since from 0.14 the
installers and `inyon update` check the signature before trusting the list.

    mock-release.py --dir <folder> --version 0.13.0 --port 8000
"""
import argparse
import hashlib
import http.server
import json
import os
import sys


class Release(http.server.SimpleHTTPRequestHandler):
    version = ""

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.rstrip("/") == "/releases/latest":
            self.send_response(302)
            self.send_header("Location", f"http://{self.headers.get('Host', 'release')}/releases/tag/v{self.version}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if path == "/download/latest.json":
            body = json.dumps(self.listing()).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        for prefix in ("/download/", "/releases/latest/download/"):
            if path.startswith(prefix):
                self.path = "/" + path[len(prefix):]
                break
        return super().do_GET()

    def listing(self):
        """latest.json as the website's build writes it."""
        files = []
        for name in sorted(os.listdir(self.directory)):
            full = os.path.join(self.directory, name)
            if os.path.isfile(full):
                with open(full, "rb") as f:
                    files.append({"name": name, "size": os.path.getsize(full), "sha256": hashlib.sha256(f.read()).hexdigest()})
        return {"version": self.version, "tag": f"v{self.version}", "date": "2026-10-13", "files": files}

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--dir", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--port", type=int, default=8000)
    a = p.parse_args()
    Release.version = a.version
    handler = lambda *args, **kw: Release(*args, directory=os.path.abspath(a.dir), **kw)
    http.server.ThreadingHTTPServer(("0.0.0.0", a.port), handler).serve_forever()


if __name__ == "__main__":
    main()

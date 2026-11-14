#!/usr/bin/env python3
"""Exercise failed and successful self-replacement using a local release server."""
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading

source = Path(sys.argv[1]).resolve()
data = source.read_bytes()
digest = hashlib.sha256(data).hexdigest()
valid_checksum = False
include_checksum = False

class ReleaseServer(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path == '/repos/rusttalkcrew/RustTalk/releases?per_page=100&page=1':
            base = f'http://127.0.0.1:{self.server.server_port}'
            assets = [
                {'name': source.name, 'browser_download_url': base + '/image'}
            ]
            if include_checksum:
                assets.append({'name': source.name + '.sha256', 'browser_download_url': base + '/sha'})
            body = json.dumps([{'tag_name': 'v999.0.0+client.abcdef1', 'draft': False,
                                'prerelease': False, 'body': None, 'html_url': base, 'assets': assets}]).encode()
        elif self.path == '/image':
            body = data
        elif self.path == '/sha':
            body = ((digest if valid_checksum else '0' * 64) + '  ' + source.name).encode()
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), ReleaseServer)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='rusttalk-update-') as directory:
        target = Path(directory) / 'RustTalk test.AppImage'
        shutil.copy2(source, target)
        env = {**os.environ, 'APPIMAGE_EXTRACT_AND_RUN': '1',
               'RUSTTALK_UPDATE_API': f'http://127.0.0.1:{server.server_port}'}
        before = target.stat().st_ino
        missing = subprocess.run([str(target), '--update'], env=env, capture_output=True, timeout=90)
        assert missing.returncode != 0, 'Missing checksum accepted'
        assert b'no checksum file' in missing.stdout + missing.stderr
        assert target.stat().st_ino == before, 'Missing checksum replaced original'
        include_checksum = True
        failed = subprocess.run([str(target), '--update'], env=env, capture_output=True, timeout=90)
        assert failed.returncode != 0, 'Bad checksum accepted'
        assert target.stat().st_ino == before, 'Failed update replaced original'
        assert hashlib.sha256(target.read_bytes()).hexdigest() == digest
        valid_checksum = True
        installed = subprocess.run([str(target), '--update'], env=env, capture_output=True, timeout=90)
        assert installed.returncode == 0, (installed.stdout + installed.stderr).decode(errors='replace')
        assert target.stat().st_ino != before, 'Update did not atomically replace original'
        assert hashlib.sha256(target.read_bytes()).hexdigest() == digest
        restarted = subprocess.run([str(target), '--version'], env=env, capture_output=True, timeout=90)
        assert restarted.returncode == 0, restarted.stderr.decode(errors='replace')
        assert b'RustTalk ' in restarted.stdout
        print('AppImage: missing/bad checksums rejected; atomic replacement and relaunch passed')
finally:
    server.shutdown()
    server.server_close()

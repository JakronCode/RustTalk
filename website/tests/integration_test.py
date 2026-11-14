"""Offline preservation check against the newer main HTML, via git show only."""
import difflib
import os
from pathlib import Path
import re
import subprocess
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest
from html.parser import HTMLParser
from integrate import transform

SITE = Path(__file__).resolve().parents[1]


class Tags(HTMLParser):
    def __init__(self, html):
        super().__init__()
        self.tags = []
        self.feed(html)

    def handle_starttag(self, name, attrs):
        self.tags.append((name, dict(attrs)))


class Integration(unittest.TestCase):
    def test_newer_main_preserved(self):
        before = subprocess.check_output(['git', 'show', 'origin/main:website/index.html'], cwd=SITE, text=True)
        after = transform(before)
        self.assertEqual(transform(after), after)
        if '<script id="component-releases">' in before:
            self.assertEqual(after, before)
            return
        old_tags, new_tags = Tags(before).tags, Tags(after).tags
        self.assertEqual(len(old_tags) + 1, len(new_tags))  # one inline loader
        for (old_name, old), (new_name, new) in zip(old_tags[:-2], new_tags[:-3]):
            self.assertEqual(old_name, new_name)
            expected = dict(old)
            href = expected.get('href', '')
            if '/releases/latest/download/' in href:
                asset = href.rsplit('/', 1)[1]
                expected.update(href='https://github.com/rusttalkcrew/RustTalk/releases',
                                **{'data-asset': asset, 'data-component': 'server' if asset.startswith('rusttalk-server-') else 'client'})
            self.assertEqual(expected, new)
        self.assertEqual(len(re.findall(r'<details class="files"', before)), 3)
        self.assertEqual(len(re.findall(r'<details class="files"', after)), 3)
        self.assertIn('code.title=\'Click to copy SHA-256\'', before)
        self.assertIn("code.title = 'Click to copy SHA-256'", after)
        self.assertNotIn('/releases/latest', after)
        self.assertNotIn('wget https://github.com', after)
        self.assertIn('# Choose a stable server or combined release', after)
        self.assertIn('navigator.clipboard.writeText(text)', after)
        old_os = before[before.index('  var ua='):before.index('  function fmt(n)')]
        new_os = after[after.index('  var ua='):after.index('\n})();', after.index('  var ua='))]
        self.assertEqual(old_os.replace("a.href=a.href.replace('x86_64','aarch64')",
                                       "a.dataset.asset=a.dataset.asset.replace('x86_64','aarch64')"), new_os)
        with tempfile.TemporaryDirectory(prefix='rusttalk-website-') as temp:
            output = Path(temp) / 'index.html'
            output.write_text(after)
            for script in re.findall(r'<script[^>]*>([\s\S]*?)</script>', after):
                subprocess.run(['node', '--check'], input=script, text=True, check=True, capture_output=True)
            subprocess.run(['node', '--test', str(SITE / 'tests/releases.test.cjs')],
                           env={**os.environ, 'RUSTTALK_TEST_HTML': str(output)}, check=True)
            # Execute the preserved chooser for each OS, including ARM Linux asset selection.
            chooser_test = '''const vm=require('node:vm'); const assert=require('node:assert/strict');
const script=JSON.parse(process.argv[1]);
for(const [platform,ua,os,asset] of [['Win32','Windows','windows','x86_64'],['Linux aarch64','Linux','linux','aarch64'],['Linux x86_64','Linux','linux','x86_64'],['MacIntel','Macintosh','mac','x86_64'],['Linux','Android','other','x86_64']]){
 const a={dataset:{asset:'rusttalk-linux-x86_64.AppImage'}},root={dataset:{}};
 vm.runInNewContext(script,{navigator:{platform,userAgent:ua},root,document:{querySelectorAll:()=>[a]}});
 assert.equal(root.dataset.os,os);assert.ok(a.dataset.asset.includes(asset));
}'''
            import json
            subprocess.run(['node', '-e', chooser_test, json.dumps(new_os)], check=True)
        patch = ''.join(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                           fromfile='a/website/index.html', tofile='b/website/index.html'))
        Path('/tmp/rusttalk-main-component-releases.patch').write_text(patch)


if __name__ == '__main__':
    unittest.main()

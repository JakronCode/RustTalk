"""Focused adapter for the older bound checkout and newer origin/main HTML.

Run with input/output paths to adapt a synthetic fixture or resolve an integration
conflict. This never imports or replaces unrelated website UI.
"""
import re
import sys
from pathlib import Path


def transform(html):
    if '<script id="component-releases">' in html:
        return html
    prefix = 'https://github.com/rusttalkcrew/RustTalk/releases'
    html = re.sub(r'href="' + re.escape(prefix) + r'/latest/download/([A-Za-z0-9._-]+)"',
                  lambda m: 'href="' + prefix + '" data-component="' +
                  ('server' if m[1].startswith('rusttalk-server-') else 'client') +
                  '" data-asset="' + m[1] + '"', html)
    html = html.replace('wget ' + prefix + '/latest/download/rusttalk-server-linux-x86_64.tar.gz',
                        '# Download rusttalk-server-linux-x86_64.tar.gz from\n# ' + prefix + '\n# Choose a stable server or combined release, then unpack:')
    # Keep the newer main OS chooser; its architecture change now targets asset metadata.
    html = html.replace("a.href=a.href.replace('x86_64','aarch64')",
                        "a.dataset.asset=a.dataset.asset.replace('x86_64','aarch64')")
    start = html.find('  function fmt(n)')
    if start < 0:
        start = html.find("  fetch('https://api.github.com/repos/rusttalkcrew/RustTalk/releases/latest')")
    if start < 0:
        raise ValueError('Expected release loader not found; inspect integration manually')
    end = html.index('\n})();', start)
    html = html[:start] + html[end:]
    source = (Path(__file__).resolve().parents[1] / 'index.html').read_text()
    loader = re.search(r'<script id="component-releases">[\s\S]*?</script>', source)[0]
    html = html.replace('</body>', loader + '\n</body>')
    if '/releases/latest' in html:
        raise ValueError('Unconverted latest link')
    return html


if __name__ == '__main__':
    Path(sys.argv[2]).write_text(transform(Path(sys.argv[1]).read_text()))

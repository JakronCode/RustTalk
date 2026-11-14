#!/usr/bin/env python3
"""Fail-closed release validation and publication. No third-party dependencies."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request

LIMIT = 2 * 1024 * 1024
PAGES = 10
CORE = r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
TAG = re.compile(r'v' + CORE + r'(?:\+(client|server|index-server)\.([0-9a-f]{7,40}))?\Z')
IMAGE = 'ghcr.io/rusttalkcrew/rusttalk-server'


class ReleaseError(ValueError):
    """A safe, operator-facing refusal reason."""


def parse_tag(tag):
    match = TAG.fullmatch(tag)
    if not match or any(len(n) > 20 or int(n) > 2**64 - 1 for n in match.groups()[:3]):
        raise ReleaseError('Unsupported tag: require stable vMAJOR.MINOR.PATCH[+component.lowercasehash]')
    major, minor, patch, component, prefix = match.groups()
    return {'tag': tag, 'core': f'{major}.{minor}.{patch}', 'minor': f'{major}.{minor}',
            'component': component or 'all', 'prefix': prefix}


def git(*args):
    return subprocess.check_output(['git', *args], text=True, stderr=subprocess.DEVNULL).strip()


def overlap(left, right):
    return left['core'] == right['core'] and (left['component'] == right['component']
            or 'all' in (left['component'], right['component']))


def reject_duplicates(info, tags):
    for tag in tags:
        if tag == info['tag']:
            continue
        try:
            other = parse_tag(tag)
        except ValueError:
            continue
        if overlap(info, other):
            raise ReleaseError('Core version already claimed by an overlapping release tag; bump version')


def validate(tag):
    info = parse_tag(tag)
    if info['component'] == 'index-server':
        raise ReleaseError('index-server is reserved; no releasable package exists')
    if git('rev-parse', '--is-shallow-repository') != 'false':
        raise ReleaseError('Full fetched Git history required')
    commit = git('rev-parse', '--verify', f'refs/tags/{tag}^{{commit}}')
    if info['prefix'] and not commit.startswith(info['prefix']):
        raise ReleaseError('Tag hash does not match peeled commit')
    if git('rev-parse', 'HEAD') != commit:
        raise ReleaseError('Checkout differs from peeled tag commit')
    if subprocess.run(['git', 'merge-base', '--is-ancestor', commit, 'refs/remotes/origin/main'],
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0:
        raise ReleaseError('Tag commit is not reachable from fetched origin/main')
    reject_duplicates(info, git('tag', '--list').splitlines())
    paths = {'client': 'Client/Windows', 'server': 'Server', 'shared': 'shared'}
    selected = paths if info['component'] == 'all' else [info['component']]
    for component in selected:
        version = tomllib.loads((Path(paths[component]) / 'Cargo.toml').read_text())['package']['version']
        if version != info['core']:
            raise ReleaseError(f'{component} package version differs from tag core')
    if info['component'] in ('server', 'all'):
        compose = Path('Server/deploy/docker-compose.yml').read_text()
        match = re.search(r'ghcr\.io/rusttalkcrew/rusttalk-server:([^\s@]+)', compose)
        if not match or match.group(1) != info['minor']:
            raise ReleaseError('Compose image alias differs from server tag minor')
    info['commit'] = commit
    return info


def docker_aliases(info):
    if info['component'] == 'client':
        return []
    return list(dict.fromkeys(['v' + info['core'], info['core'],
                              info['tag'].replace('+', '-'), info['minor'], 'latest']))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ReleaseError('Unexpected API redirect')


class API:
    def __init__(self, repo, token):
        if repo != 'rusttalkcrew/RustTalk':
            raise ReleaseError('Publication only allowed in upstream repository')
        self.base = 'https://api.github.com/repos/' + repo
        self.upload = 'https://uploads.github.com/repos/' + repo
        self.token = token
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, path, method='GET', data=None, upload=None):
        headers = {'Authorization': 'Bearer ' + self.token, 'Accept': 'application/vnd.github+json',
                   'X-GitHub-Api-Version': '2022-11-28'}
        if upload is not None:
            headers.update({'Content-Type': 'application/octet-stream',
                            'Content-Length': str(upload.stat().st_size)})
            with upload.open('rb') as stream:
                return self._read(self.upload + path, method, stream, headers)
        body = None if data is None else json.dumps(data).encode()
        if body is not None:
            headers['Content-Type'] = 'application/json'
        return self._read(self.base + path, method, body, headers)

    def _read(self, url, method, data, headers):
        try:
            with self.opener.open(urllib.request.Request(url, data=data, headers=headers, method=method),
                                  timeout=60) as response:
                return read_json(response)
        except urllib.error.HTTPError as error:
            error.close()
            raise ReleaseError(f'GitHub API refused operation (HTTP {error.code}); preserve draft and inspect run') from None
        except urllib.error.URLError:
            raise ReleaseError('GitHub API unavailable; preserve draft and inspect run') from None

    def listing(self, path):
        result = []
        for page in range(1, PAGES + 1):
            batch = self.request(f'{path}?per_page=100&page={page}')
            if not isinstance(batch, list) or len(batch) > 100 or any(not isinstance(x, dict) for x in batch):
                raise ReleaseError('Malformed GitHub list response')
            result.extend(batch)
            if len(batch) < 100:
                return result
        raise ReleaseError('GitHub pagination exhausted; completeness not established')


def read_json(response):
    # Read the limit plus one byte, never buffer an unbounded API response.
    data = response.read(LIMIT + 1)
    if len(data) > LIMIT:
        raise ReleaseError('API response exceeds 2 MiB')
    return json.loads(data)


def inventory(directory, component):
    root = Path(directory)
    expected_dirs = {f'rusttalk-{os_name}-{arch}' for os_name in ('linux', 'windows')
                     for arch in ('x86_64', 'aarch64')}
    if {p.name for p in root.iterdir()} != expected_dirs:
        raise ReleaseError('Missing or foreign matrix artifact directory')
    files = {}
    for folder in sorted(root.iterdir()):
        if not folder.is_dir() or folder.is_symlink():
            raise ReleaseError('Non-directory or symlink matrix artifact')
        os_name, arch = folder.name.removeprefix('rusttalk-').split('-')
        suffix = '-arm64' if arch == 'aarch64' else ''
        expected = set()
        if component in ('client', 'all'):
            expected.add(f'rusttalk-linux-{arch}.AppImage' if os_name == 'linux' else f'rusttalk-windows{suffix}.exe')
        if component in ('server', 'all'):
            if os_name == 'linux':
                expected.update([f'rusttalk-server-linux-{arch}', f'rusttalk-server-linux-{arch}.tar.gz'])
            else:
                expected.add(f'rusttalk-server-windows{suffix}.exe')
        actual = {p.name for p in folder.iterdir()}
        if os_name == 'windows' and component in ('client', 'all'):
            expected.update(n for n in actual if re.fullmatch(r'[A-Za-z0-9_.-]+\.dll', n))
        expected |= {n + '.sha256' for n in expected}
        if actual != expected:
            raise ReleaseError(f'Missing or foreign assets in {folder.name}')
        for name in sorted(actual):
            path = folder / name
            if not path.is_file() or path.is_symlink() or name in files or path.stat().st_size == 0:
                raise ReleaseError('Empty, duplicate or non-regular release asset')
            files[name] = path
        for name in actual - {n for n in actual if n.endswith('.sha256')}:
            digest = file_digest(folder / name)
            checksum = folder / (name + '.sha256')
            if checksum.stat().st_size > 256 or checksum.read_text().strip() != f'{digest}  {name}':
                raise ReleaseError('Release checksum mismatch')
    return files


def file_digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def claim(info):
    run = os.environ['GITHUB_RUN_ID']
    attempt = os.environ['GITHUB_RUN_ATTEMPT']
    if not run.isdecimal() or not attempt.isdecimal():
        raise ReleaseError('Invalid workflow run identity')
    return f'<!-- RustTalk release claim run={run} attempt={attempt} commit={info["commit"]} tag={info["tag"]} -->'


def owned(release, info, release_id=None):
    if (not isinstance(release, dict) or release.get('tag_name') != info['tag'] or release.get('draft') is not True
            or release.get('body') != claim(info) or release.get('target_commitish') != info['commit']
            or release.get('prerelease') is not False or type(release.get('id')) is not int
            or release['id'] <= 0 or (release_id is not None and release['id'] != release_id)):
        raise ReleaseError('Release draft does not belong to this run/attempt; explicit operator recovery required')


def reserve(api, info, files):
    releases = api.listing('/releases')
    for release in releases:
        tag = release.get('tag_name')
        if not isinstance(tag, str):
            raise ReleaseError('Malformed release tag')
        if tag == info['tag']:
            raise ReleaseError('Release already exists, including failed drafts; explicit operator recovery required')
        reject_duplicates(info, [tag])
    release = api.request('/releases', 'POST', {'tag_name': info['tag'], 'target_commitish': info['commit'],
                          'name': info['tag'], 'body': claim(info), 'draft': True,
                          'prerelease': False, 'make_latest': 'false'})
    owned(release, info)
    release_id = release['id']
    # No clobber operation exists here. Partial uploads leave the draft as evidence.
    for name, path in files.items():
        api.request(f'/releases/{release_id}/assets?name=' + urllib.parse.quote(name, safe=''),
                    'POST', upload=path)
    verify_assets(api, release_id, files)
    return release_id


def verify_assets(api, release_id, files):
    assets = api.listing(f'/releases/{release_id}/assets')
    if len(assets) != len(files) or {a.get('name') for a in assets} != set(files):
        raise ReleaseError('Draft assets differ from selected matrix artifacts')
    for asset in assets:
        path = files[asset['name']]
        if (asset.get('state') != 'uploaded' or asset.get('size') != path.stat().st_size
                or asset.get('digest') != 'sha256:' + file_digest(path)):
            raise ReleaseError('Draft asset size/digest/state mismatch')


def registry_check(info):
    # Only the immutable aliases must be absent; minor/latest intentionally move.
    query = urllib.parse.urlencode({'service': 'ghcr.io', 'scope': 'repository:rusttalkcrew/rusttalk-server:pull'})
    auth = base64.b64encode((os.environ['GITHUB_ACTOR'] + ':' + os.environ['GH_TOKEN']).encode()).decode()
    opener = urllib.request.build_opener(NoRedirect)
    with opener.open(urllib.request.Request('https://ghcr.io/token?' + query,
                     headers={'Authorization': 'Basic ' + auth}), timeout=60) as response:
        token = read_json(response)['token']
    for alias in docker_aliases(info)[:-2]:
        request = urllib.request.Request('https://ghcr.io/v2/rusttalkcrew/rusttalk-server/manifests/' + alias,
                  method='HEAD', headers={'Authorization': 'Bearer ' + token,
                  'Accept': 'application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json'})
        try:
            with opener.open(request, timeout=60):
                raise ReleaseError('Immutable Docker alias already exists; explicit operator recovery required')
        except urllib.error.HTTPError as error:
            error.close()
            if error.code != 404:
                raise ReleaseError('Cannot establish Docker alias absence') from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['validate', 'reserve', 'registry-check', 'publish'])
    parser.add_argument('--dist', default='dist')
    parser.add_argument('--release-id', type=int)
    args = parser.parse_args()
    info = validate(os.environ['GITHUB_REF_NAME'])
    if args.action == 'validate':
        outputs = {key: info[key] for key in ('component', 'core', 'commit')}
        outputs['docker_tags'] = ','.join(IMAGE + ':' + alias for alias in docker_aliases(info))
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            for key, value in outputs.items():
                output.write(f'{key}={value}\n')
        return
    if args.action in ('registry-check', 'publish') and (args.release_id is None or args.release_id <= 0):
        raise ReleaseError('A positive reserved release ID is required')
    api = API(os.environ['GITHUB_REPOSITORY'], os.environ['GH_TOKEN'])
    if args.action == 'registry-check':
        if info['component'] not in ('server', 'all'):
            raise ReleaseError('Client releases cannot publish Docker images')
        owned(api.request(f'/releases/{args.release_id}'), info, args.release_id)
        registry_check(info)
        return
    files = inventory(args.dist, info['component'])
    if args.action == 'reserve':
        release_id = reserve(api, info, files)
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            output.write(f'release_id={release_id}\n')
    else:
        owned(api.request(f'/releases/{args.release_id}'), info, args.release_id)
        verify_assets(api, args.release_id, files)
        notes = api.request('/releases/generate-notes', 'POST',
                            {'tag_name': info['tag'], 'target_commitish': info['commit']})
        if not isinstance(notes, dict) or not isinstance(notes.get('body'), str):
            raise ReleaseError('Generated release notes are malformed; draft remains unpublished')
        api.request(f'/releases/{args.release_id}', 'PATCH',
                    {'draft': False, 'make_latest': 'true' if info['component'] == 'all' else 'false',
                     'body': notes['body'] + '\n\n' + claim(info)})


if __name__ == '__main__':
    try:
        main()
    except ReleaseError as error:
        print(f'Release refused: {error}', file=sys.stderr)
        sys.exit(1)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError):
        # Do not print API bodies, headers, URLs, credentials or exception chains.
        print('Release operation refused. Check tag, versions, ancestry, duplicate claims, artifacts and draft ownership. '
              'Failed drafts/registry aliases require explicit operator recovery; never rerun to overwrite.', file=sys.stderr)
        sys.exit(1)

#!/usr/bin/env python3
"""Synthetic, offline tests: never create a tag in the real checkout."""
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location('release', Path(__file__).with_name('component-release.py'))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)
ROOT = Path(__file__).resolve().parent.parent


class FakeAPI:
    def __init__(self):
        self.releases = []
        self.assets = []
        self.calls = []
        self.fail_upload = False
        self.patches = []
        self.notes = {'body': 'Synthetic release notes'}

    def listing(self, path):
        return self.releases if path == '/releases' else self.assets

    def request(self, path, method='GET', data=None, upload=None):
        self.calls.append((path, method))
        if method == 'GET':
            return self.releases[0]
        if path == '/releases':
            item = dict(data, id=42)
            self.releases.append(item)
            return item
        if path == '/releases/generate-notes':
            return self.notes
        if upload:
            if self.fail_upload:
                raise ValueError('synthetic upload failure')
            self.assets.append({'name': upload.name, 'state': 'uploaded', 'size': upload.stat().st_size,
                                'digest': 'sha256:' + release.file_digest(upload)})
            return self.assets[-1]
        if method == 'PATCH':
            self.patches.append(data)
        return data


class BoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = patch.dict(os.environ, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1')
        self.env.start()
        self.addCleanup(self.env.stop)

    def info(self, component='server'):
        tag = 'v1.2.3' if component == 'all' else f'v1.2.3+{component}.abcdef1'
        return dict(release.parse_tag(tag), commit='abcdef1' + '0' * 33)

    def fixtures(self, component):
        for system in ('linux', 'windows'):
            for arch in ('x86_64', 'aarch64'):
                folder = self.root / f'rusttalk-{system}-{arch}'
                folder.mkdir()
                suffix = '-arm64' if arch == 'aarch64' else ''
                names = []
                if component in ('client', 'all'):
                    names.append(f'rusttalk-linux-{arch}.AppImage' if system == 'linux' else f'rusttalk-windows{suffix}.exe')
                if component in ('server', 'all'):
                    names += ([f'rusttalk-server-linux-{arch}', f'rusttalk-server-linux-{arch}.tar.gz']
                              if system == 'linux' else [f'rusttalk-server-windows{suffix}.exe'])
                for name in names:
                    path = folder / name
                    path.write_bytes(b'synthetic fixture only\n')
                    (folder / (name + '.sha256')).write_text(release.file_digest(path) + '  ' + name + '\n')
        return release.inventory(self.root, component)

    def test_strict_tags(self):
        for tag in ('v0.0.0', 'v18446744073709551615.2.3+client.abcdef1',
                    'v1.2.3+index-server.' + 'a' * 40):
            release.parse_tag(tag)
        for tag in ('1.2.3', 'v01.2.3', 'v1.02.3', 'v1.2.03', 'v1.2.3\n',
                    'v1.2.3-rc.1', 'v1.2.3+client.ABCDEF1', 'v1.2.3+client.abcdef',
                    'v1.2.3+client.' + 'a' * 41, 'v1.2.3+unknown.abcdef1',
                    'v18446744073709551616.2.3', 'v' + '9' * 5000 + '.2.3',
                    'v1.2.3+client.abcdef1;echo bad', 'v1.2.3+server.abcdef1.extra'):
            with self.subTest(tag=tag[:100]), self.assertRaises(ValueError):
                release.parse_tag(tag)

    def test_duplicate_hash_lengths_commits_legacy(self):
        info = self.info()
        for tag in ('v1.2.3+server.abcdef12', 'v1.2.3+server.0123456', 'v1.2.3'):
            with self.assertRaises(ValueError):
                release.reject_duplicates(info, [tag])
        release.reject_duplicates(info, [info['tag'], 'v1.2.3+client.abcdef1', 'v1.2.4'])
        with self.assertRaises(ValueError):
            release.reject_duplicates(self.info('all'), [info['tag']])

    def test_docker_routing(self):
        self.assertEqual(release.docker_aliases(self.info('client')), [])
        self.assertEqual(release.docker_aliases(self.info()),
                         ['v1.2.3', '1.2.3', 'v1.2.3-server.abcdef1', '1.2', 'latest'])
        self.assertEqual(release.docker_aliases(self.info('all')), ['v1.2.3', '1.2.3', '1.2', 'latest'])

    def test_selected_artifacts_and_checksums(self):
        files = self.fixtures('server')
        self.assertEqual(len(files), 12)
        next(iter(files.values())).write_bytes(b'corrupt')
        with self.assertRaises(ValueError):
            release.inventory(self.root, 'server')

    def test_client_artifacts(self):
        self.assertEqual(len(self.fixtures('client')), 8)

    def test_combined_artifacts(self):
        self.assertEqual(len(self.fixtures('all')), 20)

    def test_missing_or_foreign_artifact(self):
        self.fixtures('client')
        (self.root / 'rusttalk-linux-x86_64' / 'foreign').write_text('bad')
        with self.assertRaises(ValueError):
            release.inventory(self.root, 'client')

    def test_missing_matrix(self):
        with self.assertRaises(ValueError):
            release.inventory(self.root, 'client')

    def test_reserve_and_rerun_refused(self):
        files = self.fixtures('client')
        api = FakeAPI()
        self.assertEqual(release.reserve(api, self.info('client'), files), 42)
        self.assertTrue(api.releases[0]['draft'])
        self.assertEqual(api.releases[0]['make_latest'], 'false')
        with self.assertRaises(ValueError):
            release.reserve(api, self.info('client'), files)
        self.assertEqual(len(api.releases), 1)

    def test_partial_failure_preserves_claim(self):
        files = self.fixtures('server')
        api = FakeAPI()
        api.fail_upload = True
        with self.assertRaises(ValueError):
            release.reserve(api, self.info(), files)
        self.assertEqual(len(api.releases), 1)
        with self.assertRaises(ValueError):
            release.reserve(api, self.info(), files)

    def test_concurrent_claim_other_hash(self):
        api = FakeAPI()
        api.releases = [{'tag_name': 'v1.2.3+server.012345678'}]
        with self.assertRaises(ValueError):
            release.reserve(api, self.info(), {})
        self.assertFalse(api.calls)

    def test_ownership_attempt_commit_tag_draft(self):
        info = self.info()
        item = dict(id=42, tag_name=info['tag'], target_commitish=info['commit'],
                    body=release.claim(info), draft=True, prerelease=False)
        release.owned(item, info)
        for key, value in [('draft', False), ('body', 'foreign run'), ('prerelease', True),
                           ('target_commitish', '0' * 40), ('tag_name', 'v1.2.4')]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                release.owned(dict(item, **{key: value}), info)
        with patch.dict(os.environ, GITHUB_RUN_ATTEMPT='2'), self.assertRaises(ValueError):
            release.owned(item, info)

    def test_publish_uses_owned_id_and_latest_policy(self):
        files = self.fixtures('client')
        info = self.info('client')
        api = FakeAPI()
        release.reserve(api, info, files)
        with patch.dict(os.environ, GITHUB_REF_NAME=info['tag'],
                        GITHUB_REPOSITORY='rusttalkcrew/RustTalk', GH_TOKEN='synthetic'):
            with patch.object(release, 'validate', return_value=info), patch.object(release, 'API', return_value=api):
                with patch.object(sys, 'argv', ['component-release.py', 'publish', '--dist', str(self.root), '--release-id', '43']):
                    with self.assertRaises(ValueError):
                        release.main()
                self.assertNotIn(('/releases/43', 'PATCH'), api.calls)
                with patch.object(sys, 'argv', ['component-release.py', 'publish', '--dist', str(self.root), '--release-id', '42']):
                    release.main()
                self.assertIn(('/releases/42', 'PATCH'), api.calls)
                self.assertEqual(api.patches[-1], {'draft': False, 'make_latest': 'false',
                    'body': 'Synthetic release notes\n\n' + release.claim(info)})
                api.notes = {'body': None}
                api.calls.clear()
                with patch.object(sys, 'argv', ['component-release.py', 'publish', '--dist', str(self.root), '--release-id', '42']):
                    with self.assertRaises(ValueError):
                        release.main()
                self.assertNotIn(('/releases/42', 'PATCH'), api.calls)
                api.releases[0]['body'] = 'foreign attempt'
                api.calls.clear()
                with patch.object(sys, 'argv', ['component-release.py', 'publish', '--dist', str(self.root), '--release-id', '42']):
                    with self.assertRaises(ValueError):
                        release.main()
                self.assertNotIn(('/releases/42', 'PATCH'), api.calls)

    def test_asset_verification_rejects_replacement(self):
        api = FakeAPI()
        files = self.fixtures('client')
        release.reserve(api, self.info('client'), files)
        api.assets[0]['digest'] = 'sha256:' + '0' * 64
        with self.assertRaises(ValueError):
            release.verify_assets(api, 42, files)

    def test_api_stream_bound(self):
        class Stream(io.BytesIO):
            def read(self, size):
                self.requested = size
                return super().read(size)
        stream = Stream(b' ' * (release.LIMIT + 2))
        with self.assertRaises(ValueError):
            release.read_json(stream)
        self.assertEqual(stream.requested, release.LIMIT + 1)
        self.assertEqual(release.read_json(io.BytesIO(b'[]')), [])
        with self.assertRaises(ValueError):
            release.read_json(io.BytesIO(b'{'))

    def test_bounded_complete_pagination(self):
        api = release.API('rusttalkcrew/RustTalk', 'synthetic')
        calls = []
        def request(path):
            calls.append(path)
            return [{}] * (100 if len(calls) == 1 else 1)
        api.request = request
        self.assertEqual(len(api.listing('/releases')), 101)
        self.assertEqual(calls[-1], '/releases?per_page=100&page=2')
        api.request = lambda path: [{}] * 100
        with self.assertRaises(ValueError):
            api.listing('/releases')
        for bad in ({}, [None], [{}] * 101):
            api.request = lambda path: bad
            with self.assertRaises(ValueError):
                api.listing('/releases')

    def test_api_http_failure_and_no_redirect(self):
        api = release.API('rusttalkcrew/RustTalk', 'synthetic')
        with patch.object(api.opener, 'open', side_effect=urllib.error.HTTPError('synthetic', 500, '', {}, None)):
            with self.assertRaises(ValueError):
                api.request('/releases')
        with self.assertRaises(ValueError):
            release.NoRedirect().redirect_request(None, None, 302, '', {}, 'https://foreign.invalid')

    def test_registry_only_explicit_404_means_absent(self):
        info = self.info()
        class Opener:
            def __init__(self, status):
                self.status = status
                self.aliases = []
            def open(self, request, timeout):
                if '/token?' in request.full_url:
                    return io.BytesIO(b'{"token":"synthetic"}')
                self.aliases.append(request.full_url.rsplit('/', 1)[-1])
                if self.status:
                    raise urllib.error.HTTPError('synthetic', self.status, '', {}, None)
                return io.BytesIO(b'')
        with patch.dict(os.environ, GITHUB_ACTOR='synthetic', GH_TOKEN='synthetic'):
            for status in (401, 403, 429, 500, 0):
                with patch.object(release.urllib.request, 'build_opener', return_value=Opener(status)):
                    with self.assertRaises(ValueError):
                        release.registry_check(info)
            opener = Opener(404)
            with patch.object(release.urllib.request, 'build_opener', return_value=opener):
                release.registry_check(info)
            self.assertEqual(opener.aliases, ['v1.2.3', '1.2.3', 'v1.2.3-server.abcdef1'])

    def test_workflow_contract(self):
        text = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertIn('group: component-release-publication', text)
        self.assertIn('cancel-in-progress: false', text)
        self.assertIn('component: ${{ needs.check-version.outputs.component }}', text)
        self.assertIn('needs: [check-version, reserve]', text)
        self.assertIn('needs: [check-version, reserve, image]', text)
        self.assertNotIn('softprops/', text)
        self.assertNotIn('${{ github.ref', text)
        for line in text.splitlines():
            if 'uses:' in line and './.github/' not in line:
                self.assertRegex(line, r'@[0-9a-f]{40}(?:\s|$)')
        build = (ROOT / 'scripts/build-component.sh').read_text()
        self.assertIn('-p rusttalk-shared', build)


class GitTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        previous = Path.cwd()
        os.chdir(self.temp.name)
        self.addCleanup(os.chdir, previous)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'synthetic@example.invalid')
        self.git('config', 'user.name', 'Synthetic fixture')
        self.git('config', 'tag.gpgSign', 'false')
        self.git('config', 'commit.gpgSign', 'false')
        for folder in ('Client/Windows', 'Server', 'shared'):
            Path(folder).mkdir(parents=True, exist_ok=True)
            Path(folder, 'Cargo.toml').write_text('[package]\nversion = "1.2.3"\n')
        Path('Server/deploy').mkdir()
        Path('Server/deploy/docker-compose.yml').write_text('image: ghcr.io/rusttalkcrew/rusttalk-server:1.2\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'synthetic fixture')
        self.commit = self.git('rev-parse', 'HEAD')
        self.git('update-ref', 'refs/remotes/origin/main', self.commit)

    def git(self, *args):
        return subprocess.check_output(['git', *args], text=True, stderr=subprocess.DEVNULL).strip()

    def tag(self, component='server', prefix=None, annotated=False):
        tag = f'v1.2.3+{component}.{prefix or self.commit[:12]}'
        self.git('tag', *(['-a', '-m', 'synthetic'] if annotated else []), tag)
        return tag

    def test_lightweight_and_annotated_peeling(self):
        tag = self.tag(annotated=True)
        self.assertEqual(release.validate(tag)['commit'], self.commit)
        self.git('tag', '-d', tag)
        self.assertEqual(release.validate(self.tag())['component'], 'server')

    def test_hash_mismatch(self):
        with self.assertRaises(ValueError):
            release.validate(self.tag(prefix='0' * 40))

    def test_index_always_refused(self):
        with self.assertRaises(ValueError):
            release.validate(self.tag('index-server'))

    def test_only_selected_version_checked(self):
        Path('Client/Windows/Cargo.toml').write_text('[package]\nversion = "9.9.9"\n')
        Path('shared/Cargo.toml').write_text('[package]\nversion = "8.8.8"\n')
        release.validate(self.tag())
        self.git('tag', 'v1.2.3')
        with self.assertRaises(ValueError):
            release.validate('v1.2.3')

    def test_client_does_not_check_server_compose(self):
        Path('Server/deploy/docker-compose.yml').write_text('image: invalid fixture\n')
        self.assertEqual(release.validate(self.tag('client'))['component'], 'client')

    def test_selected_mismatch_and_compose(self):
        tag = self.tag()
        Path('Server/Cargo.toml').write_text('[package]\nversion = "9.9.9"\n')
        with self.assertRaises(ValueError):
            release.validate(tag)
        Path('Server/Cargo.toml').write_text('[package]\nversion = "1.2.3"\n')
        Path('Server/deploy/docker-compose.yml').write_text('image: ghcr.io/rusttalkcrew/rusttalk-server:9.9\n')
        with self.assertRaises(ValueError):
            release.validate(tag)

    def test_side_branch_not_on_main(self):
        self.git('checkout', '-qb', 'side')
        Path('side').write_text('fixture')
        self.git('add', 'side')
        self.git('commit', '-qm', 'side fixture')
        self.commit = self.git('rev-parse', 'HEAD')
        with self.assertRaises(ValueError):
            release.validate(self.tag())

    def test_wrong_checkout(self):
        tag = self.tag()
        self.git('commit', '--allow-empty', '-qm', 'other fixture')
        with self.assertRaises(ValueError):
            release.validate(tag)

    def test_combined_synchronized(self):
        self.git('tag', 'v1.2.3')
        self.assertEqual(release.validate('v1.2.3')['component'], 'all')

    def test_duplicate_local_claim(self):
        tag = self.tag()
        self.git('tag', f'v1.2.3+server.{self.commit[:7]}')
        with self.assertRaises(ValueError):
            release.validate(tag)


if __name__ == '__main__':
    unittest.main()

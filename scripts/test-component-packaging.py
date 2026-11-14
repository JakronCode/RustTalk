#!/usr/bin/env python3
"""Offline component routing and packaging fixtures; no native builds or downloads."""
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
BASH = shutil.which('bash')


class ComponentPackagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='rusttalk component ')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.env = {**os.environ, 'PATH': str(self.bin)}
        for name in ('bash', 'uname', 'mktemp', 'rm', 'mkdir', 'cp', 'tar', 'gzip',
                     'sha256sum', 'mv', 'chmod', 'find'):
            (self.bin / name).symlink_to(shutil.which(name))
        self.log = self.root / 'calls'
        self.env['CALLS'] = str(self.log)

    def executable(self, path, body):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('#!' + BASH + '\nset -eu\n' + body)
        path.chmod(0o755)

    def run_script(self, script, *args, check=True):
        return subprocess.run([BASH, str(ROOT / 'scripts' / script), *args],
                              cwd=self.root, env=self.env, text=True,
                              capture_output=True, check=check)

    def test_unknown_components_fail_before_any_side_effect(self):
        # Even uname/mktemp/cargo/fetch tools are unavailable.
        self.env['PATH'] = str(self.root / 'missing')
        for component in ('', 'index-server', 'CLIENT', '../server', 'server; touch injected'):
            for script in ('build-component.sh', 'package-linux.sh'):
                with self.subTest(component=component, script=script):
                    result = self.run_script(script, component, check=False)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('Unsupported component:', result.stderr)
                    self.assertEqual(list(self.root.iterdir()), [self.bin])

    def test_cargo_routes_only_selected_packages_and_keeps_serial_tests(self):
        self.executable(self.bin / 'cargo', 'printf "%s\\n" "$@" >> "$CALLS"\n')
        for component, packages in (
                ('all', ['--workspace']),
                ('client', ['-p', 'rusttalk-client', '-p', 'rusttalk-shared']),
                ('server', ['-p', 'rusttalk-server', '-p', 'rusttalk-shared'])):
            for action in ('test', 'build'):
                with self.subTest(component=component, action=action):
                    self.log.unlink(missing_ok=True)
                    self.run_script('build-component.sh', component, action)
                    expected = ([action, '--locked'] +
                                (['--release'] if action == 'build' else []) +
                                packages +
                                (['--', '--test-threads=1'] if action == 'test' else []))
                    self.assertEqual(self.log.read_text().splitlines(), expected)
        self.log.unlink()
        self.run_script('build-component.sh')
        self.assertFalse(self.log.exists())
        result = self.run_script('build-component.sh', 'server', 'unknown', check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())

    def server_fixture(self):
        binary = self.root / 'target/release/rusttalk-server'
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(b'synthetic server, never executed\n')
        (self.root / 'Server/deploy').mkdir(parents=True)
        (self.root / 'Server/server.toml').write_text('# synthetic config\n')
        (self.root / 'Server/deploy/service').write_text('# synthetic service\n')

    def client_fixture(self):
        # Stub only external tools; exercise the real packaging script and file operations.
        binary = self.root / 'target/release/rusttalk'
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(b'synthetic client\n')
        assets = self.root / 'packaging/linux'
        assets.mkdir(parents=True)
        (assets / 'AppRun').write_text('# synthetic launcher\n')
        plugins = self.root / 'plugins'
        plugins.mkdir()
        for name in ('app', 'coreelements', 'playback', 'typefindfunctions',
                     'videoconvertscale', 'video4linux2', 'jpeg', 'pipewire'):
            (plugins / f'libgst{name}.so').touch()
        (plugins / 'gstreamer-1.0').mkdir()
        (plugins / 'gstreamer-1.0/gst-plugin-scanner').touch()
        self.env['PLUGINS'] = str(plugins)
        self.executable(self.bin / 'pkg-config', 'printf "%s\\n" "$PLUGINS"\n')
        self.executable(self.bin / 'python3', """
printf '%s\\n' "$1" >> "$CALLS"
if [[ "$1" == scripts/fetch-release-tool.py ]]; then
    if [[ "$2" == AppImage/appimagetool ]]; then
        printf '#!/usr/bin/env bash\\nprintf "#!/usr/bin/env bash\\\\nexit 0\\\\n" > "${@: -1}"\\nchmod +x "${@: -1}"\\n' > "$5"
    else
        printf '#!/usr/bin/env bash\\nexit 0\\n' > "$5"
    fi
    chmod +x "$5"
fi
""")

    def assert_checksums(self, names):
        dist = self.root / 'dist'
        self.assertEqual({p.name for p in dist.iterdir()},
                         set(names) | {name + '.sha256' for name in names})
        for name in names:
            checksum = (dist / (name + '.sha256')).read_text().split()
            self.assertEqual(checksum, [hashlib.sha256((dist / name).read_bytes()).hexdigest(), name])

    def test_server_packages_without_client_or_media_or_fetch_tools(self):
        self.server_fixture()
        self.run_script('package-linux.sh', 'server')
        arch = os.uname().machine
        name = f'rusttalk-server-linux-{arch}'
        self.assert_checksums([name, name + '.tar.gz'])
        self.assertFalse(self.log.exists())
        with tarfile.open(self.root / 'dist' / (name + '.tar.gz')) as archive:
            self.assertEqual({m.name for m in archive.getmembers()},
                             {'.', './rusttalk-server', './server.toml', './deploy', './deploy/service'})
            self.assertEqual(archive.extractfile('./rusttalk-server').read(),
                             b'synthetic server, never executed\n')

    def test_client_does_not_require_server_config_or_tar(self):
        self.client_fixture()
        (self.bin / 'tar').unlink()
        self.run_script('package-linux.sh', 'client')
        self.assert_checksums([f'rusttalk-linux-{os.uname().machine}.AppImage'])
        self.assertEqual(self.log.read_text().splitlines(),
                         ['scripts/fetch-release-tool.py'] * 3 +
                         ['scripts/test-appimage-audio.py', 'scripts/test-appimage-update.py'])

    def test_default_all_retains_both_packages(self):
        self.client_fixture()
        self.server_fixture()
        self.run_script('package-linux.sh')
        arch = os.uname().machine
        self.assert_checksums([f'rusttalk-linux-{arch}.AppImage',
                               f'rusttalk-server-linux-{arch}',
                               f'rusttalk-server-linux-{arch}.tar.gz'])


class WorkflowRoutingTests(unittest.TestCase):
    def test_docker_includes_shared_sources_outside_crate_src(self):
        dockerfile = (ROOT / 'Server/Dockerfile').read_text().splitlines()
        ignore = (ROOT / '.dockerignore').read_text().splitlines()
        sources = [('shared/update_discovery.rs', 'shared/')]
        # Custody exists on the identity branch, not the standalone main port.
        if (ROOT / 'Server/src/server_custody.rs').is_file():
            sources.append(('shared/storage_guard/linux.rs', 'shared/storage_guard/'))
        for source, destination in sources:
            with self.subTest(source=source):
                self.assertTrue((ROOT / source).is_file())
                self.assertIn('!' + source, ignore)
                self.assertIn(f'COPY {source} {destination}', dockerfile)

    def test_workflow_contract_and_platform_setup(self):
        workflow = (ROOT / '.github/workflows/build.yml').read_text()
        self.assertIn('type: string\n        default: all', workflow)
        self.assertIn('type: choice\n        default: all\n        options: [all, client, server]', workflow)
        self.assertIn('COMPONENT: ${{ inputs.component }}', workflow)
        self.assertEqual(workflow.count('runner: '), 4)
        self.assertIn('permissions:\n  contents: read', workflow)
        for action in re.findall(r'uses: (\S+)', workflow):
            self.assertRegex(action, r'@[0-9a-f]{40}$')
        self.assertLess(workflow.index('- name: Validate component'),
                        workflow.index('- uses: dtolnay/'))
        blocks = dict(re.findall(r'      - name: ([^\n]+)\n(.*?)(?=      - |\Z)',
                                 workflow, re.S))
        for name in ('Linux client dependencies', 'Windows NASM', 'Windows CMake compatibility'):
            self.assertIn("env.COMPONENT != 'server'", blocks[name])
        self.assertNotRegex(blocks['Linux server dependencies'],
                            r'(?i)gstreamer|alsa|appimage|nasm|openh264')
        self.assertIn('bash scripts/build-component.sh "$COMPONENT" test', blocks['Test'])
        self.assertIn('bash scripts/build-component.sh "$COMPONENT" build', blocks['Build selected component'])
        self.assertIn('bash scripts/package-linux.sh "$COMPONENT"', blocks['Linux AppImage and server bundle'])
        # Workflow inputs may enter env, never executable shell text.
        for block in blocks.values():
            if 'run:' in block:
                self.assertNotIn('inputs.component', block)
        windows = blocks['Windows binaries']
        client = re.search(r"if \(\$env:COMPONENT -ne 'server'\) \{(.*?)\n          \}", windows, re.S).group(1)
        server = re.findall(r"if \(\$env:COMPONENT -ne 'client'\) \{(.*?)\n          \}", windows, re.S)
        self.assertIn('target/release/rusttalk.exe', client)
        self.assertIn('target/release/*.dll', client)
        self.assertIn('target/release/rusttalk-server.exe', server[0])
        self.assertIn('--version', server[1])
        self.assertNotIn('rusttalk-server', client)
        self.assertIn('Get-ChildItem dist/* -Include *.exe,*.dll -File', windows)
        self.assertIn('path: dist/*', workflow)
        self.assertIn('if-no-files-found: error', workflow)


if __name__ == '__main__':
    unittest.main()

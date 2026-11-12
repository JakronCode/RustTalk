#!/usr/bin/env python3
"""Keep ALSA/PipeWire host-owned; optionally validate a staged AppDir."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
AUDIO_ENV = ('ALSA_PLUGIN_DIR', 'SPA_PLUGIN_DIR', 'PIPEWIRE_MODULE_DIR')


def check_audio_layout(appdir):
    for path in (appdir / 'usr').rglob('*'):
        name = path.name
        if (name in ('alsa-lib', 'spa-0.2', 'pipewire-0.3')
                or name.startswith(('libasound', 'libpipewire-', 'libspa-'))):
            raise ValueError(f'Audio stack must be host-owned: {path.relative_to(appdir)}')


class AudioPackagingTests(unittest.TestCase):
    def test_layout_rejects_audio_libraries_and_plugins(self):
        with tempfile.TemporaryDirectory() as directory:
            appdir = Path(directory)
            library = appdir / 'usr/lib'
            library.mkdir(parents=True)
            for name in ('libasound.so.2', 'libpipewire-0.3.so.0',
                         'libpipewire-module-adapter.so', 'libspa-support.so',
                         'libasound_module_pcm_pipewire.so'):
                with self.subTest(name=name):
                    path = library / name
                    path.touch()
                    with self.assertRaises(ValueError):
                        check_audio_layout(appdir)
                    path.unlink()
            for name in ('alsa-lib', 'spa-0.2', 'pipewire-0.3'):
                with self.subTest(name=name):
                    path = library / name
                    path.mkdir()
                    with self.assertRaises(ValueError):
                        check_audio_layout(appdir)
                    path.rmdir()
            (library / 'libgstreamer-1.0.so.0').touch()
            check_audio_layout(appdir)

    def test_launcher_preserves_host_audio_environment(self):
        with tempfile.TemporaryDirectory(prefix='RustTalk audio test ') as directory:
            appdir = Path(directory)
            binary = appdir / 'usr/bin/rusttalk'
            binary.parent.mkdir(parents=True)
            binary.write_text(f'#!{sys.executable}\nimport json, os, sys\n'
                              'print(json.dumps([dict(os.environ), sys.argv[1:]]))\n')
            binary.chmod(0o755)
            shutil.copy2(ROOT / 'packaging/linux/AppRun', appdir / 'AppRun')
            for supplied in (False, True):
                with self.subTest(supplied=supplied):
                    env = {key: value for key, value in os.environ.items()
                           if key not in (*AUDIO_ENV, 'APPDIR', 'LD_LIBRARY_PATH')}
                    if supplied:
                        env.update({key: f'/host custom/{key}' for key in AUDIO_ENV})
                    result = subprocess.run(['bash', str(appdir / 'AppRun'), 'argument with spaces'],
                                            env=env, capture_output=True, text=True, check=True)
                    actual, args = json.loads(result.stdout)
                    for key in AUDIO_ENV:
                        self.assertEqual(actual.get(key), env.get(key))
                    self.assertEqual(args, ['argument with spaces'])
                    self.assertEqual(actual['GST_PLUGIN_PATH'], str(appdir / 'usr/lib/gstreamer-1.0'))
                    self.assertEqual(actual['GST_PLUGIN_SYSTEM_PATH'], '')
                    self.assertEqual(actual['LD_LIBRARY_PATH'], str(appdir / 'usr/lib'))


if __name__ == '__main__':
    if len(sys.argv) == 2:
        check_audio_layout(Path(sys.argv[1]))
        print('AppDir audio: ALSA/PipeWire libraries and plugins remain host-owned')
    else:
        unittest.main()

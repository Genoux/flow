import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class Channels(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.package = self.root / "release"
        shutil.copytree(Path(__file__).resolve().parents[1] / "packaging", self.package / "packaging")
        (self.package / "bin").mkdir()
        self.bin = self.root / "home/.local/bin"
        self.bin.mkdir(parents=True)
        mocks = self.root / "mocks"
        mocks.mkdir()
        for name in ("systemctl", "update-desktop-database", "gtk-update-icon-cache"):
            self.executable(mocks / name, '#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALL_LOG"\n')
        self.env = dict(os.environ, HOME=str(self.root / "home"),
                        XDG_BIN_HOME=str(self.bin), XDG_CONFIG_HOME=str(self.root / "config"),
                        XDG_DATA_HOME=str(self.root / "data"),
                        PATH=str(mocks) + os.pathsep + os.environ["PATH"],
                        CALL_LOG=str(self.root / "calls"))

    def tearDown(self):
        self.temp.cleanup()

    def executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)

    def install(self, channel, *args):
        (self.package / "packaging/channel").write_text(channel + "\n")
        for name in ("flow", "flow-console"):
            self.executable(self.package / "bin" / name, f'#!/bin/sh\necho {channel}\n')
        return subprocess.run(["bash", str(self.package / "packaging/install.sh"), *args],
                              env=self.env, capture_output=True, text=True)

    def test_install_update_opt_in_and_rollback_preserve_both_builds(self):
        self.assertEqual(self.install("stable").returncode, 0)
        self.assertEqual(self.bin.joinpath("flow").readlink(), Path("flow-stable"))
        self.root.joinpath("calls").write_text("")
        result = self.install("experimental", "--no-activate", "--no-restart")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.bin.joinpath("flow").readlink(), Path("flow-stable"))
        self.assertNotIn("restart", self.root.joinpath("calls").read_text())
        self.assertNotIn("--now", self.root.joinpath("calls").read_text())
        for channel in ("experimental", "stable"):
            for name in ("flow", "flow-console"):
                self.bin.joinpath(name).unlink()
                self.bin.joinpath(name).symlink_to(f"{name}-{channel}")
            self.assertEqual(self.install(channel).returncode, 0)
            self.assertEqual(subprocess.check_output([str(self.bin / "flow")], text=True).strip(), channel)
        for name in ("flow-stable", "flow-console-stable", "flow-experimental", "flow-console-experimental"):
            self.assertTrue(self.bin.joinpath(name).is_file())

    def test_legacy_local_binary_survives_experimental_install(self):
        for name in ("flow", "flow-console"):
            self.executable(self.bin / name, "#!/bin/sh\necho legacy-local\n")
        result = self.install("experimental", "--no-activate", "--no-restart")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(subprocess.check_output([str(self.bin / "flow")], text=True).strip(), "legacy-local")

    def test_update_repairs_mixed_channel_links(self):
        for selected in ("stable", "experimental"):
            with self.subTest(selected=selected):
                self.assertEqual(self.install("stable").returncode, 0)
                self.assertEqual(self.install("experimental", "--no-activate").returncode, 0)
                other = "experimental" if selected == "stable" else "stable"
                for name, channel in (("flow", selected), ("flow-console", other)):
                    self.bin.joinpath(name).unlink()
                    self.bin.joinpath(name).symlink_to(f"{name}-{channel}")
                result = self.install(selected)
                self.assertEqual(result.returncode, 0, result.stderr)
                for name in ("flow", "flow-console"):
                    self.assertEqual(self.bin.joinpath(name).readlink(), Path(f"{name}-{selected}"))

    def test_update_replaces_an_executing_binary(self):
        self.assertEqual(self.install("stable").returncode, 0)
        shutil.copyfile("/bin/sleep", self.bin / "flow-stable")
        process = subprocess.Popen([str(self.bin / "flow"), "30"])
        try:
            result = self.install("stable")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIsNone(process.poll())
            self.assertEqual(subprocess.check_output([str(self.bin / "flow")], text=True).strip(), "stable")
        finally:
            process.terminate()
            process.wait()

    def test_cloud_package_cannot_install_as_stable(self):
        result = self.install("experimental", "--channel", "stable")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bin.joinpath("flow-stable").exists())


if __name__ == "__main__":
    unittest.main()

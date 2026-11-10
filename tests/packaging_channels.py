import os
import re
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
        for name in ("offhand", "offhand-console"):
            self.executable(self.package / "bin" / name, f'#!/bin/sh\necho {channel}\n')
        return subprocess.run(["bash", str(self.package / "packaging/install.sh"), *args],
                              env=self.env, capture_output=True, text=True)

    def test_install_update_opt_in_and_rollback_preserve_both_builds(self):
        self.assertEqual(self.install("stable").returncode, 0)
        self.assertEqual(self.bin.joinpath("offhand").readlink(), Path("offhand-stable"))
        self.root.joinpath("calls").write_text("")
        result = self.install("experimental", "--no-activate", "--no-restart")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.bin.joinpath("offhand").readlink(), Path("offhand-stable"))
        self.assertNotIn("restart", self.root.joinpath("calls").read_text())
        self.assertNotIn("--now", self.root.joinpath("calls").read_text())
        for channel in ("experimental", "stable"):
            for name in ("offhand", "offhand-console"):
                self.bin.joinpath(name).unlink()
                self.bin.joinpath(name).symlink_to(f"{name}-{channel}")
            self.assertEqual(self.install(channel).returncode, 0)
            self.assertEqual(subprocess.check_output([str(self.bin / "offhand")], text=True).strip(), channel)
        for name in ("offhand-stable", "offhand-console-stable", "offhand-experimental", "offhand-console-experimental"):
            self.assertTrue(self.bin.joinpath(name).is_file())

    def test_upgrade_from_flow_removes_the_old_install(self):
        units = self.root / "config/systemd/user"
        apps = self.root / "data/applications"
        icons = self.root / "data/icons/hicolor/512x512/apps"
        for directory in (units, apps, icons):
            directory.mkdir(parents=True)
        leftovers = [units / "flow.service", units / "flow-tray.service",
                     apps / "flow-console.desktop", icons / "flow-console.png"]
        for path in leftovers:
            path.write_text("flow\n")
        for name in ("flow", "flow-console"):
            for channel in ("stable", "experimental"):
                self.executable(self.bin / f"{name}-{channel}", "#!/bin/sh\necho flow\n")
                leftovers.append(self.bin / f"{name}-{channel}")
            self.bin.joinpath(name).symlink_to(f"{name}-stable")
            leftovers.append(self.bin / name)
        config = self.root / "config/flow/config.toml"
        config.parent.mkdir()
        config.write_text("cleanup = \"light\"\n")

        result = self.install("stable")

        self.assertEqual(result.returncode, 0, result.stderr)
        for path in leftovers:
            self.assertFalse(path.exists() or path.is_symlink(), path)
        calls = self.root.joinpath("calls").read_text().splitlines()
        retired = calls.index("--user disable --now flow.service flow-tray.service")
        self.assertLess(retired, calls.index("--user enable offhand.service"))
        self.assertLess(retired, calls.index("--user start offhand.service"))
        self.assertEqual(self.bin.joinpath("offhand").readlink(), Path("offhand-stable"))
        self.assertTrue(units.joinpath("offhand.service").is_file())
        # The installer never moves what is the user's: offhand does on first run.
        self.assertEqual(config.read_text(), "cleanup = \"light\"\n")

    def test_a_fresh_install_has_no_flow_to_retire(self):
        self.assertEqual(self.install("stable").returncode, 0)
        self.assertNotIn("flow", self.root.joinpath("calls").read_text())

    def test_legacy_local_binary_survives_experimental_install(self):
        for name in ("offhand", "offhand-console"):
            self.executable(self.bin / name, "#!/bin/sh\necho legacy-local\n")
        result = self.install("experimental", "--no-activate", "--no-restart")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(subprocess.check_output([str(self.bin / "offhand")], text=True).strip(), "legacy-local")

    def test_update_repairs_mixed_channel_links(self):
        for selected in ("stable", "experimental"):
            with self.subTest(selected=selected):
                self.assertEqual(self.install("stable").returncode, 0)
                self.assertEqual(self.install("experimental", "--no-activate").returncode, 0)
                other = "experimental" if selected == "stable" else "stable"
                for name, channel in (("offhand", selected), ("offhand-console", other)):
                    self.bin.joinpath(name).unlink()
                    self.bin.joinpath(name).symlink_to(f"{name}-{channel}")
                result = self.install(selected)
                self.assertEqual(result.returncode, 0, result.stderr)
                for name in ("offhand", "offhand-console"):
                    self.assertEqual(self.bin.joinpath(name).readlink(), Path(f"{name}-{selected}"))

    def test_update_replaces_an_executing_binary(self):
        self.assertEqual(self.install("stable").returncode, 0)
        # Perl rather than /bin/sleep: uutils coreutils, which newer Ubuntu
        # runners ship, refuses to run under a name that is not its own.
        shutil.copyfile(shutil.which("perl"), self.bin / "offhand-stable")
        process = subprocess.Popen([str(self.bin / "offhand"), "-e", "sleep 30"])
        try:
            result = self.install("stable")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIsNone(process.poll())
            self.assertEqual(subprocess.check_output([str(self.bin / "offhand")], text=True).strip(), "stable")
        finally:
            process.terminate()
            process.wait()

    def test_cloud_package_cannot_install_as_stable(self):
        result = self.install("experimental", "--channel", "stable")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bin.joinpath("offhand-stable").exists())

    def test_macos_bundle_records_its_channel(self):
        # bundle.sh needs sips and iconutil, so it cannot run here. What the
        # console reads on a Mac is Contents/Resources/channel, and it has to be
        # the same file the release workflow checked against the tag.
        script = Path(__file__).resolve().parents[1] / "packaging/macos/bundle.sh"
        text = script.read_text()
        self.assertIn('"$here/../channel" "$app/Contents/Resources/channel"', text)
        self.assertLess(text.index("Contents/Resources/channel"), text.index("rcodesign\" sign"))

    def test_macos_helper_path_agrees_everywhere(self):
        root = Path(__file__).resolve().parents[1]
        lib = (root / "crates/offhand-paths/src/lib.rs").read_text()
        relative = re.search(r'AGENT_PROGRAM: &str = "([^"]+)"', lib).group(1)
        macos = root / "packaging/macos"
        for name in ("com.genoux.offhand.plist", "com.genoux.offhand.tray.plist"):
            self.assertIn(f"<string>@APP@/{relative}</string>", (macos / name).read_text())
        self.assertIn(f'"$app/{relative}"', (macos / "install.sh").read_text())

    def test_macos_bundle_signs_the_helper_before_the_app(self):
        text = (Path(__file__).resolve().parents[1] / "packaging/macos/bundle.sh").read_text()
        self.assertLess(text.index('for bundle in "$agent" "$app"'), text.index("signed_as "))


if __name__ == "__main__":
    unittest.main()

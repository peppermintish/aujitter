"""Store updates must fail before mutation when packages/account state are wrong."""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET
import zipfile
from contextlib import redirect_stdout


SCRIPT = Path(__file__).resolve().parents[1] / "store_release.py"
SPEC = importlib.util.spec_from_file_location("store_release", SCRIPT)
store = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(store)
IDENTITY = store.identity()


def executable(arch):
    binary = bytearray(256)
    binary[:2] = b"MZ"
    struct.pack_into("<I", binary, 60, 128)
    binary[128:132] = b"PE\0\0"
    struct.pack_into("<H", binary, 132, store.MACHINES[arch])
    return binary


def package(path, arch="x64", version="0.1.2.0", publisher=None, binary_arch=None):
    root = ET.Element("Package")
    ET.SubElement(root, "Identity", {
        "Name": IDENTITY["name"], "Publisher": publisher or IDENTITY["publisher"],
        "Version": version, "ProcessorArchitecture": arch,
    })
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("AppxManifest.xml", ET.tostring(root))
        for filename in ("aujitter.exe", "aujitter-gui.exe"):
            archive.writestr(filename, executable(binary_arch or arch))
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(path.name + ".sha256").write_text(f"{digest}  {path.name}\n")
    return path


def bundle(directory, arches=("x64", "arm64")):
    root = ET.Element("Bundle")
    ET.SubElement(root, "Identity", {
        "Name": IDENTITY["name"], "Publisher": IDENTITY["publisher"], "Version": "0.1.2.0",
    })
    packages = ET.SubElement(root, "Packages")
    target = directory / "AuJitter-0.1.2-store.msixbundle"
    with zipfile.ZipFile(target, "w") as archive:
        for index, arch in enumerate(arches):
            inner = package(directory / f"{arch}-{index}.msix", arch)
            ET.SubElement(packages, "Package", {
                "Architecture": arch, "Type": "application", "Version": "0.1.2.0", "FileName": inner.name,
            })
            archive.write(inner, inner.name)
        archive.writestr("AppxMetadata/AppxBundleManifest.xml", ET.tostring(root))
    digest = hashlib.sha256(target.read_bytes()).hexdigest()
    target.with_name(target.name + ".sha256").write_text(f"{digest}  {target.name}\n")
    return target


def application(pending=None, published=True):
    return {
        "id": IDENTITY["store_id"], "publisherName": IDENTITY["publisher"],
        "packageIdentityName": IDENTITY["name"], "packageFamilyName": IDENTITY["package_family_name"],
        "lastPublishedApplicationSubmission": {"id": "old"} if published else None,
        "pendingApplicationSubmission": pending,
    }


class StoreReleaseChecks(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def test_native_packages_and_checksums(self):
        for arch in store.MACHINES:
            path = package(self.directory / f"{arch}.msix", arch)
            store.verify_checksum(path)
            store.verify_package(path, IDENTITY, "0.1.2", arch)

    def test_corrupt_download_rejected(self):
        path = package(self.directory / "x64.msix")
        path.write_bytes(path.read_bytes() + b"changed")
        with self.assertRaisesRegex(store.ReleaseError, "Checksum mismatch"):
            store.verify_checksum(path)

    def test_checksum_cannot_refer_to_another_file(self):
        path = package(self.directory / "x64.msix")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        path.with_name(path.name + ".sha256").write_text(f"{digest}  other.msix\n")
        with self.assertRaises(store.ReleaseError):
            store.verify_checksum(path)

    def test_wrong_publisher_rejected(self):
        path = package(self.directory / "wrong.msix", publisher="CN=OtherPublisher")
        with self.assertRaisesRegex(store.ReleaseError, "identity"):
            store.verify_package(path, IDENTITY, "0.1.2", "x64")

    def test_wrong_version_and_nonzero_revision_rejected(self):
        for version in ("0.1.1.0", "0.1.2.1"):
            path = package(self.directory / "wrong.msix", version=version)
            with self.assertRaisesRegex(store.ReleaseError, "version"):
                store.verify_package(path, IDENTITY, "0.1.2", "x64")

    def test_relabelled_binary_rejected(self):
        path = package(self.directory / "arm64.msix", "arm64", binary_arch="x64")
        with self.assertRaisesRegex(store.ReleaseError, "Executable architecture"):
            store.verify_package(path, IDENTITY, "0.1.2", "arm64")

    def test_wrong_package_architecture_rejected(self):
        path = package(self.directory / "x64.msix", "arm64")
        with self.assertRaisesRegex(store.ReleaseError, "MSIX architecture"):
            store.verify_package(path, IDENTITY, "0.1.2", "x64")

    def test_bundle_contains_both_native_architectures(self):
        store.verify_bundle(bundle(self.directory), IDENTITY, "0.1.2")

    def test_missing_or_duplicated_architecture_rejected(self):
        for arches in (("x64",), ("arm64",), ("x64", "x64")):
            with self.subTest(arches=arches):
                with self.assertRaisesRegex(store.ReleaseError, "exactly x64 and ARM64"):
                    store.verify_bundle(bundle(self.directory, arches), IDENTITY, "0.1.2")

    def test_invalid_tag_cannot_reach_a_tool(self):
        for tag in ("v0.1.2-beta", "v0.1.2; echo bad", "../v0.1.2", "v65536.1.2"):
            with self.assertRaises(store.ReleaseError):
                store.release_version(tag)

    def test_missing_credentials_report_names_only(self):
        with self.assertRaises(store.ReleaseError) as error:
            store.validate_credentials({})
        self.assertTrue(all(name in str(error.exception) for name in store.SECRET_NAMES))

    def test_credentials_formats_and_seller_id_distinction(self):
        values = dict(zip(store.SECRET_NAMES, (
            "11111111-2222-4333-8444-555555555555", "66666666-7777-4888-8999-000000000000",
            "private-test-secret", "12345",
        )))
        store.validate_credentials(values)
        values["SELLER_ID"] = IDENTITY["publisher"]
        with self.assertRaisesRegex(store.ReleaseError, "numeric Partner Center seller ID"):
            store.validate_credentials(values)

    def test_wrong_store_application_rejected(self):
        app = application()
        app["id"] = "OTHERAPP"
        with self.assertRaisesRegex(store.ReleaseError, "identity"):
            store.verify_store_application(app, IDENTITY)

    def test_casing_of_cli_json_keys_is_supported(self):
        app = {key[0].upper() + key[1:]: value for key, value in application().items()}
        store.verify_store_application(app, IDENTITY)

    def test_first_submission_and_existing_draft_block_updates(self):
        with self.assertRaisesRegex(store.ReleaseError, "first Store version"):
            store.verify_update_ready(application(published=False), {}, "0.1.2")
        with self.assertRaisesRegex(store.ReleaseError, "pending submission"):
            store.verify_update_ready(application(pending={"id": "active"}), {}, "0.1.2")

    def test_same_or_older_store_version_rejected(self):
        for version in ("0.1.1", "0.1.0"):
            with self.assertRaisesRegex(store.ReleaseError, "newer"):
                store.verify_update_ready(application(), {"applicationPackages": [{"version": "0.1.1.0"}]}, version)
        store.verify_update_ready(application(), {"applicationPackages": [{"version": "0.1.1.0"}]}, "0.1.2")

    def test_all_published_versions_must_be_older(self):
        with self.assertRaisesRegex(store.ReleaseError, "newer"):
            store.verify_update_ready(application(), {"applicationPackages": [
                {"version": "0.1.1.0"}, {"version": "0.1.3.0"},
            ]}, "0.1.2")

    def test_read_only_check_never_calls_publish(self):
        target = self.directory / "bundle"
        target.mkdir()
        bundle(target)
        calls = []

        def fake(command, operation, timeout=120):
            calls.append(command)
            return json.dumps(application() if command[1] == "apps" else {
                "applicationPackages": [{"version": "0.1.1.0"}],
            })

        with patch.object(store, "configure_store"), patch.object(store, "run_capture", side_effect=fake), redirect_stdout(io.StringIO()):
            store.check_store("v0.1.2", self.directory)
        self.assertEqual([command[1:3] for command in calls], [["apps", "get"], ["submission", "get"]])

    def test_publish_cannot_replace_an_existing_submission(self):
        target = self.directory / "bundle"
        target.mkdir()
        bundle(target)
        with patch.object(store, "configure_store"), patch.object(store, "run_capture", return_value=json.dumps(
            application(pending={"id": "active"}),
        )) as run:
            with self.assertRaisesRegex(store.ReleaseError, "pending submission"):
                store.publish("v0.1.2", self.directory)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0][1:3], ["apps", "get"])

    def test_publish_rechecks_and_submits_one_bundle(self):
        target = self.directory / "bundle"
        target.mkdir()
        path = bundle(target)
        responses = [json.dumps(application()), json.dumps({"applicationPackages": [{"version": "0.1.1.0"}]}),
                     "", json.dumps(application(pending={"id": "new"}))]
        with patch.object(store, "configure_store"), patch.object(store, "run_capture", side_effect=responses) as run, redirect_stdout(io.StringIO()):
            store.publish("v0.1.2", self.directory)
        command = run.call_args_list[2].args[0]
        self.assertEqual(command[:3], ["msstore", "publish", str(path)])
        self.assertIn(IDENTITY["store_id"], command)

    def test_cli_errors_never_expose_secrets_or_raw_output(self):
        command = ["msstore", "reconfigure", "--clientSecret", "private-test-secret"]
        failures = (
            subprocess.CompletedProcess(command, 1, "private-test-secret", "private-test-secret"),
            subprocess.TimeoutExpired(command, 1, output="private-test-secret"),
        )
        for failure in failures:
            kwargs = {"side_effect": failure} if isinstance(failure, Exception) else {"return_value": failure}
            with patch.object(store.subprocess, "run", **kwargs):
                with self.assertRaises(store.ReleaseError) as error:
                    store.run_capture(command, "Store configuration")
            self.assertNotIn("private-test-secret", str(error.exception))


if __name__ == "__main__":
    unittest.main()

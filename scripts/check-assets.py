"""Verify installer assets and Store identity without starting a GUI."""
from pathlib import Path
import json
import struct
import xml.etree.ElementTree as ET
root = Path(__file__).resolve().parents[1]
for name, size in [("StoreLogo.png", 50), ("Square44x44Logo.png", 44), ("Square150x150Logo.png", 150)]:
    data = (root / "packaging/assets" / name).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    assert struct.unpack(">II", data[16:24]) == (size, size)
identity = json.loads((root / "packaging/windows/identity.json").read_text())
assert identity["name"] == "Gofor.auJitter"
assert identity["store_id"] == "9NDKBXRLSXDT"
ET.parse(root / "packaging/windows/AppxManifest.xml.in")
print("Installer assets and identity verified")

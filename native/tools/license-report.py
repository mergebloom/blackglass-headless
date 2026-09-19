"""Emit a dependency inventory and bundled license texts from Cargo metadata."""

import hashlib
import json
from pathlib import Path
import sys

if len(sys.argv) != 2:
    raise SystemExit("usage: license-report.py THIRD-PARTY-LICENSES.txt")

metadata = json.load(sys.stdin)
packages = []
notices = []
for package in sorted(metadata["packages"], key=lambda item: (item["name"], item["version"])):
    if package["name"] == "blackglass-headless-native":
        continue
    license_expression = package.get("license")
    if not license_expression:
        raise SystemExit(f"missing license metadata: {package['name']} {package['version']}")
    root = Path(package["manifest_path"]).parent
    files = sorted(
        path for path in root.iterdir()
        if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE"))
    )
    if not files:
        raise SystemExit(f"missing license notice file: {package['name']} {package['version']}")
    listed = []
    for path in files:
        content = path.read_bytes()
        listed.append({"file": path.name, "sha256": hashlib.sha256(content).hexdigest()})
        notices.append((package["name"], package["version"], path.name, content))
    packages.append(
        {
            "name": package["name"],
            "version": package["version"],
            "license": license_expression,
            "source": package.get("source"),
            "license_files": listed,
        }
    )

with open(sys.argv[1], "wb") as output:
    for name, version, filename, content in notices:
        output.write(f"\n===== {name} {version} / {filename} =====\n".encode())
        output.write(content)
        output.write(b"\n")

print(json.dumps({"dependencies": packages}, indent=2))

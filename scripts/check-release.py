import sys
import tomllib
from pathlib import Path
version = tomllib.loads((Path(__file__).resolve().parents[1] / "Cargo.toml").read_text())["package"]["version"]
if sys.argv[1] != "v" + version:
    raise SystemExit("Release tag must match Cargo.toml version")

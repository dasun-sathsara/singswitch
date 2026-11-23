#!/usr/bin/env python3
"""Fail when the app draws an icon that is not in the compiled asset source.

A missing SVG does not fail the build: `Icon` renders nothing and the button
silently loses its glyph. This walks every `IconName::` use, resolves it through
the full Lucide catalog, and checks the path is reachable from either
`gpui_kit::assets::Assets` (the default bundle) or the `DomainIcons` list in
`crates/singswitch-app/src/assets.rs`.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
APP = ROOT / "crates" / "singswitch-app" / "src"
ASSETS_RS = APP / "assets.rs"


def registry_icon_dir() -> pathlib.Path:
    candidates = sorted(
        (pathlib.Path.home() / ".cargo" / "registry" / "src").glob(
            "*/gpui-kit-assets-*/assets/icons"
        )
    )
    if not candidates:
        sys.exit("icon check: no gpui-kit-assets crate in the cargo registry")
    return candidates[-1]


def catalog_paths(icon_dir: pathlib.Path) -> dict[str, str]:
    """Map IconName variant to asset path using the same rule as the build script."""
    paths = {}
    for svg in icon_dir.glob("*.svg"):
        stem = svg.stem
        variant = "".join(
            part[:1].upper() + part[1:].lower()
            for part in re.split(r"[-_.]", stem)
            if part
        )
        paths[variant] = f"icons/{stem}.svg"
    return paths


def bundled_by_default(icon_dir: pathlib.Path) -> set[str]:
    listing = icon_dir.parent.parent / "default-icons.txt"
    if not listing.exists():
        return set()
    return {line.strip() for line in listing.read_text().split() if line.strip()}


def declared_domain_icons() -> set[str]:
    source = ASSETS_RS.read_text()
    match = re.search(r"icon_assets!\(\s*DomainIcons,\s*\[(.*?)\]\s*\);", source, re.S)
    if not match:
        sys.exit("icon check: could not find the DomainIcons list in assets.rs")
    return {name.strip() for name in match.group(1).split(",") if name.strip()}


def used_icons() -> set[str]:
    used = set()
    for source in APP.rglob("*.rs"):
        used |= set(re.findall(r"IconName::(\w+)", source.read_text()))
    return used


def main() -> int:
    icon_dir = registry_icon_dir()
    catalog = catalog_paths(icon_dir)
    default_paths = bundled_by_default(icon_dir)
    domain = declared_domain_icons()
    used = used_icons()

    unresolved = []
    for name in sorted(used):
        path = catalog.get(name)
        if path is None:
            unresolved.append((name, "not a Lucide icon name"))
        elif path not in default_paths and name not in domain:
            unresolved.append((name, f"{path} is neither bundled nor in DomainIcons"))

    # The reverse direction matters too: anything listed but unused is dead weight.
    unused = sorted(domain - used)

    if unresolved:
        print("icon check: these icons would render as blank space:", file=sys.stderr)
        for name, why in unresolved:
            print(f"  {name}: {why}", file=sys.stderr)
        return 1

    if unused:
        print("icon check: DomainIcons lists icons nothing draws:", file=sys.stderr)
        for name in unused:
            print(f"  {name}", file=sys.stderr)
        return 1

    print(f"icon check: {len(used)} icons used, {len(domain)} added to the bundle")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

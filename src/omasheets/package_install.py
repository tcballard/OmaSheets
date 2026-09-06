"""Identify installations whose files belong to the system package manager."""

from pathlib import Path


def is_package_managed() -> bool:
    return (Path(__file__).resolve().parents[2] / "package-manager").is_file()


UPDATE_HELP = (
    "OmaSheets is managed by Pacman. Close OmaSheets before updating. "
    "If installed from the AUR, use Omarchy Update or yay -Syu. "
    "For a downloaded package, install the newer .pkg.tar.zst with sudo pacman -U. "
    "Downloads: https://github.com/tcballard/OmaSheets/releases"
)

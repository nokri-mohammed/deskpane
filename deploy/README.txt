deskpane for Raspberry Pi (64-bit Raspberry Pi OS Bookworm or later)

1. Check the download:  sha256sum -c --ignore-missing SHA256SUMS
2. Extract it:          tar xzf deskpane-pi-aarch64.tar.gz
3. Install:             cd deskpane-*-pi-aarch64 && sudo ./install.sh

Run ./install.sh --help for options (headless install, device id, label).
Running a newer bundle's install.sh upgrades in place. ./uninstall.sh removes it.

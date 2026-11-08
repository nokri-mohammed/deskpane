# Install guide

This guide takes you from a new Raspberry Pi to a working dashboard. It also shows how to send
stats from your other computers to it.

You do not need to build anything. Every file comes from the project's GitHub release page.

## Quick install

1. Flash **Raspberry Pi OS Lite (64-bit)** to the SD card, with WiFi and SSH set up, and fit
   the screen. [Prepare the SD card](#prepare-the-sd-card) shows how.
2. Log in to the Pi and run this one line:

   ```bash
   curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash
   ```

3. Done. On the first install the Pi restarts once, 10 seconds after the script finishes
   (Ctrl+C cancels, `--no-reboot` skips). Upgrades do not restart it. After the restart the
   dashboard comes up on the screen.

This runs a script from the project's `main` branch as root. It downloads the release, checks it
against the release's `SHA256SUMS` and stops on any mismatch. That check catches a broken or
changed download. It does not prove who made the release; signed releases are planned. If you
would rather look first: `curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh -o get.sh`, read it, then `sudo bash get.sh`. The
manual install below does the same steps by hand.

The script finds the newest release (test versions included). It removes an old
`pi-homelab-dashboard` install if it finds one, runs the bundle's `install.sh`, and prints the
line to run on your other computers. To change what it does, add options after `bash -s --`:

```bash
curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash -s -- --no-reboot --label "Pi Zero"
```

| Option | What it does |
|---|---|
| `--version vX.Y.Z` | Install this version instead of the newest one. |
| `--no-reboot` | Do not restart the Pi at the end. Restart it yourself before you use the screen. |
| `--no-display`, `--no-reporter`, `--device-id ID`, `--label TEXT` | Passed to the installer. See [Installer options](#installer-options). |
| `--reporter-only` | Install only the `deskpane` program, on another Linux computer. See [Linux](#linux). |
| `--daemon ADDRESS` | With `--reporter-only`: report to the Pi at this address at every start. |

To upgrade later, run the same line again. Your settings stay the same.

The rest of this guide is the manual way, step by step.

In this guide:

- `192.168.1.50` is the Pi's IP address. Use your own.
- `pi-zero` is the Pi's hostname. Use your own.
- `my-pc` is one of your other computers.

## Contents

1. [What you need](#what-you-need)
2. [Prepare the SD card](#prepare-the-sd-card)
3. [Manual install](#manual-install)
4. [Check that it works](#check-that-it-works)
5. [Send stats from your other computers](#send-stats-from-your-other-computers)
6. [Show your AI agents on the screen](#show-your-ai-agents-on-the-screen)
7. [Change the settings](#change-the-settings)
8. [Upgrade](#upgrade)
9. [Roll back to the previous version](#roll-back-to-the-previous-version)
10. [Uninstall](#uninstall)
11. [Troubleshooting](#troubleshooting)

## What you need

Hardware:

- A [Raspberry Pi Zero 2 W](https://www.raspberrypi.com/products/raspberry-pi-zero-2-w/). Another Pi that runs 64-bit Raspberry Pi OS (Pi 3, 4 or 5) can
  also work, but the project is tested on the Pi Zero 2 W only.
- An Elegoo 3.5" SPI touchscreen. This is a clone of the common "LCD35" design (ILI9486
  display chip, XPT2046 touch chip). The [Elegoo manual](https://www.elegoo.com/en-gb/blogs/arduino-projects/elegoo-3-5-inch-touch-screen-for-raspberry-pi-manual) has the
  vendor's own setup notes. See [HARDWARE.md](HARDWARE.md) for the pins.
- A microSD card.
- A [power supply](https://thepihut.com/collections/raspberry-pi-power-supplies) for your Pi (5 V micro USB for the Pi Zero 2 W).
- A pin header on the Pi. Some Pi Zero 2 W boards come without one.

Software and network:

- Raspberry Pi OS Lite (64-bit), Bookworm or later. The 32-bit version does not work.
- [Raspberry Pi Imager](https://www.raspberrypi.com/software/) on your computer.
- A WiFi network that the Pi and your other computers share.
- Internet access on the Pi during the install. The installer downloads system packages and
  the display driver file.

**Note:** the display driver file (`tft35a.dtbo`) comes from the screen vendor's project,
[goodtft/LCD-show](https://github.com/goodtft/LCD-show). That project has no license, so we
are not allowed to ship the file. The installer downloads it from one fixed version of that
project. Then it checks the file's SHA-256 hash before it uses it.

## Prepare the SD card

1. Turn off the Pi.
2. Fit the touchscreen onto the Pi's pin header. The screen has a 26-pin socket. It goes on
   pins 1 to 26, at the end of the header that has pin 1. Pin 1 has a square solder pad.
3. Put the microSD card in your computer.
4. Open Raspberry Pi Imager.
5. Choose your Pi model.
6. Choose the operating system: **Raspberry Pi OS (other)**, then **Raspberry Pi OS Lite
   (64-bit)**.
7. Choose your microSD card.
8. Open the customization settings when Imager asks. Set these:
   - **Hostname:** for example `pi-zero`.
   - **User name and password:** pick your own. You need them to log in.
   - **WiFi:** your network name, password and country.
   - **SSH:** turn it on, with password login or your public key.
9. Write the card.
10. Put the card in the Pi.
11. Connect the power. The first start takes a few minutes.

**Note:** do not turn on SPI, and do not install a screen driver by hand. The installer adds
everything the screen needs. Do not run the `LCD35-show` script from the vendor. It is made for
older systems and changes many settings ([HARDWARE.md](HARDWARE.md) explains why).

## Manual install

These steps do the same as the one line in [Quick install](#quick-install), by hand.

1. Log in to the Pi from your computer:

   ```bash
   ssh <your-user>@pi-zero.local
   ```

   If `pi-zero.local` does not work, use the Pi's IP address. Your router shows it.

2. Make a new, empty folder and go into it:

   ```bash
   mkdir ~/deskpane && cd ~/deskpane
   ```

3. Set the version you want to install:

   ```bash
   VERSION=v0.1.0-beta.4 # x-release-please-version
   ```

   This is the newest release when this guide was written. All releases are listed on the
   [releases page](https://github.com/nokri-mohammed/deskpane/releases).

   > **Beta versions.** A version like `v0.1.0-beta.1` is a test version: it is marked
   > "Pre-release" on GitHub. Links that use `releases/latest` skip pre-releases, so they
   > return `404` until a stable version is out. The commands in this guide put the version
   > in the link instead, so they work for betas and stable versions alike.

4. Download the Pi bundle and the checksum file:

   ```bash
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/download/$VERSION/deskpane-pi-aarch64.tar.gz
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/download/$VERSION/SHA256SUMS
   ```

5. Check the download:

   ```bash
   sha256sum -c --ignore-missing SHA256SUMS
   ```

   You must see `deskpane-pi-aarch64.tar.gz: OK`. If you see `FAILED`, delete the
   file and download it again. Do not install a file that fails this check.

6. Extract the bundle:

   ```bash
   tar xzf deskpane-pi-aarch64.tar.gz
   ```

7. Go into the new folder. Its name contains the version, for example
   `deskpane-0.1.0-beta.1-pi-aarch64`:

   ```bash
   cd deskpane-*-pi-aarch64
   ```

8. Run the installer:

   ```bash
   sudo ./install.sh
   ```

   To give the Pi's own entry on the dashboard a different name, add options. For example:

   ```bash
   sudo ./install.sh --device-id pi-zero --label "Pi Zero"
   ```

9. Read the summary at the end. It shows the command to run on your other computers.
10. Restart the Pi when the installer tells you to:

    ```bash
    sudo systemctl reboot
    ```

On the first install with the touchscreen, the installer always asks for a restart. The screen
only starts to work after the restart.

### Installer options

These are all the options `install.sh` accepts. Run `./install.sh --help` to see them on the Pi.

| Option | What it does |
|---|---|
| `--no-display` | Install without the touchscreen. No display driver, no boot changes, no dashboard. |
| `--no-reporter` | Do not show the Pi's own stats on the dashboard. |
| `--device-id ID` | The id of the Pi's own entry. Use 1 to 64 letters, digits, dots, dashes or underscores, starting with a letter or digit. First install only. Default: automatic (see below). |
| `--label TEXT` | The name shown for the Pi on the dashboard. Use up to 64 letters, digits, spaces and `. _ ( ) -`. First install only. Default: the hostname. |
| `--overlay-file FILE` | Use a display driver file that you already downloaded. The installer still checks its hash. |
| `--no-summary` | Do not print the summary at the end. The one-line install uses this and prints its own. |
| `-h`, `--help` | Show the help. |

If you do not give `--device-id`, the Pi gets an automatic id made from its machine id, for
example `dp-3f9a0c41`. It stays the same across restarts and upgrades. Without `--label`, the
label is the hostname. Pis installed before 0.1.0-beta.3 keep their old settings (`pi-zero`).

At the end, the installer sends one test report as the Pi and prints the id it reports as. If a
computer on your network already uses that id, it prints an error and the fix, and the install
still finishes.

### What the installer does

You do not need to do any of this by hand. The list is here so you know what changes on your Pi.

1. It checks that it runs as root, on 64-bit Raspberry Pi OS, with `/boot/firmware/config.txt`
   present.
2. It removes an old install from before the rename (`homelab-*.service` units from
   `pi-homelab-dashboard`): it stops and deletes those units, and deletes the old program files
   named in them. It stops with the process name if another program still uses UDP port 8765.
3. It downloads the display driver file and checks its hash. If the hash is wrong, it stops
   before it changes anything.
4. It installs `python3-pygame` and `python3-evdev` with `apt-get`.
5. It creates a system user `deskpane`. This user cannot log in. The services run as this
   user.
6. It copies the program to `/opt/deskpane/releases/<version>/`. The link
   `/opt/deskpane/current` points to the active version.
7. On the first install only, it writes the settings file `/etc/deskpane/env`.
8. It copies the display driver file to `/boot/firmware/overlays/tft35a.dtbo`.
9. It adds a `dtoverlay=tft35a:rotate=0` block to `/boot/firmware/config.txt`.
10. It adds `fbcon=map:1` to `/boot/firmware/cmdline.txt`. This keeps the text console off the
   touchscreen.
11. Before its first change to a boot file, it saves a copy as `<file>.deskpane.bak`.
12. It installs, turns on and starts the systemd services, then sends one test report as the
    Pi (`deskpane report --once`) and prints the id it reports as.
13. It keeps the current and the previous version in `/opt/deskpane/releases/`, and
    deletes older ones.

With `--no-display`, the installer skips steps 3, 4, 8, 9 and 10.

## Check that it works

After the restart, the screen shows the overview page. It shows the time, the date and how
many devices are online. Tap the screen to go to the next page. Each device has its own page.

To check the services, log in to the Pi again and follow these steps.

1. Show the state of the four services:

   ```bash
   systemctl status deskpane-daemon deskpane-dashboard deskpane-reporter deskpane-unbind-fbcon
   ```

   `deskpane-daemon`, `deskpane-dashboard` and `deskpane-reporter` must show
   `active (running)`. `deskpane-unbind-fbcon` must show `active (exited)`. That is correct for a
   service that runs once at start-up.

2. Check that the daemon made its socket. The dashboard reads the stats from this file:

   ```bash
   sudo ls -l /run/deskpane/
   ```

   You must see `daemon.sock`. You need `sudo` because only the `deskpane` user can open
   this folder.

3. Look at the daemon log:

   ```bash
   journalctl -u deskpane-daemon -b --no-pager
   ```

   You must see `udp: listening on 0.0.0.0:8765`.

The services that run depend on the installer options:

| Service | What it does | Skipped with |
|---|---|---|
| `deskpane-daemon` | Receives the reports on UDP port 8765 and keeps the latest one for each device. | (always installed) |
| `deskpane-dashboard` | Draws the pages on the touchscreen. | `--no-display` |
| `deskpane-unbind-fbcon` | Keeps the text console off the touchscreen. | `--no-display` |
| `deskpane-reporter` | Sends the Pi's own stats to the daemon. | `--no-reporter` |

## Send stats from your other computers

Each computer that you want on the dashboard runs the `deskpane` program. The reporter sends
one small message to the Pi every few seconds.

Each computer needs its own device id. By default `deskpane` makes one from the computer's
machine id, for example `dp-3f9a0c41`, so you do not have to pick one. If two computers still
end up with the same id, the Pi keeps the first one and refuses the second. The second computer
prints an error that names the first one's IP address. If the clash goes on, it pauses for 5
minutes at a time and then tries again, until you fix the id.

The `deskpane` program has one command, `report`, with these options:

| Option | What it is |
|---|---|
| `--daemon ADDRESS` | The Pi's IP address and port, for example `192.168.1.50:8765`. Required. |
| `--device-id ID` | A fixed, unique id for this computer, for example `my-pc`: 1 to 64 letters, digits, dots, dashes or underscores, starting with a letter or digit. Optional; the default is made from the machine id. |
| `--label TEXT` | The name shown on the dashboard, for example `"My PC"`. Optional; the default is the hostname. |
| `--interval SECONDS` | Seconds between two reports. The default is 4. |
| `--once` | Send one report, wait up to 3 seconds for the Pi's answer, and exit: 0 when the Pi stored it, 10 when the id is in use, 12 when there is no answer. Good for checking a setup. |

The daemon drops a report if the label is empty or longer than 64 bytes, or if the id breaks
the rule above. Keep them short.

### Linux

The Linux reporter is one file with no dependencies. It runs on any 64-bit Linux system.

The quick way is one line. It installs `deskpane` to `/usr/local/bin` and adds a systemd
service that reports to the Pi at every start (use your Pi's IP address):

```bash
curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash -s -- --reporter-only --daemon 192.168.1.50:8765
```

The device id is automatic (made from the machine id) and the label is the hostname. Add
`--device-id my-pc --label "My PC"` to choose your own. Before it turns the service on, the
line sends one test report (`deskpane report --once`). If the Pi says the id is in use, it stops
before it changes the settings or the service, and tells you to pick another one with
`--device-id`. If the Pi does not answer (it is off, or runs a version older than
0.1.0-beta.3), it warns and installs anyway. Running the line again keeps the id and label in
`/etc/deskpane/reporter.env` unless you pass new ones. Without `--daemon`, the line only
installs the program.

The manual way:

1. Find your processor type:

   ```bash
   uname -m
   ```

   `x86_64` means you need `deskpane-linux-x86_64.tar.gz`. `aarch64` means you need
   `deskpane-linux-aarch64.tar.gz`. There is no download for 32-bit systems.

2. Make a new, empty folder and go into it:

   ```bash
   mkdir ~/deskpane && cd ~/deskpane
   ```

3. Download the reporter and the checksum file. This example is for `x86_64`. Set
   `VERSION` the same way as on the Pi (see the beta note there):

   ```bash
   VERSION=v0.1.0-beta.4 # x-release-please-version
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/download/$VERSION/deskpane-linux-x86_64.tar.gz
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/download/$VERSION/SHA256SUMS
   ```

4. Check the download. You must see `OK`:

   ```bash
   sha256sum -c --ignore-missing SHA256SUMS
   ```

5. Extract it:

   ```bash
   tar xzf deskpane-linux-x86_64.tar.gz
   ```

6. Copy the program to `/usr/local/bin`:

   ```bash
   sudo install -m 0755 deskpane-*-linux-x86_64/deskpane /usr/local/bin/deskpane
   ```

7. Start it. Use your Pi's IP address:

   ```bash
   deskpane report --daemon 192.168.1.50:8765
   ```

   It prints `deskpane: reporting to 192.168.1.50:8765 as dp-3f9a0c41 (automatic id, label "my-pc") every 4s`.
   Your computer shows up on the dashboard within a few seconds. Add
   `--device-id my-pc --label "My PC"` to choose the id and the name yourself.

8. Press `Ctrl+C` to stop it.

#### Start the reporter when the computer starts (optional)

The one-line install with `--daemon` (above) sets this up for you. To add a systemd service by hand instead:

1. Create the file `/etc/systemd/system/deskpane-reporter.service` with this text. Change the
   address, id and label:

   ```ini
   [Unit]
   Description=deskpane reporter
   After=network-online.target
   Wants=network-online.target

   [Service]
   Type=simple
   # a throwaway user with no rights; the reporter only reads system stats
   DynamicUser=yes
   ExecStart=/usr/local/bin/deskpane report --daemon 192.168.1.50:8765
   Restart=always
   RestartSec=5

   [Install]
   WantedBy=multi-user.target
   ```

2. Tell systemd about the new file:

   ```bash
   sudo systemctl daemon-reload
   ```

3. Turn on the service and start it:

   ```bash
   sudo systemctl enable --now deskpane-reporter
   ```

4. Check it:

   ```bash
   systemctl status deskpane-reporter
   ```

### Windows

1. Download `deskpane-windows-x86_64.zip` and `SHA256SUMS` from the
   [releases page](https://github.com/nokri-mohammed/deskpane/releases). Pick the newest release, including one marked "Pre-release"
   if there is no stable version yet.
2. Open PowerShell in your Downloads folder.
3. Show the hash of the zip file:

   ```powershell
   Get-FileHash .\deskpane-windows-x86_64.zip -Algorithm SHA256
   ```

4. Open `SHA256SUMS` in Notepad. Find the line for `deskpane-windows-x86_64.zip`. The hash on
   that line must be the same as the one PowerShell shows (PowerShell uses capital letters,
   which is fine).
5. Right-click the zip file and choose **Extract All**.
6. Open PowerShell in the extracted folder. Its name is like
   `deskpane-0.1.0-windows-x86_64`.
7. Start the reporter. Use your Pi's IP address:

   ```powershell
   .\deskpane.exe report --daemon 192.168.1.50:8765
   ```

8. Keep the window open. The reporter stops when you close it.

**Note:** Windows can warn that the program comes from an unknown publisher, because the file
is not signed. Check the hash as shown above before you allow it.

**Note:** Windows often does not give the CPU temperature to programs. The dashboard then
shows `N/A` for the temperature.

To start the reporter when you log in, use Task Scheduler:

1. Open Task Scheduler.
2. Choose **Create Basic Task**.
3. Set the trigger to **When I log on**.
4. Set the action to **Start a program**.
5. In **Program/script**, select `deskpane.exe`.
6. In **Add arguments**, type `report --daemon 192.168.1.50:8765`.
7. Finish the task.

A reporter that runs as a Windows service is planned for a later version.

## Show your AI agents on the screen

From 0.2.0-beta.1 the screen can show your Claude Code sessions: one row for each session, with
the project name, what it does and a big amber "NEEDS YOU" when it waits for you. Each session
sends a small status packet from a Claude Code hook. The packet holds the project folder name
(not the path), a tool name and a state word. It never holds your prompts, files or commands.
[SECURITY.md](../SECURITY.md#agent-status-packets) lists what is sent and what is not.

**Upgrade the Pi first.** An older daemon cannot read agent packets. It drops each one and
writes one line to its log. Nothing breaks, but nothing shows. See [Upgrade](#upgrade).

On each computer where you run Claude Code, you need the `deskpane` program of the same
version. Then pick one of two ways.

### Look at the settings first

This prints the hooks block and changes nothing. Use your Pi's IP address:

```bash
deskpane agent hooks-snippet --daemon 192.168.1.50:8765
```

Merge the block into `~/.claude/settings.json` by hand if you like.

### Let deskpane edit the settings

```bash
deskpane agent install-hooks --daemon 192.168.1.50:8765
```

This adds one hook handler to the events it needs and keeps every other entry. It first copies
your settings file to `settings.json.deskpane-backup-<time>` next to it (mode 0600, and it never
overwrites an older backup). It stops with an error if the file is not valid JSON. Run it twice
and nothing changes the second time. To look first, add `--dry-run`: it prints the result and
writes nothing.

Options of `install-hooks` and `hooks-snippet`:

| Option | What it is |
|---|---|
| `--daemon ADDRESS` | The Pi's IP address and port. Required. |
| `--device-id ID` | The id of this computer. Optional; the default is the same id that `deskpane report` makes. It is written into the hook, so the hook does not need the machine id later. |
| `--project NAME` | Show this name instead of the project folder name. |
| `--hide-project` | Send no project name. The screen shows `?`. |
| `--settings FILE` | `install-hooks` and `uninstall-hooks` only: edit this file instead of `~/.claude/settings.json`. |
| `--dry-run` | `install-hooks` and `uninstall-hooks` only: print the result, write nothing. |

The hook stores the absolute path of the `deskpane` program that you ran. If that program is in
a build folder (`target`) or a temp folder, the command prints a warning, because the path may
disappear. Install `deskpane` first (for example in `/usr/local/bin`) and run `install-hooks`
from there.

### Check it

```bash
deskpane agent test --daemon 192.168.1.50:8765
```

This sends a fake session through every state in about 10 seconds. Watch the screen: a row
appears, works, waits for you, fails and ends. The Pi sends no answer, so the screen is the
check. A session that is already open may need to be restarted before it picks up new hooks.

### Remove it

```bash
deskpane agent uninstall-hooks
```

This removes only the deskpane handlers, after another backup, and keeps the rest of your
settings. Add `--dry-run` to look first.

### Linux, Windows and WSL

| Where Claude Code runs | Settings file | Command |
|---|---|---|
| Linux | `~/.claude/settings.json` | `deskpane agent install-hooks --daemon 192.168.1.50:8765` |
| Windows | `%USERPROFILE%\.claude\settings.json` | `.\deskpane.exe agent install-hooks --daemon 192.168.1.50:8765` in PowerShell |
| WSL | the Linux file inside WSL | the Linux `deskpane`, run inside WSL |

Windows and WSL are separate Claude Code installs with their own settings, so set up each one.
The WSL machine has another id than the Windows machine, so the screen shows the id (for
example `dp-3f9a0c41`) instead of a label unless a reporter runs there with the same id. Run
`deskpane report` in WSL as well if you want the label.

The hook always exits 0, writes nothing to stdout and gives up after 500 ms, so it cannot block
or slow down Claude Code. If the Pi is off, the hook just sends into the void.

### Character packs (optional)

The agents page can draw a small pixel character for each project. This needs a character pack
that you own: a zip in the format of [CHARACTERS.md](CHARACTERS.md). The project does not ship a
pack. Without one, the page shows built-in icons and works the same.

Copy the zip to the Pi (for example with `scp my-pack.zip my-user@pi-zero:`), then use one of:

```bash
# with the one-line installer, on the Pi
curl -fsSL https://raw.githubusercontent.com/nokri-mohammed/deskpane/main/get.sh | sudo bash -s -- --characters ~/my-pack.zip

# with an extracted bundle, on the Pi
sudo ./install.sh --characters ~/my-pack.zip
```

`get.sh` checks the pack before it installs anything, and stops with a message if the bundle is
too old for the flag. `install.sh` checks it again. The zip must be a normal file (not a
symlink) of at most 4 MiB. It is checked and copied file by file, never unzipped. If the check
refuses the pack, the installer stops and changes nothing. If the copy itself fails after
that check, the installer prints a warning and goes on with the built-in icons (an older
installed pack stays as it was). The pack goes to
`/var/lib/deskpane/characters`, and the dashboard restarts to load it.

- An upgrade without `--characters` keeps the installed pack.
- `sudo ./install.sh --remove-characters` removes it (the icons come back).
- `sudo ./uninstall.sh` keeps it; `sudo ./uninstall.sh --purge` removes it.
- `--characters` cannot be used with `--reporter-only` or `--no-display`.

## Change the settings

The settings are in `/etc/deskpane/env` on the Pi. An upgrade never changes this
file.

| Setting | What it does | Default |
|---|---|---|
| `DEVICE_ID` | The id of the Pi's own entry. Empty means automatic. | empty (automatic, `pi-zero` on Pis installed before 0.1.0-beta.3) |
| `LABEL` | The name shown for the Pi. Empty means the hostname. | empty (hostname) |
| `LISTEN_ADDR` | The address and port where the daemon receives reports. | `0.0.0.0:8765` |

1. Open the file:

   ```bash
   sudo nano /etc/deskpane/env
   ```

2. Change the values and save the file.
3. Restart the services:

   ```bash
   sudo systemctl restart deskpane-daemon deskpane-reporter
   ```

**Caution:** the Pi's own reporter always sends to port 8765. If you change the port in
`LISTEN_ADDR`, the Pi's own stats stop showing. Keep port 8765 unless you have a reason to
change it.

## Upgrade

To upgrade, run the `install.sh` of a newer bundle. Your settings stay the same.

1. Make a new, empty folder for the new version, and go into it:

   ```bash
   mkdir ~/deskpane-new && cd ~/deskpane-new
   ```

2. Download, check and extract the new bundle. Use steps 3 to 7 of
   [Manual install](#manual-install).
3. Run the installer with the same options that you used the first time:

   ```bash
   sudo ./install.sh
   ```

   For example, if you installed with `--no-reporter`, give `--no-reporter` again. If you do
   not, the installer turns on the Pi's own reporter.

Upgrade the Pi before you set up agent hooks on your computers (see
[Show your AI agents on the screen](#show-your-ai-agents-on-the-screen)). An installed character
pack stays in place.

The installer restarts the services with the new version. `--device-id` and `--label` have no
effect on an upgrade. To change them, edit the settings file (see
[Change the settings](#change-the-settings)).

**Note:** use a new folder for each version. If two extracted bundles are in the same folder,
`cd deskpane-*-pi-aarch64` can go into the wrong one.

## Roll back to the previous version

The installer keeps the previous version. After an upgrade, its summary prints the exact
rollback command.

1. List the versions on the Pi:

   ```bash
   ls /opt/deskpane/releases
   ```

2. Point `current` at the older version. Replace `0.1.0` with the version that you want:

   ```bash
   sudo ln -sfn releases/0.1.0 /opt/deskpane/current
   ```

3. Restart the services:

   ```bash
   sudo systemctl restart deskpane-daemon deskpane-dashboard deskpane-reporter
   ```

You can also run the `install.sh` of the older bundle. That works the same as an upgrade.

## Uninstall

The uninstaller is in the extracted bundle folder. It is not copied to `/opt`. If you deleted
the folder, download and extract the bundle again.

1. Go into the extracted bundle folder:

   ```bash
   cd ~/deskpane/deskpane-*-pi-aarch64
   ```

2. Run the uninstaller:

   ```bash
   sudo ./uninstall.sh
   ```

   This stops and removes the services. It deletes `/opt/deskpane`. It keeps
   your settings, the `deskpane` user, the display driver file and the boot settings. A
   later install continues from there.

3. To remove everything, run it with `--purge` instead:

   ```bash
   sudo ./uninstall.sh --purge
   ```

   This also removes `/etc/deskpane`, the `deskpane` user and the display driver
   file. It removes the installer's lines from `config.txt` and `cmdline.txt`.

4. Restart the Pi if the uninstaller tells you to:

   ```bash
   sudo systemctl reboot
   ```

`--purge` does not remove `python3-pygame` or `python3-evdev`. It does not delete the backup
copies `/boot/firmware/*.deskpane.bak`.

To remove the reporter from a Linux computer, stop your service if you made one. Then delete
`/usr/local/bin/deskpane`. On Windows, delete the folder and the Task Scheduler task.

## Troubleshooting

To see the logs of a service, use `journalctl`. Add `-f` to follow new lines as they come.

```bash
journalctl -u deskpane-daemon -b --no-pager       # daemon, since the last start
journalctl -u deskpane-dashboard -b --no-pager    # touchscreen program
journalctl -u deskpane-reporter -b --no-pager     # the Pi's own reporter
journalctl -u deskpane-daemon -f                  # daemon, live
```

### The installer stops with an error

| Message | What to do |
|---|---|
| `run as root` | Run it with `sudo`. |
| `this bundle is for 64-bit Raspberry Pi OS (aarch64)` | Flash Raspberry Pi OS Lite (64-bit). The 32-bit version does not work. |
| `/boot/firmware/config.txt not found` | Use Raspberry Pi OS Bookworm or later. |
| `bundle is incomplete` | Download and extract the bundle again. |
| `curl is missing` | Run `sudo apt-get install curl`. |
| `downloaded overlay hash mismatch` | The display driver file was damaged or changed. Nothing on the Pi changed. Try again later. If it keeps failing, open an issue. |

### The screen stays blank or white

1. Make sure that you restarted the Pi after the first install.
2. Check the dashboard service:

   ```bash
   systemctl status deskpane-dashboard deskpane-unbind-fbcon
   ```

3. Read the dashboard log for errors:

   ```bash
   journalctl -u deskpane-dashboard -b --no-pager
   ```

4. Check that the boot settings are there:

   ```bash
   grep tft35a /boot/firmware/config.txt
   ls -l /boot/firmware/overlays/tft35a.dtbo
   ```

5. Check that the system found the screen. One line must say `fb_ili9486`:

   ```bash
   cat /sys/class/graphics/fb*/name
   ```

   If no line says `fb_ili9486`, turn off the Pi. Check that the screen sits on pins 1 to 26.

**The screen showed a "Network problem" page and then stopped.** A tap on that page closes the
dashboard on purpose. Start it again:

```bash
sudo systemctl start deskpane-dashboard
```

**The colours look pale or grey.** See [HARDWARE.md](HARDWARE.md). That file explains the
vendor display driver and the known symptoms.

### Text or a blinking cursor shows on the touchscreen

The installer keeps the text console off the touchscreen in two ways: `fbcon=map:1` in
`cmdline.txt`, and the `deskpane-unbind-fbcon` service. Check both:

```bash
grep fbcon /boot/firmware/cmdline.txt
systemctl status deskpane-unbind-fbcon
```

Because of this, the touchscreen never shows a login prompt. An HDMI monitor does not show one
either. [RECOVERY.md](RECOVERY.md) explains how to get a console when you need one, for example
when the WiFi does not work.

### Taps do nothing

The dashboard reads taps from `/dev/input/event0`. A USB keyboard or mouse that is connected at
start-up can take that name. Disconnect it and restart the Pi.

### The overview says "daemon not reachable"

The daemon is not running, or the dashboard cannot open its socket.

```bash
systemctl status deskpane-daemon
sudo ls -l /run/deskpane/
journalctl -u deskpane-daemon -b --no-pager
```

### A computer does not show on the dashboard

1. Check the reporter's output on that computer. It must print
   `deskpane: reporting to 192.168.1.50:8765 ...`. `udp: send failed` means the computer cannot
   send to that address. Run `deskpane report --once --daemon 192.168.1.50:8765` for a quick
   check: it prints `deskpane: reported to ...` when the Pi stored the report. `no answer`
   right after another check from the same computer is the Pi's limit of one answer per 10
   seconds: wait 10 seconds and run it again.
2. Check the IP address in `--daemon`. It must be the Pi's address, with `:8765` at the end.
3. Check the firewalls. The reporter sends UDP packets to port 8765 on the Pi. Raspberry Pi OS
   Lite has no firewall by default. If you turned one on (for example `ufw`), allow UDP port
   8765 from your computers only:

   ```bash
   sudo ufw allow from 192.168.1.0/24 to any port 8765 proto udp
   ```

4. Watch the daemon log on the Pi while the reporter runs:

   ```bash
   journalctl -u deskpane-daemon -f
   ```

   Look for these lines:

   | Log line | Meaning |
   |---|---|
   | `daemon: bad report from <address>: ...` | The packet was not a valid report. Check that the reporter and the Pi run the same version. |
   | `daemon: rejected report from <address>: <reason>` | The report broke a limit, for example a label longer than 64 bytes. The reason names the field. See [CONTRACT.md](CONTRACT.md) for the limits. |
   | `daemon: device table full, dropping report from <address>` | The daemon already holds 32 devices. It drops new devices until a slot is free. |
   | `daemon: refused device id <id> from <address>: in use by <owner>` | Another computer already uses that id. See the next section. |
   | `daemon: forgot <id> after 300 s of silence` | That device sent nothing for 5 minutes, so the daemon removed it. |

   If no line shows up for your computer, the packets do not reach the Pi. Check the network
   and the firewalls again.

A device that stops sending shows as offline after 15 seconds. The daemon removes it after 5
minutes. That frees its slot in the table of 32 devices.

### "device id ... is already used by ..."

The reporter prints:

```
deskpane: error: device id "my-pc" is already used by 192.168.1.60. Pick another one: --device-id <name>
```

If the clash lasts more than about 16 seconds, it prints the line below, sends nothing for 5
minutes, then tries again. It keeps doing that, one line per pause, until the clash is gone:

```
deskpane: error: device id "my-pc" is still used by 192.168.1.60. Pausing reports for 300 s, then trying again. Pick another one: --device-id <name>
```

Two computers report with the same id. The Pi keeps the one that reported first (at
192.168.1.60) and refuses the other. Give this computer its own id with `--device-id`, or in
`/etc/deskpane/reporter.env` (`DEVICE_ID=`), then restart it:
`sudo systemctl restart deskpane-reporter`. `(the Pi itself)` after the address means the Pi's
own reporter uses that id.

Automatic ids only clash when two machines share a machine id, for example a VM or an SD card
cloned after its first start. Either pass `--device-id <name>`, or give the clone a new machine
id: `sudo rm /etc/machine-id && sudo systemd-machine-id-setup`, then reboot.

`no machine id found ... Pass a name: --device-id <name>` (exit code 2) means the system has no
usable machine id. Pass `--device-id <name>`.

| Exit code | Program | Meaning |
|---|---|---|
| 2 | `deskpane` | Wrong options, or no machine id. |
| 10 | `deskpane --once` | The device id is in use by another computer. (The running reporter never exits for this; it pauses and retries.) |
| 12 | `deskpane --once` | No answer from the Pi: it is off, a firewall blocks UDP 8765, it runs a version older than 0.1.0-beta.3, or this computer already ran a check (or got a reply) in the last 10 seconds. The Pi answers each computer at most once per 10 seconds, so wait 10 seconds and try again. |
| 11 | `deskpane-daemon` | UDP port 8765 is in use by another program. systemd does not restart it. |

### The daemon stops with "UDP port 8765 is already in use"

```
deskpane-daemon: error: UDP port 8765 is already in use by another program. Find it with: sudo ss -ulpn 'sport = :8765'
```

Run that `ss` command to see which program holds the port, stop it, then start the daemon again:
`sudo systemctl restart deskpane-daemon`.

### The Pi's own stats do not show

```bash
systemctl status deskpane-reporter
cat /etc/deskpane/env
```

If you installed with `--no-reporter`, this service is not installed. Run `install.sh` again
without that option.

### The Pi does not join the WiFi

See [RECOVERY.md](RECOVERY.md).

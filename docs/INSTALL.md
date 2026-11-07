# Install guide

This guide takes you from a new Raspberry Pi to a working dashboard. It also shows how to send
stats from your other computers to it.

You do not need to build anything. Every file comes from the project's GitHub release page.

In this guide:

- `192.168.1.50` is the Pi's IP address. Use your own.
- `pi-zero` is the Pi's hostname. Use your own.
- `my-pc` is one of your other computers.

## Contents

1. [What you need](#what-you-need)
2. [Prepare the SD card](#prepare-the-sd-card)
3. [Install the dashboard on the Pi](#install-the-dashboard-on-the-pi)
4. [Check that it works](#check-that-it-works)
5. [Send stats from your other computers](#send-stats-from-your-other-computers)
6. [Change the settings](#change-the-settings)
7. [Upgrade](#upgrade)
8. [Roll back to the previous version](#roll-back-to-the-previous-version)
9. [Uninstall](#uninstall)
10. [Troubleshooting](#troubleshooting)

## What you need

Hardware:

- A Raspberry Pi Zero 2 W. Another Pi that runs 64-bit Raspberry Pi OS (Pi 3, 4 or 5) can
  also work, but the project is tested on the Pi Zero 2 W only.
- An Elegoo 3.5" SPI touchscreen. This is a clone of the common "LCD35" design (ILI9486
  display chip, XPT2046 touch chip). See [HARDWARE.md](HARDWARE.md) for the pins.
- A microSD card.
- A power supply for your Pi.
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

## Install the dashboard on the Pi

1. Log in to the Pi from your computer:

   ```bash
   ssh <your-user>@pi-zero.local
   ```

   If `pi-zero.local` does not work, use the Pi's IP address. Your router shows it.

2. Make a new, empty folder and go into it:

   ```bash
   mkdir ~/deskpane && cd ~/deskpane
   ```

3. Download the Pi bundle:

   ```bash
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/latest/download/deskpane-pi-aarch64.tar.gz
   ```

4. Download the checksum file:

   ```bash
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/latest/download/SHA256SUMS
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
   `deskpane-0.1.0-pi-aarch64`:

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
| `--device-id ID` | The id of the Pi's own entry. Use 1 to 64 letters, digits, dots, dashes or underscores. First install only. |
| `--label TEXT` | The name shown for the Pi on the dashboard. Use up to 64 letters, digits, spaces and `. _ ( ) -`. First install only. |
| `--overlay-file FILE` | Use a display driver file that you already downloaded. The installer still checks its hash. |
| `-h`, `--help` | Show the help. |

If you do not give `--device-id` or `--label`, the Pi shows up as `pi-zero` with the label
`Raspberry Pi`.

### What the installer does

You do not need to do any of this by hand. The list is here so you know what changes on your Pi.

1. It checks that it runs as root, on 64-bit Raspberry Pi OS, with `/boot/firmware/config.txt`
   present.
2. It downloads the display driver file and checks its hash. If the hash is wrong, it stops
   before it changes anything.
3. It installs `python3-pygame` and `python3-evdev` with `apt-get`.
4. It creates a system user `deskpane`. This user cannot log in. The services run as this
   user.
5. It copies the program to `/opt/deskpane/releases/<version>/`. The link
   `/opt/deskpane/current` points to the active version.
6. On the first install only, it writes the settings file `/etc/deskpane/env`.
7. It copies the display driver file to `/boot/firmware/overlays/tft35a.dtbo`.
8. It adds a `dtoverlay=tft35a:rotate=0` block to `/boot/firmware/config.txt`.
9. It adds `fbcon=map:1` to `/boot/firmware/cmdline.txt`. This keeps the text console off the
   touchscreen.
10. Before its first change to a boot file, it saves a copy as `<file>.deskpane.bak`.
11. It installs, turns on and starts the systemd services.
12. It keeps the current and the previous version in `/opt/deskpane/releases/`, and
    deletes older ones.

With `--no-display`, the installer skips steps 2, 3, 7, 8 and 9.

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

Each computer needs its own device id. If two computers use the same id, they overwrite each
other on the dashboard.

The `deskpane` program has one command, `report`, with these options:

| Option | What it is |
|---|---|
| `--daemon ADDRESS` | The Pi's IP address and port, for example `192.168.1.50:8765`. Required. |
| `--device-id ID` | A fixed, unique id for this computer, for example `my-pc`. Required. |
| `--label TEXT` | The name shown on the dashboard, for example `"My PC"`. Required. |
| `--interval SECONDS` | Seconds between two reports. The default is 4. |

The daemon drops a report if the id or the label is empty or longer than 64 bytes. Keep them
short.

### Linux

The Linux reporter is one file with no dependencies. It runs on any 64-bit Linux system.

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

3. Download the reporter and the checksum file. This example is for `x86_64`:

   ```bash
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/latest/download/deskpane-linux-x86_64.tar.gz
   curl -fLO https://github.com/nokri-mohammed/deskpane/releases/latest/download/SHA256SUMS
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
   deskpane report --daemon 192.168.1.50:8765 --device-id my-pc --label "My PC"
   ```

   It prints `reporter: pushing to 192.168.1.50:8765 as 'my-pc' every 4s`. Your computer shows
   up on the dashboard within a few seconds.

8. Press `Ctrl+C` to stop it.

#### Start the reporter when the computer starts (optional)

The reporter cannot install itself as a service yet. You can add a systemd service by hand.

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
   ExecStart=/usr/local/bin/deskpane report --daemon 192.168.1.50:8765 --device-id my-pc --label "My PC"
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

1. Download `deskpane-windows-x86_64.zip` and `SHA256SUMS` from
   `https://github.com/nokri-mohammed/deskpane/releases/latest`.
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
   .\deskpane.exe report --daemon 192.168.1.50:8765 --device-id my-pc --label "My PC"
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
6. In **Add arguments**, type `report --daemon 192.168.1.50:8765 --device-id my-pc --label "My PC"`.
7. Finish the task.

A reporter that runs as a Windows service is planned for a later version.

## Change the settings

The settings are in `/etc/deskpane/env` on the Pi. An upgrade never changes this
file.

| Setting | What it does | Default |
|---|---|---|
| `DEVICE_ID` | The id of the Pi's own entry. | `pi-zero` |
| `LABEL` | The name shown for the Pi. | `Raspberry Pi` |
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
   [Install the dashboard on the Pi](#install-the-dashboard-on-the-pi).
3. Run the installer with the same options that you used the first time:

   ```bash
   sudo ./install.sh
   ```

   For example, if you installed with `--no-reporter`, give `--no-reporter` again. If you do
   not, the installer turns on the Pi's own reporter.

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
   `reporter: pushing to 192.168.1.50:8765 ...`. `udp: send failed` means the computer cannot
   send to that address.
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
   | `daemon: forgot <id> after 300 s of silence` | That device sent nothing for 5 minutes, so the daemon removed it. |

   If no line shows up for your computer, the packets do not reach the Pi. Check the network
   and the firewalls again.

A device that stops sending shows as offline after 15 seconds. The daemon removes it after 5
minutes. That frees its slot in the table of 32 devices.

### The Pi's own stats do not show

```bash
systemctl status deskpane-reporter
cat /etc/deskpane/env
```

If you installed with `--no-reporter`, this service is not installed. Run `install.sh` again
without that option.

### The Pi does not join the WiFi

See [RECOVERY.md](RECOVERY.md).

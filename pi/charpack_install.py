"""Installs a character pack zip into a folder. Thin wrapper, stdlib only:
the checks live in charpack.py (plan_pack_install, check_pack_file).

install.sh runs it as root, isolated from the environment:
    python3 -I charpack_install.py [--check] <pack.zip> <target-folder>

It never calls unzip and never runs anything from the zip. Only files on
the allowlist are written, under fixed names, into <target>.new; then the
old folder is swapped out. Any failure leaves the old pack as it was (a
failed swap renames it back). --check validates the whole zip and writes nothing.

Exit codes: 0 installed (or checked), 1 refused, 2 wrong arguments.
"""
import errno
import io
import os
import shutil
import stat
import sys
import zipfile

# -I leaves the script's folder off sys.path; add only that folder (root
# owned in the release), so the working folder can never supply a module
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import charpack  # noqa: E402

FILE_MODE = 0o644
DIR_MODE = 0o755


def read_zip_file(zip_path):
    """The zip's bytes, opened once: no symlink is followed (O_NOFOLLOW), a
    FIFO or device never blocks (O_NONBLOCK), and fstat on the same open
    file checks it is regular and at most MAX_ZIP_BYTES. Reading the whole
    file (at most 4 MiB plus one byte) means nothing can change it later."""
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    try:
        fd = os.open(zip_path, flags)
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise charpack.PackError("the zip must be a regular file, not a symlink or device")
        raise charpack.PackError("cannot read the zip file")
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode):
            raise charpack.PackError("the zip must be a regular file, not a symlink or device")
        if info.st_size > charpack.MAX_ZIP_BYTES:
            raise charpack.PackError("the zip is bigger than 4 MiB")
        f = os.fdopen(fd, "rb")
    except BaseException:
        os.close(fd)
        raise
    with f:
        try:
            data = f.read(charpack.MAX_ZIP_BYTES + 1)
        except OSError:
            raise charpack.PackError("cannot read the zip file")
    # the file may have grown after fstat
    if len(data) > charpack.MAX_ZIP_BYTES:
        raise charpack.PackError("the zip is bigger than 4 MiB")
    return data


def read_planned_files(zf, plan):
    """Reads every planned file with a cap, so a lying header or a zip
    bomb cannot fill memory, and checks each one. Returns {target: bytes}."""
    contents = {}
    for item in plan["files"]:
        with zf.open(item["entry"]) as f:
            # one byte past the cap is enough to know it is too big
            data = f.read(charpack.MAX_ENTRY_BYTES + 1)
        charpack.check_pack_file(item["target"], data)
        contents[item["target"]] = data
    return contents


def write_new_folder(new_dir, contents):
    """Writes the files under their fixed names into a fresh folder."""
    os.mkdir(new_dir, DIR_MODE)
    os.chmod(new_dir, DIR_MODE)
    for target, data in contents.items():
        path = os.path.join(new_dir, *target.split("/"))
        folder = os.path.dirname(path)
        if not os.path.isdir(folder):
            os.mkdir(folder, DIR_MODE)
            os.chmod(folder, DIR_MODE)
        # O_EXCL and O_NOFOLLOW: never write through something already there
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, FILE_MODE)
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            # on disk before the swap, so a power cut never leaves half a PNG
            f.flush()
            os.fsync(f.fileno())
        os.chmod(path, FILE_MODE)
    # the folder entries too: every subfolder, then the folder itself
    for folder, dirs, files in os.walk(new_dir):
        fsync_dir(folder)


def fsync_dir(path):
    """Flushes a folder's entries (new names, renames) to disk."""
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def remove_tree(path):
    """Removes a folder we created; a symlink is only unlinked, never followed."""
    if os.path.islink(path):
        os.unlink(path)
    elif os.path.isdir(path):
        shutil.rmtree(path)


def install_pack(zip_path, target, check_only=False):
    """Checks the zip and, unless check_only, installs it into target.
    Returns the plan counts. Raises charpack.PackError when refused or when
    the folder cannot be written; the old pack is then left in place."""
    data = read_zip_file(zip_path)
    try:
        zf = zipfile.ZipFile(io.BytesIO(data))
    except (zipfile.BadZipFile, OSError, ValueError, EOFError):
        raise charpack.PackError("not a zip file")

    with zf:
        try:
            entries = [charpack.entry_from_zipinfo(info) for info in zf.infolist()]
        except Exception:
            raise charpack.PackError("not a zip file")
        plan = charpack.plan_pack_install(entries)
        # any decoder error (zlib, bz2, lzma, CRC, truncation) is a refusal,
        # never a traceback; our own refusals pass through unchanged
        try:
            contents = read_planned_files(zf, plan)
        except charpack.PackError:
            raise
        except Exception:
            raise charpack.PackError("a file in the zip cannot be read")

    counts = {"characters": plan["characters"], "emotes": plan["emotes"], "skipped": plan["skipped"]}
    if check_only:
        return counts

    parent = os.path.dirname(target)
    new_dir = target + ".new"
    old_dir = target + ".old"

    # a clean place to write: the parent exists, no leftovers from a crash
    try:
        os.makedirs(parent, DIR_MODE, exist_ok=True)
        remove_tree(new_dir)
        remove_tree(old_dir)
    except OSError:
        raise charpack.PackError("cannot prepare the pack folder")

    try:
        write_new_folder(new_dir, contents)
    except OSError:
        remove_quietly(new_dir)
        raise charpack.PackError("cannot write the new pack folder")

    # swap in two renames; if the second fails, the old pack goes back
    had_old = os.path.lexists(target)
    try:
        if had_old:
            os.rename(target, old_dir)
        os.rename(new_dir, target)
    except OSError:
        if had_old and not os.path.lexists(target) and os.path.lexists(old_dir):
            try:
                os.rename(old_dir, target)
            except OSError:
                pass
        remove_quietly(new_dir)
        raise charpack.PackError("cannot swap in the new pack; the old pack is kept")

    # the new name is on disk; a leftover .old is harmless and cleared next time
    try:
        fsync_dir(parent)
    except OSError:
        pass
    remove_quietly(old_dir)
    return counts


def remove_quietly(path):
    """remove_tree for clean-up paths, where a failure must not hide the
    real error."""
    try:
        remove_tree(path)
    except OSError:
        pass


def main(argv):
    args = list(argv)
    check_only = False
    if args and args[0] == "--check":
        check_only = True
        args = args[1:]
    if len(args) != 2:
        print("usage: charpack_install.py [--check] <pack.zip> <target-folder>", file=sys.stderr)
        return 2
    zip_path, target = args
    if not os.path.isabs(target):
        print("charpack_install.py: the target folder must be an absolute path", file=sys.stderr)
        return 2

    try:
        counts = install_pack(zip_path, target, check_only=check_only)
    except charpack.PackError as error:
        print("charpack_install.py: pack refused: %s" % error, file=sys.stderr)
        return 1

    verb = "pack is valid" if check_only else "installed"
    print("%s: %d characters, %d emotes, skipped %d extra entries"
          % (verb, counts["characters"], counts["emotes"], counts["skipped"]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

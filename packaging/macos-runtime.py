import ctypes
import os
import posixpath
import shutil
import struct
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


MTP = 'libmtp.9.dylib'
USB = 'libusb-1.0.0.dylib'
LIBRARIES = {MTP, USB}
ALLOWED = {None: {MTP, USB}, MTP: {USB}, USB: set()}
LOADS = {0xc, 0x80000018, 0x8000001f, 0x20, 0x80000023}
ID = 0xd
DYLINKER = 0xe
RPATH = 0x8000001c
UNSUPPORTED = {0x6, 0x7, 0xf, 0x10, 0x12, 0x13, 0x14, 0x15, 0x27, 0x80000035}


@dataclass
class Image:
    loads: list
    rpaths: list
    identity: str


def require(condition, message):
    if not condition:
        raise ValueError(message)


def inspect(path, dylib):
    with path.open('rb') as file:
        header = file.read(32)
        require(len(header) == 32, f'{path}: truncated Mach-O header')
        magic, cpu, subtype, kind, count, size, _, _ = struct.unpack('<8I', header)
        require((magic, cpu, subtype) == (0xfeedfacf, 0x100000c, 0),
                f'{path}: expected thin arm64 Mach-O')
        require(kind == (6 if dylib else 2), f'{path}: unexpected Mach-O file type')
        require(count <= size // 8 and size <= os.fstat(file.fileno()).st_size - 32,
                f'{path}: invalid load command table')
        commands = file.read(size)
    loads, ids, rpaths, linkers = [], [], [], []
    offset = 0
    for _ in range(count):
        require(offset + 8 <= len(commands), f'{path}: truncated load command')
        cmd, length = struct.unpack_from('<2I', commands, offset)
        end = offset + length
        require(length >= 8 and length % 8 == 0 and end <= len(commands),
                f'{path}: invalid load command size')
        require(cmd not in UNSUPPORTED, f'{path}: unsupported load command {cmd:#x}')
        if cmd in LOADS or cmd in (ID, DYLINKER, RPATH):
            minimum = 24 if cmd in LOADS or cmd == ID else 12
            require(length > minimum, f'{path}: truncated load command string')
            start, = struct.unpack_from('<I', commands, offset + 8)
            require(minimum <= start < length, f'{path}: invalid load command string offset')
            value, nul, _ = commands[offset + start:end].partition(b'\0')
            require(nul and value, f'{path}: empty or unterminated load command string')
            name = value.decode('utf-8')
            if cmd in LOADS:
                loads.append((cmd, name))
            elif cmd == ID:
                ids.append(name)
            elif cmd == RPATH:
                rpaths.append(name)
            else:
                linkers.append(name)
        offset = end
    require(offset == len(commands), f'{path}: invalid load command count')
    require(len(ids) == int(dylib), f'{path}: unexpected dylib install ID')
    require(linkers == ([] if dylib else ['/usr/lib/dyld']), f'{path}: unexpected dynamic linker')
    return Image(loads, rpaths, ids[0] if ids else None)


class SystemLibraries:
    def __init__(self):
        self.contains = ctypes.CDLL(None)._dyld_shared_cache_contains_path
        self.contains.argtypes = [ctypes.c_char_p]
        self.contains.restype = ctypes.c_bool

    def check(self, name):
        if (name != posixpath.normpath(name)
                or not name.startswith(('/usr/lib/', '/System/Library/'))):
            return False
        if not self.contains(os.fsencode(name)):
            path = Path(name)
            require(path.is_file() and str(path.resolve(strict=True)).startswith(('/usr/lib/', '/System/Library/')),
                    f'unresolved or non-system macOS dependency: {name}')
        return True


def reference(owner, leaf):
    directory = '../Frameworks/' if owner is None else ''
    return f'@loader_path/{directory}{leaf}'


def tool(name, *args):
    subprocess.run(['/usr/bin/' + name, *args], check=True)


def stage_runtime(contents, prefix, system):
    app = contents / 'MacOS/sliploom'
    frameworks = contents / 'Frameworks'
    libdir = prefix / 'lib'
    require(libdir.is_absolute() and libdir.resolve(strict=True) == libdir,
            f'{libdir}: expected canonical build library directory')
    paths = {leaf: libdir / leaf for leaf in LIBRARIES}
    for leaf, path in paths.items():
        target = path.resolve(strict=True)
        require(target.parent == libdir and target.is_file()
                and (target.name == leaf or (target.name.startswith(leaf[:-6] + '.')
                                             and target.suffix == '.dylib')),
                f'{path}: unexpected library target {target}')

    images, links = {}, {}
    for owner, source in {None: app, **paths}.items():
        image = inspect(source, owner is not None)
        require(image.identity == (str(source) if owner is not None else None),
                f'{source}: unexpected install ID {image.identity}')
        rewritten = {}
        for cmd, name in image.loads:
            if system.check(name):
                continue
            leaf = posixpath.basename(name)
            require(cmd == 0xc and leaf in ALLOWED[owner] and name == str(paths[leaf]),
                    f'{source}: unexpected dependency {name}; expected pinned build linkage')
            rewritten[name] = reference(owner, leaf)
        images[owner], links[owner] = image, rewritten
    require(str(paths[MTP]) in links[None] and str(paths[USB]) in links[MTP],
            'expected Sliploom to link to libmtp and libmtp to link to libusb')

    frameworks.mkdir()
    for leaf, path in paths.items():
        target = frameworks / leaf
        shutil.copyfile(path, target)
        target.chmod(0o755)
    for owner, image in images.items():
        path = frameworks / owner if owner is not None else app
        require(len(image.rpaths) == len(set(image.rpaths)), f'{path}: duplicate LC_RPATH entries')
        args = [] if owner is None else ['-id', reference(owner, owner)]
        for rpath in image.rpaths:
            args.extend(('-delete_rpath', rpath))
        for old, new in links[owner].items():
            if old != new:
                args.extend(('-change', old, new))
        if args:
            tool('install_name_tool', *args, path)
        rewritten = inspect(path, owner is not None)
        require(rewritten.loads == [(cmd, links[owner].get(name, name)) for cmd, name in image.loads],
                f'{path}: rewritten load commands do not match original dependency graph')
    for leaf in paths:
        tool('codesign', '--force', '--sign', '-', '--timestamp=none', frameworks / leaf)
    tool('codesign', '--force', '--sign', '-', '--timestamp=none', contents.parent)


def verify_runtime(contents, version, system):
    app = contents / 'MacOS/sliploom'
    frameworks = contents / 'Frameworks'
    bundled = set()

    def visit(owner, path):
        require(path.is_file() and not path.is_symlink(), f'missing or unsafe Mach-O: {path}')
        image = inspect(path, owner is not None)
        require(not image.rpaths, f'{path}: unexpected LC_RPATH')
        require(image.identity == (reference(owner, owner) if owner is not None else None),
                f'{path}: unexpected install ID {image.identity}')
        linked = set()
        for cmd, name in image.loads:
            if system.check(name):
                continue
            leaf = posixpath.basename(name)
            require(cmd == 0xc and leaf in ALLOWED[owner] and name == reference(owner, leaf),
                    f'{path}: unexpected bundled dependency {name}')
            linked.add(leaf)
            if leaf not in bundled:
                bundled.add(leaf)
                visit(leaf, frameworks / leaf)
        if owner == MTP:
            require(USB in linked, f'{path}: expected libusb linkage')
        if owner is not None:
            tool('codesign', '--verify', '--strict', '--all-architectures', path)

    require(not frameworks.is_symlink(), f'{frameworks}: symlink not allowed')
    visit(None, app)
    require(bundled == LIBRARIES, f'{frameworks}: incomplete runtime closure')
    require({item.name for item in frameworks.iterdir()} == bundled,
            f'{frameworks}: unexpected files')
    tool('codesign', '--verify', '--deep', '--strict', '--all-architectures', contents.parent)
    result = subprocess.run([str(app), '--version'], env={'LC_ALL': 'C'},
                            check=True, stdout=subprocess.PIPE, timeout=30)
    require(result.stdout == f'Sliploom {version}\n'.encode(),
            f'{app}: unexpected --version output {result.stdout!r}')


def main():
    operation, contents, argument = sys.argv[1:]
    require(sys.platform == 'darwin' and sys.version_info >= (3, 9),
            'requires macOS with the Xcode/CLT Python 3.9 or newer')
    system = SystemLibraries()
    if operation == 'stage':
        stage_runtime(Path(contents), Path(argument), system)
    elif operation == 'verify':
        verify_runtime(Path(contents), argument, system)
    else:
        raise ValueError(f'unknown operation: {operation}')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, RuntimeError, AttributeError, subprocess.SubprocessError) as error:
        sys.exit(f'macOS runtime: {error}')

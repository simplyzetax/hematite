"""Create a fresh 26.x world from a template world, with a chosen seed and spawn.

usage: python3 tools/new_world.py <template world dir> <new world dir> <name> <seed> <x> <y> <z>
"""
import gzip, shutil, struct, sys, time
from pathlib import Path

FMT = {1: '>b', 2: '>h', 3: '>i', 4: '>q', 5: '>f', 6: '>d'}
ARR = {7: 'b', 11: 'i', 12: 'q'}


def read(b, i, t):
    if t in FMT:
        return struct.unpack_from(FMT[t], b, i)[0], i + struct.calcsize(FMT[t])
    if t in ARR:
        n = struct.unpack_from('>i', b, i)[0]
        return list(struct.unpack_from(f'>{n}{ARR[t]}', b, i + 4)), i + 4 + n * struct.calcsize(ARR[t])
    if t == 8:
        n = struct.unpack_from('>H', b, i)[0]
        return b[i + 2:i + 2 + n].decode(), i + 2 + n
    if t == 9:
        et, n = b[i], struct.unpack_from('>i', b, i + 1)[0]
        i += 5
        out = []
        for _ in range(n):
            v, i = read(b, i, et)
            out.append(v)
        return (et, out), i
    d = {}
    while (et := b[i]) != 0:
        n = struct.unpack_from('>H', b, i + 1)[0]
        k = b[i + 3:i + 3 + n].decode()
        d[k], i = read(b, i + 3 + n, et)
        d[k] = (et, d[k])
    return d, i + 1


def write(t, v):
    if t in FMT:
        return struct.pack(FMT[t], v)
    if t in ARR:
        return struct.pack(f'>i{len(v)}{ARR[t]}', len(v), *v)
    if t == 8:
        s = v.encode()
        return struct.pack('>H', len(s)) + s
    if t == 9:
        et, items = v
        return struct.pack('>bi', et, len(items)) + b''.join(write(et, x) for x in items)
    out = b''
    for k, (et, x) in v.items():
        kb = k.encode()
        out += struct.pack('>bH', et, len(kb)) + kb + write(et, x)
    return out + b'\0'


def load(p):
    b = gzip.decompress(Path(p).read_bytes())
    n = struct.unpack_from('>H', b, 1)[0]
    return read(b, 3 + n, 10)[0]


def save(p, root):
    Path(p).write_bytes(gzip.compress(b'\x0a\0\0' + write(10, root)))


def main(template, dest, name, seed, x, y, z):
    template, dest = Path(template), Path(dest)
    assert not dest.exists(), f'{dest} already exists'
    (dest / 'data/minecraft').mkdir(parents=True)
    level = load(template / 'level.dat')
    data = level['Data'][1]
    data['LevelName'] = (8, name)
    # initialized=1 with our own spawn: MC skips its startup spawn search, which loads chunks before
    # Chunky is ready and crashes DH 3.3.4 ("Chunky is not loaded")
    data['initialized'] = (1, 1)
    data['spawn'][1]['pos'] = (11, [int(x), int(y), int(z)])
    data['Time'] = (4, 0)
    data['LastPlayed'] = (4, int(time.time() * 1000))
    save(dest / 'level.dat', level)
    gen = load(template / 'data/minecraft/world_gen_settings.dat')
    gen['data'][1]['seed'] = (4, int(seed))
    save(dest / 'data/minecraft/world_gen_settings.dat', gen)
    shutil.copy(template / 'data/minecraft/game_rules.dat', dest / 'data/minecraft/game_rules.dat')
    assert load(dest / 'level.dat')['Data'][1]['LevelName'][1] == name
    assert load(dest / 'data/minecraft/world_gen_settings.dat')['data'][1]['seed'][1] == int(seed)
    print(f'created {dest}')


if __name__ == '__main__':
    main(*sys.argv[1:])

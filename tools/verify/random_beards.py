"""Random structure pieces for check.sh, one line per chunk in the ints the plugin passes to Rust: the affected
box (min x y z, max x y z), the rigid count, per rigid its box, adjustment (0 none .. 4 encapsulate) and ground
level delta, then x, ground y, z per junction. Every fifth chunk has none."""
import random, sys

c = list(map(int, sys.argv[1:]))
for i, (cx, cz) in enumerate(zip(c[::2], c[1::2])):
    r = random.Random(i)
    if i % 5 == 4:
        print()
        continue
    rigids, boxes = [], []
    for _ in range(r.randint(1, 4)):
        x0, z0 = cx * 16 + r.randint(-24, 30), cz * 16 + r.randint(-24, 30)
        y0 = r.choice([r.randint(40, 100), r.randint(-50, 30)])
        b = (x0, y0, z0, x0 + r.randint(0, 30), y0 + r.randint(2, 24), z0 + r.randint(0, 30))
        rigids += [*b, r.choice([0, 1, 2, 2, 3, 3, 4]), r.choice([0, 0, 1, 2, -1])]
        boxes.append(b)
    junctions = []
    for _ in range(r.randint(0, 6)):
        j = (cx * 16 + r.randint(-11, 26), r.randint(50, 100), cz * 16 + r.randint(-11, 26))
        junctions += j
        boxes.append(j + j)
    box = [min(b[a] for b in boxes) - 24 for a in range(3)] + [max(b[a + 3] for b in boxes) + 24 for a in range(3)]
    print(*box, len(rigids) // 8, *rigids, *junctions)

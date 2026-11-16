r"""Lists every cue of the game's sound files: cue sheet, cue name, length.

The names are what replacement files are called (`<cue name>.wav`, ...).
Reads the game's own files: `meta` (SQLite3MultipleCiphers, ChaCha20) and the
ACBs under `dat\`. Needs `pip install apsw-sqlite3mc`.

    python tools/list_cues.py                      # default game data folder
    python tools/list_cues.py --data "D:\\...\\Umamusume" --out cues.csv --filter snd_bgm

Only files the game has downloaded are listed.
"""
import argparse
import csv
import os
import struct
import sys

META_KEY = "c753a5e8f5f78294f7fef57df4a14ffbf9a896cea1d4e09947e0d904e7fde8eaf0"


def utf(b):
    assert b[:4] == b"@UTF", b[:4]
    size = struct.unpack(">I", b[4:8])[0]
    t = b[8:8 + size]
    _, rows_off, str_off, data_off, _, ncol, rowlen, nrows = struct.unpack(">HHIIIHHI", t[:24])

    def s(o):
        e = t.index(b"\0", str_off + o)
        return t[str_off + o:e].decode("utf-8", "replace")

    fmt = {0: ">B", 1: ">b", 2: ">H", 3: ">h", 4: ">I", 5: ">i", 6: ">Q", 7: ">q", 8: ">f", 9: ">d"}

    def rd(p, ty):
        if ty == 0xA:
            return s(struct.unpack(">I", t[p:p + 4])[0]), 4
        if ty == 0xB:
            o, n = struct.unpack(">II", t[p:p + 8])
            return t[data_off + o:data_off + o + n], 8
        f = fmt[ty]
        n = struct.calcsize(f)
        return struct.unpack(f, t[p:p + n])[0], n

    p = 24
    cols = []
    for _ in range(ncol):
        fl = t[p]
        nm = s(struct.unpack(">I", t[p + 1:p + 5])[0])
        p += 5
        st, ty, const = fl & 0xF0, fl & 0x0F, None
        if st in (0x30, 0x70):
            const, n = rd(p, ty)
            p += n
        cols.append((nm, st, ty, const))
    out = []
    for r in range(nrows):
        p = rows_off + r * rowlen
        row = {}
        for nm, st, ty, const in cols:
            if st == 0x50:
                v, n = rd(p, ty)
                p += n
            elif st == 0x10:
                v = None
            else:
                v = const
            row[nm] = v
        out.append(row)
    return out


def cues(b):
    h = utf(b)[0]
    names = utf(h["CueNameTable"]) if h.get("CueNameTable") else []
    table = utf(h["CueTable"]) if h.get("CueTable") else []
    for n in names:
        i = n["CueIndex"]
        length = table[i].get("Length") if i < len(table) else None
        yield h.get("Name"), n["CueName"], length


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default=os.path.expandvars(r"%USERPROFILE%\AppData\LocalLow\Cygames\Umamusume"))
    ap.add_argument("--out", default="cues.csv")
    ap.add_argument("--filter", default="", help="only cue sheets whose file name contains this")
    a = ap.parse_args()
    try:
        import apsw
    except ImportError:
        sys.exit("needs: pip install apsw-sqlite3mc")
    con = apsw.Connection(os.path.join(a.data, "meta"), flags=apsw.SQLITE_OPEN_READONLY)
    cur = con.cursor()
    cur.execute("pragma cipher='chacha20'")
    cur.execute(f"pragma hexkey='{META_KEY}'")
    rows = list(cur.execute("select n, h from a where n like 'sound/%.acb' and n like ?", (f"%{a.filter}%",)))
    count = 0
    with open(a.out, "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["cue_sheet", "cue_name", "length_ms", "file"])
        for name, h in sorted(rows):
            path = os.path.join(a.data, "dat", h[:2], h)
            if not os.path.exists(path):
                continue
            try:
                with open(path, "rb") as fh:
                    for sheet, cue, length in cues(fh.read()):
                        w.writerow([sheet, cue, length, name])
                        count += 1
            except Exception as e:  # damaged or unexpected file: skip it
                print(f"skip {name}: {e}", file=sys.stderr)
    print(f"{count} cues from {len(rows)} cue sheets -> {a.out}")


if __name__ == "__main__":
    main()

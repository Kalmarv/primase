"""The sqlite oracle set's corpus/shred/main-db.tsv: every table of a database as Python's sqlite3 reads it.

  python -I repos/rme-formats/tests/oracles/sqlite_dump.py <database> <output.tsv>

Output, one block per table (sqlite_master first, then the schema's tables in schema order):
  @<table><TAB><column><TAB>...            the table and the column names sqlite3 reports
  <rowid><TAB><value><TAB>...              one line per row in rowid order; a value is
      n            NULL
      i:<decimal>  INTEGER
      r:<hex>      REAL, the 8 big-endian bytes of the double
      t:<hex>      TEXT, its UTF-8 bytes (the raw bytes sqlite3 holds, undecoded)
      b:<hex>      BLOB
The database is opened read-only and immutable.
"""

import sqlite3
import struct
import sys
from pathlib import Path


class Text(bytes):
    pass


def value(v):
    if v is None:
        return "n"
    if isinstance(v, Text):
        return "t:" + bytes(v).hex()
    if isinstance(v, bytes):
        return "b:" + v.hex()
    if isinstance(v, int):
        return "i:%d" % v
    if isinstance(v, float):
        return "r:" + struct.pack(">d", v).hex()
    raise SystemExit("unexpected value %r" % (v,))


def main():
    database, output = sys.argv[1:3]
    con = sqlite3.connect(Path(database).resolve().as_uri() + "?mode=ro&immutable=1", uri=True)
    con.text_factory = Text
    names = [r[0].decode() for r in con.execute("select name from sqlite_master where type = 'table' order by rowid")]
    lines = []
    for name in ["sqlite_master"] + names:
        q = '"%s"' % name.replace('"', '""')
        cur = con.execute("select rowid, * from %s order by rowid" % q)
        cols = [d[0] for d in cur.description][1:]
        lines.append("\t".join(["@" + name] + cols))
        for row in cur:
            lines.append("\t".join([str(row[0])] + [value(v) for v in row[1:]]))
    Path(output).write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(len(names) + 1, "tables,", sum(1 for l in lines if not l.startswith("@")), "rows")


main()

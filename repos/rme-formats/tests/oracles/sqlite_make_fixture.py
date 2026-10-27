"""Writes fixtures/multi.db with Python's sqlite3: a database shaped to need every part of the reader.

  python -I repos/rme-formats/tests/oracles/sqlite_make_fixture.py repos/rme-formats/tests/fixtures
  python -I repos/rme-formats/tests/oracles/sqlite_dump.py repos/rme-formats/tests/fixtures/multi.db repos/rme-formats/tests/fixtures/multi.tsv

512-byte pages so that 300 rows need interior pages (two levels), a 5,000-byte text and a 3,000-byte blob need
overflow chains, and integers of every width, reals, NULLs, an INTEGER PRIMARY KEY alias and an empty table occur.
"""

import sqlite3
import struct
import sys
from pathlib import Path

out = Path(sys.argv[1]) / "multi.db"
out.unlink(missing_ok=True)
con = sqlite3.connect(out)
con.execute("pragma page_size = 512")
con.execute("pragma journal_mode = delete")
con.execute("create table wide (id INTEGER PRIMARY KEY, name TEXT, n, x REAL, data BLOB)")
con.execute('create table "odd name" ("a b", [c], `d`)')
con.execute("create table empty_table (a, b)")
ints = [0, 1, -1, 127, -128, 128, 32767, -32768, 40000, 8388607, -8388608, 2**31 - 1, -(2**31), 2**40, -(2**47), 2**47, 2**62, 2**63 - 1, -(2**63)]
rows = []
for i in range(1, 301):
    rows.append((i * 3 if i % 5 else None, "row %d " % i + "x" * (i % 40), ints[i % len(ints)], i / 7.0 if i % 3 else None, bytes(range(i % 50))))
con.executemany("insert into wide (id, name, n, x, data) values (?,?,?,?,?)", rows)
con.execute("insert into wide (id, name, n, x, data) values (1000, ?, ?, ?, ?)", ("long " * 1000, float("inf"), -0.0, bytes(i * 7 % 256 for i in range(3000))))
con.execute("insert into wide (name, n) values ('after the long row', 'text number')")
con.execute('insert into "odd name" values (1, \'two\', x\'03\')')
con.execute('insert into "odd name" values (NULL, \'\', x\'\')')
con.commit()
con.close()
print(out, out.stat().st_size, "bytes,", out.stat().st_size // 512, "pages")

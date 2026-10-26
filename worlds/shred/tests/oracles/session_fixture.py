"""Synthetic source rows, written by Python SQLite; no game data."""
import sqlite3
from pathlib import Path
out=Path(__file__).parents[1]/'fixtures/sessions.db'
db=sqlite3.connect(':memory:')
db.executescript('''
PRAGMA page_size=512;
CREATE TABLE levels(id TEXT,NAME TEXT,SESSIONS TEXT,unknown TEXT);
INSERT INTO levels VALUES('7','Fixture','2&3','kept');
CREATE TABLE Fixture_sessions(id TEXT,NAME TEXT,TYPE TEXT,GOALS TEXT,IDENTIFIERS TEXT,unknown TEXT);
INSERT INTO Fixture_sessions VALUES('0','dormant','3','','NULL',NULL);
INSERT INTO Fixture_sessions VALUES('2','lesson','8','5','Boundary&Marker','unknown value');
INSERT INTO Fixture_sessions VALUES('3','free','3','','none','');
CREATE TABLE Fixture_goals(id TEXT,NAME TEXT,TASKS_REQ TEXT,TASKS_OPT TEXT,TASKS_DONT TEXT,TASKS_HELP TEXT,SEQUENTIAL TEXT,IDENTIFIERS TEXT);
INSERT INTO Fixture_goals VALUES('5','goal','8','[10-12]','13','14','1','GoalMarker');
CREATE TABLE Fixture_tasks(id TEXT,NAME TEXT,STATE TEXT,IDENTIFIERS TEXT,unknown TEXT);
INSERT INTO Fixture_tasks VALUES('8','required','1','Active_Icon','keep');
INSERT INTO Fixture_tasks VALUES('10','optionA','2','OptionA','');
INSERT INTO Fixture_tasks VALUES('11','optionB','2','OptionB','');
INSERT INTO Fixture_tasks VALUES('12','optionC','2','OptionC','');
INSERT INTO Fixture_tasks VALUES('13','forbidden','3','Forbidden','');
INSERT INTO Fixture_tasks VALUES('14','helper','4','-1','');
INSERT INTO Fixture_tasks VALUES('8','same id, distinct row','9','Another_Icon','preserve duplicate');
''')
db.commit()
out.write_bytes(db.serialize())
db.close()

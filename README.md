# primase

A game-asset conversion library: readers turn a game's data into one shared world, passes transform it, writers
produce an output (a skate. map for ReSkate, glTF, a three.js web world; later USD, STL and pen-plotter drawings).
Named after the enzyme that lays down the starting piece every new DNA strand is built from.

Status: being built. Design: `docs/DESIGN.md`; how to contribute: `CONTRIBUTING.md`.

Licence: GPL-3.0-or-later (`LICENSE`).

## Examples / consumers

- [primase-viewer](https://github.com/kalmarv/primase-viewer): a three.js viewer for the tiles `formats/web` writes.
  It shows how an application consumes primase output; it is not part of the library.

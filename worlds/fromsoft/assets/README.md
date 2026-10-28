# Community names and event templates

Names and TAE parameter templates are copied from the Smithbox community lists
(revision cbd477a8), originally bundled with primase-lab. Authors: Vawser,
Katalash, Meowmaritus and contributors. The MIT licence is preserved verbatim in
`names/SMITHBOX-LICENSE` and applies to both the lists and event templates here.

These labels and field definitions are community interpretations. Lowerings mark
their names as `NameOrigin::Community`, retain original ids/bytes/source order,
and refuse unknown field encodings instead of assigning an invented meaning.

Sekiro's event 715 (`Override Weapon Absorption Position`) has two more `Weapon Model Type` entries than the Smithbox
template: 2 `Mortal Blade` and 3 `Grappling Hook`. The Smithbox template lists 0 and 1 only; the data holds all four (372 events
in 19 of the 326 Sekiro event files: first byte 0 on 174, 1 on 167, 2 on 16, 3 on 15, the three bytes after it 0 on all),
and the two names are the facts of the SekiroAnimationBridge (its sword, prosthetics, Mortal Blade and grappling-hook slots);
only the names are taken, none of its code.

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

Two more events differ from the Smithbox templates, where a second source (soulstruct's event classes) and the stored events
agree against them. Event 730 (`Action Flag [730]`) is two 32-bit integers, not two floats: 1,086 Sekiro events store -45, 75, -90
and others (860 and 873 non-zero words, none a float) and 116 Elden Ring events store 180 (115 non-zero words each), which the
`f32` type refused; soulstruct types both words `int32`. In Sekiro's event 112 (`Spawn FFX [Floor Determined]`) the word after the
16-bit `Dummy Poly ID` is a 16-bit field and a word (Elden Ring's template and soulstruct split it so); 4 of the 29,350 events
store 110 at byte 8, which the Sekiro template's word at byte 6 read as 7,208,960. Neither change moves a named field or an
asserted one: every asserted field still holds on all stored events of both games.

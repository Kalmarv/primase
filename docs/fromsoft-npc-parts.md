# Human equipment and empty character binders

MSB's `NPCParamID` and `CharaInitID` are separate fields. SoulsFormats MSBS/MSBE
`EnemyBase.ReadTypeData` puts the latter at type data +0x18. Primase exposes it through
`MapLayout::character_init` and `NpcPlacement.character_init`; it never infers the initializer
by dividing or reusing an NPC row number.

Sekiro's Paramdex SDT `CHARACTER_INIT_PARAM` v4 stores the helm/body/arms/legs protector IDs at
32/36/40/44 and `npcPlayerSex` at 228. `EQUIP_PARAM_PROTECTOR_ST` v4 stores `equipModelId` at
160 and `equipModelGender` at 209. The reader follows the equipped IDs, then checks the resulting
part binders actually exist. It prefers the full-detail `_l` spelling; head equipment may be a
face part, as in the previously documented initializer 9000 -> protector 100000 -> `fc_m_0200_l`
Wolf baseline (`worlds/fromsoft/tests/regression_census.rs`). These are field-layout facts and
archive-presence checks, not a claim of runtime face/body/equipment simulation. Unresolved or
ambiguous part bindings retain their reasons.

Elden Ring uses the same chain through `fromsoft_formats::equipment::OutfitLayout::of(game)`: `CHARACTER_INIT_PARAM`
version 2 (320 bytes, slots at 32/36/40/44, `npcPlayerSex` at 228) and `EQUIP_PARAM_PROTECTOR_ST` version 6 (416
bytes, `equipModelId` at 164, category at 212, gender at 213). The protector's category says which part it is (head 5,
body 2, arms 1, legs 6; Sekiro's category 3 in the head slot is the face, `fc`); a protector in a slot of another
category is refused, and a slot whose protector row does not exist leaves only that slot unresolved. Elden Ring's
gender 3 (split by sex) is resolved by which of `m` and `f` the install holds; where it holds both the outfit is
ambiguous and says so (88 of the base game's 793 protector rows with a model), since the mapping of `npcPlayerSex` to
a letter is not established.

Unspecified protector gender modes 0/1 require a unique held `m`, `f` or `a` binder. They are not the split-by-sex
mode 3. SDT describes `npcPlayerSex` as an NPC-player field that does not apply to the normal player: filtering
unspecified modes to its letter discarded the actual male Wolf part binders. Shared mode 4 still uses `a`;
split mode 3 still uses the stated sex. Several held candidates remain ambiguous, never chosen by preference.

A bare Sekiro `c0000` character read, without an explicit NPC/initializer, keeps the player's skeleton and dummy
sockets and says that no outfit was selected. It must not infer a costume from a random map placement. Explicit
parts or an authored outfit select wearable geometry; other meshless NPCs retain their authored-outfit fallback.

The lab reads the first authored outfit with readable parts and lists every alternative with
the map and enemy placement that supplied it. `init` selects a listed initializer independently
of `npc`. Multiple valid outfits do not refuse the read. The parts use the existing character
assembly and animation correspondence passes; no synthetic merged skeleton is invented here.

An empty character binder is not automatically a part-built human. The Smithbox SDT character
alias list labels c1000 and c1001 **Invisible**, tagged `interactable_placeholder`. This is
reused provenance for a no-model explanation only after inspecting the binder and finding no
authored equipment parts. Actual geometry or a resolved outfit takes precedence. Other empty
characters remain named real misses rather than inheriting that label. Alias facts do not prove
what an MSB placement activates; its actual NPC and initializer IDs must be counted.

`tests/character_geometry_census.rs` writes `FS_CHARACTER_GEOMETRY` for every dictionary-named
character binder present in each configured Sekiro/Elden Ring archive, including exact errors,
drawable source meshes, all MSB uses of zero-geometry owners and their initializer/outfit choices.
Zero geometry is classified as placeholder, part-built or real-miss; no hidden geometry or
equipment is guessed. The denominator excludes unnamed hashed paths and measures source FLVER
geometry, not post-material render draws. Elden equipment layouts remain explicitly unread.
This census is written but has not been executed in the write-only round.

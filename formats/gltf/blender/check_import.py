"""Import a primase GLB into Blender headless and check what arrived.

    blender -b --factory-startup --python check_import.py -- <file.glb> [--report out.json] [--collections]
        [--save out.blend]

Prints (and writes as JSON with --report): object, mesh, material, image and collection counts, triangles, how
many objects/meshes/materials/scenes carry each extras key as a custom property (and how many of those Blender
could only store as a string), mirrored objects, and the largest difference between every placed object's world
matrix and the node's world matrix read from the GLB itself (converted to Blender's Z-up frame).

--collections turns the writer's group empties (extras "primase.collection") into Blender collections first, the
way a user would after import (primase_collections below).
"""

import json
import re
import struct
import sys
import time

import bpy
from mathutils import Matrix


def glb_json(path):
    with open(path, "rb") as f:
        head = f.read(20)
        magic, version, _total, json_len, kind = struct.unpack("<IIIII", head)
        assert magic == 0x46546C67 and version == 2 and kind == 0x4E4F534A, "not a GLB"
        return json.loads(f.read(json_len))


def node_world_matrices(doc):
    """Node index -> world matrix (glTF frame), walking the scene from its roots."""
    nodes = doc.get("nodes", [])

    def local(n):
        if "matrix" in n:
            m = n["matrix"]
            return Matrix([[m[c * 4 + r] for c in range(4)] for r in range(4)])
        t = n.get("translation", [0, 0, 0])
        assert "rotation" not in n and "scale" not in n, "the writer writes matrix or translation only"
        return Matrix.Translation(t)

    out = {}

    def walk(i, parent):
        w = parent @ local(nodes[i])
        out[i] = w
        for c in nodes[i].get("children", []):
            walk(c, w)

    for root in doc["scenes"][doc.get("scene", 0)]["nodes"]:
        walk(root, Matrix.Identity(4))
    return out


# glTF (+Y up, -Z forward) -> Blender (+Z up): Blender's importer maps (x, y, z) to (x, -z, y).
TO_BLENDER = Matrix(((1, 0, 0, 0), (0, 0, -1, 0), (0, 1, 0, 0), (0, 0, 0, 1)))


def primase_collections(scene):
    """Group empties (extras "primase.collection") -> collections holding their children; the empties go."""
    made = 0
    for empty in [o for o in scene.objects if o.type == "EMPTY" and "primase.collection" in o]:
        coll = bpy.data.collections.new(str(empty["primase.collection"]))
        scene.collection.children.link(coll)
        for child in list(empty.children):
            world = child.matrix_world.copy()
            child.parent = None
            child.matrix_world = world
            for c in list(child.users_collection):
                c.objects.unlink(child)
            coll.objects.link(child)
        bpy.data.objects.remove(empty)
        made += 1
    return made


def prop_census(ids):
    """Extras key -> [holders, holders where Blender stored a string instead of the structure]."""
    out = {}
    for idb in ids:
        for k in idb.keys():
            if k.startswith("_") or k == "cycles":
                continue
            v = idb[k]
            if k == "primase" and hasattr(v, "keys"):
                # extras.primase = {"<component>": {"version": n, "value": ...}}: count per component
                for name in v.keys():
                    sub = v[name]
                    entry = out.setdefault("primase/" + name, [0, 0])
                    entry[0] += 1
                    val = sub.get("value") if hasattr(sub, "get") else sub
                    if isinstance(sub, str) or (isinstance(val, str) and val[:1] in "{["):
                        entry[1] += 1
                continue
            entry = out.setdefault(k, [0, 0])
            entry[0] += 1
            if isinstance(v, str) and v[:1] in "{[":
                entry[1] += 1
    return out


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :]
    path = argv[0]
    opt = lambda name: argv[argv.index(name) + 1] if name in argv else None
    doc = glb_json(path)

    bpy.ops.wm.read_factory_settings(use_empty=True)
    t0 = time.time()
    bpy.ops.import_scene.gltf(filepath=path)
    seconds = time.time() - t0
    scene = bpy.context.scene

    # World matrices: Blender objects against the GLB's nodes. Objects are matched to nodes of the same name
    # (Blender appends .001, .002 to repeats, in an order of its own), each node to the nearest unused object.
    expected = node_world_matrices(doc)
    by_name = {}
    for i, n in enumerate(doc.get("nodes", [])):
        if "mesh" in n:
            by_name.setdefault(n.get("name", ""), []).append(expected[i])
    got = {}
    for o in scene.objects:
        if o.type == "MESH":
            base = re.sub(r"\.\d{3,}$", "", o.name)
            got.setdefault(base, []).append((o.name, o.matrix_world.copy()))
    worst = 0.0
    matched = 0
    unmatched = []
    def diff(a, b):
        return max(abs(a[r][c] - b[r][c]) for r in range(4) for c in range(4))

    for name, mats in by_name.items():
        objs = [m for _, m in got.get(name, [])]
        if len(objs) != len(mats):
            unmatched.append((name, len(mats), len(objs)))
            continue
        for want in mats:
            w = TO_BLENDER @ want @ TO_BLENDER.inverted()
            k = min(range(len(objs)), key=lambda i: diff(w, objs[i]))
            worst = max(worst, diff(w, objs.pop(k)))
            matched += 1

    collections_made = primase_collections(scene) if "--collections" in argv else 0

    mesh_objects = [o for o in scene.objects if o.type == "MESH"]
    for m in bpy.data.meshes:
        m.calc_loop_triangles()
    mesh_triangles = sum(len(m.loop_triangles) for m in bpy.data.meshes)
    placed_triangles = sum(len(o.data.loop_triangles) for o in mesh_objects)
    report = {
        "glb": path,
        "import_seconds": round(seconds, 1),
        "objects": len(scene.objects),
        "mesh_objects": len(mesh_objects),
        "empties": sum(1 for o in scene.objects if o.type == "EMPTY"),
        "meshes": len(bpy.data.meshes),
        "materials": len(bpy.data.materials),
        "images": len(bpy.data.images),
        "collections": len(bpy.data.collections),
        "collections_made": collections_made,
        "mesh_triangles": mesh_triangles,
        "placed_triangles": placed_triangles,
        "mirrored_objects": sum(1 for o in mesh_objects if o.matrix_world.determinant() < 0),
        "matrix_checked": matched,
        "matrix_max_abs_diff": worst,
        "matrix_unmatched": unmatched[:20],
        "custom_props": {
            "objects": prop_census(scene.objects),
            "meshes": prop_census(bpy.data.meshes),
            "materials": prop_census(bpy.data.materials),
            "scene": prop_census([scene]),
        },
        "glb_counts": {
            "nodes": len(doc.get("nodes", [])),
            "meshes": len(doc.get("meshes", [])),
            "materials": len(doc.get("materials", [])),
            "images": len(doc.get("images", [])),
        },
    }
    text = json.dumps(report, indent=1)
    print("PRIMASE_CHECK " + text)
    if opt("--report"):
        with open(opt("--report"), "w") as f:
            f.write(text)
    if opt("--save"):
        bpy.ops.wm.save_as_mainfile(filepath=opt("--save"))


main()

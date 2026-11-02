# SPDX-License-Identifier: GPL-3.0-or-later
"""Build a .blend from a folder primase's Blender writer wrote (scene.glb, materials.json, textures/).

    blender --background --factory-startup --python-exit-code 1 --python build.py -- <folder> <out.blend>

It interprets nothing: the graphs in materials.json are built as they are.

1. A derived glTF document, scene.blender.gltf, reads its geometry from scene.glb itself (the buffer is the GLB file,
   each buffer view moved past the GLB's header and JSON chunk) and differs from it only in what the importer would
   otherwise lose or decide:
   - each primitive's material is a name-only placeholder for its graph (`pm:<k>`), or none;
   - every COLOR_n becomes a float RGBA custom attribute (_COLOR_n, a copy decoded into scene.blender.bin): the
     importer stores COLOR_n as 8-bit sRGB;
   - a primitive's missing UV sets below its highest are zero accessors, so the importer's UV maps keep their set
     numbers (it numbers them consecutively from 0);
   - images, textures and samplers are left out (the graphs sample textures/).
2. Blender's glTF importer reads it (vertices not merged, no bind pose guessed, lights in physical units). Its UV maps
   are renamed TEXCOORD_n and its _COLOR_n attributes COLOR_n, the names the graphs read.
3. Each graph is built into its placeholder material, renamed to the world's name; the images are loaded from
   textures/ (channel-packed alpha, read as the graph states); the view layer gets the passes the graphs write and
   their alpha threshold; the world is black (no light but the scene's).
4. Orphans are purged and the .blend is saved, its image paths relative to it.

Prints one line `primase-blender-save: {json}` with what it built.
"""

import json
import os
import struct
import sys

import bpy
import numpy as np

SCHEMA = "primase-blender-materials/2"

COMPONENT = {5126: np.float32, 5121: np.uint8, 5123: np.uint16}
WIDTH = {"VEC3": 3, "VEC4": 4}
NORMALIZE = {5121: 255.0, 5123: 65535.0}


def glb_parts(path):
    """The GLB's JSON document and where its binary chunk's data starts and how long it is."""
    with open(path, "rb") as f:
        magic, version, total = struct.unpack("<III", f.read(12))
        assert magic == 0x46546C67 and version == 2, "not a GLB"
        json_len, kind = struct.unpack("<II", f.read(8))
        assert kind == 0x4E4F534A, "the GLB's first chunk is not JSON"
        doc = json.loads(f.read(json_len))
        at = 12 + 8 + json_len
        if at >= total:
            return doc, None, 0
        bin_len, kind = struct.unpack("<II", f.read(8))
        assert kind == 0x004E4942, "the GLB's second chunk is not BIN"
        return doc, at + 8, bin_len


def float_colours(doc, glb):
    """Every COLOR_n of every primitive as a float RGBA custom attribute _COLOR_n: the decoded values, appended to a
    new buffer. The buffer views' offsets are already the GLB file's. Returns the new buffer's bytes."""
    out = bytearray()
    made = {}
    with open(glb, "rb") as f:

        def decode(a):
            acc = doc["accessors"][a]
            dtype = COMPONENT[acc["componentType"]]
            width = WIDTH[acc["type"]]
            count = acc["count"]
            view = doc["bufferViews"][acc["bufferView"]]
            item = np.dtype(dtype).itemsize * width
            stride = view.get("byteStride", item)
            assert view["buffer"] == 0
            f.seek(view.get("byteOffset", 0) + acc.get("byteOffset", 0))
            raw = f.read(stride * (count - 1) + item)
            rows = np.lib.stride_tricks.as_strided(
                np.frombuffer(raw, dtype=np.dtype(dtype).newbyteorder("<")),
                shape=(count, width),
                strides=(stride, np.dtype(dtype).itemsize),
            )
            values = rows.astype(np.float32)
            if acc["componentType"] in NORMALIZE:
                assert acc.get("normalized"), "an integer COLOR_n that is not normalized"
                values = values / np.float32(NORMALIZE[acc["componentType"]])
            if width == 3:
                values = np.concatenate([values, np.ones((count, 1), np.float32)], axis=1)
            return values.astype("<f4")

        for mesh in doc.get("meshes", []):
            for prim in mesh["primitives"]:
                attrs = prim["attributes"]
                for name in [k for k in attrs if k.startswith("COLOR_")]:
                    a = attrs.pop(name)
                    if a not in made:
                        values = decode(a)
                        while len(out) % 4:
                            out.append(0)
                        doc["bufferViews"].append(
                            {"buffer": 1, "byteOffset": len(out), "byteLength": values.nbytes}
                        )
                        out += values.tobytes()
                        doc["accessors"].append(
                            {
                                "bufferView": len(doc["bufferViews"]) - 1,
                                "componentType": 5126,
                                "count": int(values.shape[0]),
                                "type": "VEC4",
                            }
                        )
                        made[a] = len(doc["accessors"]) - 1
                    attrs["_" + name] = made[a]
    return bytes(out)


def fill_uv_sets(doc):
    """A zero accessor for each UV set a primitive lacks below its highest."""
    zeros = {}
    for mesh in doc.get("meshes", []):
        for prim in mesh["primitives"]:
            attrs = prim["attributes"]
            sets = [int(k[len("TEXCOORD_"):]) for k in attrs if k.startswith("TEXCOORD_")]
            if not sets:
                continue
            count = doc["accessors"][attrs["POSITION"]]["count"]
            for n in range(max(sets)):
                if "TEXCOORD_%d" % n not in attrs:
                    if count not in zeros:
                        doc["accessors"].append({"componentType": 5126, "count": count, "type": "VEC2"})
                        zeros[count] = len(doc["accessors"]) - 1
                    attrs["TEXCOORD_%d" % n] = zeros[count]


def derive(folder, spec):
    glb = os.path.join(folder, "scene.glb")
    doc, bin_start, bin_len = glb_parts(glb)
    if bin_start is not None:
        for view in doc.get("bufferViews", []):
            assert view["buffer"] == 0
            view["byteOffset"] = view.get("byteOffset", 0) + bin_start
        doc["buffers"] = [{"uri": "scene.glb", "byteLength": os.path.getsize(glb)}]
    for key in ("images", "textures", "samplers"):
        doc.pop(key, None)
    doc["materials"] = [{"name": "pm:%d" % k} for k in range(len(spec["materials"]))]
    if not doc["materials"]:
        del doc["materials"]
    for name in ("extensionsUsed", "extensionsRequired"):
        kept = [e for e in doc.get(name, []) if e == "KHR_lights_punctual"]
        if kept:
            doc[name] = kept
        else:
            doc.pop(name, None)
    meshes = doc.get("meshes", [])
    assert len(meshes) == len(spec["meshes"]), "materials.json describes %d meshes, scene.glb has %d" % (
        len(spec["meshes"]),
        len(meshes),
    )
    for mesh, drawn in zip(meshes, spec["meshes"]):
        assert len(mesh["primitives"]) == len(drawn)
        for prim, k in zip(mesh["primitives"], drawn):
            prim.pop("material", None)
            if k is not None:
                prim["material"] = k
    if bin_start is not None:
        colours = float_colours(doc, glb)
        if colours:
            with open(os.path.join(folder, "scene.blender.bin"), "wb") as f:
                f.write(colours)
            doc["buffers"].append({"uri": "scene.blender.bin", "byteLength": len(colours)})
    fill_uv_sets(doc)
    path = os.path.join(folder, "scene.blender.gltf")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f)
    return path


def rename_layers():
    for mesh in bpy.data.meshes:
        for i, layer in enumerate(list(mesh.uv_layers)):
            expected = "UVMap" if i == 0 else "UVMap.%03d" % i
            assert layer.name == expected, "UV map %r of %s, expected %r" % (layer.name, mesh.name, expected)
        for i, layer in enumerate(list(mesh.uv_layers)):
            layer.name = "TEXCOORD_%d" % i
        for attr in list(mesh.attributes):
            if attr.name.startswith("_COLOR_"):
                attr.name = attr.name[1:]


def socket(sockets, identifier, node):
    for s in sockets:
        if s.identifier == identifier:
            return s
    raise KeyError("%s has no socket %r (it has %s)" % (node.bl_idname, identifier, [s.identifier for s in sockets]))


def build(material, entry, images):
    material.name = entry["name"]
    for key, value in entry["settings"].items():
        setattr(material, key, value)
    tree = material.node_tree
    for n in list(tree.nodes):
        tree.nodes.remove(n)
    nodes = []
    for i, spec in enumerate(entry["graph"]["nodes"]):
        node = tree.nodes.new(spec["type"])
        node.location = (300 * (i % 12), -250 * (i // 12))
        for key, value in spec.get("props", []):
            setattr(node, key, value)
            assert getattr(node, key) == value, "%s.%s is %r, not %r" % (spec["type"], key, getattr(node, key), value)
        if "image" in spec:
            node.image = images[spec["image"]]
        for identifier, value in spec.get("inputs", []):
            socket(node.inputs, identifier, node).default_value = value
        for identifier, expression in spec.get("drivers", []):
            curve = socket(node.inputs, identifier, node).driver_add("default_value")
            curve.driver.type = "SCRIPTED"
            curve.driver.expression = expression
            # evaluated without Python (no auto-run needed), and in a background render
            assert curve.driver.is_simple_expression, "driver %r is not a simple expression" % (expression,)
        nodes.append(node)
    for link in entry["graph"]["links"]:
        a, b = nodes[link["from"]], nodes[link["to"]]
        made = tree.links.new(socket(a.outputs, link["output"], a), socket(b.inputs, link["input"], b))
        assert made.is_valid, "link %s.%s -> %s.%s is not valid" % (
            a.bl_idname,
            link["output"],
            b.bl_idname,
            link["input"],
        )
    return len(nodes), len(entry["graph"]["links"])


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :]
    folder, out = os.path.abspath(argv[0]), os.path.abspath(argv[1])
    with open(os.path.join(folder, "materials.json"), encoding="utf-8") as f:
        spec = json.load(f)
    assert spec["schema"] == SCHEMA, "materials.json is %r, not %r" % (spec["schema"], SCHEMA)
    derived = derive(folder, spec)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    result = bpy.ops.import_scene.gltf(
        filepath=derived,
        merge_vertices=False,
        guess_original_bind_pose=False,
        export_import_convert_lighting_mode="SPEC",
    )
    assert result == {"FINISHED"}, "the glTF importer returned %r" % (result,)
    rename_layers()
    images = []
    for entry in spec["images"]:
        image = bpy.data.images.load(os.path.join(folder, entry["file"]), check_existing=False)
        image.colorspace_settings.name = entry["reading"]
        image.alpha_mode = "CHANNEL_PACKED"
        image.name = entry["name"]
        images.append(image)
    built = {"nodes": 0, "links": 0}
    for k, entry in enumerate(spec["materials"]):
        material = bpy.data.materials.get("pm:%d" % k)
        assert material is not None, "no primitive drew material %d (%s)" % (k, entry["name"])
        n, l = build(material, entry, images)
        built["nodes"] += n
        built["links"] += l
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    # the graphs' drivers take frame f as f / fps seconds
    scene.render.fps = spec["fps"]
    scene.render.fps_base = 1.0
    layer = scene.view_layers[0]
    layer.pass_alpha_threshold = spec["pass_alpha_threshold"]
    for name, kind in spec["aovs"]:
        aov = layer.aovs.add()
        aov.name = name
        aov.type = kind
    world = bpy.data.worlds.new("primase world")
    world.color = (0.0, 0.0, 0.0)
    background = world.node_tree.nodes.get("Background") if world.node_tree else None
    if background is not None:
        background.inputs["Color"].default_value = (0.0, 0.0, 0.0, 1.0)
        background.inputs["Strength"].default_value = 0.0
    scene.world = world
    bpy.data.orphans_purge(do_local_ids=True, do_linked_ids=True, do_recursive=True)
    bpy.ops.wm.save_as_mainfile(filepath=out, relative_remap=True, compress=False)
    summary = {
        "objects": len(bpy.data.objects),
        "meshes": len(bpy.data.meshes),
        "materials": len(bpy.data.materials),
        "images": len(bpy.data.images),
        "lights": len(bpy.data.lights),
        **built,
    }
    print("primase-blender-save: " + json.dumps(summary))


main()

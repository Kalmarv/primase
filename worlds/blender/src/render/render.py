# SPDX-License-Identifier: GPL-3.0-or-later
"""Render a .blend in Cycles from a stated camera, in the background.

    blender --background --factory-startup <scene.blend> --python-exit-code 1 --python render.py -- <render.json> <out>

render.json (written by primase's render stage) states everything: the camera in the world frame (glTF's: +Y up;
Blender's frame is (x, -z, y)), the image size, samples, seed, pixel filter width and device. The render is written
as <out>/render.exr (multilayer, 32-bit float: Combined and every AOV), <out>/render.png (Combined, the Standard
view transform) and each pass as <out>/<pass>.f32: little-endian float32, rows from the top, its channels
interleaved (RGBA for a colour pass, one for a value pass).

Prints one line `primase-blender-render: {json}` with the size and each pass's file and channels.
"""

import json
import os
import sys

import bpy
import numpy as np
import OpenImageIO as oiio
from mathutils import Matrix, Vector


def blender(p):
    """A world-frame point or direction in Blender's frame."""
    return Vector((p[0], -p[2], p[1]))


def camera(scene, c):
    data = bpy.data.cameras.new("primase camera")
    data.sensor_fit = "VERTICAL"
    projection = c["projection"]
    if "perspective" in projection:
        data.type = "PERSP"
        data.angle = projection["perspective"]["yfov"]
    else:
        data.type = "ORTHO"
        data.ortho_scale = projection["orthographic"]["height"]
    data.clip_start, data.clip_end = c["near"], c["far"]
    obj = bpy.data.objects.new("primase camera", data)
    scene.collection.objects.link(obj)
    eye, target, up = blender(c["eye"]), blender(c["target"]), blender(c["up"])
    forward = (target - eye).normalized()
    right = forward.cross(up).normalized()
    true_up = right.cross(forward)
    rotation = Matrix((right, true_up, -forward)).transposed()
    obj.matrix_world = Matrix.Translation(eye) @ rotation.to_4x4()
    scene.camera = obj


def device(scene, name):
    if name == "CPU":
        scene.cycles.device = "CPU"
        return
    prefs = bpy.context.preferences.addons["cycles"].preferences
    prefs.compute_device_type = name
    prefs.get_devices()
    chosen = [d for d in prefs.devices if d.type == name]
    assert chosen, "no %s device" % name
    for d in prefs.devices:
        d.use = d.type == name
    scene.cycles.device = "GPU"


def passes(path, out):
    """Each pass of the multilayer EXR as a raw float32 file; returns {pass: {file, channels}}."""
    found = {}
    image = oiio.ImageInput.open(path)
    assert image is not None, oiio.geterror()
    k = 0
    while image.seek_subimage(k, 0):
        spec = image.spec()
        pixels = image.read_image(oiio.FLOAT)
        names = list(spec.channelnames)
        by_pass = {}
        for c, name in enumerate(names):
            by_pass.setdefault(name.split(".")[-2], []).append(c)
        for name, channels in by_pass.items():
            data = np.ascontiguousarray(pixels[:, :, channels], dtype="<f4")
            file = name + ".f32"
            data.tofile(os.path.join(out, file))
            found[name] = {"file": file, "channels": [names[c].split(".")[-1] for c in channels]}
        k += 1
    image.close()
    return found


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :]
    with open(argv[0], encoding="utf-8") as f:
        o = json.load(f)
    out = os.path.abspath(argv[1])
    os.makedirs(out, exist_ok=True)
    scene = bpy.context.scene
    camera(scene, o["camera"])
    scene.render.engine = "CYCLES"
    device(scene, o["device"])
    cycles = scene.cycles
    cycles.samples = o["samples"]
    cycles.use_adaptive_sampling = False
    cycles.use_denoising = False
    cycles.seed = o["seed"]
    cycles.filter_width = o["filter_width"]
    render = scene.render
    render.resolution_x, render.resolution_y = o["size"]
    render.resolution_percentage = 100
    render.film_transparent = True
    render.use_compositing = False
    render.use_sequencer = False
    view = scene.view_settings
    view.view_transform = "Standard"
    view.look = "None"
    view.exposure = 0.0
    view.gamma = 1.0
    settings = render.image_settings
    settings.media_type = "MULTI_LAYER_IMAGE"
    settings.file_format = "OPEN_EXR_MULTILAYER"
    settings.color_depth = "32"
    settings.exr_codec = "ZIP"
    exr = os.path.join(out, "render.exr")
    render.filepath = exr
    bpy.ops.render.render(write_still=True)
    settings.media_type = "IMAGE"
    settings.file_format = "PNG"
    settings.color_mode = "RGBA"
    settings.color_depth = "8"
    bpy.data.images["Render Result"].save_render(filepath=os.path.join(out, "render.png"))
    summary = {"size": o["size"], "passes": passes(exr, out)}
    print("primase-blender-render: " + json.dumps(summary))


main()

"""Renders the mocap setup illustrations (editor/assets/*.png) with Blender.

    blender -b -P render_setups.py -- <out dir> [samples] [name ...]

Names: ring arc line body space. A staged living room with a poseable mannequin, the cameras on
tripods with their fields of view, and the measurements that matter. World: metres, Z up, the
subject at the origin facing -Y (the open side of the room, where the viewer stands).
"""

import math
import random
import sys

import bpy
from mathutils import Vector

ARGS = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
OUT = ARGS[0] if ARGS else "."
SAMPLES = int(ARGS[1]) if len(ARGS) > 1 and ARGS[1].isdigit() else 128
ONLY = [a for a in ARGS[1:] if not a.isdigit()]

ROOM = 3.6  # half extent: the room is 7.2 m square, walls at x=+ROOM and y=+ROOM


# ---------------------------------------------------------------- materials
def material(name, color, rough=0.5, metal=0.0, emit=None, strength=0.0, alpha=1.0, coat=0.0):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    b = m.node_tree.nodes["Principled BSDF"]
    b.inputs["Base Color"].default_value = (*color, 1)
    b.inputs["Roughness"].default_value = rough
    b.inputs["Metallic"].default_value = metal
    b.inputs["Alpha"].default_value = alpha
    if coat:
        b.inputs["Coat Weight"].default_value = coat
    if emit:
        b.inputs["Emission Color"].default_value = (*emit, 1)
        b.inputs["Emission Strength"].default_value = strength
    return m


def wood_floor():
    m = bpy.data.materials.new("floor")
    m.use_nodes = True
    nt = m.node_tree
    b = nt.nodes["Principled BSDF"]
    coord = nt.nodes.new("ShaderNodeTexCoord")
    brick = nt.nodes.new("ShaderNodeTexBrick")
    brick.inputs["Color1"].default_value = (0.30, 0.18, 0.10, 1)
    brick.inputs["Color2"].default_value = (0.40, 0.25, 0.14, 1)
    brick.inputs["Mortar"].default_value = (0.05, 0.03, 0.02, 1)
    brick.inputs["Scale"].default_value = 1.0
    brick.inputs["Mortar Size"].default_value = 0.006
    brick.inputs["Brick Width"].default_value = 1.4
    brick.inputs["Row Height"].default_value = 0.16
    brick.offset = 0.4
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 9.0
    ramp = nt.nodes.new("ShaderNodeMix")
    ramp.data_type = "RGBA"
    ramp.inputs["Factor"].default_value = 0.18
    nt.links.new(coord.outputs["Object"], brick.inputs["Vector"])
    nt.links.new(coord.outputs["Object"], noise.inputs["Vector"])
    nt.links.new(brick.outputs["Color"], ramp.inputs["A"])
    nt.links.new(noise.outputs["Color"], ramp.inputs["B"])
    nt.links.new(ramp.outputs["Result"], b.inputs["Base Color"])
    b.inputs["Roughness"].default_value = 0.32
    b.inputs["Coat Weight"].default_value = 0.25
    return m


MAT = {}


def materials():
    MAT["floor"] = wood_floor()
    MAT["wall"] = material("wall", (0.86, 0.86, 0.84), 0.9)
    MAT["wall2"] = material("wall2", (0.55, 0.62, 0.68), 0.9)
    MAT["base"] = material("base", (0.95, 0.95, 0.93), 0.5)
    MAT["rug"] = material("rug", (0.10, 0.28, 0.32), 1.0)
    MAT["sofa"] = material("sofa", (0.62, 0.30, 0.22), 0.85)
    MAT["cushion"] = material("cushion", (0.92, 0.80, 0.55), 0.9)
    MAT["darkwood"] = material("darkwood", (0.16, 0.10, 0.07), 0.45, coat=0.3)
    MAT["lightwood"] = material("lightwood", (0.62, 0.45, 0.28), 0.5, coat=0.2)
    MAT["metal"] = material("metal", (0.7, 0.7, 0.72), 0.3, 1.0)
    MAT["black"] = material("black", (0.02, 0.02, 0.025), 0.35, 0.2)
    MAT["screen"] = material("screen", (0.05, 0.1, 0.16), 0.2, emit=(0.25, 0.55, 1.0), strength=1.6)
    MAT["leaf"] = material("leaf", (0.12, 0.42, 0.18), 0.7)
    MAT["pot"] = material("pot", (0.82, 0.78, 0.7), 0.6)
    MAT["window"] = material("window", (1, 1, 1), 1.0, emit=(0.85, 0.93, 1.0), strength=3.5)
    MAT["frame"] = material("frame", (0.92, 0.92, 0.9), 0.5)
    MAT["body"] = material("body", (0.93, 0.93, 0.95), 0.35, coat=0.4)
    MAT["ghost"] = material("ghost", (0.8, 0.9, 1.0), 0.1, alpha=0.45)
    MAT["joint"] = material("joint", (1.0, 0.42, 0.08), 0.35, emit=(1.0, 0.4, 0.05), strength=1.5)
    MAT["bone"] = material("bone", (0.2, 1.0, 0.7), 0.3, emit=(0.1, 1.0, 0.6), strength=3.0)
    MAT["lens"] = material("lens", (0.01, 0.02, 0.05), 0.05, 0.6)
    MAT["led"] = material("led", (1, 0.1, 0.1), 0.3, emit=(1, 0.1, 0.1), strength=12.0)
    MAT["fov"] = material("fov", (0.25, 0.6, 1.0), 0.5, emit=(0.2, 0.55, 1.0), strength=0.5, alpha=0.05)
    MAT["fovline"] = material("fovline", (0.3, 0.7, 1.0), 0.5, emit=(0.3, 0.7, 1.0), strength=2.5)
    MAT["clear"] = material("clear", (0.2, 1.0, 0.5), 0.5, emit=(0.2, 1.0, 0.5), strength=0.5, alpha=0.08)
    MAT["clearline"] = material("clearline", (0.3, 1.0, 0.6), 0.5, emit=(0.3, 1.0, 0.6), strength=5.0)
    MAT["ring"] = material("ring", (1.0, 0.8, 0.2), 0.5, emit=(1.0, 0.75, 0.15), strength=4.0)
    MAT["shared"] = material("shared", (0.3, 1.0, 0.4), 0.5, emit=(0.3, 1.0, 0.4), strength=1.2, alpha=0.35)
    MAT["title"] = material("title", (0.05, 0.09, 0.18), 0.6)
    MAT["label"] = material("label", (1, 1, 1), 0.5, emit=(1, 1, 1), strength=3.0)
    MAT["book1"] = material("book1", (0.7, 0.18, 0.15), 0.7)
    MAT["book2"] = material("book2", (0.18, 0.35, 0.6), 0.7)
    MAT["book3"] = material("book3", (0.85, 0.7, 0.2), 0.7)
    MAT["book4"] = material("book4", (0.25, 0.5, 0.35), 0.7)


# ---------------------------------------------------------------- primitives
def finish(obj, mat, parent=None, smooth=True, bevel=0.0):
    if mat:
        obj.data.materials.append(MAT[mat] if isinstance(mat, str) else mat)
    if smooth and obj.type == "MESH":
        for p in obj.data.polygons:
            p.use_smooth = True
    if bevel:
        m = obj.modifiers.new("bevel", "BEVEL")
        m.width = bevel
        m.segments = 3
        m.limit_method = "ANGLE"
    if parent:
        obj.parent = parent
    return obj


def box(center, size, mat, bevel=0.012, parent=None, rot=0.0):
    bpy.ops.mesh.primitive_cube_add(size=1, location=center)
    o = bpy.context.active_object
    o.scale = size
    o.rotation_euler = (0, 0, rot)
    bpy.ops.object.transform_apply(scale=True)
    return finish(o, mat, parent, smooth=False, bevel=bevel)


def sphere(center, radius, mat, scale=(1, 1, 1), parent=None, seg=32):
    bpy.ops.mesh.primitive_uv_sphere_add(radius=radius, location=center, segments=seg, ring_count=seg // 2)
    o = bpy.context.active_object
    o.scale = scale
    return finish(o, mat, parent)


def cylinder(center, radius, depth, mat, parent=None, verts=32):
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=radius, depth=depth, location=center)
    return finish(bpy.context.active_object, mat, parent)


def bar(a, b, radius, mat, parent=None, verts=16):
    a, b = Vector(a), Vector(b)
    d = b - a
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=radius, depth=d.length, location=(a + b) / 2)
    o = bpy.context.active_object
    o.rotation_mode = "QUATERNION"
    o.rotation_quaternion = d.to_track_quat("Z", "Y")
    return finish(o, mat, parent)


def torus(center, major, minor, mat):
    bpy.ops.mesh.primitive_torus_add(major_radius=major, minor_radius=minor, location=center, major_segments=128, minor_segments=10)
    return finish(bpy.context.active_object, mat)


def disc(center, radius, mat, thick=0.004):
    return cylinder((center[0], center[1], center[2] + thick / 2), radius, thick, mat, verts=96)


def text(body, location, size, rot=(0, 0, 0), mat="label", extrude=0.006, align="CENTER", billboard=False):
    c = bpy.data.curves.new("t", "FONT")
    c.body = body
    c.size = size
    c.extrude = extrude
    c.align_x = align
    c.align_y = "CENTER"
    o = bpy.data.objects.new("t", c)
    bpy.context.collection.objects.link(o)
    o.location = location
    if billboard:
        o.rotation_mode = "QUATERNION"
        o.rotation_quaternion = (Vector(VIEW[1]) - Vector(VIEW[0])).to_track_quat("-Z", "Y")
    else:
        o.rotation_euler = rot
    o.data.materials.append(MAT[mat])
    return o


def mesh(name, verts, faces, mat):
    me = bpy.data.meshes.new(name)
    me.from_pydata(verts, [], faces)
    me.update()
    o = bpy.data.objects.new(name, me)
    bpy.context.collection.objects.link(o)
    o.data.materials.append(MAT[mat])
    return o


# ---------------------------------------------------------------- the room
def room():
    box((0, 0, -0.05), (2 * ROOM + 0.3, 2 * ROOM + 0.3, 0.1), "floor", bevel=0)
    box((0, ROOM + 0.06, 1.4), (2 * ROOM + 0.3, 0.12, 2.8), "wall2", bevel=0)
    box((ROOM + 0.06, 0, 1.4), (0.12, 2 * ROOM, 2.8), "wall", bevel=0)
    box((0, ROOM - 0.01, 0.07), (2 * ROOM, 0.03, 0.14), "base", bevel=0.004)
    box((ROOM - 0.01, 0, 0.07), (0.03, 2 * ROOM, 0.14), "base", bevel=0.004)
    # rug
    cylinder((0, 0, 0.006), 2.9, 0.012, "rug", verts=96)
    torus((0, 0, 0.013), 2.85, 0.02, "cushion")
    # window on the right wall, with the light of it
    box((ROOM - 0.02, 1.7, 1.55), (0.02, 1.8, 1.3), "window", bevel=0)
    for dy in (-0.9, 0.0, 0.9):
        box((ROOM - 0.045, 1.7 + dy, 1.55), (0.04, 0.05, 1.4), "frame")
    for dz in (-0.65, 0.0, 0.65):
        box((ROOM - 0.045, 1.7, 1.55 + dz), (0.04, 1.9, 0.05), "frame")
    # pictures on the back wall
    for x, w, h in ((-1.2, 0.9, 0.65), (0.1, 0.55, 0.75)):
        box((x, ROOM - 0.03, 1.7), (w + 0.1, 0.03, h + 0.1), "darkwood")
        box((x, ROOM - 0.045, 1.7), (w, 0.02, h), "wall2" if x > 0 else "cushion", bevel=0)
    # bookcase
    bx, bw = -2.6, 1.6
    box((bx, ROOM - 0.04, 1.1), (bw, 0.03, 2.2), "darkwood")
    box((bx - bw / 2 + 0.02, ROOM - 0.2, 1.1), (0.04, 0.4, 2.2), "darkwood")
    box((bx + bw / 2 - 0.02, ROOM - 0.2, 1.1), (0.04, 0.4, 2.2), "darkwood")
    box((bx, ROOM - 0.2, 2.19), (bw, 0.4, 0.04), "darkwood")
    box((bx, ROOM - 0.2, 0.03), (bw, 0.4, 0.06), "darkwood")
    random.seed(4)
    for row in range(5):
        z = 0.2 + row * 0.42
        box((bx, ROOM - 0.19, z), (bw - 0.1, 0.36, 0.03), "lightwood")
        x = bx - bw / 2 + 0.1
        while x < bx + bw / 2 - 0.15:
            w = random.uniform(0.04, 0.09)
            h = random.uniform(0.22, 0.34)
            box((x + w / 2, ROOM - 0.2, z + 0.015 + h / 2), (w, 0.24, h), random.choice(["book1", "book2", "book3", "book4"]), bevel=0.004)
            x += w + 0.005
    # desk, monitor, chair
    dx = 1.0
    box((dx, ROOM - 0.4, 0.74), (1.9, 0.8, 0.05), "lightwood")
    for sx in (-0.9, 0.9):
        box((dx + sx, ROOM - 0.4, 0.37), (0.05, 0.7, 0.72), "black")
    box((dx, ROOM - 0.6, 0.98), (0.8, 0.04, 0.46), "black")
    box((dx, ROOM - 0.58, 0.98), (0.76, 0.01, 0.42), "screen", bevel=0)
    box((dx, ROOM - 0.62, 0.81), (0.12, 0.1, 0.1), "metal")
    cylinder((dx, ROOM - 1.25, 0.28), 0.04, 0.5, "metal")
    cylinder((dx, ROOM - 1.25, 0.5), 0.23, 0.07, "black")
    box((dx, ROOM - 1.45, 0.78), (0.44, 0.07, 0.52), "black")
    for a in range(5):
        t = a * math.tau / 5
        bar((dx, ROOM - 1.25, 0.06), (dx + 0.26 * math.cos(t), ROOM - 1.25 + 0.26 * math.sin(t), 0.03), 0.015, "black")
    # sofa along the right wall, side table, lamp
    sy = -1.0
    box((ROOM - 0.5, sy, 0.22), (0.9, 2.2, 0.44), "sofa", bevel=0.04)
    box((ROOM - 0.55, sy, 0.55), (0.7, 2.0, 0.22), "sofa", bevel=0.05)
    box((ROOM - 0.2, sy, 0.7), (0.28, 2.2, 0.6), "sofa", bevel=0.05)
    for dy in (-0.7, 0.7):
        box((ROOM - 0.62, sy + dy, 0.62), (0.16, 0.45, 0.4), "cushion", bevel=0.05, rot=0.0)
    box((ROOM - 0.4, 1.0, 0.28), (0.5, 0.5, 0.56), "lightwood")
    cylinder((ROOM - 0.4, 1.0, 0.7), 0.05, 0.28, "metal")
    cylinder((ROOM - 0.4, 1.0, 0.95), 0.17, 0.22, "cushion")
    bpy.ops.object.light_add(type="POINT", location=(ROOM - 0.4, 1.0, 0.95))
    lamp = bpy.context.active_object
    lamp.data.energy = 60
    lamp.data.color = (1.0, 0.8, 0.55)
    lamp.data.shadow_soft_size = 0.15
    # plant in the corner
    cylinder((ROOM - 0.45, ROOM - 0.45, 0.25), 0.22, 0.5, "pot")
    for i in range(9):
        t = i * 0.7
        sphere((ROOM - 0.45 + 0.18 * math.cos(t), ROOM - 0.45 + 0.18 * math.sin(t), 0.8 + 0.12 * (i % 4)), 0.2, "leaf", scale=(1, 1, 1.5))
    # coffee table on the left, storage crates
    box((-1.5, -2.6, 0.2), (1.0, 0.6, 0.4), "lightwood")
    box((-1.5, -2.6, 0.41), (1.1, 0.7, 0.03), "black")
    for i, c in enumerate(("book1", "book3", "book2")):
        box((-3.1, -1.2 + i * 0.5, 0.18), (0.45, 0.4, 0.36), c)
    # a floor lamp on the left of the desk
    cylinder((-0.3, ROOM - 0.3, 0.8), 0.015, 1.6, "metal")
    cylinder((-0.3, ROOM - 0.3, 0.01), 0.15, 0.02, "metal")
    sphere((-0.3, ROOM - 0.3, 1.7), 0.17, "window", scale=(1, 1, 1.15))


def lighting(strength=1.0):
    w = bpy.data.worlds.new("w")
    bpy.context.scene.world = w
    w.use_nodes = True
    bg = w.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = (0.05, 0.06, 0.08, 1)
    bg.inputs["Strength"].default_value = 1.0
    bpy.ops.object.light_add(type="AREA", location=(0.5, -0.5, 4.8))
    a = bpy.context.active_object
    a.data.shape = "DISK"
    a.data.size = 5.0
    a.data.energy = 650 * strength
    a.data.color = (1.0, 0.95, 0.9)
    bpy.ops.object.light_add(type="AREA", location=(ROOM - 0.3, 1.7, 1.55))
    win = bpy.context.active_object
    win.data.shape = "RECTANGLE"
    win.data.size = 1.8
    win.data.size_y = 1.3
    win.data.energy = 420 * strength
    win.data.color = (0.85, 0.93, 1.0)
    win.rotation_euler = (0, math.radians(-90), 0)
    bpy.ops.object.light_add(type="AREA", location=(-5, -5, 3.5))
    fill = bpy.context.active_object
    fill.data.size = 4
    fill.data.energy = 160 * strength
    fill.rotation_euler = (math.radians(60), 0, math.radians(-45))


# ---------------------------------------------------------------- the subject
JOINTS = {
    "pelvis": (0, 0, 0.95), "chest": (0, 0, 1.33), "neck": (0, 0, 1.5), "head": (0, 0, 1.66),
    "lsh": (0.2, 0, 1.45), "rsh": (-0.2, 0, 1.45), "lel": (0.46, 0, 1.2), "rel": (-0.46, 0, 1.2),
    "lwr": (0.64, 0, 0.97), "rwr": (-0.64, 0, 0.97),
    "lhp": (0.1, 0, 0.93), "rhp": (-0.1, 0, 0.93), "lkn": (0.13, 0, 0.5), "rkn": (-0.13, 0, 0.5),
    "lan": (0.13, 0, 0.08), "ran": (-0.13, 0, 0.08),
}
BONES = [("pelvis", "chest"), ("chest", "neck"), ("neck", "head"), ("neck", "lsh"), ("neck", "rsh"),
         ("lsh", "lel"), ("lel", "lwr"), ("rsh", "rel"), ("rel", "rwr"), ("pelvis", "lhp"), ("pelvis", "rhp"),
         ("lhp", "lkn"), ("lkn", "lan"), ("rhp", "rkn"), ("rkn", "ran")]


def mannequin(raise_arm=False, ghost=True, location=(0, 0, 0)):
    j = {k: Vector(v) for k, v in JOINTS.items()}
    if raise_arm:
        j["lel"] = Vector((0.36, 0, 1.78))
        j["lwr"] = Vector((0.3, 0, 2.05))
    off = Vector(location)
    skin = "ghost" if ghost else "body"
    for a, b in BONES:
        radius = 0.045 if a in ("lsh", "rsh", "lel", "rel") else 0.055
        if a in ("lhp", "rhp", "lkn", "rkn"):
            radius = 0.075 if a in ("lhp", "rhp") else 0.055
        if a in ("pelvis", "chest", "neck") and b != "neck" and b not in ("lhp", "rhp"):
            continue
        bar(j[a] + off, j[b] + off, 0.02 if ghost else radius, "bone" if ghost else skin)
    if ghost:
        # a translucent body around the armature
        sphere(j["head"] + off, 0.11, skin, scale=(0.9, 1.0, 1.08))
        sphere((0 + off.x, off.y, 1.3 + off.z), 0.2, skin, scale=(1.0, 0.62, 1.35))
        sphere((0 + off.x, off.y, 0.98 + off.z), 0.17, skin, scale=(1.0, 0.68, 0.8))
        for a, b in BONES:
            if a in ("lsh", "rsh", "lel", "rel", "lhp", "rhp", "lkn", "rkn"):
                bar(j[a] + off, j[b] + off, 0.05 if a[1:] in ("sh", "hp") else 0.042, skin)
    else:
        sphere(j["head"] + off, 0.115, skin, scale=(0.9, 1.0, 1.1))
        sphere((off.x, off.y, 1.3 + off.z), 0.2, skin, scale=(1.0, 0.6, 1.4))
        sphere((off.x, off.y, 0.98 + off.z), 0.17, skin, scale=(1.0, 0.65, 0.8))
        cylinder((off.x, off.y, 1.1 + off.z), 0.12, 0.3, skin)
    for name, p in j.items():
        r = 0.03 if name != "head" else 0.0
        if r:
            sphere(p + off, r, "joint", seg=16)
    sphere(j["head"] + off + Vector((0, -0.11, -0.01)), 0.018, "joint", seg=12)  # a nose: the way it faces
    for s in (1, -1):
        box((s * 0.13 + off.x, -0.06 + off.y, 0.03 + off.z), (0.1, 0.24, 0.06), "joint" if ghost else "black", bevel=0.02)
    return j


# ---------------------------------------------------------------- cameras
def aim_quat(direction):
    return Vector(direction).to_track_quat("Y", "Z")


def camera_device(position, target, hfov, aspect=16 / 9, reach=None):
    pos, tgt = Vector(position), Vector(target)
    d = (tgt - pos).normalized()
    foot = Vector((pos.x, pos.y, 0))
    # tripod
    bar(foot + Vector((0, 0, 0.0)), pos - Vector((0, 0, 0.08)), 0.012, "black")
    for k in range(3):
        t = k * math.tau / 3 + math.atan2(d.y, d.x)
        leg = foot + Vector((0.32 * math.cos(t), 0.32 * math.sin(t), 0))
        bar(leg, pos - Vector((0, 0, 0.55)), 0.01, "black")
    # head: a body and a lens, built facing +Y and rotated to aim
    head = bpy.data.objects.new("cam", None)
    bpy.context.collection.objects.link(head)
    head.location = pos
    head.rotation_mode = "QUATERNION"
    head.rotation_quaternion = aim_quat(d)
    box((0, 0, 0), (0.16, 0.09, 0.09), "black", bevel=0.015, parent=head)
    lens = cylinder((0, 0.07, 0), 0.032, 0.06, "lens", parent=head)
    lens.rotation_euler = (math.pi / 2, 0, 0)
    sphere((0.055, 0.04, 0.03), 0.008, "led", seg=8, parent=head)
    # the field of view
    reach = reach or (tgt - pos).length * 1.0
    th, tv = math.tan(hfov / 2), math.tan(hfov / 2) / aspect
    right = d.cross(Vector((0, 0, 1))).normalized()
    up = right.cross(d).normalized()
    far = pos + d * reach
    corners = [far + right * reach * th * sx + up * reach * tv * sy for sx, sy in ((-1, -1), (1, -1), (1, 1), (-1, 1))]
    verts = [pos] + corners
    mesh("fov", [tuple(v) for v in verts], [(0, 1, 2), (0, 2, 3), (0, 3, 4), (0, 4, 1), (1, 2, 3, 4)], "fov")
    for c in corners:
        bar(pos, c, 0.0035, "fovline", verts=6)
    for a, b in zip(corners, corners[1:] + corners[:1]):
        bar(a, b, 0.0035, "fovline", verts=6)
    return d


def ring_positions(n, dist, height):
    return [(dist * math.sin(k * math.tau / n), -dist * math.cos(k * math.tau / n), height) for k in range(n)]


def arc_positions(n, dist, height):
    out = []
    for k in range(n):
        a = math.radians(-60 + 120 * k / (n - 1))
        out.append((dist * math.sin(a), -dist * math.cos(a), height))
    return out


def line_positions(n, dist, height):
    sp = dist * 0.6
    return [((k - (n - 1) / 2) * sp, -dist, height) for k in range(n)]


# ---------------------------------------------------------------- annotations
def dimension(a, b, label, lift=0.0, size=0.2, tick=0.12, upright=False):
    a, b = Vector(a), Vector(b)
    bar(a, b, 0.012, "label", verts=8)
    d = (b - a).normalized()
    n = Vector((-d.y, d.x, 0)) if abs(d.z) < 0.9 else Vector((1, 0, 0))
    for p in (a, b):
        bar(p - n * tick, p + n * tick, 0.012, "label", verts=8)
    mid = (a + b) / 2
    if abs(d.z) > 0.9 or upright:
        # Upright, in the plane facing the open side of the room.
        text(label, mid + Vector((-0.05, -0.2 + lift, 0.25 if upright else 0)), size, rot=(math.pi / 2, 0, 0))
        return
    # Flat on the floor, reading along the line, the right way up for the viewer.
    ang = math.atan2(d.y, d.x)
    view = Vector(VIEW[1]) - Vector(VIEW[0])
    if (-math.sin(ang)) * view.x + math.cos(ang) * view.y < 0:
        ang += math.pi
    side = Vector((-math.sin(ang), math.cos(ang), 0))
    text(label, mid + side * (0.24 + lift) + Vector((0, 0, 0.02)), size, rot=(0, 0, ang))


def floor_label(body, pos, size=0.3, rot=0.0):
    # Standing against the back wall, fully in front of it.
    text(body, (-0.6, ROOM - 0.1, 2.45), size, rot=(math.pi / 2, 0, 0), mat="title")


def render_camera(location, target, lens=34):
    bpy.ops.object.camera_add(location=location)
    cam = bpy.context.active_object
    cam.rotation_mode = "QUATERNION"
    cam.rotation_quaternion = (Vector(target) - Vector(location)).to_track_quat("-Z", "Y")
    cam.data.lens = lens
    cam.data.dof.use_dof = True
    cam.data.dof.focus_distance = (Vector(target) - Vector(location)).length
    cam.data.dof.aperture_fstop = 11.0
    bpy.context.scene.camera = cam
    return cam


# ---------------------------------------------------------------- scenes
def scene_cameras(positions, hfov=70.0):
    for p in positions:
        camera_device(p, (0, 0, 1.0), math.radians(hfov))


def build(name):
    sub_ghost = True
    if name == "ring":
        mannequin(ghost=sub_ghost)
        scene_cameras(ring_positions(4, 2.0, 1.5))
        torus((0, 0, 0.02), 2.0, 0.018, "ring")
        dimension((0, 0, 0.02), (2.0 * math.sin(math.radians(90)), 0, 0.02), "2.0 m", size=0.24)
        floor_label("RING: evenly around the subject", (0.3, 3.0), 0.24)
    elif name == "arc":
        mannequin(ghost=sub_ghost)
        scene_cameras(arc_positions(3, 2.4, 1.5))
        dimension((0, 0, 0.02), (0, -2.4, 0.02), "2.4 m", size=0.24)
        floor_label("ARC: in front, like a stage", (0.3, 3.0), 0.24)
    elif name == "line":
        mannequin(ghost=sub_ghost)
        scene_cameras(line_positions(3, 2.4, 1.5))
        dimension((-1.44, -2.4, 0.02), (1.44, -2.4, 0.02), "spacing 1.4 m", size=0.17)
        dimension((0, 0, 0.02), (0, -2.4, 0.02), "2.4 m", size=0.24)
        floor_label("LINE: a wall of cameras", (0.3, 3.0), 0.24)
    elif name == "body":
        mannequin(raise_arm=True, ghost=sub_ghost)
        a, b = (-2.2, -2.0, 1.5), (2.2, -2.0, 1.5)
        scene_cameras([a, b], hfov=70.0)
        disc((0, 0, 0.012), 0.95, "shared")
        torus((0, 0, 0.02), 0.95, 0.014, "clearline")
        floor_label("both cameras see you here", (0, 1.15), 0.18)
        dimension((-2.2, -2.0, 0.02), (0, 0, 0.02), "3 m", size=0.18)
    elif name == "space":
        mannequin(ghost=False)
        w, h = 3.0, 2.3
        box((0, 0, h / 2), (w, w, h), "clear", bevel=0)
        for sx in (-1, 1):
            for sy in (-1, 1):
                bar((sx * w / 2, sy * w / 2, 0), (sx * w / 2, sy * w / 2, h), 0.014, "clearline")
        for z in (0.01, h):
            for sx, sy, ex, ey in ((-1, -1, 1, -1), (1, -1, 1, 1), (1, 1, -1, 1), (-1, 1, -1, -1)):
                bar((sx * w / 2, sy * w / 2, z), (ex * w / 2, ey * w / 2, z), 0.014, "clearline")
        dimension((-w / 2, -w / 2 - 0.2, 0.02), (w / 2, -w / 2 - 0.2, 0.02), "3.0 m clear", size=0.24)
        dimension((-w / 2 - 0.25, -w / 2, 0), (-w / 2 - 0.25, -w / 2, h), "2.3 m", size=0.24)
        dimension((w / 2, 0, 0.02), (ROOM - 0.9, 0, 0.02), "1.1 m to sofa", size=0.2, upright=True)
        floor_label("keep this space free", (0, 0.9), 0.2)


VIEW = ((-5.3, -7.3, 4.6), (0.1, 0.0, 0.9))
VIEWS = {
    "ring": ((-5.3, -7.3, 4.6), (0.1, 0.0, 0.9)),
    "arc": ((-5.3, -7.3, 4.6), (0.1, 0.0, 0.9)),
    "line": ((-5.3, -7.3, 4.6), (0.1, 0.0, 0.9)),
    "body": ((-5.3, -7.3, 4.6), (0.1, 0.0, 0.9)),
    "space": ((-5.3, -7.3, 4.6), (0.1, 0.0, 0.9)),
}


def render(name):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    s = bpy.context.scene
    s.render.engine = "CYCLES"
    s.cycles.samples = SAMPLES
    s.cycles.use_denoising = True
    s.cycles.device = "CPU"
    s.render.resolution_x, s.render.resolution_y = 1280, 800
    s.render.image_settings.file_format = "PNG"
    s.render.image_settings.color_depth = "8"
    s.view_settings.view_transform = "AgX"
    s.view_settings.look = "AgX - High Contrast"
    materials()
    room()
    lighting()
    build(name)
    loc, tgt = VIEWS[name]
    render_camera(loc, tgt)
    s.render.filepath = f"{OUT}/{name}.png"
    bpy.ops.render.render(write_still=True)


for n in ONLY or ["ring", "arc", "line", "body", "space"]:
    render(n)

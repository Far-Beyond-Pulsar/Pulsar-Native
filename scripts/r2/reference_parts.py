"""Writes measurement variants of the reference path tracer's shader
(`helio-pass-reference/shaders/reference.wgsl`), each summing one part of
the light reaching the camera, for `rerun_reference.sh`'s components run
(read from disk with `HELIO_SHADER_FROM_DISK=1`):

    visible  the sky and emitters the camera sees directly
    direct   the lights' samples at the first hit (the sun, lamps)
    sky      the sky reaching the first hit directly (a bounce ray's miss)
    first    light reaching the first hit off one other surface
    later    light reaching it off two or more
    albedo   the first hit's albedo (no light): the hit surface as the
             tracers fill it, against the G-buffer's albedo view (4)

    python reference_parts.py <reference.wgsl> <out_dir>

writes `<out_dir>/reference_<part>.wgsl`. The parts of a path's light sum
to the full image (`visible + direct + sky + first + later`).
"""

import os
import sys

PARTS = ["visible", "direct", "sky", "first", "later", "albedo"]


def variant(source, part):
    nl = "\r\n" if "\r\n" in source else "\n"
    code = PARTS.index(part)

    def rep(text, old, new):
        if old not in text:
            raise SystemExit(f"reference.wgsl changed: {old[:60]!r} not found")
        return text.replace(old, new, 1)

    t = rep(
        source,
        "fn add(path: ptr<function, Path>, c: vec3<f32>, depth: u32) {" + nl,
        f"const PART: u32 = {code}u;" + nl
        + "fn add(path: ptr<function, Path>, c: vec3<f32>, depth: u32) { add_part(path, c, depth, PART); }" + nl
        + "fn add_part(path: ptr<function, Path>, c: vec3<f32>, depth: u32, part: u32) {" + nl
        + "    if part != PART { return; }" + nl,
    )
    # A miss at depth d: visible (0), sky (1), first (2), later.
    t = rep(
        t,
        "add(&path, path.throughput * sky_radiance(ray.origin, ray.dir), depth);",
        "add_part(&path, path.throughput * sky_radiance(ray.origin, ray.dir), depth, "
        "select(select(select(4u, 3u, depth == 2u), 2u, depth == 1u), 0u, depth == 0u));",
    )
    # An emitter met at depth d: visible (0), first (1), later.
    t = rep(
        t,
        "add(&path, path.throughput * s.emissive, depth);",
        "add_part(&path, path.throughput * s.emissive, depth, "
        "select(select(4u, 3u, depth == 1u), 0u, depth == 0u));",
    )
    # A light's sample at the hit of stage d: direct (0), first (1), later.
    t = rep(
        t,
        "add(&path, bitcast<vec3<f32>>(c.xyz) * scene_occlusion_through(shadow_hits[n]), stage.x);",
        "add_part(&path, bitcast<vec3<f32>>(c.xyz) * scene_occlusion_through(shadow_hits[n]), "
        "stage.x, select(select(4u, 3u, stage.x == 1u), 1u, stage.x == 0u));",
    )
    if part == "albedo":
        row = "            let row = cur_surfaces[r];"
        t = rep(
            t,
            row,
            row + nl + "            if depth == 0u { add_part(&path, scene_surface_unpack(row).albedo.rgb, "
            "0u, 5u); path.throughput = vec3<f32>(0.0); }",
        )
    return t


def main():
    source = open(sys.argv[1], encoding="utf-8", newline="").read()
    os.makedirs(sys.argv[2], exist_ok=True)
    for part in PARTS:
        path = os.path.join(sys.argv[2], f"reference_{part}.wgsl")
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(variant(source, part))
        print(path)


if __name__ == "__main__":
    main()

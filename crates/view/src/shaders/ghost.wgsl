// The x-ray pass.
//
// Draws the building, the steelwork and everything else that is not plant as a
// fresnel shell: bright at grazing angles, invisible face-on. A wall seen
// straight on contributes almost nothing, while edges, cylinders and silhouettes
// read as a clean wireframe. That is what keeps de_nuke's interior legible —
// naive alpha blending of three million triangles of structure saturates to flat
// grey and the pipework vanishes inside it.
//
// Two properties of the pipeline this runs in matter as much as the shading:
//
//   * It depth-tests against the buffer the solid pass already filled, with
//     depth writes off. A ghost fragment behind a pipe is discarded, so the
//     pipework is genuinely in front of the building rather than fighting it.
//   * Blending is additive, which is commutative, so the pass never needs
//     sorting. No per-frame CPU sort of 8,000 instances, and no order-dependent
//     flicker while the camera moves — which would be fatal in captured video.

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    // x: ghost gain, y: fresnel power, z: fade distance, w: unused.
    ghost: vec4<f32>,
};

struct Instance {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    colour: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<storage, read> instances: array<Instance>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) colour: vec3<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(instance_index) id: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VsOut {
    let inst = instances[id];
    let p = vec4<f32>(position, 1.0);
    let world = vec3<f32>(dot(inst.r0, p), dot(inst.r1, p), dot(inst.r2, p));

    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.normal = vec3<f32>(
        dot(inst.r0.xyz, normal),
        dot(inst.r1.xyz, normal),
        dot(inst.r2.xyz, normal),
    );
    out.colour = inst.colour.rgb;
    out.world = world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let to_eye = frame.camera_pos.xyz - in.world;
    let distance = length(to_eye);
    let v = to_eye / max(distance, 1e-4);

    // abs() because the pass draws back faces too — an inward-facing wall
    // should glow at its edges exactly like an outward-facing one.
    let facing = abs(dot(n, v));
    let rim = pow(1.0 - facing, frame.ghost.y);

    // Without a distance falloff the far side of a 320 m map piles up into a
    // bright smear behind everything you are actually looking at.
    let fade = exp(-distance / max(frame.ghost.z, 1.0));

    let alpha = rim * frame.ghost.x * fade;
    return vec4<f32>(in.colour * alpha, alpha);
}

// Flat-shaded pass over the whole map.
//
// Every instance is one draw call over a slice of the shared index buffer. The
// instance's own record is fetched by `instance_index`, which the draw sets
// through `first_instance` — so the vertex buffer carries no per-instance data
// at all and the whole map is one buffer binding.

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    // Ghost-pass parameters, unused here but the uniform is shared.
    ghost: vec4<f32>,
};

// A 3x4 row-major world matrix plus the colour to draw with. Rows rather than a
// mat4x4 because the fourth row is always [0,0,0,1] and 16 bytes of every
// instance is not worth spending.
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

    // The placement matrices are rotations times a uniform scale, so the
    // inverse transpose is the same basis — rotating the normal by the upper
    // 3x3 and renormalising is exact here, and skips a matrix inverse.
    let n = vec3<f32>(
        dot(inst.r0.xyz, normal),
        dot(inst.r1.xyz, normal),
        dot(inst.r2.xyz, normal),
    );

    var out: VsOut;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.normal = n;
    out.colour = inst.colour.rgb;
    out.world = world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);

    // World geometry has plenty of single-sided faces you end up behind when
    // flying inside a building; flip toward the viewer rather than going black.
    let view = normalize(frame.camera_pos.xyz - in.world);
    if (dot(n, view) < 0.0) {
        n = -n;
    }

    let key = max(dot(n, -frame.light_dir.xyz), 0.0);

    // Hemisphere ambient: sky above, bounced ground below. Cheap, and it keeps
    // upward faces reading differently from downward ones, which is what makes
    // an untextured render legible as architecture.
    let sky = vec3<f32>(0.42, 0.47, 0.58);
    let ground = vec3<f32>(0.20, 0.18, 0.16);
    let ambient = mix(ground, sky, n.y * 0.5 + 0.5);

    let lit = in.colour * (ambient + vec3<f32>(0.85, 0.83, 0.76) * key);
    return vec4<f32>(tonemap(lit), 1.0);
}

// Reinhard. The scene is lit above 1.0 in places and clipping to white loses
// the shape of exactly the metal surfaces we care about.
//
// No gamma here: the surface format is sRGB, so the hardware encodes on write.
// Doing it in the shader as well washed the whole map out to near-white.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let exposed = c * 1.35;
    return exposed / (exposed + vec3<f32>(1.0));
}

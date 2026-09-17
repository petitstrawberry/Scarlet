struct Transform { mvp: mat4x4<f32>, };
@group(0) @binding(0) var<uniform> transform: Transform;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var filtering: sampler;
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
};
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>, @location(2) uv: vec2<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.position = transform.mvp * vec4<f32>(position, 1.0);
    output.color = color;
    output.uv = uv;
    return output;
}
@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(image, filtering, input.uv) * vec4<f32>(input.color, 1.0);
}

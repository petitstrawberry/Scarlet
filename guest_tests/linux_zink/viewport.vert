#version 450
void main() {
    vec2 positions[3] = vec2[3](vec2(-0.75,-0.75), vec2(0.25,-0.75), vec2(-0.75,0.25));
    gl_Position = vec4(positions[gl_VertexIndex], 0.0, 1.0);
}

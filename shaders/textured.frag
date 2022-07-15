#version 450

layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec2 v_texcoord;
layout(location = 0) out vec4 f_color;

void main() {
    f_color = vec4(0.5 * v_normal + 0.5, 1);
}

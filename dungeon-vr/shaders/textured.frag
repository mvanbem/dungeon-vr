#version 450

layout(set = 1, binding = 0) uniform sampler2D base_color;

layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec2 v_texcoord;

layout(location = 0) out vec4 f_color;

void main() {
    const vec3 LIGHT_DIR = normalize(vec3(0.1, 1.0, 0.3));
    float ndotl = dot(v_normal, LIGHT_DIR) * 0.5 + 0.5;
    vec3 color = texture(base_color, v_texcoord).rgb * ndotl;
    f_color = vec4(color, 1);
}

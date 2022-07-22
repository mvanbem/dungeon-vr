#version 450

layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec2 v_texcoord;
layout(location = 0) out vec4 f_color;

void main() {
    const vec3 LIGHT_DIR = normalize(vec3(0.1, 1.0, 0.3));
    float ndotl = dot(v_normal, LIGHT_DIR) * 0.5 + 0.5;
    
    f_color = vec4(vec3(ndotl), 1);
}

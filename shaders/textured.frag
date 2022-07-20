#version 450

layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec2 v_texcoord;
layout(location = 0) out vec4 f_color;

void main() {
    const vec3 LIGHT_DIR = normalize(vec3(0.1, 1.0, 0.3));
    float ndotl = dot(v_normal, LIGHT_DIR) * 0.5 + 0.5;
    
    f_color = vec4(
        mix(
            vec3(26.0/255.0, 32.0/255.0, 52.0/255.0),
            vec3(255.0/255.0, 217.0/255.0, 127.0/255.0),
            ndotl),
        1);
}

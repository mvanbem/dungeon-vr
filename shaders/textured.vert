#version 450
#extension GL_EXT_multiview : require

layout(binding = 0) uniform Matrices {
    mat4 viewProj[2];
} u_matrices;

layout(push_constant) uniform Constants {
    mat4 model;
} push_constants;

layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec2 a_texcoord;

layout(location = 0) out vec3 v_normal;
layout(location = 1) out vec2 v_texcoord;

void main()  {
    gl_Position = u_matrices.viewProj[gl_ViewIndex]
        * push_constants.model
        * vec4(a_position, 1.0);
    v_normal = (push_constants.model * vec4(a_normal, 0.0)).xyz;
    v_texcoord = a_texcoord;
}

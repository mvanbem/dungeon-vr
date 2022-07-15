#version 450
#extension GL_EXT_multiview : require

layout(binding = 0) uniform Matrices {
    mat4 viewProj[2];
} u_matrices;

layout(push_constant) uniform Constants {
    mat4 model;
} push_constants;

layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_color;

layout(location = 0) out vec3 v_color;

void main()  {
    gl_Position = u_matrices.viewProj[gl_ViewIndex]
        * push_constants.model
        * vec4(a_position, 1.0);
    v_color = a_color;
}

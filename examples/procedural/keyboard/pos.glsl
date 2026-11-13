// WASD or the arrows move the dot.
float held(int code) { return texelFetch(iChannel1, ivec2(code, 0), 0).x; }

void mainImage(out vec4 c, in vec2 p) {
    vec2 pos = iFrame == 0 ? iImageResolution.xy * 0.5 : texelFetch(iChannel0, ivec2(0), 0).xy;
    vec2 dir = vec2(max(held(68), held(39)) - max(held(65), held(37)),
                    max(held(87), held(38)) - max(held(83), held(40)));
    pos = clamp(pos + dir * 700.0 * iTimeDelta, vec2(0.0), iImageResolution.xy);
    c = vec4(pos, 0.0, 1.0);
}

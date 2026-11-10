void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 s = texture(iChannel0, uv);
    vec3 background = mix(vec3(0.02, 0.03, 0.06), vec3(0.06, 0.02, 0.09), uv.y);
    vec3 glow = vec3(0.1, 0.45, 0.9) * s.y * 0.6;
    vec3 cell = vec3(0.85, 0.95, 1.0) * s.x * 0.8;
    // A little noise hides bands in the dark gradient.
    float dither = (hash(fragCoord) - 0.5) / 255.0;
    fragColor = vec4(background + glow + cell + dither, 1.0);
}

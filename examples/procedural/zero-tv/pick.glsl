// The actions buffer. Each pixel holds the number of the front-most tape
// there, as the image pass draws it. So a click on tape 3 runs the action
// `tape3`. iChannel0 is the state.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 state = texelFetch(iChannel0, ivec2(0), 0);
    int tape = isBusy(state) ? 0 : hitTape(iChannel0, toScene(fragCoord, iResolution.xy));
    fragColor = vec4(float(tape) / 255.0, 0.0, 0.0, 1.0);
}

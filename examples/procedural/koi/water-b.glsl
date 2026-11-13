// Water surface, the second simulation step for each frame.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = iFrame == 0 ? vec4(0.0) : vec4(waterStep(fragCoord), 0.0, 0.0);
}

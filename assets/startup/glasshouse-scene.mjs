/**
 * Source geometry for the Glasshouse startup animation: a pitched-roof
 * glasshouse, glazed in panes, with a seed suspended at its centre.
 *
 * The loop is exact because every rotation over PERIOD is a whole multiple of
 * the subject's own symmetry: the house is two-fold about Y and turns 180deg,
 * the seed is two-fold about its long axis and turns 360deg, and the scan
 * pulse opens and closes inside the period rather than wrapping.
 *
 * Rendered head-on by a2sg (`anythingToSomethingGreat`), which turns the frame
 * into glyphs. Everything here is therefore authored for what survives that:
 * large luminance gradients for the density ramp, and hard silhouettes for the
 * directional edge glyphs. Colour is deliberately absent - the terminal supplies
 * it from the active theme.
 *
 * Run through: assets/startup/build.sh
 */

export default ({ THREE, scene, camera, renderer }) => {
  const PERIOD = 7.68; // seconds; 96 frames at 12.5 fps (80 ms = 5 Glasshouse ticks)
  const TAU = Math.PI * 2;
  const TERM_ASPECT = 68 / 2 / 20; // 68 columns by 20 rows, cells half as wide as tall

  renderer.setClearColor(0x000000, 1);
  scene.background = new THREE.Color(0x000000);
  scene.environment = null;

  const root = new THREE.Group();
  scene.add(root);

  // ── the house ──────────────────────────────────────────────────────────────
  // Plan 2.0 x 3.0, eaves at 1.25, ridge at 2.05. Half-extents kept as names so
  // the pane and bar builders below read as elevations rather than arithmetic.
  const HX = 1.1, HZ = 1.3, EAVE = 1.28, RIDGE = 2.12;

  const uniforms = {
    scanY: { value: -2 },
    scanAmp: { value: 0 },
    camPos: { value: new THREE.Vector3() },
  };

  // Panes are additive: two sheets of glass seen through each other read
  // brighter than one, which is the whole reason the form survives at 20 rows.
  const paneMat = new THREE.ShaderMaterial({
    uniforms,
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
    blending: THREE.AdditiveBlending,
    vertexShader: `
      varying vec3 vN; varying vec3 vW; varying float vY;
      void main(){
        vec4 w = modelMatrix * vec4(position, 1.0);
        vW = w.xyz; vY = w.y;
        vN = normalize(mat3(modelMatrix) * normal);
        gl_Position = projectionMatrix * viewMatrix * w;
      }`,
    fragmentShader: `
      uniform float scanY; uniform float scanAmp; uniform vec3 camPos;
      varying vec3 vN; varying vec3 vW; varying float vY;
      void main(){
        vec3 v = normalize(camPos - vW);
        vec3 n = normalize(vN);
        float facing = abs(dot(n, v));
        float fres = pow(1.0 - facing, 2.4);           // rim glint on glancing panes
        float lam = abs(dot(n, normalize(vec3(-0.42, 0.72, 0.55))));
        float band = exp(-pow((vY - scanY) * 4.6, 2.0)) * scanAmp;
        float depth = clamp(1.0 - (length(camPos - vW) - 4.2) / 5.2, 0.18, 1.0);
        float v0 = (0.02 + fres * 0.15 + lam * lam * 0.06) * depth + band * 0.11;
        gl_FragColor = vec4(vec3(v0), 1.0);
      }`,
  });

  // Bars are opaque and depth-faded, so the far side of the frame is dimmer
  // than the near side. That single cue is what makes the glyph grid read as
  // a solid in space rather than a flat symbol.
  const barMat = new THREE.ShaderMaterial({
    uniforms,
    vertexShader: `
      varying vec3 vW; varying float vY;
      void main(){
        vec4 w = modelMatrix * vec4(position, 1.0);
        vW = w.xyz; vY = w.y;
        gl_Position = projectionMatrix * viewMatrix * w;
      }`,
    fragmentShader: `
      uniform float scanY; uniform float scanAmp; uniform vec3 camPos;
      varying vec3 vW; varying float vY;
      void main(){
        float d = length(camPos - vW);
        float near = clamp(1.0 - (d - 3.9) / 5.0, 0.0, 1.0);
        float band = exp(-pow((vY - scanY) * 4.6, 2.0)) * scanAmp;
        float v0 = mix(0.13, 0.98, near * near) + band * 0.22;
        gl_FragColor = vec4(vec3(v0), 1.0);
      }`,
  });

  const seedMat = new THREE.ShaderMaterial({
    uniforms,
    vertexShader: `
      varying vec3 vN; varying vec3 vW;
      void main(){
        vec4 w = modelMatrix * vec4(position, 1.0);
        vW = w.xyz; vN = normalize(mat3(modelMatrix) * normal);
        gl_Position = projectionMatrix * viewMatrix * w;
      }`,
    fragmentShader: `
      uniform vec3 camPos; uniform float scanAmp;
      varying vec3 vN; varying vec3 vW;
      void main(){
        vec3 v = normalize(camPos - vW);
        float lam = clamp(dot(normalize(vN), normalize(vec3(-0.45, 0.8, 0.6))), 0.0, 1.0);
        float rim = pow(1.0 - abs(dot(normalize(vN), v)), 2.0);
        gl_FragColor = vec4(vec3(0.30 + lam * 0.62 + rim * 0.5 + scanAmp * 0.18), 1.0);
      }`,
  });

  const quad = (a, b, c, d) => {
    const g = new THREE.BufferGeometry();
    const p = [...a, ...b, ...c, ...a, ...c, ...d];
    g.setAttribute('position', new THREE.Float32BufferAttribute(p, 3));
    g.computeVertexNormals();
    return new THREE.Mesh(g, paneMat);
  };
  const tri = (a, b, c) => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute([...a, ...b, ...c], 3));
    g.computeVertexNormals();
    return new THREE.Mesh(g, paneMat);
  };
  // A glazing bar: a thin box laid along a-b. Real geometry, not a GL line,
  // because a one-pixel line does not survive the downsample to glyphs.
  const bar = (a, b, t = 0.042) => {
    const A = new THREE.Vector3(...a), B = new THREE.Vector3(...b);
    const len = A.distanceTo(B);
    const m = new THREE.Mesh(new THREE.BoxGeometry(t, t, len), barMat);
    m.position.copy(A).add(B).multiplyScalar(0.5);
    m.lookAt(B);
    return m;
  };

  const add = (o) => { root.add(o); return o; };

  // Walls, roof slopes, gables.
  add(quad([-HX, 0, HZ], [HX, 0, HZ], [HX, EAVE, HZ], [-HX, EAVE, HZ]));
  add(quad([-HX, 0, -HZ], [HX, 0, -HZ], [HX, EAVE, -HZ], [-HX, EAVE, -HZ]));
  add(quad([HX, 0, -HZ], [HX, 0, HZ], [HX, EAVE, HZ], [HX, EAVE, -HZ]));
  add(quad([-HX, 0, -HZ], [-HX, 0, HZ], [-HX, EAVE, HZ], [-HX, EAVE, -HZ]));
  add(quad([-HX, EAVE, -HZ], [-HX, EAVE, HZ], [0, RIDGE, HZ], [0, RIDGE, -HZ]));
  add(quad([HX, EAVE, -HZ], [HX, EAVE, HZ], [0, RIDGE, HZ], [0, RIDGE, -HZ]));
  add(tri([-HX, EAVE, HZ], [HX, EAVE, HZ], [0, RIDGE, HZ]));
  add(tri([-HX, EAVE, -HZ], [HX, EAVE, -HZ], [0, RIDGE, -HZ]));

  // Frame: sills, eaves, ridge, corner posts.
  for (const z of [-HZ, HZ]) {
    add(bar([-HX, 0, z], [HX, 0, z], 0.085));
    add(bar([-HX, EAVE, z], [HX, EAVE, z], 0.075));
    add(bar([-HX, EAVE, z], [0, RIDGE, z], 0.075));
    add(bar([HX, EAVE, z], [0, RIDGE, z], 0.075));
  }
  for (const x of [-HX, HX]) {
    add(bar([x, 0, -HZ], [x, 0, HZ], 0.085));
    add(bar([x, EAVE, -HZ], [x, EAVE, HZ], 0.075));
    for (const z of [-HZ, HZ]) add(bar([x, 0, z], [x, EAVE, z], 0.085));
  }
  add(bar([0, RIDGE, -HZ], [0, RIDGE, HZ], 0.10));

  // Glazing bars: two bays a side, one a gable, none on the roof. The count is
  // set by the three-quarter view, where every bar is seen through two walls at
  // once - a third bar per bay turns that view into moire at 68 columns.
  for (const x of [-HX, HX]) add(bar([x, 0, 0], [x, EAVE, 0]));
  for (const z of [-HZ, HZ]) add(bar([0, 0, z], [0, EAVE, z]));

  // Plinth: the house stands on something, which anchors the silhouette.
  const P = HX + 0.26, PZ = HZ + 0.26, PY = -0.11;
  add(bar([-P, PY, -PZ], [P, PY, -PZ], 0.06));
  add(bar([-P, PY, PZ], [P, PY, PZ], 0.06));
  add(bar([-P, PY, -PZ], [-P, PY, PZ], 0.06));
  add(bar([P, PY, -PZ], [P, PY, PZ], 0.06));

  // ── the seed ───────────────────────────────────────────────────────────────
  // Two-fold about its own long axis, so a full turn per period stays exact.
  const seed = new THREE.Group();
  const body = new THREE.Mesh(new THREE.IcosahedronGeometry(0.20, 1), seedMat);
  body.scale.set(0.70, 1.0, 0.70);
  seed.add(body);
  seed.position.set(0, 0.62, 0);
  root.add(seed);

  // ── the scan rule ──────────────────────────────────────────────────────────
  // A measuring rule, not part of the building: it hangs in world space so the
  // house turns behind it. It is what makes the loop read as an instrument
  // taking a reading rather than a logo spinning.
  const ruleMat = new THREE.ShaderMaterial({
    uniforms,
    transparent: true,
    depthWrite: false,
    depthTest: false,
    blending: THREE.AdditiveBlending,
    vertexShader: `
      varying float vX;
      void main(){
        vec4 w = modelMatrix * vec4(position, 1.0);
        vX = w.x;
        gl_Position = projectionMatrix * viewMatrix * w;
      }`,
    fragmentShader: `
      uniform float scanAmp;
      varying float vX;
      void main(){
        // Fade at both ends so the rule does not butt against the frame edge.
        // Hard cut, not a long fade: a fading rule dithers into loose glyphs at
        // its ends, which reads as debris floating beside the building.
        float ends = 1.0 - smoothstep(1.95, 2.45, abs(vX));
        gl_FragColor = vec4(vec3(0.62 * scanAmp * ends), 1.0);
      }`,
  });
  const rule = new THREE.Group();
  const rail = new THREE.Mesh(new THREE.BoxGeometry(5.2, 0.045, 0.045), ruleMat);
  rule.add(rail);
  for (const x of [-1.86, -1.24, 1.24, 1.86]) {
    const tick = new THREE.Mesh(new THREE.BoxGeometry(0.06, 0.20, 0.06), ruleMat);
    tick.position.set(x, 0, 0);
    rule.add(tick);
  }
  // Same depth as the building's centre, so its ends land where the frame
  // expects them; additive and depth-test-free is what keeps it in front.
  rule.position.z = 0;
  scene.add(rule);

  return {
    update(t) {
      const phase = (t % PERIOD) / PERIOD;

      // 180deg over the loop, but not at a constant rate: the turn slows through
      // the gable and broadside views, which are the two that read cleanly at 20
      // rows, and hurries through the three-quarter views, which do not. The
      // correction is a whole number of cycles over the period, so both the
      // angle and its rate still match at the seam.
      const EASE = 0.55 * Math.PI;
      root.rotation.y = phase * Math.PI - (EASE * Math.sin(2 * TAU * phase)) / (2 * TAU);
      seed.rotation.y = phase * TAU;          // and the seed onto itself
      seed.position.y = 0.62 + Math.sin(phase * TAU) * 0.05;

      // The scan opens at the plinth, rises, and closes before it leaves the
      // ridge - amplitude is zero at both ends of the period, so it never cuts.
      uniforms.scanAmp.value = Math.pow(Math.sin(phase * Math.PI), 2.0);
      uniforms.scanY.value = -0.16 + phase * 2.52;
      rule.position.y = uniforms.scanY.value;

      const orbit = 0.10 * Math.sin(phase * TAU);
      camera.fov = 31;
      // Aspect is the TERMINAL's, not the framebuffer's. a2sg samples the frame
      // into cells of 28x48, and a terminal cell is about 1x2, so a frame drawn
      // at the framebuffer's own aspect arrives horizontally squeezed. Framing
      // to cols/2 : rows cancels it exactly.
      camera.aspect = TERM_ASPECT;
      camera.position.set(Math.sin(orbit) * 6.15, 1.68, Math.cos(orbit) * 6.15);
      camera.lookAt(0, 1.04, 0);
      camera.updateProjectionMatrix();
      uniforms.camPos.value.copy(camera.position);
    },
  };
};

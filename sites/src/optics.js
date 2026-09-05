import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/addons/geometries/RoundedBoxGeometry.js';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { gsap } from 'gsap';
import { ScrollTrigger } from 'gsap/ScrollTrigger';
import { instrumentRenderer } from './render-metrics.js';

gsap.registerPlugin(ScrollTrigger);
const PAPER = 0xf3f5f4;
const specimenFragment = `
precision highp float;
uniform sampler2D specimen;
uniform vec2 resolution;
uniform vec2 pointer;
uniform float time;
uniform float phase;
uniform float crop;
varying vec2 vUv;
mat2 rot(float a){return mat2(cos(a),-sin(a),sin(a),cos(a));}
float sdBox(vec2 p){vec2 q=abs(p)-vec2(.265,.31)+.035;return length(max(q,0.))+min(max(q.x,q.y),0.)-.035;}
vec3 sampleField(vec2 p){
  p=rot(sin(time*.16)*.065)*p;
  vec2 uv=p*vec2(1.3,.975)+.5;
  if(any(lessThan(uv,vec2(0.)))||any(greaterThan(uv,vec2(1.)))) return vec3(.953,.961,.957);
  vec2 cells=vec2(92.,120.);
  vec2 quantized=(floor(uv*cells)+.5)/cells;
  vec2 readUV=mix(uv,quantized,phase);
  float value=dot(texture2D(specimen,vec2(readUV.x*.5+crop,readUV.y)).rgb,vec3(.2126,.7152,.0722));
  float signal=smoothstep(.045,.88,value);
  vec2 sub=fract(uv*cells);
  float dots=step(length((sub-.5)*vec2(1.,.75)),.34);
  signal=mix(signal,step(.16,signal)*dots,phase);
  vec3 ink=mix(vec3(.045,.075,.055),vec3(.25,.34,.015),phase);
  return mix(vec3(.953,.961,.957),ink,signal);
}
void main(){
  vec2 p=vUv-.5;p.x*=resolution.x/resolution.y;
  vec2 center=vec2(sin(time*.28)*.11,cos(time*.19)*.035)+pointer*.045;
  float angle=.16+sin(time*.14)*.07;
  vec2 local=rot(angle)*(p-center);
  float d=sdBox(local);
  float aa=max(fwidth(d),.0004);
  float inside=1.-smoothstep(-aa,aa,d);
  float eps=.0003;
  vec2 n=normalize(vec2(sdBox(local+vec2(eps,0.))-sdBox(local-vec2(eps,0.)),sdBox(local+vec2(0.,eps))-sdBox(local-vec2(0.,eps)))+vec2(.000001));
  n=rot(-angle)*n;
  // Smooth bounded bevel: zero discontinuity across the glass silhouette.
  float depth=clamp(-d/.045,0.,1.);
  float bevel=sin(depth*3.14159265)*inside;
  vec2 bend=n*(bevel*.023+inside*.002);
  vec3 col;
  col.r=sampleField(p-bend*1.035).r;
  col.g=sampleField(p-bend).g;
  col.b=sampleField(p-bend*.965).b;
  float rim=1.-smoothstep(aa,aa*2.2,abs(d));
  float light=dot(n,normalize(vec2(-.6,.8)));
  col=mix(col,light>0.?vec3(1.):vec3(.30,.39,.33),rim*(light>0.?.72:.30));
  gl_FragColor=vec4(col,1.);
}`;

function makeSpecimen(renderer, host, texture) {
  const scene=new THREE.Scene();
  const camera=new THREE.OrthographicCamera(-1,1,1,-1,0,1);
  const uniforms={specimen:{value:texture},resolution:{value:new THREE.Vector2(1,1)},pointer:{value:new THREE.Vector2()},time:{value:3},phase:{value:0},crop:{value:host.dataset.scene==='shell'?.5:0}};
  const material=new THREE.ShaderMaterial({uniforms,vertexShader:'varying vec2 vUv;void main(){vUv=uv;gl_Position=vec4(position.xy,0.,1.);}',fragmentShader:specimenFragment});
  scene.add(new THREE.Mesh(new THREE.PlaneGeometry(2,2),material));
  const timeline=gsap.timeline({paused:true,repeat:-1,repeatDelay:2});
  timeline.to(uniforms.phase,{value:1,duration:1.1,ease:'steps(8)'},2.4).to(uniforms.phase,{value:0,duration:1.3,ease:'power2.inOut'},5.2);
  return {timeline,resize(w,h){uniforms.resolution.value.set(w,h);},draw(time,pointer){uniforms.time.value=time;uniforms.pointer.value.copy(pointer);renderer.render(scene,camera);}};
}

function makeHouse(renderer) {
  const scene=new THREE.Scene();scene.background=new THREE.Color(PAPER);
  const camera=new THREE.PerspectiveCamera(34,1,.1,100);
  camera.position.set(5,3.1,6.5);camera.lookAt(0,.3,0);
  const pmrem=new THREE.PMREMGenerator(renderer);
  const room=new RoomEnvironment();
  const environment=pmrem.fromScene(room,0);
  scene.environment=environment.texture;
  room.dispose();pmrem.dispose();
  scene.add(new THREE.HemisphereLight(0xffffff,0x9ba39d,2));
  const light=new THREE.DirectionalLight(0xffffff,3);light.position.set(-3,6,5);scene.add(light);
  const house=new THREE.Group();scene.add(house);
  const glass=new THREE.MeshPhysicalMaterial({color:0xffffff,metalness:0,roughness:0,transmission:1,thickness:.085,ior:1.46,dispersion:.12,envMapIntensity:.65,attenuationColor:0xe7fff0,attenuationDistance:8,depthWrite:false});
  function panel(w,h,d,x,y,z,rz=0){const mesh=new THREE.Mesh(new RoundedBoxGeometry(w,h,d,3,.025),glass);mesh.position.set(x,y,z);mesh.rotation.z=rz;house.add(mesh);return mesh;}
  const frameMat=new THREE.MeshBasicMaterial({color:0x53615a});
  function beam(w,h,d,x,y,z){const mesh=new THREE.Mesh(new THREE.BoxGeometry(w,h,d),frameMat);mesh.position.set(x,y,z);house.add(mesh);}
  // Original simple glasshouse silhouette, with an empty interior.
  panel(.065,1.9,2.7,-1.4,0,0);panel(.065,1.9,2.7,1.4,0,0);
  for(const z of [-1.35,1.35]){
    for(let x=-.935;x<1;x+=.935)panel(.90,1.9,.065,x,0,z);
    const shape=new THREE.Shape();shape.moveTo(-1.4,.97);shape.lineTo(1.4,.97);shape.lineTo(0,1.95);shape.closePath();
    const gable=new THREE.Mesh(new THREE.ExtrudeGeometry(shape,{depth:.055,bevelEnabled:true,bevelSegments:3,steps:1,bevelSize:.012,bevelThickness:.012}),glass);gable.position.z=z-.025;house.add(gable);
    for(const x of [-1.4,-.467,.467,1.4])beam(.012,1.95,.016,x,0,z);
    beam(2.82,.012,.016,0,-.97,z);beam(2.82,.012,.016,0,.97,z);
  }
  const slope=Math.atan2(.98,1.4),roofWidth=Math.hypot(.98,1.4);
  panel(roofWidth,.065,2.8,-.7,1.46,0,slope);panel(roofWidth,.065,2.8,.7,1.46,0,-slope);
  beam(.016,.016,2.85,0,1.98,0);
  for(const x of [-1.4,1.4]){beam(.014,.014,2.75,x,.97,0);beam(.014,.014,2.75,x,-.97,0);}
  const floor=new THREE.Mesh(new THREE.BoxGeometry(2.82,.04,2.74),new THREE.MeshBasicMaterial({color:0xdfff00}));floor.position.y=-1.01;house.add(floor);
  const sessions=[];
  const roles=['PLAN','BUILD','REVIEW'];
  for(const [index,role] of roles.entries()){
    const surface=document.createElement('canvas');surface.width=640;surface.height=400;
    const context=surface.getContext('2d');
    context.fillStyle='#111614';context.fillRect(0,0,640,400);
    context.fillStyle='#dfff00';context.fillRect(0,0,640,5);
    context.font='bold 78px monospace';context.fillText(role,38,107);
    context.fillStyle='#c4d0c7';context.fillRect(38,170,410,9);context.fillRect(38,213,310,9);context.fillRect(38,256,365,9);
    context.fillStyle='#dfff00';context.font='38px monospace';context.fillText('> _',38,351);
    const map=new THREE.CanvasTexture(surface);map.colorSpace=THREE.SRGBColorSpace;map.generateMipmaps=false;map.minFilter=THREE.LinearFilter;map.anisotropy=renderer.capabilities.getMaxAnisotropy();
    // Render the illustrative UI in the transparent pass, after transmission:
    // glass remains refractive, but typography is deliberately not re-sampled.
    const screen=new THREE.Mesh(new THREE.PlaneGeometry(.94,.588),new THREE.MeshBasicMaterial({map,side:THREE.DoubleSide,transparent:true,depthTest:true,depthWrite:true,toneMapped:false}));
    screen.renderOrder=5;
    screen.position.set((index-1)*.83,.32-index*.25,.08);house.add(screen);
    sessions.push({screen,index});
  }
  const inverse=new THREE.Quaternion();
  return {
    resize(w,h){camera.aspect=w/h;camera.updateProjectionMatrix();},
    draw(time,pointer){
      house.rotation.y=(time-3)*.10+pointer.x*.10;
      inverse.copy(house.quaternion).invert();
      for(const {screen,index} of sessions){
        // Legible from every view while remaining located inside the house.
        screen.quaternion.copy(inverse).multiply(camera.quaternion);
        screen.position.set((index-1)*.82,-.38+index*.51,0);
      }
      renderer.render(scene,camera);
    },
  };
}

export async function startOptics(hosts,button){
  const reduced=matchMedia('(prefers-reduced-motion: reduce)');
  let paused=reduced.matches,time=3,last=0;
  const records=[];
  const entrances=[];
  // Keep pause within reach of the animation, rather than at the page footer.
  document.querySelector('.hero').append(button);
  button.classList.add('hero-motion-toggle');
  const texture=await new THREE.TextureLoader().loadAsync(`${hosts[0].dataset.root}specimens.png`);
  function setLabel(){button.textContent=paused?'Resume motion':'Pause motion';button.setAttribute('aria-pressed',String(paused));}
  function sync(){for(const animation of entrances){if(paused||document.hidden)animation.pause();else if(animation.progress()<1)animation.resume();}for(const r of records){if(r.engine.timeline){if(paused||document.hidden||!r.visible)r.engine.timeline.pause();else r.engine.timeline.play();}}}
  const visibility=new IntersectionObserver(entries=>{for(const entry of entries){let r=records.find(r=>r.host===entry.target);if(!r && entry.isIntersecting){
    try{
      const renderer=new THREE.WebGLRenderer({canvas:entry.target.querySelector('canvas'),antialias:true,powerPreference:'low-power'});
      renderer.setPixelRatio(Math.min(Math.max(devicePixelRatio,1.5),2));
      const engine=entry.target.dataset.scene==='house'?makeHouse(renderer):makeSpecimen(renderer,entry.target,texture);
      instrumentRenderer(engine,renderer,entry.target.dataset.scene);
      r={host:entry.target,renderer,engine,visible:true,pointer:new THREE.Vector2(),target:new THREE.Vector2()};records.push(r);
      const resize=new ResizeObserver(()=>{const {width,height}=r.host.getBoundingClientRect();renderer.setSize(width,height,false);engine.resize(width,height);engine.draw(time,r.pointer);});resize.observe(r.host);
      r.host.addEventListener('pointermove',e=>{const rect=r.host.getBoundingClientRect();r.target.set((e.clientX-rect.left)/rect.width-.5,.5-(e.clientY-rect.top)/rect.height);});
      r.host.addEventListener('pointerleave',()=>r.target.set(0,0));
      r.host.querySelector('canvas').addEventListener('webglcontextlost',e=>{e.preventDefault();r.visible=false;r.engine.timeline?.pause();r.host.classList.remove('ready');});
      r.host.classList.add('ready');
    }catch{entry.target.classList.add('static-only');}
  }if(r)r.visible=entry.isIntersecting;}sync();},{rootMargin:'100px'});
  hosts.forEach(host=>visibility.observe(host));
  const tick=()=>{const now=performance.now();if(now-last<32)return;const delta=last?Math.min((now-last)/1000,.06):0;last=now;if(!paused&&!document.hidden)time+=delta;for(const r of records){if(!r.visible||document.hidden)continue;if(!paused)r.pointer.lerp(r.target,.08);if(!paused)r.engine.draw(time,r.pointer);}};
  gsap.ticker.add(tick);
  button.addEventListener('click',()=>{paused=!paused;setLabel();sync();});
  reduced.addEventListener('change',()=>{paused=reduced.matches;setLabel();sync();});
  document.addEventListener('visibilitychange',sync);
  const media=gsap.matchMedia();
  media.add('(prefers-reduced-motion: no-preference)',()=>{
    const entrance=gsap.timeline({defaults:{ease:'power2.out'}});
    entrance.from('.hero h1',{y:24,duration:.65})
      .from('.hero>.optics',{y:22,scale:.965,duration:.85},.24)
      .from('.hero-bottom',{y:12,duration:.5},.7);
    entrances.push(entrance);
    document.querySelectorAll('.feature-list article').forEach(el=>gsap.from(el,{y:18,duration:.6,ease:'power2.out',scrollTrigger:{trigger:el,start:'top 94%',once:true}}));
  });
  setLabel();
}

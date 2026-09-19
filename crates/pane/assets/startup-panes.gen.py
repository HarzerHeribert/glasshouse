import math, os, sys
N = int(sys.argv[1]) if len(sys.argv)>1 else 28
W = 1024
os.makedirs('/tmp/pane-art/seq', exist_ok=True)

# Four square panes stacked on the Y axis, each a quad in 3D.
def pane(y, s):
    return [(-s,y,-s),( s,y,-s),( s,y, s),(-s,y, s)]

PANES = [pane(-260, 300), pane(-80, 250), pane(100, 200), pane(275, 150)]

def project(p, yaw, pitch, assemble):
    x,y,z = p
    y = y * assemble            # panes slide together from spread out
    cy, sy = math.cos(yaw), math.sin(yaw)
    x, z = x*cy - z*sy, x*sy + z*cy
    cp, sp = math.cos(pitch), math.sin(pitch)
    y, z = y*cp - z*sp, y*sp + z*cp
    d = 2400.0
    f = d / (d + z)
    return (W/2 + x*f, W/2 + y*f)

for i in range(N):
    t = i / (N-1)
    ease = 1 - (1-t)**3                     # settles rather than stops dead
    yaw = -0.9 + ease * (math.pi*0.5 + 0.9) # three quarter turn into place
    pitch = 0.62
    assemble = 0.35 + 0.65*ease
    body = []
    for idx, quad in enumerate(PANES):
        pts = [project(p, yaw, pitch, assemble) for p in quad]
        d = ' '.join(f'{x:.1f},{y:.1f}' for x,y in pts)
        shade = ['#0a0a0a','#4a4a4a','#8f8f8f','#d8d8d8'][idx]
        body.append(f'<polygon points="{d}" fill="{shade}" stroke="#fff" stroke-width="26"/>')
        # mullions: the cross inside each pane
        a,b,c,e = pts
        m1 = ((a[0]+b[0])/2,(a[1]+b[1])/2); m2=((c[0]+e[0])/2,(c[1]+e[1])/2)
        m3 = ((b[0]+c[0])/2,(b[1]+c[1])/2); m4=((e[0]+a[0])/2,(e[1]+a[1])/2)
        body.append(f'<line x1="{m1[0]:.1f}" y1="{m1[1]:.1f}" x2="{m2[0]:.1f}" y2="{m2[1]:.1f}" stroke="#fff" stroke-width="14" opacity="0.95"/>')
        body.append(f'<line x1="{m3[0]:.1f}" y1="{m3[1]:.1f}" x2="{m4[0]:.1f}" y2="{m4[1]:.1f}" stroke="#fff" stroke-width="14" opacity="0.95"/>')
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{W}" viewBox="0 0 {W} {W}">'
           f'<rect width="{W}" height="{W}" fill="#000"/>' + ''.join(body) + '</svg>')
    open(f'/tmp/pane-art/seq/{i:03d}.svg','w').write(svg)
print("wrote", N)

# Glasshouse + Pane product site

Static, independently built product pages. Everything for the website lives here;
the Rust build and project-status documents are unaffected.

## Local preview

```sh
cd sites
npm ci
npm run dev
```

## Production build

```sh
npm run build
```

Publish `sites/dist/` as the GitHub Pages artifact. The homepage, `glasshouse/`,
and `pane/` use relative links and assets, supporting both the repository Pages
prefix and a later custom domain. No server, API keys, or external runtime CDN
is required. Fonts are bundled locally.

For GitHub Pages, the repository owner/orchestrator can place the example workflow
below at `.github/workflows/pages.yml` and enable GitHub Actions as the Pages
source in repository settings. It is intentionally documented here rather than
changing the active orchestrator's workflow files. No publication has been performed.

```yaml
name: Product site
on:
  push:
    branches: [main]
    paths: ['sites/**', '.github/workflows/pages.yml']
  workflow_dispatch:
permissions:
  contents: read
  pages: write
  id-token: write
concurrency:
  group: pages
  cancel-in-progress: true
jobs:
  deploy:
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
          cache: npm
          cache-dependency-path: sites/package-lock.json
      - run: npm ci
        working-directory: sites
      - run: npm run build
        working-directory: sites
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: sites/dist
      - name: Deploy
        id: deployment
        uses: actions/deploy-pages@v4
```

Custom domain setup: configure the domain in GitHub Pages settings, configure its
DNS according to GitHub's instructions, and enable HTTPS after verification.
https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site

## Design contract

- No frosted cards, backdrop blur, Gaussian blur, or diffuse glass surfaces.
- The hero is a complete 3D glasshouse with zero-roughness transmissive walls
  and a pitched roof. Natural specimens recur behind clear beveled glass.
- Three.js handles transmission and refraction. GSAP sequences the specimens
  between detailed and terminal-raster states, with quiet holds between them.
- Six striking features per product live in `src/products.js`; the homepage
  highlights three each, and the product pages show all six.
- Page text remains in the DOM and outside the optical effect.
- Motion pauses offscreen and in background tabs. Reduced-motion starts still;
  a visible pause/resume control is always available when WebGL works.
- A static typographic or specimen fallback remains visible without WebGL or
  if the effect cannot load.
- Glasshouse and Pane have distinct product pages and acquisition destinations.
- Pane runtime concepts are labeled as in development, not advertised as shipped.

The library comparison, source links, product-positioning rationale, and original
artwork prompt are in [DESIGN-RESEARCH.md](DESIGN-RESEARCH.md).

## References

- https://threejs.org/docs/pages/ShaderMaterial.html
- https://threejs.org/docs/pages/WebGLRenderer.html
- https://docs.github.com/en/pages

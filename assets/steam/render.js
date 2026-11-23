// Renders the Steam artwork SVG files to PNG at their own size.
// Usage (repo root): node assets/steam/render.js
// Needs Playwright with Chromium. The logo keeps a transparent ground.
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const dir = __dirname;
const jobs = ['capsule', 'wide', 'hero', 'logo'];

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  for (const name of jobs) {
    const svg = fs.readFileSync(path.join(dir, `${name}.svg`));
    const [, w, h] = svg.toString().match(/width="(\d+)" height="(\d+)"/).map(Number);
    await page.setViewportSize({ width: w, height: h });
    const data = svg.toString('base64');
    await page.setContent(
      `<style>html,body{margin:0;background:transparent}</style>` +
      `<img src="data:image/svg+xml;base64,${data}" width="${w}" height="${h}" style="display:block">`);
    await page.waitForTimeout(100);
    await page.screenshot({ path: path.join(dir, `${name}.png`), omitBackground: true });
  }
  await browser.close();
})();

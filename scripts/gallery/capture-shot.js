// Usage:
//   node capture-shot.js <url> <output_png> [query] [wait_ms] [width] [height] [open_mode]

"use strict";

const { chromium } = require(require.resolve("playwright", {
  paths: [process.cwd(), __dirname],
}));

async function main() {
  const url = process.argv[2];
  const outputPath = process.argv[3];
  const query = process.argv[4] || "browser bug office hours";
  const waitMs = Number(process.argv[5] || 20000);
  const width = Number(process.argv[6] || 1280);
  const height = Number(process.argv[7] || 720);
  const openMode = process.argv[8] || "ctrlk";
  const profileDir = process.env.EDDIE_PLAYWRIGHT_PROFILE || "/tmp/eddie-gallery-playwright-profile";

  if (!url || !outputPath) {
    throw new Error("usage: node capture-shot.js <url> <output_png> [query] [wait_ms] [width] [height]");
  }

  const context = await chromium.launchPersistentContext(profileDir, {
    headless: true,
    viewport: { width, height },
  });

  const page = context.pages()[0] || (await context.newPage());
  await page.goto(url, { waitUntil: "domcontentloaded", timeout: 120000 });
  await page.waitForTimeout(1200);

  if (openMode === "click") {
    await page.mouse.click(width - 28, height - 28);
    await page.waitForTimeout(700);
  } else if (openMode === "ctrlk") {
    await page.keyboard.press("Control+KeyK");
    await page.waitForTimeout(600);
  } else {
    throw new Error(`unsupported open_mode: ${openMode}`);
  }

  await page.keyboard.press("Control+KeyA");
  await page.keyboard.type(query, { delay: 22 });
  await page.waitForTimeout(waitMs);

  await page.screenshot({ path: outputPath, fullPage: false });
  await context.close();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

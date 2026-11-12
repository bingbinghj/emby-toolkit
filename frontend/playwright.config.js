import { defineConfig, devices } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

process.env.TMPDIR ||= fileURLToPath(new URL('./test-results/tmp', import.meta.url));
mkdirSync(process.env.TMPDIR, { recursive: true });

export default defineConfig({
  testDir: './tests/ui',
  timeout: 30_000,
  expect: { timeout: 8_000 },
  fullyParallel: true,
  workers: 1,
  reporter: [['list']],
  outputDir: './test-results/ui',
  use: {
    baseURL: process.env.ETK_UI_BASE_URL || 'http://127.0.0.1:4173',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    serviceWorkers: 'block',
    reducedMotion: 'reduce',
    launchOptions: process.env.ETK_UI_CHROMIUM_PATH ? { executablePath: process.env.ETK_UI_CHROMIUM_PATH } : {},
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});

import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/browser",
  workers: 1,
  retries: 0,
  forbidOnly: !!process.env.CI,
  timeout: 30_000,
  globalTimeout: 120_000,
  reporter: "list",
  use: {
    browserName: "webkit",
    baseURL: "http://127.0.0.1:1421",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "pnpm exec vite --host 127.0.0.1 --port 1421 --strictPort",
    url: "http://127.0.0.1:1421",
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
